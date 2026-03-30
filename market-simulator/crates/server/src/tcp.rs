use std::net::{TcpListener, TcpStream};
use std::io::{Read, Write};
use fix::engine::{FixRawMsg};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use crossbeam_channel::{unbounded};
use web::state::{
    EventBus,
    WsEvent,
};


pub struct FixServer<const N: usize> {
    fifo_in: Arc<crossbeam_channel::Sender<FixRawMsg<N>>>,
    shutdown: Arc<AtomicBool>,
    bus: EventBus,
}

impl <'a, const N: usize> FixServer<N> {
    pub fn new(
        fifo_in: Arc<crossbeam_channel::Sender<FixRawMsg<N>>>,
        bus: EventBus,
    ) -> Self {
        Self { 
            fifo_in,
            shutdown: Arc::new(AtomicBool::new(false)),
            bus,
        }
    }

    /// Returns a handle to the shutdown flag so external code (e.g. the
    /// Ctrl-C handler in main) can stop the accept loop cleanly.
    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shutdown)
    }

    pub fn accept_loop(&self, listener: TcpListener) {
        // Non-blocking mode lets us poll the shutdown flag between accepts.
        listener.set_nonblocking(true)
            .expect("Cannot set non-blocking on TCP listener");

        loop {
            if self.shutdown.load(Ordering::Relaxed) {
                tracing::info!("TCP accept loop: shutdown signal received, stopping");
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
                    let bus      = self.bus.clone();

                    tracing::info!("New client connected from {}", addr);
                    std::thread::spawn(move || {
                        Self::handle_client(stream, queue, shutdown, bus);
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // No pending connection yet — sleep briefly and retry.
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(e) => {
                    tracing::error!("TCP accept error: {e}");
                    break;
                }
            }
        }

        tracing::info!("TCP server exited gracefully");
    }

    fn handle_client(
        mut stream: TcpStream,
        queue:   Arc<crossbeam_channel::Sender<FixRawMsg<N>>>,
        shutdown: Arc<AtomicBool>,
        bus: EventBus,
    ) {

        let peer = stream.peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "unknown".into());

        let (response_tx, response_rx) = unbounded();
        let mut buf = [0u8; 4096];

        // Notify the web layer that a new client has connected
        bus.publish(WsEvent::FixMessage {
            label: "INFO".into(),
            body:  format!("FIX client connected: {peer}"),
            tag:   "info".into(),
        });

        while !shutdown.load(Ordering::Relaxed) {
            match stream.read(&mut buf) {
                Ok(0) => {
                    tracing::info!("Client disconnected");
                    break
                }, // no message, client closed connection
                Ok(n) => {

                    // ── forward inbound FIX message to browsers ──────────
                    // (this is what the Python client sent — the order)
                    let sent_body = pretty_fix(&buf[..n]);
                    let sent_label = classify_fix_msg(&buf[..n]);
                    bus.publish(WsEvent::FixMessage {
                        label: format!("SENT ▶  {sent_label}"),
                        body:  sent_body,
                        tag:   "send".into(),
                    });
        
                    // ── forward inbound FIX message to FIX engine ──────────
                    let mut msg = FixRawMsg::<N>::default();
                    msg.len = n as u16;
                    msg.data[..n].copy_from_slice(&buf[..n]);
                    msg.resp_queue = Some(response_tx.clone());

                    queue.send(msg).expect("Failed to send message to FIX engine");
                    tracing::debug!("Message from client forwarded to FIX engine, waiting for response...");

                    loop {
                        if let Ok(response) = response_rx.recv() {
                            if let Err(e) = stream.write_all(&response.data[..response.len as usize]) {
                                tracing::error!("Failed to send response to client: {}", e);
                                break;
                            }

                            let body = pretty_fix(&response.data[..response.len as usize]);
                            let label = classify_fix_msg(&response.data[..response.len as usize]);
                            // Notify the web layer of the raw FIX message sent back to the client
                            bus.publish(WsEvent::FixMessage {
                                label,
                                body,
                                tag: "feed".into(),
                            });
                            tracing::debug!("Response from FIX engine sent back to client");
                            break; // response sent, go back to reading from the client
                        } else {
                            continue; // no more responses, go back to reading from the client
                        }
                    }
                }
                Err(_) => {
                    tracing::error!("Error reading from client");
                    break
                },
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