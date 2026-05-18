/// Persistent per-player FIX TCP session manager.
///
/// The background reader task lives independently of any WebSocket connection:
/// it continuously applies execution reports via gRPC to the player service
/// even when the browser is offline, and publishes them to the event bus for
/// display when the browser reconnects.
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::TrySendError;
use fix::engine::FixRawMsg;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use sqlx::PgPool;
use types::consts::RB_SIZE;
use types::{ExecutionReportMessage, ExecReportData};

use crate::order_book::OrderBookState;
use crate::player_client::PlayerClient;
use crate::server::Metrics;
use crate::state::{EventBus, TradeView, WsEvent};
use utils::market_name;

/// Coalesces OrderBook snapshot publishes by symbol.
/// Under load we only publish the latest snapshot per dirty symbol on a fixed cadence.
const ORDERBOOK_FLUSH_INTERVAL_MS: u64 = 50;

#[derive(Default)]
struct OrderBookDebouncer {
    /// Symbols with dirty (changed) order books
    dirty_symbols: std::collections::HashSet<String>,
}

impl OrderBookDebouncer {
    fn mark_dirty(&mut self, symbol: &str) {
        self.dirty_symbols.insert(symbol.to_string());
    }

    fn take_dirty_symbols(&mut self) -> Vec<String> {
        self.dirty_symbols.drain().collect()
    }
}

fn publish_dirty_order_books(
    debouncer: &mut OrderBookDebouncer,
    order_book: &Arc<Mutex<OrderBookState>>,
    bus: &EventBus,
) {
    for symbol in debouncer.take_dirty_symbols() {
        let book = order_book.lock().unwrap();
        if let Some(symbol_book) = book.get(&symbol) {
            bus.publish(WsEvent::OrderBook {
                symbol: symbol.clone(),
                bids: symbol_book.l3_bids_sorted(),
                asks: symbol_book.l3_asks_sorted(),
                timestamp_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
            });
        }
    }
}

// ── Session ───────────────────────────────────────────────────────────────────

pub struct FIXSession {
    /// Per-player response channel used by FIX outbound engine routing.
    pub response_tx: mpsc::Sender<ExecutionReportMessage<RB_SIZE>>,
    /// Set to `true` to tear down the session (e.g. on server shutdown).
    pub stop: Arc<AtomicBool>,
}

// ── Manager ───────────────────────────────────────────────────────────────────

/// Thread-safe registry of live FIX sessions, keyed by username.
#[derive(Clone)]
pub struct FIXSessionManager {
    fix_tx: Arc<crossbeam_channel::Sender<FixRawMsg<RB_SIZE>>>,
    inner: Arc<Mutex<HashMap<String, FIXSession>>>,
}

