/// Persistent per-player FIX TCP session manager.
///
/// Each player gets exactly one long-lived TCP connection to the FIX engine.
/// The background reader task lives independently of any WebSocket connection:
/// it continuously applies execution reports via gRPC to the player service
/// even when the browser is offline, and publishes them to the event bus for
/// display when the browser reconnects.
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::TrySendError;
use fix::engine::FixRawMsg;
use tokio::sync::mpsc;
use types::consts::RB_SIZE;

use crate::player_client::PlayerClient;
use crate::state::{EventBus, TradeView, WsEvent};
use crate::server::Metrics;
use crate::order_book::{OrderBookState, stable_order_id_from_cl_ord_id};
use utils::market_name;

/// Debounces OrderBook snapshot publishes to reduce bandwidth.
/// Batches updates: publishes when 50ms has elapsed OR 5 orders processed, whichever first.
#[derive(Default)]
struct OrderBookDebouncer {
    /// Symbols with dirty (changed) order books
    dirty_symbols: std::collections::HashSet<String>,
    /// Last publication time per symbol
    last_publish_time: HashMap<String, Instant>,
    /// Orders processed since last publish
    orders_since_publish: usize,
}

impl OrderBookDebouncer {
    fn mark_dirty(&mut self, symbol: &str) {
        self.dirty_symbols.insert(symbol.to_string());
    }

    fn should_publish(&self, symbol: &str, now: Instant) -> bool {
        if !self.dirty_symbols.contains(symbol) {
            return false;
        }
        
        let last_publish = self.last_publish_time.get(symbol);
        let time_elapsed = match last_publish {
            Some(last) => now.duration_since(*last),
            None => Duration::from_millis(100), // Force publish on first update
        };

        // Publish if 50ms elapsed OR 5 orders processed
        time_elapsed >= Duration::from_millis(50) || self.orders_since_publish >= 5
    }

    fn publish(&mut self, symbol: &str, now: Instant) {
        self.dirty_symbols.remove(symbol);
        self.last_publish_time.insert(symbol.to_string(), now);
        self.orders_since_publish = 0;
    }

    fn increment_order_count(&mut self) {
        self.orders_since_publish += 1;
    }
}

// ── Session ───────────────────────────────────────────────────────────────────

