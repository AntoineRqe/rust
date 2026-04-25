/// Persistent per-player FIX TCP session manager.
///
/// Each player gets exactly one long-lived TCP connection to the FIX engine.
/// The background reader thread lives independently of any WebSocket connection:
/// it continuously applies execution reports to the player store even when the
/// browser is offline, and publishes them to the event bus for display when the
/// browser reconnects.
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::players::PlayerStore;
use crate::state::{EventBus, WsEvent};
use utils::market_name;

// ── Session ───────────────────────────────────────────────────────────────────

pub struct FIXSession {
    /// Write-side of the TCP connection (used for sending FIX orders).
    pub writer: Arc<Mutex<TcpStream>>,
    /// Set to `true` to tear down the session (e.g. on server shutdown).
    pub stop: Arc<AtomicBool>,
}

// ── Manager ───────────────────────────────────────────────────────────────────

/// Thread-safe registry of live FIX sessions, keyed by username.
#[derive(Clone)]
pub struct FIXSessionManager {
    inner: Arc<Mutex<HashMap<String, FIXSession>>>,
}

impl FIXSessionManager {
    pub fn new() -> Self {
        FIXSessionManager {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Return the write-half of the existing FIX session for `username`, or
    /// create a new one by connecting to `fix_tcp_addr`.
    ///
    /// The background reader thread is spawned automatically and handles:
    ///
    /// - Calling `player_store.apply_fix_execution_report()` for every
    ///   execution report received, regardless of WebSocket state.
    /// - Publishing every received message to the event bus so connected
    ///   browser clients can display it.
    ///
    /// Returns `Err` if a new TCP connection could not be established.
    pub fn get_or_create_session(
        &self,
        username: &str,
        fix_tcp_addr: &str,
        player_store: &PlayerStore,
        bus: &EventBus,
    ) -> Result<Arc<Mutex<TcpStream>>, std::io::Error> {
        let mut sessions = self.inner.lock().unwrap();

        if let Some(session) = sessions.get(username) {
            tracing::debug!(
                "[{}] Reusing existing FIX session for '{username}'",
                market_name()
            );
            return Ok(Arc::clone(&session.writer));
        }

        let stream = TcpStream::connect(fix_tcp_addr)?;
        stream.set_read_timeout(Some(std::time::Duration::from_millis(100)))?;

        let writer = Arc::new(Mutex::new(stream.try_clone()?));
        let stop = Arc::new(AtomicBool::new(false));

        {
            let username = username.to_string();
            let stop_flag = Arc::clone(&stop);
            let player_store = player_store.clone();
            let bus = bus.clone();
            let sessions_ref = Arc::clone(&self.inner);

            // Spawn the background reader thread for this session.
            std::thread::spawn(move || {
                let mut tcp_reader = stream;
                let mut buf = [0u8; 4096];
                let mut stream_buf: Vec<u8> = Vec::with_capacity(8192);

                tracing::info!(
                    "[{}] FIX session reader started for '{username}'",
                    market_name()
                );

                while !stop_flag.load(Ordering::Relaxed) {
                    match tcp_reader.read(&mut buf) {
                        Ok(0) => {
                            tracing::info!(
                                "[{}] FIX session closed by engine for '{username}'",
                                market_name()
                            );
                            break;
                        }
                        Ok(n) => {
                            stream_buf.extend_from_slice(&buf[..n]);
                            for raw_msg in extract_fix_messages(&mut stream_buf) {
                                let body = pretty_fix(&raw_msg);

                                // Always update the portfolio DB, even when browser is offline.
                                player_store.apply_fix_execution_report(&body);

                                // Broadcast so connected browser clients can display it.
                                bus.publish(WsEvent::FixMessage {
                                    label: classify_fix_msg(&raw_msg),
                                    body,
                                    tag: "feed".into(),
                                    recipient: Some(username.clone()),
                                });
                            }
                        }
                        Err(e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                            ) => {}
                        Err(e) => {
                            tracing::warn!(
                                "[{}] FIX TCP read error for '{username}': {e}",
                                market_name()
                            );
                            bus.publish(WsEvent::FixMessage {
                                label: "ERROR".into(),
                                body: format!("FIX TCP connection lost: {e}"),
                                tag: "err".into(),
                                recipient: Some(username.clone()),
                            });
                            break;
                        }
                    }
                }

                sessions_ref.lock().unwrap().remove(&username);
                tracing::debug!(
                    "[{}] FIX session reader exited for '{username}'",
                    market_name()
                );
            });
        }

        tracing::info!(
            "[{}] New FIX session created for '{username}'",
            market_name()
        );
        sessions.insert(
            username.to_string(),
            FIXSession { writer: Arc::clone(&writer), stop },
        );
        Ok(writer)
    }

    /// Gracefully tear down all active sessions (called on server shutdown).
    pub fn shutdown_all(&self) {
        let sessions = self.inner.lock().unwrap();
        for (username, session) in sessions.iter() {
            session.stop.store(true, Ordering::Relaxed);
            if let Ok(stream) = session.writer.lock() {
                let _ = stream.shutdown(Shutdown::Both);
            }
            tracing::debug!(
                "[{}] FIX session stopped for '{username}' (shutdown)",
                market_name()
            );
        }
    }
}

// ── FIX helpers ───────────────────────────────────────────────────────────────

/// Extract complete `8=FIX...` messages from a stream buffer, consuming them
/// in place.
pub fn extract_fix_messages(stream_buf: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut out = Vec::new();

    loop {
        let Some(start) = find_subslice(stream_buf, b"8=FIX.") else {
            if stream_buf.len() > 6 {
                let keep_from = stream_buf.len() - 6;
                stream_buf.drain(..keep_from);
            }
            break;
        };

        if start > 0 {
            stream_buf.drain(..start);
        }

        let Some(checksum_start) = find_checksum_field(stream_buf) else {
            break;
        };

        let frame_end = checksum_start + 7; // "10=" + 3 digits + SOH
        if stream_buf.len() < frame_end {
            break;
        }

        out.push(stream_buf[..frame_end].to_vec());
        stream_buf.drain(..frame_end);
    }

    out
}

pub fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

pub fn find_checksum_field(data: &[u8]) -> Option<usize> {
    if data.len() < 7 {
        return None;
    }
    for i in 0..=data.len() - 7 {
        if (i == 0 || data[i - 1] == b'\x01')
            && data[i] == b'1'
            && data[i + 1] == b'0'
            && data[i + 2] == b'='
            && data[i + 3].is_ascii_digit()
            && data[i + 4].is_ascii_digit()
            && data[i + 5].is_ascii_digit()
            && data[i + 6] == b'\x01'
        {
            return Some(i);
        }
    }
    None
}

/// Pipe-delimited human-readable FIX string.
pub fn pretty_fix(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).replace('\x01', " │ ")
}

/// Short label for a FIX message.
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
        t   => format!("◀ MSG ({t})"),
    }
}

/// Send raw bytes over the TCP writer.
pub fn send_fix_over_tcp(
    tcp_writer: &Arc<Mutex<TcpStream>>,
    bytes: &[u8],
) -> std::io::Result<()> {
    let mut stream = tcp_writer.lock().unwrap();
    stream.write_all(bytes)
}
