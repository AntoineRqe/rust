use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossbeam_channel::TrySendError;
use fix::engine::FixRawMsg;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
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

pub struct FixServer<const N: usize> {
    tcp_to_fix: Arc<crossbeam_channel::Sender<FixRawMsg<N>>>,
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
    pub fn accept_loop(&self, listener: TcpListener) {
        if let Err(e) = listener.set_nonblocking(true) {
            tracing::error!("[{}] Cannot set non-blocking on TCP listener: {e}", market_name());
            return;
        }

        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(tcp_runtime_worker_threads())
            .enable_all()
            .thread_name("fix-tcp-async")
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!("[{}] Failed to build TCP Tokio runtime: {e}", market_name());
                return;
            }
        };

        runtime.block_on(self.accept_loop_async(listener));
    }

    /// Async accept loop: accept clients and spawn one Tokio task per connection.
    async fn accept_loop_async(&self, listener: TcpListener) {
        let listener = match tokio::net::TcpListener::from_std(listener) {
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

                            let queue = Arc::clone(&self.tcp_to_fix);
                            let shutdown = Arc::clone(&self.shutdown);

                            tracing::info!("[{}] New client connected from {}", market_name(), addr);
                            tokio::spawn(async move {
                                Self::handle_client(stream, queue, shutdown).await;
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

        let client_alive = Arc::new(AtomicBool::new(true));
        let writer_alive = Arc::clone(&client_alive);
        let writer_shutdown = Arc::clone(&shutdown);

        let writer_task = tokio::spawn(async move {
            Self::writer_loop(write_stream, response_rx, writer_alive, writer_shutdown).await;
        });

        let mut buf = [0u8; 4096];
        while client_alive.load(Ordering::Relaxed) && !shutdown.load(Ordering::Relaxed) {
            match tokio::time::timeout(Duration::from_millis(100), read_stream.read(&mut buf)).await {
                Ok(Ok(0)) => {
                    tracing::info!("[{}] Client disconnected", market_name());
                    client_alive.store(false, Ordering::Relaxed);
                    break;
                }
                Ok(Ok(n)) => {
                    let mut msg = FixRawMsg::<N>::default();
                    msg.len = n as u16;
                    msg.data[..n].copy_from_slice(&buf[..n]);
                    msg.resp_queue = Some(response_tx.clone());

                    if let Err(e) = Self::enqueue_to_fix(&queue, msg, &shutdown).await {
                        tracing::error!("[{}] Failed to send message to FIX engine: {e}", market_name());
                        client_alive.store(false, Ordering::Relaxed);
                        break;
                    }

                    tracing::debug!("[{}] Message from client forwarded to FIX engine", market_name());
                }
                Ok(Err(e)) => {
                    tracing::error!("[{}] Error reading from client: {e}", market_name());
                    client_alive.store(false, Ordering::Relaxed);
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
    async fn writer_loop(
        mut write_stream: tokio::net::tcp::OwnedWriteHalf,
        mut response_rx: mpsc::Receiver<FixRawMsg<N>>,
        writer_alive: Arc<AtomicBool>,
        writer_shutdown: Arc<AtomicBool>,
    ) {
        while writer_alive.load(Ordering::Relaxed) && !writer_shutdown.load(Ordering::Relaxed) {
            tokio::select! {
                maybe_response = response_rx.recv() => {
                    match maybe_response {
                        Some(response) => {
                            if let Err(e) = write_stream.write_all(&response.data[..response.len as usize]).await {
                                tracing::error!("[{}] Failed to send response to client: {}", market_name(), e);
                                writer_alive.store(false, Ordering::Relaxed);
                                return;
                            }
                            tracing::debug!("[{}] Response from FIX engine sent back to client", market_name());
                        }
                        None => {
                            tracing::warn!("[{}] Response channel disconnected", market_name());
                            return;
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
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