pub struct FIXSession {
    /// Per-player response channel used by FIX outbound engine routing.
    pub response_tx: mpsc::Sender<FixRawMsg<RB_SIZE>>,
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
        bus: &EventBus,
        metrics: Arc<Metrics>,
        order_book: Arc<Mutex<OrderBookState>>,
        trades_queue: Arc<Mutex<VecDeque<TradeView>>>,
    ) -> mpsc::Sender<FixRawMsg<RB_SIZE>> {
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

        let (response_tx, mut response_rx) = mpsc::channel::<FixRawMsg<RB_SIZE>>(1024);
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
            let session_key = session_key.clone();

            // Spawn the background reader task for this session.
            tokio::spawn(async move {
                let mut debouncer = OrderBookDebouncer::default();

                tracing::info!(
                    "[{}] FIX session reader started for '{username}'",
                    market_name()
                );

                while !stop_flag.load(Ordering::Relaxed) {
                    match tokio::time::timeout(Duration::from_millis(100), response_rx.recv()).await {
                        Ok(Some(msg)) => {
                            let raw_msg = msg.data[..msg.len as usize].to_vec();
                            let body = pretty_fix(&raw_msg);
                            let exec_start_time = std::time::Instant::now();

                            // Apply execution report via gRPC to player service
                            let mut client = player_client.lock().await;
                            if let Err(e) = client.apply_fix_execution_report(&username, &body).await {
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

                            // Update backend order book from ExecutionReport
                            if let Some(exec_data) = parse_execution_report(&body) {
                                update_backend_order_book_from_exec_report(
                                    &exec_data,
                                    &order_book,
                                );

                                // Mark symbol as dirty (debounced publish)
                                debouncer.mark_dirty(&exec_data.symbol);
                                debouncer.increment_order_count();

                                // Publish new/partial (status 0/1) and terminal updates immediately without debounce delay.
                                // This ensures the UI sees orders as soon as they're added, and removals when they're canceled/filled.
                                let now = Instant::now();
                                let should_publish_immediately = is_terminal_order_status(exec_data.ord_status)
                                    || matches!(exec_data.ord_status, 0 | 1); // New or Partially Filled
                                
                                if should_publish_immediately || debouncer.should_publish(&exec_data.symbol, now) {
                                    let book = order_book.lock().unwrap();
                                    if let Some(symbol_book) = book.get(&exec_data.symbol) {
                                        bus.publish(WsEvent::OrderBook {
                                            symbol: exec_data.symbol.clone(),
                                            bids: symbol_book.l3_bids_sorted(),
                                            asks: symbol_book.l3_asks_sorted(),
                                            timestamp_ms: std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .map(|d| d.as_millis() as u64)
                                                .unwrap_or(0),
                                        });
                                    }
                                    debouncer.publish(&exec_data.symbol, now);
                                }
                            }

                            // Track execution latency
                            let exec_elapsed_ms = exec_start_time.elapsed().as_millis() as u64;
                            if let Ok(mut samples) = metrics.execution_latency_ms.lock() {
                                samples.push(exec_elapsed_ms);
                            }

                            // Track trades: execution reports with ExecType=F (Trade) indicate executed orders
                            if is_trade_execution(&body) {
                                metrics.trades.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                                if let Some(trade_view) = extract_trade_view(&body) {
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
                            }

                            // Broadcast so connected browser clients can display it.
                            bus.publish(WsEvent::FixMessage {
                                label: classify_fix_msg(&raw_msg),
                                body,
                                tag: "feed".into(),
                                recipient: Some(username.clone()),
                            });
                        }
                        Ok(None) => {
                            tracing::info!(
                                "[{}] FIX session channel closed for '{username}'",
                                market_name()
                            );
                            break;
                        }
                        Err(_) => {
                            // Timeout is expected on idle - flush any pending dirty symbols
                            let now = Instant::now();
                            for symbol in debouncer.dirty_symbols.iter().cloned().collect::<Vec<_>>() {
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
                                debouncer.publish(&symbol, now);
                            }
                        }
                    }
                }

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
        "9" => "◀ CANCEL REJECT (9)".into(),
        "W" => "◀ MD SNAPSHOT (W)".into(),
        "X" => "◀ MD INCREMENTAL (X)".into(),
        "Y" => "◀ MD REJECT (Y)".into(),
        "0" => "◀ HEARTBEAT (0)".into(),
        "A" => "◀ LOGON (A)".into(),
        "5" => "◀ LOGOUT (5)".into(),
        t   => format!("◀ MSG ({t})"),
    }
}

/// Detect if an execution report indicates a trade execution.
/// Trades have ExecType=F (FIX field 150) in the execution report.
fn is_trade_execution(body: &str) -> bool {
    // ExecType=F means a trade was executed
    // The pretty_fix format contains fields like "150=F" for ExecType
    body.contains("150=F") || body.contains("ExecType=F")
}

fn extract_trade_view(body: &str) -> Option<TradeView> {
    if !is_trade_execution(body) {
        return None;
    }

    // Venue payloads are not always uniform: some reports include LastPx/LastQty (31/32),
    // others only include order-level fields (44/38/151).
    let price = extract_field(body, "31")
        .or_else(|| extract_field(body, "44"))
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)?;

    let quantity = extract_field(body, "32")
        .or_else(|| {
            let total_qty = extract_field(body, "38")?.parse::<f64>().ok()?;
            let leaves_qty = extract_field(body, "151")
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or(0.0);
            let exec_qty = total_qty - leaves_qty;
            if exec_qty.is_finite() && exec_qty > 0.0 {
                Some(exec_qty.to_string())
            } else if total_qty.is_finite() && total_qty > 0.0 {
                Some(total_qty.to_string())
            } else {
                None
            }
        })
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)?;

    let id = extract_field(body, "17")
        .or_else(|| extract_field(body, "37"))
        .or_else(|| extract_field(body, "11"))
        .and_then(|value| value.parse::<u64>().ok())
        .or_else(|| {
            extract_field(body, "17")
                .or_else(|| extract_field(body, "37"))
                .or_else(|| extract_field(body, "11"))
                .map(|value| stable_order_id_from_cl_ord_id(&value))
        })
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_micros() as u64)
                .unwrap_or(0)
        });
    let cl_ord_id = extract_field(body, "11").unwrap_or_default();

    Some(TradeView {
        id,
        price,
        quantity,
        cl_ord_id,
    })
}