impl FIXSessionManager {
    pub fn new(fix_tx: Arc<crossbeam_channel::Sender<FixRawMsg<RB_SIZE>>>) -> Self {
        FIXSessionManager {
            fix_tx,
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Return (or create) the per-player FIX response channel.
    ///
    /// The background reader task is spawned automatically and handles:
    ///
    /// - Calling `player_client.apply_fix_execution_report()` via gRPC for every
    ///   execution report received, regardless of WebSocket state.
    /// - Publishing every received message to the event bus so connected
    ///   browser clients can display it.
    /// - Updating the backend order book from ExecutionReport fields.
    ///
    /// This no longer creates a TCP socket; backend injects FIX directly via channel.
    pub fn get_or_create_session(
        &self,
        username: &str,
        player_client: Arc<tokio::sync::Mutex<PlayerClient>>,
        market_db_pool: PgPool,
        bus: &EventBus,
        metrics: Arc<Metrics>,
        order_book: Arc<Mutex<OrderBookState>>,
        trades_queue: Arc<Mutex<VecDeque<TradeView>>>,
    ) -> mpsc::Sender<ExecutionReportMessage<RB_SIZE>> {
        let mut sessions = self.inner.lock().unwrap();
        // Cache sessions by both username and market to prevent cross-market session reuse
        let session_key = format!("{}:{}", username, market_name());

        if let Some(session) = sessions.get(&session_key) {
            tracing::debug!(
                "[{}] Reusing existing FIX session for '{username}'",
                market_name()
            );
            return session.response_tx.clone();
        }

        let (response_tx, mut response_rx) = mpsc::channel::<ExecutionReportMessage<RB_SIZE>>(1024);
        let stop = Arc::new(AtomicBool::new(false));
        // Session cache key includes market to avoid cross-market session reuse
        let session_key = format!("{}:{}", username, market_name());

        {
            let username = username.to_string();
            let stop_flag = Arc::clone(&stop);
            let player_client = player_client.clone();
            let bus = bus.clone();
            let sessions_ref = Arc::clone(&self.inner);
            let metrics = Arc::clone(&metrics);
            let order_book = order_book.clone();
            let trades_queue = trades_queue.clone();
            let market_db_pool = market_db_pool.clone();
            let session_key = session_key.clone();

            // Spawn the background reader task for this session.
            tokio::spawn(async move {
                let mut debouncer = OrderBookDebouncer::default();
                let mut order_book_flush_tick =
                    tokio::time::interval(Duration::from_millis(ORDERBOOK_FLUSH_INTERVAL_MS));
                order_book_flush_tick
                    .set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                tracing::info!(
                    "[{}] FIX session reader started for '{username}'",
                    market_name()
                );

                while !stop_flag.load(Ordering::Relaxed) {
                    tokio::select! {
                        maybe_msg = response_rx.recv() => match maybe_msg {
                            Some(exec_msg) => {
                                let exec_start_time = std::time::Instant::now();
                                let exec_data = &exec_msg.exec_report_data;

                                // Reconstruct raw FIX bytes for gRPC call (client display)
                                let raw_msg = exec_msg.fix_data[..exec_msg.fix_len as usize].to_vec();
                                let body = pretty_fix_for_display(&raw_msg);

                                // Apply execution report via gRPC to player service
                                let mut client = player_client.lock().await;
                                if let Err(e) =
                                    client.apply_fix_execution_report(&username, &body).await
                                {
                                    tracing::error!(
                                        "[{}] FIX execution report failed for username='{username}' | error='{e}' | full_fix_msg='{body}'",
                                        market_name()
                                    );
                                    // Also log at debug level with structured format for easier parsing
                                    tracing::debug!(
                                        "[{}] execution_report_error: username='{}', error='{}', msg_len={}, first_100_chars='{}'",
                                        market_name(),
                                        username,
                                        e,
                                        body.len(),
                                        body.chars().take(100).collect::<String>()
                                    );
                                }
                                drop(client); // Release lock before publishing

                                // Persist the latest FIX response for idempotent replays.
                                let response_event = WsEvent::FixMessage {
                                    label: classify_fix_msg(&raw_msg),
                                    body: body.clone(),
                                    tag: "feed".into(),
                                    recipient: Some(username.clone()),
                                };
                                if let Err(e) = db::update_idempotency_key_response(
                                    &market_db_pool,
                                    &idempotency_key_for(&username, &exec_data.cl_ord_id),
                                    "completed",
                                    serde_json::to_value(&response_event).unwrap(),
                                )
                                .await
                                {
                                    tracing::error!(
                                        "[{}] Failed to persist FIX response for idempotency: {}",
                                        market_name(),
                                        e
                                    );
                                }

                                // Update backend order book using pre-parsed data (no FIX parsing!)
                                update_backend_order_book_from_exec_report(exec_data, &order_book);

                                // Mark symbol dirty; snapshots are coalesced and flushed on tick.
                                debouncer.mark_dirty(&exec_data.symbol);

                                // Track execution latency
                                let exec_elapsed_ms = exec_start_time.elapsed().as_millis() as u64;
                                if let Ok(mut samples) = metrics.execution_latency_ms.lock() {
                                    samples.push(exec_elapsed_ms);
                                }

                                // Track trades: check ord_status for trade execution (1=PartialFill, 2=Fill)
                                if exec_data.ord_status == 1 || exec_data.ord_status == 2 {
                                    metrics
                                        .trades
                                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                                    let trade_view = TradeView {
                                        id: exec_data.order_id,
                                        price: exec_data.price,
                                        quantity: exec_data.qty,
                                        cl_ord_id: exec_data.cl_ord_id.clone(),
                                    };
                                    {
                                        let mut queue = trades_queue.lock().unwrap();
                                        queue.push_back(trade_view.clone());
                                        while queue.len() > 200 {
                                            queue.pop_front();
                                        }
                                    }
                                    bus.publish(WsEvent::Trades {
                                        trades: vec![trade_view],
                                    });
                                }

                                // Broadcast so connected browser clients can display it.
                                bus.publish(response_event);
                            },
                            None => {
                                tracing::info!(
                                    "[{}] FIX session channel closed for '{username}'",
                                    market_name()
                                );
                                break;
                            }
                        },
                        _ = order_book_flush_tick.tick() => {
                            publish_dirty_order_books(&mut debouncer, &order_book, &bus);
                        }
                    }
                }

                // Flush any pending snapshots before exiting the session reader.
                publish_dirty_order_books(&mut debouncer, &order_book, &bus);

                sessions_ref.lock().unwrap().remove(&session_key);
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
            session_key,
            FIXSession {
                response_tx: response_tx.clone(),
                stop,
            },
        );
        response_tx
    }

    pub async fn send_fix(
        &self,
        username: &str,
        bytes: &[u8],
        player_client: Arc<tokio::sync::Mutex<PlayerClient>>,
        market_db_pool: PgPool,
        bus: &EventBus,
        metrics: Arc<Metrics>,
        order_book: Arc<Mutex<OrderBookState>>,
        trades_queue: Arc<Mutex<VecDeque<TradeView>>>,
    ) -> Result<(), String> {
        if bytes.len() > RB_SIZE {
            return Err(format!(
                "FIX message too large: {} bytes (max {})",
                bytes.len(),
                RB_SIZE
            ));
        }

        let response_tx = self.get_or_create_session(
            username,
            player_client,
            market_db_pool,
            bus,
            metrics,
            order_book,
            trades_queue,
        );

        let mut pending = FixRawMsg::<RB_SIZE>::new(bytes, Some(response_tx));
        loop {
            match self.fix_tx.try_send(pending) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Disconnected(_)) => {
                    return Err("FIX engine queue disconnected".to_string());
                }
                Err(TrySendError::Full(returned)) => {
                    pending = returned;
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            }
        }
    }

    /// Gracefully tear down all active sessions (called on server shutdown).
    pub fn shutdown_all(&self) {
        let sessions = self.inner.lock().unwrap();
        for (username, session) in sessions.iter() {
            session.stop.store(true, Ordering::Relaxed);
            tracing::debug!(
                "[{}] FIX session shutdown requested for '{username}'",
                market_name()
            );
        }
        // Drop the sessions, which will cause the async tasks to see stop flag and exit
        // The streams will be closed automatically when dropped
    }
}

// ── FIX helpers ───────────────────────────────────────────────────────────────

fn idempotency_key_for(username: &str, clord_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(username.as_bytes());
    hasher.update(b"/");
    hasher.update(clord_id.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Pipe-delimited human-readable FIX string.
pub fn pretty_fix(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).replace('\x01', " │ ")
}

/// Human-friendly FIX string for browser display.
///
/// Execution reports use fixed-point fields with 8 fractional digits in the
/// protocol payload, but the UI should show trimmed decimals.
pub fn pretty_fix_for_display(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    s.split('\x01')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let Some((tag, value)) = part.split_once('=') else {
                return part.to_string();
            };

            let value = match tag {
                "6" | "14" | "31" | "32" | "38" | "44" | "151" => trim_fixed_decimal(value),
                _ => value.to_string(),
            };

            format!("{tag}={value}")
        })
        .collect::<Vec<_>>()
        .join(" │ ")
}

fn trim_fixed_decimal(value: &str) -> String {
    let trimmed = value.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
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
        "9" => "◀ CANCEL REJECT (9)".into(),
        "W" => "◀ MD SNAPSHOT (W)".into(),
        "X" => "◀ MD INCREMENTAL (X)".into(),
        "Y" => "◀ MD REJECT (Y)".into(),
        "0" => "◀ HEARTBEAT (0)".into(),
        "A" => "◀ LOGON (A)".into(),
        "5" => "◀ LOGOUT (5)".into(),
        t => format!("◀ MSG ({t})"),
    }
}



/// Update backend order book based on ExecutionReport status.
fn update_backend_order_book_from_exec_report(
    exec: &ExecReportData,
    order_book: &Arc<Mutex<OrderBookState>>,
) {
    tracing::debug!(
        "[ORDER BOOK UPDATE] symbol={}, order_id={}, cl_ord_id={}, ord_status={}, side={}, price={}, qty={}",
        exec.symbol,
        exec.order_id,
        exec.cl_ord_id,
        exec.ord_status,
        exec.side,
        exec.price,
        exec.qty
    );

    let mut book = order_book.lock().unwrap();
    let symbol_book = book.get_or_create(&exec.symbol);

    match exec.ord_status {
        0 => {
            // New: add or update order
            tracing::debug!(
                "[ORDER BOOK UPDATE] Adding new order: order_id={}, cl_ord_id={}",
                exec.order_id,
                exec.cl_ord_id
            );
            symbol_book.add_or_update_order(
                exec.order_id,
                exec.side,
                exec.price,
                exec.qty,
                Some(exec.cl_ord_id.clone()),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
            );
        }
        1 => {
            // PartialFill: update qty to leaves_qty
            tracing::debug!(
                "[ORDER BOOK UPDATE] Partial fill: order_id={}, new_qty={}",
                exec.order_id,
                exec.leaves_qty
            );
            symbol_book.modify_order(
                exec.order_id,
                exec.price,
                exec.leaves_qty,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
            );
        }
        2 | 3 | 4 | 8 => {
            // Filled (2), DoneForDay (3), Canceled (4): remove from order book
            tracing::debug!(
                "[ORDER BOOK UPDATE] Deleting order: order_id={}, status={}",
                exec.order_id,
                exec.ord_status
            );
            symbol_book.delete_order(
                exec.order_id,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
            );
        }
        _ => {} // Unknown status, ignore
    }
}
