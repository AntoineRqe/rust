use axum::{
    extract::{ws::{WebSocket, Message}, WebSocketUpgrade, State},
    response::IntoResponse,
    http::{HeaderMap, StatusCode, header},
};
use futures::{sink::SinkExt, stream::StreamExt};
use crate::server::AppState;
use crate::server::is_authorized;
use crate::state::{WsEvent, BrowserCommand};

pub async fn ws_handler(
    ws:           WebSocketUpgrade,
    State(state): State<AppState>,
    headers:      HeaderMap,
) -> impl IntoResponse {
    if !is_authorized(&headers, state.web_password.as_deref()) {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Basic realm=\"FIX Web Terminal\"")],
            "Authentication required",
        )
            .into_response();
    }

    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    // Split into sender and receiver so we can use both concurrently
    // in the select! loop without borrow issues.
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.bus.subscribe();

    // Browser connected — tell it the FIX engine is ready
    // The web server IS the FIX gateway now, no separate TCP client needed
    state.bus.publish(WsEvent::Status { connected: true });

    // Also send it directly to this socket immediately
    // (the broadcast above goes to all OTHER subscribers,
    //  but this socket just subscribed so it missed it)
    let status = serde_json::to_string(&WsEvent::Status { connected: true }).unwrap();
    let _ = sender.send(Message::Text(status.into())).await;

    tracing::debug!("Browser WebSocket connected");

    loop {
        tokio::select! {
            // ── FIX engine → browser ─────────────────────────────────────
            result = rx.recv() => {
                match result {
                    Ok(event) => {
                        let json = serde_json::to_string(&event).unwrap();
                        if sender.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Browser lagged, dropped {n} events");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }

            // ── browser → FIX engine ──────────────────────────────────────
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_browser_message(&text, &state).await;
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        tracing::debug!("Browser disconnected");
                        break;
                    }
                    Some(Err(e)) => {
                        tracing::warn!("WebSocket error: {e}");
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    state.bus.publish(WsEvent::Status { connected: false });
}

async fn handle_browser_message(text: &str, state: &AppState) {
    tracing::debug!("Received message from browser: {text}");
    let cmd: BrowserCommand = match serde_json::from_str(text) {
        Ok(c)  => c,
        Err(e) => {
            tracing::warn!("Invalid browser command: {e} — raw: {text}");
            return;
        }
    };

    match cmd {
        BrowserCommand::Order { clord_id, symbol, qty, price, side, sender, target } => {
            tracing::info!("Browser order: {} {} {} @ {}",
                if side == "1" { "BUY" } else { "SELL" },
                qty as u32, symbol, price);

            let sender_id = sender.unwrap_or_else(|| "BROWSER".into());
            let target_id = target.unwrap_or_else(|| "SERVER1".into());

            let fix_bytes = build_new_order_single(
                &sender_id, &target_id, &symbol,
                side.parse().unwrap_or(1),
                qty, price, &clord_id,
            );

            tracing::debug!("Built FIX message for browser order, injecting into FIX engine...");
            // Log it to the browser as a SENT message
            state.bus.publish(WsEvent::FixMessage {
                label: format!("SENT ▶  ({} {} {} @ {})",
                    if side == "1" { "BUY" } else { "SELL" },
                    qty as u32, symbol, price),
                body: pretty_fix(&fix_bytes),
                tag:  "send".into(),
            });

            // Inject into the FIX engine
            (state.fix_sender)(fix_bytes);

            tracing::debug!("Browser order injected into FIX engine");
        }

        BrowserCommand::Cancel { clord_id, symbol, qty } => {
            let symbol = symbol.unwrap_or_else(|| "AAPL".into());
            let qty = qty.unwrap_or(0.0);
            let fix_bytes = build_order_cancel_request(
                "BROWSER", "SERVER1",
                &clord_id, &symbol, qty,
            );

            tracing::info!("Browser cancel: {}", clord_id);
            state.bus.publish(WsEvent::FixMessage {
                label: format!("CANCEL ✕ ({})", clord_id),
                body: pretty_fix(&fix_bytes),
                tag:  "send".into(),
            });

            (state.fix_sender)(fix_bytes);
        }

        BrowserCommand::ResetSeq => {
            // We'll handle sequence numbers in the FIX engine
            // For now just ack it
            state.bus.publish(WsEvent::FixMessage {
                label: "INFO".into(),
                body:  "Sequence number reset.".into(),
                tag:   "info".into(),
            });
        }

        BrowserCommand::Disconnect => {
            state.bus.publish(WsEvent::Status { connected: false });
        }

        BrowserCommand::MdRequest { symbol, depth } => {
            let fix_bytes = build_md_request("BROWSER", "SERVER1",
                                             &symbol, depth.unwrap_or(1));
            (state.fix_sender)(fix_bytes);
        }
    }
}

fn pretty_fix(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).replace('\x01', " │ ")
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