/// ExecutionReport data extracted from FIX message
#[derive(Debug, Clone)]
struct ExecReportData {
    order_id: u64,       // Stable hash of effective_cl_ord_id (consistent with startup load)
    cl_ord_id: String,   // Effective ClOrdId of the target order (field 11 for new, field 41 for cancel ack)
    symbol: String,      // FIX field 55
    side: u8,            // FIX field 54
    ord_status: u8,      // FIX field 39 (0=New, 1=PartialFill, 2=Fill, 3=DoneForDay, 4=Canceled)
    price: f64,          // FIX field 44
    qty: f64,            // FIX field 38
    leaves_qty: f64,     // FIX field 151
}

fn is_terminal_order_status(ord_status: u8) -> bool {
    matches!(ord_status, 2 | 3 | 4 | 8)
}

/// Extract a single FIX field value from pretty_fix format.
/// Example: "35=8 | 11=ORDER123" -> extract_field("11") -> Some("ORDER123")
fn extract_field(body: &str, field_num: &str) -> Option<String> {
    // Split by | and find the field
    for part in body.split('│').chain(body.split('|')) {
        let trimmed = part.trim();
        if let Some(value) = trimmed.strip_prefix(&format!("{}=", field_num)) {
            return Some(value.to_string());
        }
    }
    None
}

/// Parse ExecutionReport FIX message and extract relevant fields for order book update.
fn parse_execution_report(body: &str) -> Option<ExecReportData> {
    // Only process execution reports (35=8)
    if !body.contains("35=8") {
        return None;
    }

    let cl_ord_id_raw = extract_field(body, "11")?; // ClOrdId of the request
    let orig_cl_ord_id = extract_field(body, "41"); // OrigClOrdId (present in cancel acks)
    let symbol = extract_field(body, "55")?;
    let ord_status = extract_field(body, "39")?.parse::<u8>().ok()?;
    let side = extract_field(body, "54")
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(0);
    let (price, qty, leaves_qty) = match ord_status {
        // New/partial reports need full pricing fields for add/modify behavior.
        0 | 1 => (
            extract_field(body, "44")?.parse::<f64>().ok()?,
            extract_field(body, "38")?.parse::<f64>().ok()?,
            extract_field(body, "151")?.parse::<f64>().ok()?,
        ),
        // Terminal statuses only need identity fields to remove from the book.
        _ => (0.0, 0.0, 0.0),
    };

    // For cancel acks (status=4), the target order's ClOrdId is in field 41 (OrigClOrdId).
    // For new/fill reports, it's in field 11 (ClOrdId).
    let effective_cl_ord_id = if ord_status == 4 {
        orig_cl_ord_id.unwrap_or(cl_ord_id_raw)
    } else {
        cl_ord_id_raw
    };

    let order_id = stable_order_id_from_cl_ord_id(&effective_cl_ord_id);

    Some(ExecReportData {
        order_id,
        cl_ord_id: effective_cl_ord_id,
        symbol,
        side,
        ord_status,
        price,
        qty,
        leaves_qty,
    })
}

