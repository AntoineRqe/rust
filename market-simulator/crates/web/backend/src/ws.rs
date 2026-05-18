use crate::auth::require_admin;
use crate::fix_session::pretty_fix;
use crate::server::{AppState, Metrics};
use crate::state::{BrowserCommand, BusEvent, WsEvent};
use axum::{
    Extension,
    extract::{
        ConnectInfo, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::IntoResponse,
};
use futures::{sink::SinkExt, stream::StreamExt};
use players::players::PendingOrder;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use utils::market_name;

const PLAYER_STATE_FEED_DEBOUNCE_MS: u64 = 75;

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
        let total = self
            .total_counter
            .load(std::sync::atomic::Ordering::Relaxed);
        self.bus.publish(WsEvent::VisitorCount {
            count: after,
            total_count: total,
        });
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
    username: Option<String>,
}

/// WebSocket handler for incoming browser connections. Authenticates the token, sets up the FIX session, and enters a loop to handle messages in both directions.
/// Arguments:
/// - `ws`: The WebSocket upgrade request from Axum.
/// - `state`: Shared application state containing the event bus, player store, FIX session manager, and order book.
/// - `params`: Query parameters from the WebSocket connection URL, expected to contain `token` and `username` for authentication.
/// - `addr`: The client's socket address, used for logging and recording connection info in the player store.
/// Returns:
/// - An HTTP response that upgrades to a WebSocket connection if authentication succeeds, or an error
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Extension(metrics): Extension<Arc<Metrics>>,
    Query(params): Query<WsParams>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    let token = params.token.unwrap_or_default();
    let username = match params.username {
        Some(u) if !u.is_empty() => u,
        _ => return (StatusCode::UNAUTHORIZED, "Missing username parameter").into_response(),
    };

    if token.is_empty() {
        return (StatusCode::UNAUTHORIZED, "Missing token parameter").into_response();
    }

    ws.on_upgrade(move |socket| handle_socket(socket, state, metrics, username, token, addr))
}

