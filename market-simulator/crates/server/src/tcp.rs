use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use crossbeam_channel::TrySendError;
use tokio::sync::Semaphore;
use fix::engine::FixRawMsg;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::mpsc;
use utils::market_name;

/// Determine the capacity of the response queue for each client connection.
fn response_queue_capacity() -> usize {
    static CAPACITY: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *CAPACITY.get_or_init(|| {
        std::env::var("TCP_RESPONSE_QUEUE_CAPACITY")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(1024)
    })
}

/// Determine the maximum number of concurrent client connections.
fn tcp_max_connections() -> usize {
    static MAX: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *MAX.get_or_init(|| {
        std::env::var("TCP_MAX_CONNECTIONS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(2048)
    })
}

/// Determine the number of worker threads for the Tokio runtime used in the TCP server.
fn tcp_runtime_worker_threads() -> usize {
    static WORKERS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *WORKERS.get_or_init(|| {
        std::env::var("TCP_RUNTIME_WORKERS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| *v > 0)
            .unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map(|n| n.get().max(2))
                    .unwrap_or(2)
            })
    })
}

/// The `FixServer` struct encapsulates the TCP server logic, including accepting client connections, handling communication with clients, and forwarding messages to the FIX engine. It uses Tokio for async I/O and supports graceful shutdown.
pub struct FixServer<const N: usize> {
    /// Channel to send messages from TCP handlers to the FIX engine.
    tcp_to_fix: Arc<crossbeam_channel::Sender<FixRawMsg<N>>>,
    /// Atomic flag to signal shutdown across all tasks.
    shutdown: Arc<AtomicBool>,
}

