use axum::{
    extract::{ws::{WebSocket, Message}, WebSocketUpgrade, State, Query, ConnectInfo},
    response::IntoResponse,
    http::StatusCode,
};
use futures::{sink::SinkExt, stream::StreamExt};
use std::net::{TcpStream, SocketAddr};
use std::sync::{Arc, Mutex};
use serde::Deserialize;
use crate::server::{AppState, SessionInfo, authenticate_token};
use crate::state::{WsEvent, BrowserCommand, PendingOrder};
use crate::fix_session::{pretty_fix, send_fix_over_tcp};
use utils::market_name;

struct VisitorCounterGuard {
    counter: Arc<std::sync::atomic::AtomicUsize>,
    total_counter: Arc<std::sync::atomic::AtomicUsize>,
    bus: crate::state::EventBus,
}

impl Drop for VisitorCounterGuard {
    fn drop(&mut self) {
        let after = self
            .counter
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed)
            .saturating_sub(1);
        let total = self.total_counter.load(std::sync::atomic::Ordering::Relaxed);
        self.bus.publish(WsEvent::VisitorCount { count: after, total_count: total });
    }
}

fn is_expected_ws_disconnect(err_text: &str) -> bool {
    let text = err_text.to_ascii_lowercase();
    text.contains("without closing handshake")
        || text.contains("connection reset")
        || text.contains("broken pipe")
        || text.contains("connection closed")
}

#[derive(Deserialize)]
pub struct WsParams {
    token: Option<String>,
}

/// WebSocket handler for incoming browser connections. Authenticates the session token, sets up the FIX session, and enters a loop to handle messages in both directions.
/// Arguments:
/// - `ws`: The WebSocket upgrade request from Axum.
/// - `state`: Shared application state containing the event bus, player store, FIX session manager, and order book.
/// - `params`: Query parameters from the WebSocket connection URL, expected to contain a `token` for authentication.
/// - `addr`: The client's socket address, used for logging and recording connection info in the player store.
/// Returns:
/// - An HTTP response that upgrades to a WebSocket connection if authentication succeeds, or an error
pub async fn ws_handler(
    ws:           WebSocketUpgrade,
    State(state): State<AppState>,
    Query(params): Query<WsParams>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    let token = params.token.unwrap_or_default();
    let Some(session) = authenticate_token(&state, &token) else {
        return (StatusCode::UNAUTHORIZED, "Invalid or missing session token").into_response();
    };

    ws.on_upgrade(move |socket| handle_socket(socket, state, session, addr))
}

