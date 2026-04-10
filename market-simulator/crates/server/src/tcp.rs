use std::net::{TcpListener, TcpStream};
use std::io::{Read, Write};
use fix::engine::{FixRawMsg};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use crossbeam_channel::{bounded};
use std::time::Duration;
use utils::market_name;

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


pub struct FixServer<const N: usize> {
    fifo_in: Arc<crossbeam_channel::Sender<FixRawMsg<N>>>,
    shutdown: Arc<AtomicBool>,
}

impl <'a, const N: usize> FixServer<N> {
    pub fn new(
        fifo_in: Arc<crossbeam_channel::Sender<FixRawMsg<N>>>,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self { 
            fifo_in,
            shutdown,
        }
    }

    pub fn accept_loop(&self, listener: TcpListener) {
        // Non-blocking mode lets us poll the shutdown flag between accepts.
        listener.set_nonblocking(true)
            .expect("Cannot set non-blocking on TCP listener");

        loop {
            if self.shutdown.load(Ordering::Relaxed) {
                tracing::info!("[{}] TCP accept loop: shutdown signal received, stopping", market_name());
                break;
            }

            match listener.accept() {
                Ok((stream, addr)) => {
                    // Restore blocking mode for the connected socket.
                    stream.set_nonblocking(false).unwrap();
                    // Disable Nagle's algorithm for lower latency
                    stream.set_nodelay(true).unwrap();

                    let queue    = Arc::clone(&self.fifo_in);
                    let shutdown = Arc::clone(&self.shutdown);

                    tracing::info!("[{}] New client connected from {}", market_name(), addr);
                    std::thread::spawn(move || {
                        Self::handle_client(stream, queue, shutdown);
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // No pending connection yet — sleep briefly and retry.
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(e) => {
                    tracing::error!("[{}] TCP accept error: {e}", market_name());
                    break;
                }
            }
        }

        tracing::info!("[{}] TCP server exited gracefully", market_name());
    }

    fn handle_client(
        stream: TcpStream,
        queue:   Arc<crossbeam_channel::Sender<FixRawMsg<N>>>,
        shutdown: Arc<AtomicBool>,
    ) {
        let (response_tx, response_rx) = bounded::<FixRawMsg<N>>(response_queue_capacity());

        let mut read_stream = stream;
        let mut write_stream = match read_stream.try_clone() {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("[{}] Failed to clone client stream: {e}", market_name());
                return;
            }
        };

        let client_alive = Arc::new(AtomicBool::new(true));
        let writer_alive = Arc::clone(&client_alive);
        let writer_shutdown = Arc::clone(&shutdown);

        let writer_thread = std::thread::spawn(move || {
            while writer_alive.load(Ordering::Relaxed) && !writer_shutdown.load(Ordering::Relaxed) {
                match response_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(response) => {
                        if let Err(e) = write_stream.write_all(&response.data[..response.len as usize]) {
                            tracing::error!("[{}] Failed to send response to client: {}", market_name(), e);
                            writer_alive.store(false, Ordering::Relaxed);
                            break;
                        }
                        tracing::debug!("[{}] Response from FIX engine sent back to client", market_name());
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                        continue;
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        break;
                    }
                }
            }
        });

        if let Err(e) = read_stream.set_read_timeout(Some(Duration::from_millis(100))) {
            tracing::warn!("[{}] Failed to set read timeout on client stream: {e}", market_name());
        }

        let mut buf = [0u8; 4096];
        while client_alive.load(Ordering::Relaxed) && !shutdown.load(Ordering::Relaxed) {
            match read_stream.read(&mut buf) {
                Ok(0) => {
                    tracing::info!("[{}] Client disconnected", market_name());
                    client_alive.store(false, Ordering::Relaxed);
                    break;
                }
                Ok(n) => {
                    let mut msg = FixRawMsg::<N>::default();
                    msg.len = n as u16;
                    msg.data[..n].copy_from_slice(&buf[..n]);
                    msg.resp_queue = Some(response_tx.clone());

                    if let Err(e) = queue.send(msg) {
                        tracing::error!("[{}] Failed to send message to FIX engine: {e}", market_name());
                        client_alive.store(false, Ordering::Relaxed);
                        break;
                    }

                    tracing::debug!("[{}] Message from client forwarded to FIX engine", market_name());
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                    continue;
                }
                Err(e) => {
                    tracing::error!("[{}] Error reading from client: {e}", market_name());
                    client_alive.store(false, Ordering::Relaxed);
                    break;
                }
            }
        }

        drop(response_tx);
        let _ = writer_thread.join();
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
    // find "35=X" between SOH delimiters
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
        t   => format!("◀ MSG ({t})"),
    }
}