/// Update backend order book based on ExecutionReport status.
fn update_backend_order_book_from_exec_report(
    exec: &ExecReportData,
    order_book: &Arc<Mutex<OrderBookState>>,
) {
    let mut book = order_book.lock().unwrap();
    let symbol_book = book.get_or_create(&exec.symbol);

    match exec.ord_status {
        0 => {
            // New: add or update order
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
            symbol_book.delete_order(
                exec.order_id,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
            );
        }
        _ => {}  // Unknown status, ignore
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_field_pretty_fix_format() {
        let msg = "35=8 │ 11=5544843609372783994 │ 41=ARETEST988945002 │ 39=0 │ 55=AAPL │ 54=1 │ 44=150.25 │ 38=100";
        assert_eq!(extract_field(msg, "35"), Some("8".to_string()));
        assert_eq!(extract_field(msg, "11"), Some("5544843609372783994".to_string()));
        assert_eq!(extract_field(msg, "41"), Some("ARETEST988945002".to_string()));
        assert_eq!(extract_field(msg, "39"), Some("0".to_string()));
        assert_eq!(extract_field(msg, "55"), Some("AAPL".to_string()));
        assert_eq!(extract_field(msg, "54"), Some("1".to_string()));
        assert_eq!(extract_field(msg, "44"), Some("150.25".to_string()));
        assert_eq!(extract_field(msg, "38"), Some("100".to_string()));
        assert_eq!(extract_field(msg, "999"), None);  // Non-existent field
    }

    #[test]
    fn test_parse_execution_report_new_order() {
        // Real FIX new order exec report: field 11 = ClOrdId string (not numeric), no field 41
        let body = "35=8 │ 11=ARETEST988945002 │ 39=0 │ 55=AAPL │ 54=1 │ 44=150.25 │ 38=100 │ 151=100";
        
        let exec_data = parse_execution_report(body);
        assert!(exec_data.is_some());
        
        let data = exec_data.unwrap();
        // cl_ord_id = field 11 for new orders; order_id = stable hash of cl_ord_id
        assert_eq!(data.cl_ord_id, "ARETEST988945002");
        assert_eq!(data.order_id, stable_order_id_from_cl_ord_id("ARETEST988945002"));
        assert_eq!(data.symbol, "AAPL");
        assert_eq!(data.side, 1);
        assert_eq!(data.ord_status, 0);  // New
        assert_eq!(data.price, 150.25);
        assert_eq!(data.qty, 100.0);
        assert_eq!(data.leaves_qty, 100.0);
    }

    #[test]
    fn test_parse_execution_report_partial_fill() {
        let body = "35=8 │ 11=ARETEST988945002 │ 39=1 │ 55=AAPL │ 54=1 │ 44=150.25 │ 38=100 │ 151=50";
        
        let data = parse_execution_report(body).unwrap();
        assert_eq!(data.ord_status, 1);  // PartialFill
        assert_eq!(data.leaves_qty, 50.0);
    }

    #[test]
    fn test_parse_execution_report_filled() {
        let body = "35=8 │ 11=ARETEST988945002 │ 39=2 │ 55=AAPL │ 54=1 │ 44=150.25 │ 38=100 │ 151=0";
        
        let data = parse_execution_report(body).unwrap();
        assert_eq!(data.ord_status, 2);  // Filled
        assert_eq!(data.leaves_qty, 0.0);
    }

    #[test]
    fn test_parse_execution_report_canceled() {
        // Cancel ack: field 11 = cancel req id, field 41 = original order's ClOrdId.
        // Price/qty/leaves may be absent in delete/cancel payloads.
        let body = "35=8 │ 11=ORD-WEB-8 │ 41=ARETEST988945002 │ 39=4 │ 55=AAPL";
        
        let data = parse_execution_report(body).unwrap();
        assert_eq!(data.ord_status, 4);  // Canceled
        // For cancel acks, effective ClOrdId = field 41 (original order)
        assert_eq!(data.cl_ord_id, "ARETEST988945002");
        assert_eq!(data.order_id, stable_order_id_from_cl_ord_id("ARETEST988945002"));
    }

    #[test]
    fn test_parse_non_execution_report_message() {
        let body = "35=A │ 49=CLIENT │ 56=SERVER";  // Logon message, not exec report
        
        let exec_data = parse_execution_report(body);
        assert!(exec_data.is_none());
    }

    #[test]
    fn test_parse_execution_report_missing_fields() {
        // Missing field 151 (leaves_qty)
        let body = "35=8 │ 11=ARETEST988945002 │ 39=0 │ 55=AAPL │ 54=1 │ 44=150.25 │ 38=100";
        
        let exec_data = parse_execution_report(body);
        assert!(exec_data.is_none());  // Missing required field
    }

    #[test]
    fn test_is_trade_execution_with_field_150() {
        let msg_with_trade = "35=8 │ 11=123 │ 41=ORDER1 │ 150=F │ 55=AAPL";
        assert!(is_trade_execution(msg_with_trade));
    }

    #[test]
    fn test_is_trade_execution_without_trade() {
        let msg_without_trade = "35=8 │ 11=123 │ 41=ORDER1 │ 39=0 │ 55=AAPL";
        assert!(!is_trade_execution(msg_without_trade));
    }
}