/// Main loop for handling a WebSocket connection with a browser client. Listens for events from the FIX engine and messages from the browser, and routes them appropriately.
/// Arguments:
/// - `socket`: The WebSocket connection to the browser client.
/// - `state`: Shared application state containing the event bus, player store, FIX session manager, and order book.
/// - `session`: Authenticated session information for this connection, including username and admin status.
/// - `addr`: The client's socket address, used for logging and recording connection info in the player store.
async fn handle_socket(socket: WebSocket, state: AppState, session: SessionInfo, addr: SocketAddr) {
    // Split into sender and receiver so we can use both concurrently
    let (mut sender, mut receiver) = socket.split();
    let mut rx: tokio::sync::broadcast::Receiver<WsEvent> = state.bus.subscribe();
    let username = session.username.clone();
    let is_admin = session.is_admin;

    let current_visitors = state
        .active_visitors
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .saturating_add(1);
    let all_time_visitors = state.player_store.record_visit();
    // Sync the in-memory AtomicUsize with the newly persisted total
    state.total_visitors.store(all_time_visitors as usize, std::sync::atomic::Ordering::Relaxed);

    // Publish the updated visitor count to all connected clients so they see the new count immediately.
    state
        .bus
        .publish(WsEvent::VisitorCount { count: current_visitors, total_count: all_time_visitors as usize });

    let _visitor_guard = VisitorCounterGuard {
        counter: Arc::clone(&state.active_visitors),
        total_counter: Arc::clone(&state.total_visitors),
        bus: state.bus.clone(),
    };

    // Record the connection in the player store with the client's IP address. This is used for analytics and can be displayed in the admin dashboard.
    state
        .player_store
        .record_connection(&username, Some(&addr.ip().to_string()));

    // Get or create a persistent FIX session for this player.
    // The session thread lives beyond this WebSocket connection, so fills are
    // processed and persisted to the portfolio DB even when offline.
    let tcp_writer = match state.fix_session_manager.get_or_create_session(
        &username,
        &state.fix_tcp_addr,
        &state.player_store,
        &state.bus,
    ) {
        Ok(writer) => writer,
        Err(e) => {
            tracing::warn!("[{}] FIX session connect failed for '{username}': {e}", market_name());
            let err = serde_json::to_string(&WsEvent::FixMessage {
                label: "ERROR".into(),
                body: format!("Unable to connect to FIX TCP server: {e}"),
                tag: "err".into(),
                recipient: Some(username.clone()),
            }).unwrap();
            let _ = sender.send(Message::Text(err.into())).await;
            let status = serde_json::to_string(&WsEvent::Status { connected: false }).unwrap();
            let _ = sender.send(Message::Text(status.into())).await;
            return;
        }
    };

    // Browser connected — tell it the FIX engine is ready
    // The web server IS the FIX gateway now, no separate TCP client needed
    state.bus.publish(WsEvent::Status { connected: true });

    // Also send it directly to this socket immediately
    // (the broadcast above goes to all OTHER subscribers,
    //  but this socket just subscribed so it missed it)
    let status = serde_json::to_string(&WsEvent::Status { connected: true }).unwrap();
    let _ = sender.send(Message::Text(status.into())).await;

    // Send the player's current state (tokens + pending orders) on every new connection.
    send_player_state(&mut sender, &state, &username, is_admin).await;

    // Send current order book state for all symbols
    send_order_book_snapshots(&mut sender, &state).await;

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

                        if let WsEvent::FixMessage { tag, .. } = &event {
                            if tag == "feed" {
                                // Refresh this socket's player state so the browser
                                // sees up-to-date tokens/pending orders.
                                // Portfolio DB update already happened in the FIX session thread.
                                send_player_state(&mut sender, &state, &username, is_admin).await;
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
                        // Handle the browser command, which may involve sending FIX messages over TCP to the engine. The FIX session thread will process those messages and publish events back to the bus, which will then be sent to this browser as needed.
                        handle_browser_message(&text, &state, &username, is_admin, &tcp_writer).await;
                        // After every command, push the updated player state to this client.
                        send_player_state(&mut sender, &state, &username, is_admin).await;
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        tracing::debug!("[{}] Browser disconnected", market_name());
                        break;
                    }
                    Some(Err(e)) => {
                        let err_text = e.to_string();
                        if is_expected_ws_disconnect(&err_text) {
                            tracing::debug!("[{}] WebSocket disconnected: {}", market_name(), err_text);
                        } else {
                            tracing::warn!("[{}] WebSocket error: {}", market_name(), err_text);
                        }
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    // WebSocket disconnected — FIX session stays alive for offline portfolio updates.
    state.bus.publish(WsEvent::Status { connected: false });
}

/// Send player to the browser so it can display the current token balance, pending orders, and holdings.
/// This is called on every new connection and after every relevant event (like order updates or executions) to keep the browser in sync with the latest state.
async fn send_player_state(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    state: &AppState,
    username: &str,
    is_admin: bool,
) {
    let event = if let Some(player) = state.player_store.get_player(username) {
        let id_suffix = player.password.chars().rev().take(4).collect::<String>().chars().rev().collect::<String>();
        WsEvent::PlayerState {
            username: player.username.clone(),
            tokens: player.tokens,
            pending_orders: player.pending_orders.clone(),
            holdings: state.player_store.get_holdings_summary(&player.username),
            order_owners: state.player_store.get_order_owners(),
            is_admin,
            visitor_count: state.active_visitors.load(std::sync::atomic::Ordering::Relaxed),
            total_visitor_count: state.total_visitors.load(std::sync::atomic::Ordering::Relaxed),
            id_suffix,
        }
    } else {
        WsEvent::PlayerState {
            username: username.to_string(),
            tokens: 0.0,
            pending_orders: Vec::new(),
            holdings: std::collections::HashMap::new(),
            order_owners: state.player_store.get_order_owners(),
            is_admin,
            visitor_count: state.active_visitors.load(std::sync::atomic::Ordering::Relaxed),
            total_visitor_count: state.total_visitors.load(std::sync::atomic::Ordering::Relaxed),
            id_suffix: String::new(),
        }
    };

    let json = serde_json::to_string(&event).unwrap();
    let _ = sender.send(Message::Text(json.into())).await;
}

/// Send the full order book snapshots to the browser client. This is called on every new connection and after every relevant event (like order updates or executions) to keep the browser in sync with the latest state.
async fn send_order_book_snapshots(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    state: &AppState,
) {
    // Clone the books data so we don't hold the lock
    let (books, total_bid_levels, total_ask_levels) = {
        let ob = state.order_book.lock().unwrap();
        let total_bid_levels: usize = ob.books.values().map(|book| book.bids.len()).sum();
        let total_ask_levels: usize = ob.books.values().map(|book| book.asks.len()).sum();

        tracing::info!(
            "[{}] Order book summary before WS snapshot: symbols={}, bid_levels={}, ask_levels={}",
            market_name(),
            ob.books.len(),
            total_bid_levels,
            total_ask_levels
        );

        (ob.books.clone(), total_bid_levels, total_ask_levels)
    };

    if books.is_empty() {
        tracing::warn!("[{}] Order book is empty at client connection", market_name());
    } else if total_bid_levels == 0 && total_ask_levels == 0 {
        tracing::warn!("[{}] Order book has symbols but no price levels at client connection", market_name());
    }

    for (symbol, book) in books {
        tracing::debug!("[{}] Sending {} bids and {} asks for {}", market_name(), book.bids.len(), book.asks.len(), symbol);
        let event = WsEvent::OrderBook {
            symbol,
            bids: book.l3_bids_sorted(),
            asks: book.l3_asks_sorted(),
            timestamp_ms: book.last_update_ms,
        };

        // Send the full order book snapshot for this symbol to the client.
        // The client will use this to populate the initial state of the order book display, and then rely on incremental updates from the FIX engine to keep it up-to-date.
        let json = serde_json::to_string(&event).unwrap();
        let _ = sender.send(Message::Text(json.into())).await;
    }
}

fn require_admin(state: &AppState, username: &str, is_admin: bool) -> bool {
    if is_admin {
        return true;
    }

    state.bus.publish(WsEvent::FixMessage {
        label: "ADMIN ONLY".into(),
        body: "This action is reserved to the admin user.".into(),
        tag: "err".into(),
        recipient: Some(username.to_string()),
    });
    false
}

/// Handle a command received from the browser client. This may involve sending FIX messages over TCP to the engine, which will then process them and publish events back to the bus.
async fn handle_browser_message(text: &str, state: &AppState, username: &str, is_admin: bool, tcp_writer: &Arc<Mutex<TcpStream>>) {
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

            if side == "1" {
                let required_notional = qty * price;
                if required_notional.is_finite() && required_notional > 0.0 {
                    if let Some(player) = state.player_store.get_player(username) {
                        // Calculate the notional value of the player's existing pending BUY orders to determine how many tokens are currently reserved.
                        // This ensures that the player cannot exceed their token balance by placing multiple pending orders.
                        let reserved_notional: f64 = player
                            .pending_orders
                            .iter()
                            .filter(|order| order.side == "1")
                            .map(|order| order.qty * order.price)
                            .sum();

                        let available_tokens = player.tokens - reserved_notional;
                        if available_tokens + 1e-9 < required_notional {
                            state.bus.publish(WsEvent::FixMessage {
                                label: "REJECTED ✕".into(),
                                body: format!(
                                    "Insufficient tokens for BUY order: required {:.2}, available {:.2}.",
                                    required_notional,
                                    available_tokens.max(0.0)
                                ),
                                tag: "err".into(),
                                recipient: Some(username.to_string()),
                            });
                            return;
                        }
                    }
                }
            } else if side == "2" {
                if qty.is_finite() && qty > 0.0 {
                    // Admin bypass: admins can generate SELL orders without owning inventory
                    if !is_admin {
                        if let Some(player) = state.player_store.get_player(username) {
                            let normalized_symbol = symbol.to_uppercase();
                            let holdings = state.player_store.get_holdings_summary(username);
                            let owned_qty = holdings
                                .get(&normalized_symbol)
                                .map(|holding| holding.quantity)
                                .unwrap_or(0.0);
                            let reserved_sell_qty: f64 = player
                                .pending_orders
                                .iter()
                                .filter(|order| {
                                    order.side == "2" && order.symbol.eq_ignore_ascii_case(&normalized_symbol)
                                })
                                .map(|order| order.qty)
                                .sum();

                            let available_qty = (owned_qty - reserved_sell_qty).max(0.0);
                            if available_qty + 1e-9 < qty {
                                state.bus.publish(WsEvent::FixMessage {
                                    label: "REJECTED ✕".into(),
                                    body: format!(
                                        "Insufficient equity inventory for SELL {}: required {:.0}, available {:.0}.",
                                        normalized_symbol,
                                        qty,
                                        available_qty
                                    ),
                                    tag: "err".into(),
                                    recipient: Some(username.to_string()),
                                });
                                return;
                            }
                        }
                    }
                }
            }

            let sender_id = sender
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("BROWSER"))
                .unwrap_or_else(|| username.to_string());
            let target_id = target
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "SERVER1".into());

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
                username, "SERVER1",
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
            if !require_admin(state, username, is_admin) {
                return;
            }
            // We'll handle sequence numbers in the FIX engine
            // For now just ack it
            state.bus.publish(WsEvent::FixMessage {
                label: "INFO".into(),
                body:  "Sequence number reset.".into(),
                tag:   "info".into(),
                recipient: Some(username.to_string()),
            });
        }

        BrowserCommand::ClearBook => {
            if !require_admin(state, username, is_admin) {
                return;
            }
            use grpc::proto::market_control_client::MarketControlClient;
            use grpc::proto::ResetRequest;
            match MarketControlClient::connect(state.grpc_addr.clone()).await {
                Ok(mut client) => {
                    match client.reset_market(ResetRequest {}).await {
                        Ok(resp) => {
                            let r = resp.into_inner();
                            let (tag, label, body, recipient) = if r.success {
                                let (players_touched, orders_removed) = state.player_store.reset_market_state();
                                let cleared_symbols = {
                                    let mut order_book = state.order_book.lock().unwrap();
                                    let symbols: Vec<String> = order_book.books.keys().cloned().collect();
                                    order_book.books.clear();
                                    symbols
                                };

                                let timestamp_ms = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|duration| duration.as_millis() as u64)
                                    .unwrap_or(0);

                                for symbol in cleared_symbols {
                                    state.bus.publish(WsEvent::OrderBook {
                                        symbol,
                                        bids: Vec::new(),
                                        asks: Vec::new(),
                                        timestamp_ms,
                                    });
                                }

                                (
                                    "info",
                                    "RESET ✓  Order book and database cleared.".to_string(),
                                    format!(
                                        "{} Player state cleared: {} pending order(s) removed across {} player(s).",
                                        r.message,
                                        orders_removed,
                                        players_touched
                                    ),
                                    None,
                                )
                            } else {
                                (
                                    "err",
                                    format!("RESET FAILED: {}", r.message),
                                    r.message,
                                    Some(username.to_string()),
                                )
                            };
                            state.bus.publish(WsEvent::FixMessage {
                                label,
                                body,
                                tag: tag.into(),
                                recipient,
                            });
                        }
                        Err(e) => {
                            state.bus.publish(WsEvent::FixMessage {
                                label: "ERROR".into(),
                                body: format!("gRPC ResetMarket call failed: {e}"),
                                tag: "err".into(),
                                recipient: Some(username.to_string()),
                            });
                        }
                    }
                }
                Err(e) => {
                    state.bus.publish(WsEvent::FixMessage {
                        label: "ERROR".into(),
                        body: format!("Failed to connect to gRPC server: {e}"),
                        tag: "err".into(),
                        recipient: Some(username.to_string()),
                    });
                }
            }
        }

        BrowserCommand::ResetTokens => {
            if !require_admin(state, username, is_admin) {
                return;
            }

            let players_reset = state.player_store.reset_all_tokens();
            if players_reset > 0 {
                state.bus.publish(WsEvent::FixMessage {
                    label: "INFO".into(),
                    body: format!("Reset token balances for {players_reset} player(s)."),
                    tag: "info".into(),
                    recipient: None,
                });
            } else {
                state.bus.publish(WsEvent::FixMessage {
                    label: "ERROR".into(),
                    body: "Unable to reset tokens: no players found.".into(),
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

// ── FIX message builders ──────────────────────────────────────────────────────

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