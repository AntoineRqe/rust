use axum::{
    extract::{ws::{WebSocket, Message}, WebSocketUpgrade, State},
    response::IntoResponse,
    http::{HeaderMap, StatusCode, header},
};
use futures::{sink::SinkExt, stream::StreamExt};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use crate::server::AppState;
use crate::server::authenticate;
use crate::state::{WsEvent, BrowserCommand, PendingOrder};

fn market_name() -> &'static str {
    static MARKET_NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    MARKET_NAME
        .get_or_init(|| std::env::var("MARKET_NAME").unwrap_or_else(|_| "unknown".to_string()))
        .as_str()
}

pub async fn ws_handler(
    ws:           WebSocketUpgrade,
    State(state): State<AppState>,
    headers:      HeaderMap,
) -> impl IntoResponse {
    let Some(username) = authenticate(&headers, &state.player_store) else {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Basic realm=\"Market Simulator\"")],
            "Authentication required",
        )
            .into_response();
    };

    ws.on_upgrade(move |socket| handle_socket(socket, state, username))
}

async fn handle_socket(socket: WebSocket, state: AppState, username: String) {
    // Split into sender and receiver so we can use both concurrently
    // in the select! loop without borrow issues.
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.bus.subscribe();

    let tcp_stream = match TcpStream::connect(&state.fix_tcp_addr) {
        Ok(stream) => stream,
        Err(e) => {
            tracing::warn!("[{}] Browser FIX TCP connect failed: {e}", market_name());
            let err = serde_json::to_string(&WsEvent::FixMessage {
                label: "ERROR".into(),
                body: format!("Unable to connect to FIX TCP server: {e}"),
                tag: "err".into(),
                recipient: Some(username.clone()),
            }).unwrap();
            let _ = sender.send(Message::Text(err.into())).await;
            return;
        }
    };

    let _ = tcp_stream.set_nodelay(true);
    let _ = tcp_stream.set_read_timeout(Some(std::time::Duration::from_millis(200)));
    let _ = tcp_stream.set_write_timeout(Some(std::time::Duration::from_secs(2)));

    let tcp_reader = match tcp_stream.try_clone() {
        Ok(stream) => stream,
        Err(e) => {
            tracing::warn!("[{}] Browser FIX TCP clone failed: {e}", market_name());
            let err = serde_json::to_string(&WsEvent::FixMessage {
                label: "ERROR".into(),
                body: format!("Unable to initialize FIX TCP session: {e}"),
                tag: "err".into(),
                recipient: Some(username.clone()),
            }).unwrap();
            let _ = sender.send(Message::Text(err.into())).await;
            return;
        }
    };

    let tcp_writer = Arc::new(Mutex::new(tcp_stream));
    let tcp_stop = Arc::new(AtomicBool::new(false));

    {
        let bus = state.bus.clone();
        let username = username.clone();
        let tcp_stop = Arc::clone(&tcp_stop);
        std::thread::spawn(move || {
            let mut stream = tcp_reader;
            let mut buf = [0u8; 4096];

            while !tcp_stop.load(Ordering::Relaxed) {
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        bus.publish(WsEvent::FixMessage {
                            label: classify_fix_msg(&buf[..n]),
                            body: pretty_fix(&buf[..n]),
                            tag: "feed".into(),
                            recipient: Some(username.clone()),
                        });
                    }
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {
                        continue;
                    }
                    Err(e) => {
                        bus.publish(WsEvent::FixMessage {
                            label: "ERROR".into(),
                            body: format!("FIX TCP read error: {e}"),
                            tag: "err".into(),
                            recipient: Some(username.clone()),
                        });
                        break;
                    }
                }
            }
        });
    }

    // Browser connected — tell it the FIX engine is ready
    // The web server IS the FIX gateway now, no separate TCP client needed
    state.bus.publish(WsEvent::Status { connected: true });

    // Also send it directly to this socket immediately
    // (the broadcast above goes to all OTHER subscribers,
    //  but this socket just subscribed so it missed it)
    let status = serde_json::to_string(&WsEvent::Status { connected: true }).unwrap();
    let _ = sender.send(Message::Text(status.into())).await;

    // Send the player's current state (tokens + pending orders) on every new connection.
    if let Some(player) = state.player_store.get_player(&username) {
        let id_suffix = player.password.chars().rev().take(4).collect::<String>().chars().rev().collect::<String>();
        let ps = serde_json::to_string(&WsEvent::PlayerState {
            username: player.username.clone(),
            tokens:   player.tokens,
            pending_orders: player.pending_orders.clone(),
            id_suffix,
        }).unwrap();
        let _ = sender.send(Message::Text(ps.into())).await;
    }

    tracing::debug!("[{}] Browser WebSocket connected", market_name());

    loop {
        tokio::select! {
            // ── FIX engine → browser ─────────────────────────────────────
            result = rx.recv() => {
                match result {
                    Ok(event) => {
                        let allow_event = match &event {
                            WsEvent::FixMessage { recipient: Some(recipient), .. } => recipient == &username,
                            _ => true,
                        };

                        if !allow_event {
                            continue;
                        }

                        if let WsEvent::FixMessage { tag, body, .. } = &event {
                            if tag == "feed" {
                                // Apply once globally (deduped inside PlayerStore), then
                                // always refresh this socket's player state so every client
                                // sees up-to-date tokens/pending orders.
                                state.player_store.apply_fix_execution_report(body);
                                if let Some(player) = state.player_store.get_player(&username) {
                                    let id_suffix = player.password.chars().rev().take(4).collect::<String>().chars().rev().collect::<String>();
                                    let ps = serde_json::to_string(&WsEvent::PlayerState {
                                        username: player.username.clone(),
                                        tokens:   player.tokens,
                                        pending_orders: player.pending_orders.clone(),
                                        id_suffix,
                                    }).unwrap();
                                    let _ = sender.send(Message::Text(ps.into())).await;
                                }
                            }
                        }

                        let json = serde_json::to_string(&event).unwrap();
                        if sender.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("[{}] Browser lagged, dropped {n} events", market_name());
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }

            // ── browser → FIX engine ──────────────────────────────────────
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_browser_message(&text, &state, &username, &tcp_writer).await;
                        // After every command, push the updated player state to this client.
                        if let Some(player) = state.player_store.get_player(&username) {
                            let id_suffix = player.password.chars().rev().take(4).collect::<String>().chars().rev().collect::<String>();
                            let ps = serde_json::to_string(&WsEvent::PlayerState {
                                username: player.username.clone(),
                                tokens:   player.tokens,
                                pending_orders: player.pending_orders.clone(),
                                id_suffix,
                            }).unwrap();
                            let _ = sender.send(Message::Text(ps.into())).await;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        tracing::debug!("[{}] Browser disconnected", market_name());
                        break;
                    }
                    Some(Err(e)) => {
                        tracing::warn!("[{}] WebSocket error: {e}", market_name());
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    tcp_stop.store(true, Ordering::Relaxed);
    if let Ok(stream) = tcp_writer.lock() {
        let _ = stream.shutdown(Shutdown::Both);
    }

    state.bus.publish(WsEvent::Status { connected: false });
}

async fn handle_browser_message(text: &str, state: &AppState, username: &str, tcp_writer: &Arc<Mutex<TcpStream>>) {
    tracing::debug!("[{}] Received message from browser: {text}", market_name());
    let cmd: BrowserCommand = match serde_json::from_str(text) {
        Ok(c)  => c,
        Err(e) => {
            tracing::warn!("[{}] Invalid browser command: {e} — raw: {text}", market_name());
            return;
        }
    };

    match cmd {
        BrowserCommand::Order { clord_id, symbol, qty, price, side, sender, target } => {
            tracing::info!("[{}] Browser order: {} {} {} @ {}",
                market_name(),
                if side == "1" { "BUY" } else { "SELL" },
                qty as u32, symbol, price);

            let sender_id = sender.unwrap_or_else(|| "BROWSER".into());
            let target_id = target.unwrap_or_else(|| "SERVER1".into());

            // Record the order in the player's pending list.
            // Token balance is updated only on transaction (execution reports).
            state.player_store.add_pending_order(username, PendingOrder {
                cl_ord_id: clord_id.clone(),
                symbol: symbol.clone(),
                side: side.clone(),
                qty,
                price,
            });

            let fix_bytes = build_new_order_single(
                &sender_id, &target_id, &symbol,
                side.parse().unwrap_or(1),
                qty, price, &clord_id,
            );

            tracing::debug!("[{}] Built FIX message for browser order, injecting into FIX engine...", market_name());
            // Log it to the browser as a SENT message
            state.bus.publish(WsEvent::FixMessage {
                label: format!("SENT ▶  ({} {} {} @ {})",
                    if side == "1" { "BUY" } else { "SELL" },
                    qty as u32, symbol, price),
                body: pretty_fix(&fix_bytes),
                tag:  "send".into(),
                recipient: Some(username.to_string()),
            });

            if let Err(e) = send_fix_over_tcp(tcp_writer, &fix_bytes) {
                state.bus.publish(WsEvent::FixMessage {
                    label: "ERROR".into(),
                    body: format!("Unable to send FIX order over TCP: {e}"),
                    tag: "err".into(),
                    recipient: Some(username.to_string()),
                });
            }

            tracing::debug!("[{}] Browser order injected into FIX engine", market_name());
        }

        BrowserCommand::Cancel { clord_id, symbol, qty } => {
            let symbol = symbol.unwrap_or_else(|| "AAPL".into());
            let qty = qty.unwrap_or(0.0);

            // Drop the pending order. Token balance is not changed on cancel.
            state.player_store.remove_pending_order(username, &clord_id);

            let fix_bytes = build_order_cancel_request(
                "BROWSER", "SERVER1",
                &clord_id, &symbol, qty,
            );

            tracing::info!("[{}] Browser cancel: {}", market_name(), clord_id);
            state.bus.publish(WsEvent::FixMessage {
                label: format!("CANCEL ✕ ({})", clord_id),
                body: pretty_fix(&fix_bytes),
                tag:  "send".into(),
                recipient: Some(username.to_string()),
            });

            if let Err(e) = send_fix_over_tcp(tcp_writer, &fix_bytes) {
                state.bus.publish(WsEvent::FixMessage {
                    label: "ERROR".into(),
                    body: format!("Unable to send FIX cancel over TCP: {e}"),
                    tag: "err".into(),
                    recipient: Some(username.to_string()),
                });
            }
        }

        BrowserCommand::ResetSeq => {
            // We'll handle sequence numbers in the FIX engine
            // For now just ack it
            state.bus.publish(WsEvent::FixMessage {
                label: "INFO".into(),
                body:  "Sequence number reset.".into(),
                tag:   "info".into(),
                recipient: Some(username.to_string()),
            });
        }

        BrowserCommand::ResetTokens => {
            if state.player_store.reset_tokens(username) {
                state.bus.publish(WsEvent::FixMessage {
                    label: "INFO".into(),
                    body: "Player token balance reset to initial value.".into(),
                    tag: "info".into(),
                    recipient: Some(username.to_string()),
                });
            } else {
                state.bus.publish(WsEvent::FixMessage {
                    label: "ERROR".into(),
                    body: "Unable to reset tokens: player not found.".into(),
                    tag: "err".into(),
                    recipient: Some(username.to_string()),
                });
            }
        }

        BrowserCommand::Disconnect => {
            state.bus.publish(WsEvent::Status { connected: false });
        }

        BrowserCommand::MdRequest { symbol, depth } => {
            let fix_bytes = build_md_request("BROWSER", "SERVER1",
                                             &symbol, depth.unwrap_or(1));
            if let Err(e) = send_fix_over_tcp(tcp_writer, &fix_bytes) {
                state.bus.publish(WsEvent::FixMessage {
                    label: "ERROR".into(),
                    body: format!("Unable to send market data request over TCP: {e}"),
                    tag: "err".into(),
                    recipient: Some(username.to_string()),
                });
            }
        }
    }
}

fn send_fix_over_tcp(tcp_writer: &Arc<Mutex<TcpStream>>, bytes: &[u8]) -> std::io::Result<()> {
    let mut stream = tcp_writer.lock().unwrap();
    stream.write_all(bytes)
}

fn pretty_fix(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).replace('\x01', " │ ")
}

fn classify_fix_msg(raw: &[u8]) -> String {
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

// ── FIX message builders ──────────────────────────────────────────────────────
// These mirror your Python client's build functions, now in Rust.

static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
const SOH: char = '\x01';

fn next_seq() -> u32 {
    SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn fix_now() -> String {
    chrono::Utc::now().format("%Y%m%d-%H:%M:%S").to_string()
}

fn checksum(raw: &str) -> String {
    let sum: u32 = raw.bytes().map(|b| b as u32).sum();
    format!("{:03}", sum % 256)
}

fn wrap(body: String) -> Vec<u8> {
    let begin = format!("8=FIX.4.2{SOH}");
    let blen  = format!("9={}{SOH}", begin.len() + body.len());
    let raw   = format!("{begin}{blen}{body}");
    let chk   = checksum(&raw);
    format!("{raw}10={chk}{SOH}").into_bytes()
}

fn build_new_order_single(
    sender: &str, target: &str,
    symbol: &str, side: u8,
    qty: f64, price: f64,
    clord_id: &str,
) -> Vec<u8> {
    let now = fix_now();
    let body = format!(
        "35=D{SOH}49={sender}{SOH}56={target}{SOH}34={seq}{SOH}52={now}{SOH}\
         11={clord_id}{SOH}21=1{SOH}55={symbol}{SOH}54={side}{SOH}60={now}{SOH}\
         38={qty}{SOH}40=2{SOH}44={price:.4}{SOH}",
        seq = next_seq(),
        qty = qty as u32,
    );
    wrap(body)
}

fn build_md_request(sender: &str, target: &str, symbol: &str, depth: u32) -> Vec<u8> {
    let rid = format!("MDR-WEB-{}", next_seq());
    let now = fix_now();
    let body = format!(
        "35=V{SOH}49={sender}{SOH}56={target}{SOH}34={seq}{SOH}52={now}{SOH}\
         262={rid}{SOH}263=1{SOH}264={depth}{SOH}\
         267=2{SOH}269=0{SOH}269=1{SOH}146=1{SOH}55={symbol}{SOH}",
        seq = next_seq(),
    );
    wrap(body)
}

fn build_order_cancel_request(
    sender: &str, target: &str,
    orig_clord_id: &str, symbol: &str,
    qty: f64,
) -> Vec<u8> {
    let now = fix_now();
    let clord_id = format!("ORD-WEB-{}", next_seq());
    let body = format!(
        "35=F{SOH}49={sender}{SOH}56={target}{SOH}34={seq}{SOH}52={now}{SOH}\
         11={clord_id}{SOH}41={orig_clord_id}{SOH}55={symbol}{SOH}38={qty}{SOH}",
        seq = next_seq(),
        qty = qty as u32,
    );
    wrap(body)
}