impl<'a, const N: usize> FixServer<N> {
    pub fn new(
        tcp_to_fix: Arc<crossbeam_channel::Sender<FixRawMsg<N>>>,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            tcp_to_fix,
            shutdown,
        }
    }

    /// Starts the TCP server and blocks the current thread until shutdown.
    ///
    /// `cpu_ids` controls runtime worker-thread CPU pinning. When empty,
    /// no worker affinity is applied.
    pub fn accept_loop(
        &self,
        listener: std::net::TcpListener, cpu_ids: Vec<usize>
    ) -> Result<(), Box<dyn std::error::Error>> {

        if let Err(e) = listener.set_nonblocking(true) {
            tracing::error!("[{}] Cannot set non-blocking on TCP listener: {e}", market_name());
            return Err(Box::new(e));
        }

        let mut builder = tokio::runtime::Builder::new_multi_thread();
        builder
            .worker_threads(tcp_runtime_worker_threads())
            .enable_all()
            .thread_name("fix-tcp-async");

        if !cpu_ids.is_empty() {
            let bind_ids = Arc::new(cpu_ids);
            let bind_cursor = Arc::new(AtomicUsize::new(0));
            builder.on_thread_start({
                let bind_ids = Arc::clone(&bind_ids);
                let bind_cursor = Arc::clone(&bind_cursor);
                move || {
                    let idx = bind_cursor.fetch_add(1, Ordering::Relaxed);
                    let core_id = bind_ids[idx % bind_ids.len()];
                    if !core_affinity::set_for_current(core_affinity::CoreId { id: core_id }) {
                        tracing::warn!(
                            "[{}] Failed to pin fix-tcp-async worker to CPU {}",
                            market_name(),
                            core_id
                        );
                    }
                }
            });
        }

        let runtime = match builder.build() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!("[{}] Failed to build TCP Tokio runtime: {e}", market_name());
                return Err(Box::new(e));
            }
        };

        let semaphore = Arc::new(Semaphore::new(tcp_max_connections()));
        runtime.block_on(self.accept_loop_async(listener, semaphore));
        Ok(())
    }

    /// Async accept loop: accept clients and spawn one Tokio task per connection.
    async fn accept_loop_async(&self, listener: std::net::TcpListener, semaphore: Arc<Semaphore>) {
        let listener = match TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(e) => {
                tracing::error!("[{}] Failed to convert TCP listener to Tokio: {e}", market_name());
                return;
            }
        };

        while !self.shutdown.load(Ordering::Relaxed) {
            tokio::select! {
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, addr)) => {
                            if let Err(e) = stream.set_nodelay(true) {
                                tracing::warn!("[{}] Failed to set TCP_NODELAY for {addr}: {e}", market_name());
                            }

                            let permit = match Arc::clone(&semaphore).try_acquire_owned() {
                                Ok(p) => p,
                                Err(_) => {
                                    tracing::warn!(
                                        "[{}] Connection limit reached ({} max), rejecting {addr}",
                                        market_name(),
                                        tcp_max_connections()
                                    );
                                    continue;
                                }
                            };

                            let queue = Arc::clone(&self.tcp_to_fix);
                            let shutdown = Arc::clone(&self.shutdown);

                            tracing::info!("[{}] New client connected from {}", market_name(), addr);
                            tokio::spawn(async move {
                                Self::handle_client(stream, queue, shutdown).await;
                                drop(permit); // returns the slot when the client disconnects
                            });
                        }
                        Err(e) => {
                            tracing::error!("[{}] TCP accept error: {e}", market_name());
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
        }

        tracing::info!("[{}] TCP accept loop: shutdown signal received, stopping", market_name());
        tracing::info!("[{}] TCP server exited gracefully", market_name());
    }

    /// This function handles communication with a single client. It runs an async loop that reads messages from the client, forwards them to the FIX engine, and sends responses back to the client.
    /// It will exit when the client disconnects or when the server is shutting down.
    /// Arguments:
    /// - stream: the TCP stream representing the connection to the client. We will read messages from this stream and write responses back to it.
    /// - queue: the channel sender that represents the FIX engine's input queue. We will send messages received from the client through this channel to the FIX engine.
    /// - shutdown: an atomic boolean that indicates whether the server is shutting down. If this becomes true, we will stop processing messages and exit.
    async fn handle_client(
        stream: TcpStream,
        queue: Arc<crossbeam_channel::Sender<FixRawMsg<N>>>,
        shutdown: Arc<AtomicBool>,
    ) {
        let (response_tx, response_rx) = mpsc::channel::<FixRawMsg<N>>(response_queue_capacity());
        let (mut read_stream, write_stream) = stream.into_split();

        let writer_task = tokio::spawn(async move {
            Self::writer_loop(write_stream, response_rx).await;
        });

        let mut buf = [0u8; 4096];
        loop {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }
            match tokio::time::timeout(Duration::from_millis(100), read_stream.read(&mut buf)).await {
                Ok(Ok(0)) => {
                    tracing::info!("[{}] Client disconnected", market_name());
                    break;
                }
                Ok(Ok(n)) => {
                    let mut msg = FixRawMsg::<N>::default();
                    msg.len = n as u16;
                    msg.data[..n].copy_from_slice(&buf[..n]);
                    msg.resp_queue = Some(response_tx.clone());

                    if let Err(e) = Self::enqueue_to_fix(&queue, msg, &shutdown).await {
                        tracing::error!("[{}] Failed to send message to FIX engine: {e}", market_name());
                        break;
                    }

                    tracing::debug!("[{}] Message from client forwarded to FIX engine", market_name());
                }
                Ok(Err(e)) => {
                    tracing::error!("[{}] Error reading from client: {e}", market_name());
                    break;
                }
                Err(_) => continue,
            }
        }

        drop(response_tx);
        if let Err(e) = writer_task.await {
            tracing::warn!("[{}] Writer task join error: {e}", market_name());
        }
    }

    /// Async write loop: wait for FIX responses and write them back to the socket.
    /// Exits automatically when all senders are dropped (client disconnected or server shutdown).
    async fn writer_loop(
        mut write_stream: OwnedWriteHalf,
        mut response_rx: mpsc::Receiver<FixRawMsg<N>>,
    ) {
        while let Some(response) = response_rx.recv().await {
            if let Err(e) = write_stream.write_all(&response.data[..response.len as usize]).await {
                tracing::error!("[{}] Failed to send response to client: {}", market_name(), e);
                return;
            }
            tracing::debug!("[{}] Response from FIX engine sent back to client", market_name());
        }
    }

    /// Try to enqueue a message to the FIX engine; retry briefly while the queue is full.
    async fn enqueue_to_fix(
        queue: &crossbeam_channel::Sender<FixRawMsg<N>>,
        msg: FixRawMsg<N>,
        shutdown: &Arc<AtomicBool>,
    ) -> Result<(), String> {
        let mut pending = msg;
        loop {
            match queue.try_send(pending) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Disconnected(_)) => {
                    return Err("FIX engine queue disconnected".to_string());
                }
                Err(TrySendError::Full(returned)) => {
                    if shutdown.load(Ordering::Relaxed) {
                        return Err("server is shutting down".to_string());
                    }
                    pending = returned;
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            }
        }
    }
}

/// Replace SOH (0x01) with " | " for display.
pub fn pretty_fix(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw)
        .replace('\x01', " │ ")
}

/// Extract MsgType (tag 35) and return a human-readable label.
pub fn classify_fix_msg(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    let msg_type = s
        .split('\x01')
        .find(|f| f.starts_with("35="))
        .and_then(|f| f.strip_prefix("35="))
        .unwrap_or("?");

    match msg_type {
        "8" => "◀ EXEC REPORT (8)".into(),
        "W" => "◀ MD SNAPSHOT (W)".into(),
        "X" => "◀ MD INCREMENTAL (X)".into(),
        "Y" => "◀ MD REJECT (Y)".into(),
        "0" => "◀ HEARTBEAT (0)".into(),
        "A" => "◀ LOGON (A)".into(),
        "5" => "◀ LOGOUT (5)".into(),
        t => format!("◀ MSG ({t})"),
    }
}