/// Main loop for handling a WebSocket connection with a browser client. Listens for events from the FIX engine and messages from the browser, and routes them appropriately.
/// Arguments:
/// - `socket`: The WebSocket connection to the browser client.
/// - `state`: Shared application state containing the event bus, player store, FIX session manager, and order book.
/// - `username`: The authenticated username (sent by the browser after login).
/// - `token`: The bearer token from the Player Service (sent by the browser).
/// - `addr`: The client's socket address, used for logging and recording connection info in the player store.
async fn handle_socket(
    socket: WebSocket,
    state: AppState,
    metrics: Arc<Metrics>,
    username: String,
    token: String,
    _addr: SocketAddr,
) {
    // Validate JWT token before proceeding
    let claims = match players::validate_token(&token) {
        Ok(claims) => {
            // Verify that the token's username matches the query parameter username
            if claims.username != username {
                tracing::warn!(
                    "[{}] Token/username mismatch: token claims '{}' but got query param '{}'",
                    market_name(),
                    claims.username,
                    username
                );
                // Send error message and disconnect
                let mut sender = socket;
                let _ = sender
                    .send(Message::Text(
                        serde_json::to_string(&WsEvent::Status {
                            connected: false,
                            recipient: None,
                        })
                        .unwrap()
                        .into(),
                    ))
                    .await;
                return;
            }
            claims
        }
        Err(e) => {
            tracing::warn!("[{}] Invalid JWT token: {}", market_name(), e);
            // Send error message and disconnect
            let mut sender = socket;
            let _ = sender
                .send(Message::Text(
                    serde_json::to_string(&WsEvent::Status {
                        connected: false,
                        recipient: None,
                    })
                    .unwrap()
                    .into(),
                ))
                .await;
            return;
        }
    };

    // Split into sender and receiver so we can use both concurrently
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.bus.subscribe();
    let mut targeted_rx = state.bus.subscribe_targeted(&username);
    let ws_metrics = state.bus.metrics();
    let is_admin = claims.is_admin || username.eq_ignore_ascii_case("admin");

    let current_visitors = state
        .active_visitors
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .saturating_add(1);

    // TODO : Add latency by increasing hte counter in the upgrade loop, need to be moved on a cold path
    // Record the visit with the gRPC player service
    let all_time_visitors = state.player_client.lock().await.record_visit().await;
    // Sync the in-memory AtomicUsize with the newly persisted total
    state.total_visitors.store(
        all_time_visitors as usize,
        std::sync::atomic::Ordering::Relaxed,
    );

    // Publish the updated visitor count to all connected clients so they see the new count immediately.
    state.bus.publish(WsEvent::VisitorCount {
        count: current_visitors,
        total_count: all_time_visitors as usize,
    });

    let _visitor_guard = VisitorCounterGuard {
        counter: Arc::clone(&state.active_visitors),
        total_counter: Arc::clone(&state.total_visitors),
        bus: state.bus.clone(),
    };

    // Browser connected — tell this user that the FIX engine is ready.
    state.bus.publish(WsEvent::Status {
        connected: true,
        recipient: Some(username.clone()),
    });

    // Also send it directly to this socket immediately.
    // This keeps first-render UX deterministic even if targeted queue delivery is delayed.
    let status = serde_json::to_string(&WsEvent::Status {
        connected: true,
        recipient: None,
    })
    .unwrap();
    let _ = send_text_message(&mut sender, status.into(), ws_metrics.clone()).await;

    // Send the player's current state (tokens + pending orders) on every new connection.
    send_player_state(&mut sender, &state, &username, is_admin).await;

    // Send current order book state for all symbols
    send_order_book_snapshots(&mut sender, &state).await;

    // Send trades snapshot for price chart initialization
    send_trades_snapshot(&mut sender, &state).await;

    tracing::debug!("[{}] Browser WebSocket connected", market_name());
    let mut player_state_dirty = false;
    let mut player_state_flush_armed = false;
    let mut player_state_flush_sleep =
        Box::pin(tokio::time::sleep(std::time::Duration::from_secs(86_400)));

    loop {
        tokio::select! {
            // ── FIX engine → browser ─────────────────────────────────────
            result = rx.recv() => {
                match result {
                    Ok(bus_event) => {
                        if handle_bus_event(
                            bus_event,
                            &username,
                            &mut sender,
                            ws_metrics.as_ref(),
                            &mut player_state_dirty,
                            &mut player_state_flush_armed,
                            &mut player_state_flush_sleep,
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("[{}] Browser lagged, dropped {n} events", market_name());
                        if let Some(metrics) = ws_metrics.as_ref() {
                            metrics
                                .websocket_lagged_events
                                .fetch_add(n as usize, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }

            // ── targeted events → browser ────────────────────────────────
            result = targeted_rx.recv() => {
                match result {
                    Ok(bus_event) => {
                        if handle_bus_event(
                            bus_event,
                            &username,
                            &mut sender,
                            ws_metrics.as_ref(),
                            &mut player_state_dirty,
                            &mut player_state_flush_armed,
                            &mut player_state_flush_sleep,
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("[{}] Targeted browser stream lagged, dropped {n} events", market_name());
                        if let Some(metrics) = ws_metrics.as_ref() {
                            metrics
                                .websocket_lagged_events
                                .fetch_add(n as usize, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }

            // ── debounced player-state refresh (feed/command-triggered) ──
            _ = &mut player_state_flush_sleep, if player_state_flush_armed => {
                player_state_flush_armed = false;
                if player_state_dirty {
                    player_state_dirty = false;
                    send_player_state(&mut sender, &state, &username, is_admin).await;
                }
            }

            // ── browser → FIX engine ──────────────────────────────────────
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        // Handle the browser command, which may involve sending FIX messages over TCP to the engine. The FIX session thread will process those messages and publish events back to the bus, which will then be sent to this browser as needed.
                        handle_browser_message(&text, &state, &metrics, &username, is_admin).await;
                        // Coalesce post-command state refreshes with feed-triggered refreshes.
                        player_state_dirty = true;
                        if !player_state_flush_armed {
                            player_state_flush_armed = true;
                            player_state_flush_sleep.as_mut().reset(
                                tokio::time::Instant::now()
                                    + std::time::Duration::from_millis(PLAYER_STATE_FEED_DEBOUNCE_MS),
                            );
                        }
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
    state.bus.publish(WsEvent::Status {
        connected: false,
        recipient: Some(username),
    });
}

async fn handle_bus_event(
    bus_event: BusEvent,
    username: &str,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    ws_metrics: Option<&Arc<types::MarketMetrics>>,
    player_state_dirty: &mut bool,
    player_state_flush_armed: &mut bool,
    player_state_flush_sleep: &mut std::pin::Pin<Box<tokio::time::Sleep>>,
) -> Result<(), ()> {
    let BusEvent {
        event,
        published_at,
    } = bus_event;
    let allow_event = match &event {
        WsEvent::FixMessage {
            recipient: Some(recipient),
            ..
        } => recipient == username,
        WsEvent::Status {
            recipient: Some(recipient),
            ..
        } => recipient == username,
        _ => true,
    };

    if !allow_event {
        return Ok(());
    }

    if let WsEvent::FixMessage { tag, .. } = &event {
        if tag == "feed" {
            // Coalesce high-frequency feed updates and send player state
            // on a short trailing debounce window.
            *player_state_dirty = true;
            if !*player_state_flush_armed {
                *player_state_flush_armed = true;
                player_state_flush_sleep.as_mut().reset(
                    tokio::time::Instant::now()
                        + std::time::Duration::from_millis(PLAYER_STATE_FEED_DEBOUNCE_MS),
                );
            }
        }
    }

    let json = serde_json::to_string(&event).unwrap();
    if send_text_message(sender, json.into(), ws_metrics.cloned())
        .await
        .is_err()
    {
        return Err(());
    }

    if let Some(metrics) = ws_metrics {
        let elapsed_us = published_at.elapsed().as_micros() as u64;
        if let Ok(mut samples) = metrics.websocket_fanout_to_browser_latency_us.lock() {
            samples.push(elapsed_us);
        }
    }

    Ok(())
}

/// Send player to the browser so it can display the current token balance, pending orders, and holdings.
/// This is called on every new connection and after every relevant event (like order updates or executions) to keep the browser in sync with the latest state.
async fn send_player_state(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    state: &AppState,
    username: &str,
    is_admin: bool,
) {
    let started = Instant::now();
    // Acquire and release the lock before the match to avoid deadlock:
    // holding the guard across the match scrutinee would prevent the None arm
    // from re-acquiring the lock for get_order_owners().
    let player_state_opt = state
        .player_client
        .lock()
        .await
        .get_player_state(username)
        .await;

    match player_state_opt {
        Some(player_state) => {
            let event = WsEvent::PlayerState {
                username: player_state.username,
                tokens: player_state.tokens,
                pending_orders: player_state.pending_orders,
                holdings: player_state.holdings,
                order_owners: player_state.order_owners,
                is_admin,
                visitor_count: state
                    .active_visitors
                    .load(std::sync::atomic::Ordering::Relaxed),
                total_visitor_count: state
                    .total_visitors
                    .load(std::sync::atomic::Ordering::Relaxed),
                id_suffix: player_state.id_suffix,
            };
            let json = serde_json::to_string(&event).unwrap();
            let _ = send_text_message(sender, json.into(), state.bus.metrics()).await;
        }
        None => {
            // Player not found, send empty state
            let order_owners = state.player_client.lock().await.get_order_owners().await;
            let event = WsEvent::PlayerState {
                username: username.to_string(),
                tokens: 0.0,
                pending_orders: Vec::new(),
                holdings: std::collections::HashMap::new(),
                order_owners,
                is_admin,
                visitor_count: state
                    .active_visitors
                    .load(std::sync::atomic::Ordering::Relaxed),
                total_visitor_count: state
                    .total_visitors
                    .load(std::sync::atomic::Ordering::Relaxed),
                id_suffix: String::new(),
            };
            let json = serde_json::to_string(&event).unwrap();
            let _ = send_text_message(sender, json.into(), state.bus.metrics()).await;
        }
    }

    if let Some(metrics) = state.bus.metrics() {
        if let Ok(mut samples) = metrics.websocket_player_state_send_latency_us.lock() {
            samples.push(started.elapsed().as_micros() as u64);
        }
    }
}

/// Send full order book state to the browser client.
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
        tracing::warn!(
            "[{}] Order book is empty at client connection",
            market_name()
        );
    } else if total_bid_levels == 0 && total_ask_levels == 0 {
        tracing::warn!(
            "[{}] Order book has symbols but no price levels at client connection",
            market_name()
        );
    }

    for (symbol, book) in books {
        tracing::debug!(
            "[{}] Sending {} bids and {} asks for {}",
            market_name(),
            book.bids.len(),
            book.asks.len(),
            symbol
        );
        let event = WsEvent::OrderBook {
            symbol,
            bids: book.l3_bids_sorted(),
            asks: book.l3_asks_sorted(),
            timestamp_ms: book.last_update_ms,
        };

        // Send the full order book state for this symbol.
        let json = serde_json::to_string(&event).unwrap();
        let _ = send_text_message(sender, json.into(), state.bus.metrics()).await;
    }
}

/// Send trades snapshot to the browser for price chart initialization.
async fn send_trades_snapshot(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    state: &AppState,
) {
    let (trades, trades_count) = {
        let trades_queue = state.trades_queue.lock().unwrap();
        let trades: Vec<crate::state::TradeView> = trades_queue.iter().cloned().collect();
        let count = trades.len();
        (trades, count)
    };

    let event = WsEvent::Trades { trades };
    let json = serde_json::to_string(&event).unwrap();
    let _ = send_text_message(sender, json.into(), state.bus.metrics()).await;

    if trades_count > 0 {
        tracing::debug!(
            "[{}] Sent {} trades to client for chart initialization",
            market_name(),
            trades_count
        );
    }
}

async fn send_text_message(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    text: String,
    metrics: Option<Arc<Metrics>>,
) -> Result<(), ()> {
    let started = Instant::now();
    let result = sender
        .send(Message::Text(text.into()))
        .await
        .map_err(|_| ());
    if let Some(metrics) = metrics {
        if let Ok(mut samples) = metrics.websocket_send_latency_us.lock() {
            samples.push(started.elapsed().as_micros() as u64);
        }
    }
    result
}

/// Handle a command received from the browser client. This may involve sending FIX messages over TCP to the engine, which will then process them and publish events back to the bus.
async fn handle_browser_message(
    text: &str,
    state: &AppState,
    metrics: &Arc<Metrics>,
    username: &str,
    is_admin: bool,
) {
    tracing::debug!("[{}] Received message from browser: {text}", market_name());
    let cmd: BrowserCommand = match serde_json::from_str(text) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                "[{}] Invalid browser command: {e} — raw: {text}",
                market_name()
            );
            return;
        }
    };

    match cmd {
        BrowserCommand::Order {
            clord_id,
            symbol,
            qty,
            price,
            side,
            sender,
            target,
        } => {
            let order_start_time = std::time::Instant::now();

            let sender_id = sender
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("BROWSER"))
                .unwrap_or_else(|| username.to_string());
            let target_id = target
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "SERVER1".into());
            let symbol = normalize_symbol(&symbol);
            if !state.supported_symbols.contains(&symbol) {
                let event = WsEvent::FixMessage {
                    label: format!("REJECTED ✕ [{}]", clord_id),
                    body: format!("Unsupported symbol '{}'.", symbol),
                    tag: "err".into(),
                    recipient: Some(username.to_string()),
                };
                state.bus.publish(event);
                return;
            }

            let idempotency_key = order_idempotency_key(&sender_id, &clord_id);
            let request_hash = order_request_hash(
                &sender_id, &clord_id, &symbol, &side, qty, price, &target_id,
            );

            match db::claim_idempotency_key(
                &state.market_db_pool,
                &idempotency_key,
                Some(request_hash.as_str()),
            )
            .await
            {
                Ok(db::IdempotencyClaim::Inserted) => {}
                Ok(db::IdempotencyClaim::Existing(record)) => {
                    if record.request_hash.as_deref() == Some(request_hash.as_str())
                        || record.request_hash.is_none()
                    {
                        match record.status.as_str() {
                            "processing" => {
                                state.bus.publish(WsEvent::FixMessage {
                                    label: format!("IN PROGRESS [{}]", clord_id),
                                    body: "Order is already being processed.".into(),
                                    tag: "info".into(),
                                    recipient: Some(username.to_string()),
                                });
                            }
                            "completed" | "failed" => {
                                if let Some(response) = record.response {
                                    match serde_json::from_value::<WsEvent>(response) {
                                        Ok(event) => {
                                            state.bus.publish(event);
                                        }
                                        Err(e) => {
                                            tracing::error!(
                                                "[{}] Failed to replay idempotent response for '{}': {}",
                                                market_name(),
                                                clord_id,
                                                e
                                            );
                                            state.bus.publish(WsEvent::FixMessage {
                                                label: "ERROR".into(),
                                                body: "Stored idempotent response is invalid."
                                                    .into(),
                                                tag: "err".into(),
                                                recipient: Some(username.to_string()),
                                            });
                                        }
                                    }
                                } else {
                                    state.bus.publish(WsEvent::FixMessage {
                                        label: "ERROR".into(),
                                        body: "Idempotent response is missing.".into(),
                                        tag: "err".into(),
                                        recipient: Some(username.to_string()),
                                    });
                                }
                            }
                            other => {
                                tracing::warn!(
                                    "[{}] Unknown idempotency status '{}' for key '{}'",
                                    market_name(),
                                    other,
                                    idempotency_key
                                );
                                state.bus.publish(WsEvent::FixMessage {
                                    label: "ERROR".into(),
                                    body: "Order idempotency state is invalid.".into(),
                                    tag: "err".into(),
                                    recipient: Some(username.to_string()),
                                });
                            }
                        }
                        return;
                    }

                    tracing::warn!(
                        "[{}] Idempotency key '{}' reused with different request hash",
                        market_name(),
                        idempotency_key
                    );
                    let event = WsEvent::FixMessage {
                        label: format!("REJECTED ✕ [{}]", clord_id),
                        body: "Idempotency key reused with a different order payload.".into(),
                        tag: "err".into(),
                        recipient: Some(username.to_string()),
                    };
                    let response = serde_json::to_value(&event).unwrap();
                    if let Err(e) = db::update_idempotency_key_response(
                        &state.market_db_pool,
                        &idempotency_key,
                        "failed",
                        response,
                    )
                    .await
                    {
                        tracing::error!(
                            "[{}] Failed to persist idempotency mismatch response: {}",
                            market_name(),
                            e
                        );
                    }
                    state.bus.publish(event);
                    return;
                }
                Err(e) => {
                    tracing::error!(
                        "[{}] Failed to claim idempotency key for '{}': {}",
                        market_name(),
                        clord_id,
                        e
                    );
                    state.bus.publish(WsEvent::FixMessage {
                        label: "ERROR".into(),
                        body: "Unable to reserve idempotency key.".into(),
                        tag: "err".into(),
                        recipient: Some(username.to_string()),
                    });
                    return;
                }
            }

            tracing::info!(
                "[{}] Browser order: {} {} {} @ {}",
                market_name(),
                if side == "1" { "BUY" } else { "SELL" },
                qty as u32,
                symbol,
                price
            );

            // Fetch player state once for validation
            let player_state = state
                .player_client
                .lock()
                .await
                .get_player_state(username)
                .await;

            if side == "1" {
                let required_notional = qty * price;
                if required_notional.is_finite() && required_notional > 0.0 {
                    if let Some(player) = &player_state {
                        let reserved_notional: f64 = player
                            .pending_orders
                            .iter()
                            .filter(|order| order.side == "1")
                            .map(|order| order.qty * order.price)
                            .sum();

                        let available_tokens = player.tokens - reserved_notional;
                        if available_tokens + 1e-9 < required_notional {
                            let event = WsEvent::FixMessage {
                                label: format!("REJECTED ✕ [{}]", clord_id),
                                body: format!(
                                    "Insufficient tokens for BUY order: required {:.2}, available {:.2}.",
                                    required_notional,
                                    available_tokens.max(0.0)
                                ),
                                tag: "err".into(),
                                recipient: Some(username.to_string()),
                            };
                            let response = serde_json::to_value(&event).unwrap();
                            if let Err(e) = db::update_idempotency_key_response(
                                &state.market_db_pool,
                                &idempotency_key,
                                "failed",
                                response,
                            )
                            .await
                            {
                                tracing::error!(
                                    "[{}] Failed to persist BUY rejection response: {}",
                                    market_name(),
                                    e
                                );
                            }
                            state.bus.publish(event);
                            return;
                        }
                    }
                }
            } else if side == "2" {
                if qty.is_finite() && qty > 0.0 {
                    if !is_admin {
                        if let Some(player) = &player_state {
                            let normalized_symbol = symbol.to_uppercase();
                            let owned_qty = player
                                .holdings
                                .get(&normalized_symbol)
                                .map(|holding| holding.quantity)
                                .unwrap_or(0.0);
                            let reserved_sell_qty: f64 = player
                                .pending_orders
                                .iter()
                                .filter(|order| {
                                    order.side == "2"
                                        && order.symbol.eq_ignore_ascii_case(&normalized_symbol)
                                })
                                .map(|order| order.qty)
                                .sum();

                            let available_qty = (owned_qty - reserved_sell_qty).max(0.0);
                            if available_qty + 1e-9 < qty {
                                let event = WsEvent::FixMessage {
                                    label: format!("REJECTED ✕ [{}]", clord_id),
                                    body: format!(
                                        "Insufficient equity inventory for SELL {}: required {:.0}, available {:.0}.",
                                        normalized_symbol, qty, available_qty
                                    ),
                                    tag: "err".into(),
                                    recipient: Some(username.to_string()),
                                };
                                let response = serde_json::to_value(&event).unwrap();
                                if let Err(e) = db::update_idempotency_key_response(
                                    &state.market_db_pool,
                                    &idempotency_key,
                                    "failed",
                                    response,
                                )
                                .await
                                {
                                    tracing::error!(
                                        "[{}] Failed to persist SELL rejection response: {}",
                                        market_name(),
                                        e
                                    );
                                }
                                state.bus.publish(event);
                                return;
                            }
                        }
                    }
                }
            }

            // Record the order in the player's pending list.
            // Token balance is updated only on transaction (execution reports).
            let pending_order_added = {
                let mut client = state.player_client.lock().await;
                client
                    .add_pending_order(
                        username,
                        PendingOrder {
                            cl_ord_id: clord_id.clone(),
                            symbol: symbol.clone(),
                            side: side.clone(),
                            qty,
                            price,
                        },
                    )
                    .await
            };

            if !pending_order_added {
                let event = WsEvent::FixMessage {
                    label: format!("REJECTED ✕ [{}]", clord_id),
                    body: "Unable to record pending order.".into(),
                    tag: "err".into(),
                    recipient: Some(username.to_string()),
                };
                let response = serde_json::to_value(&event).unwrap();
                if let Err(e) = db::update_idempotency_key_response(
                    &state.market_db_pool,
                    &idempotency_key,
                    "failed",
                    response,
                )
                .await
                {
                    tracing::error!(
                        "[{}] Failed to persist pending-order rejection response: {}",
                        market_name(),
                        e
                    );
                }
                state.bus.publish(event);
                return;
            }

            {
                let mut keys = state.idempotency_keys.lock().unwrap();
                keys.insert(clord_id.clone(), idempotency_key.clone());
            }

            // Track the order event for metrics and latency
            metrics
                .order_events
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let order_elapsed_ms = order_start_time.elapsed().as_millis() as u64;
            if let Ok(mut samples) = metrics.order_latency_ms.lock() {
                samples.push(order_elapsed_ms);
            }

            let fix_bytes = build_new_order_single(
                &sender_id,
                &target_id,
                &symbol,
                side.parse().unwrap_or(1),
                qty,
                price,
                &clord_id,
            );

            tracing::debug!(
                "[{}] Built FIX message for browser order, injecting into FIX engine...",
                market_name()
            );
            let sent_event = WsEvent::FixMessage {
                label: format!(
                    "SENT ▶  ({} {} {} @ {})",
                    if side == "1" { "BUY" } else { "SELL" },
                    qty as u32,
                    symbol,
                    price
                ),
                body: pretty_fix(&fix_bytes),
                tag: "send".into(),
                recipient: Some(username.to_string()),
            };
            // Log it to the browser as a SENT message
            state.bus.publish(sent_event);

            match state
                .fix_session_manager
                .send_fix(
                    &username,
                    &fix_bytes,
                    state.player_client.clone(),
                    state.market_db_pool.clone(),
                    state.idempotency_keys.clone(),
                    &state.bus,
                    Arc::clone(metrics),
                    state.order_book.clone(),
                    state.trades_queue.clone(),
                )
                .await
            {
                Ok(_) => {
                    tracing::info!(
                        "[{}] FIX order sent for '{}': {}",
                        market_name(),
                        username,
                        pretty_fix(&fix_bytes)
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "[{}] Failed to send FIX order for '{}': {}",
                        market_name(),
                        username,
                        e
                    );
                    state
                        .player_client
                        .lock()
                        .await
                        .remove_pending_order(username, &clord_id)
                        .await;
                    {
                        let mut keys = state.idempotency_keys.lock().unwrap();
                        keys.remove(&clord_id);
                    }
                    let error_event = WsEvent::FixMessage {
                        label: "ERROR".into(),
                        body: format!("Failed to send FIX order: {}", e),
                        tag: "error".into(),
                        recipient: Some(username.to_string()),
                    };
                    if let Err(e) = db::update_idempotency_key_response(
                        &state.market_db_pool,
                        &idempotency_key,
                        "failed",
                        serde_json::to_value(&error_event).unwrap(),
                    )
                    .await
                    {
                        tracing::error!(
                            "[{}] Failed to persist failed idempotency response: {}",
                            market_name(),
                            e
                        );
                    }
                    state.bus.publish(error_event);
                }
            }
            tracing::debug!("[{}] Browser order injected into FIX engine", market_name());
        }

        BrowserCommand::Cancel {
            clord_id,
            symbol,
            qty,
        } => {
            let symbol = normalize_symbol(&symbol.unwrap_or_else(|| "AAPL".into()));
            if !state.supported_symbols.contains(&symbol) {
                state.bus.publish(WsEvent::FixMessage {
                    label: format!("REJECTED ✕ [{}]", clord_id),
                    body: format!("Unsupported symbol '{}'.", symbol),
                    tag: "err".into(),
                    recipient: Some(username.to_string()),
                });
                return;
            }
            let qty = qty.unwrap_or(0.0);

            // Drop the pending order. Token balance is not changed on cancel.
            state
                .player_client
                .lock()
                .await
                .remove_pending_order(username, &clord_id)
                .await;

            // Track the cancel order event for metrics
            metrics
                .cancel_orders
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            let fix_bytes =
                build_order_cancel_request(username, "SERVER1", &clord_id, &symbol, qty);

            tracing::info!("[{}] Browser cancel: {}", market_name(), clord_id);
            state.bus.publish(WsEvent::FixMessage {
                label: format!("CANCEL ✕ ({})", clord_id),
                body: pretty_fix(&fix_bytes),
                tag: "send".into(),
                recipient: Some(username.to_string()),
            });

            match state
                .fix_session_manager
                .send_fix(
                    &username,
                    &fix_bytes,
                    state.player_client.clone(),
                    state.market_db_pool.clone(),
                    state.idempotency_keys.clone(),
                    &state.bus,
                    Arc::clone(metrics),
                    state.order_book.clone(),
                    state.trades_queue.clone(),
                )
                .await
            {
                Ok(_) => {
                    tracing::info!(
                        "[{}] FIX cancel sent for '{}': {}",
                        market_name(),
                        username,
                        pretty_fix(&fix_bytes)
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "[{}] Failed to send FIX cancel for '{}': {}",
                        market_name(),
                        username,
                        e
                    );
                    state.bus.publish(WsEvent::FixMessage {
                        label: "ERROR".into(),
                        body: format!("Failed to send FIX cancel: {}", e),
                        tag: "error".into(),
                        recipient: Some(username.to_string()),
                    });
                }
            }
        }

        BrowserCommand::ResetSeq => {
            if !require_admin(&state.bus, username, is_admin) {
                return;
            }
            // We'll handle sequence numbers in the FIX engine
            // For now just ack it
            state.bus.publish(WsEvent::FixMessage {
                label: "INFO".into(),
                body: "Sequence number reset.".into(),
                tag: "info".into(),
                recipient: Some(username.to_string()),
            });
        }

        BrowserCommand::ClearBook => {
            if !require_admin(&state.bus, username, is_admin) {
                return;
            }
            use grpc::proto::ResetRequest;
            use grpc::proto::market_control_client::MarketControlClient;
            match MarketControlClient::connect(state.grpc_addr.clone()).await {
                Ok(mut client) => match client.reset_market(ResetRequest {}).await {
                    Ok(resp) => {
                        let r = resp.into_inner();
                        let (tag, label, body, recipient) = if r.success {
                            let (players_touched, orders_removed) =
                                state.player_client.lock().await.reset_market_state().await;
                            let cleared_symbols = {
                                let mut order_book = state.order_book.lock().unwrap();
                                let symbols: Vec<String> =
                                    order_book.books.keys().cloned().collect();
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
                                    r.message, orders_removed, players_touched
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
                },
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
            if !require_admin(&state.bus, username, is_admin) {
                return;
            }

            let players_reset = state.player_client.lock().await.reset_all_tokens().await;
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
            state.bus.publish(WsEvent::Status {
                connected: false,
                recipient: Some(username.to_string()),
            });
        }

        BrowserCommand::UiOrderLatency { latency_ms } => {
            if let Ok(mut samples) = metrics.ui_order_round_trip_latency_ms.lock() {
                samples.push(latency_ms);
            }
        }

        BrowserCommand::GetPlayerState => {
            // No-op here: handle_socket sends PlayerState after each browser command.
            tracing::debug!(
                "[{}] Browser requested immediate player state",
                market_name()
            );
        }
    }
}

fn order_idempotency_key(sender_id: &str, clord_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(sender_id.as_bytes());
    hasher.update(b"/");
    hasher.update(clord_id.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn order_request_hash(
    sender_id: &str,
    clord_id: &str,
    symbol: &str,
    side: &str,
    qty: f64,
    price: f64,
    target_id: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(sender_id.as_bytes());
    hasher.update(b"|");
    hasher.update(clord_id.as_bytes());
    hasher.update(b"|");
    hasher.update(symbol.as_bytes());
    hasher.update(b"|");
    hasher.update(side.as_bytes());
    hasher.update(b"|");
    hasher.update(format!("{qty:.10}").as_bytes());
    hasher.update(b"|");
    hasher.update(format!("{price:.10}").as_bytes());
    hasher.update(b"|");
    hasher.update(target_id.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn normalize_symbol(symbol: &str) -> String {
    symbol.trim().to_uppercase()
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
    let blen = format!("9={}{SOH}", begin.len() + body.len());
    let raw = format!("{begin}{blen}{body}");
    let chk = checksum(&raw);
    format!("{raw}10={chk}{SOH}").into_bytes()
}

fn build_new_order_single(
    sender: &str,
    target: &str,
    symbol: &str,
    side: u8,
    qty: f64,
    price: f64,
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

fn build_order_cancel_request(
    sender: &str,
    target: &str,
    orig_clord_id: &str,
    symbol: &str,
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
