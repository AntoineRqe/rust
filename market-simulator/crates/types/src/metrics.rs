use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

/// Shared market-wide metrics used across backend and engine path.
#[derive(Clone)]
pub struct MarketMetrics {
    /// Total login attempts
    pub login_attempts: Arc<AtomicUsize>,
    /// Successful logins
    pub login_success: Arc<AtomicUsize>,
    /// Failed login attempts
    pub login_failure: Arc<AtomicUsize>,
    /// Total order events processed at backend ingress
    pub order_events: Arc<AtomicUsize>,
    /// Total trades executed
    pub trades: Arc<AtomicUsize>,
    /// Total cancel orders submitted
    pub cancel_orders: Arc<AtomicUsize>,
    /// Login latency histogram samples (milliseconds)
    pub login_latency_ms: Arc<Mutex<Vec<u64>>>,
    /// Order submission latency histogram samples (milliseconds)
    pub order_latency_ms: Arc<Mutex<Vec<u64>>>,
    /// Execution latency histogram samples (milliseconds)
    pub execution_latency_ms: Arc<Mutex<Vec<u64>>>,
    /// Total order book events processed by the order book engine
    pub order_book_events: Arc<AtomicUsize>,
    /// Latency from order book event dequeue to fan-out completion (milliseconds)
    pub order_book_event_to_fanout_latency_ms: Arc<Mutex<Vec<u64>>>,
    /// Latency from publish to browser send completion (microseconds)
    pub websocket_fanout_to_browser_latency_us: Arc<Mutex<Vec<u64>>>,
    /// Last observed order book size when broadcasting an order book event
    pub websocket_fanout_order_book_levels: Arc<AtomicUsize>,
    /// Total websocket events dropped because a browser lagged behind
    pub websocket_lagged_events: Arc<AtomicUsize>,
    /// Latency for sending a websocket message to a browser (microseconds)
    pub websocket_send_latency_us: Arc<Mutex<Vec<u64>>>,
    /// Latency for sending the post-event player state to a browser (microseconds)
    pub websocket_player_state_send_latency_us: Arc<Mutex<Vec<u64>>>,
    /// Total execution report events processed by the execution-report engine
    pub execution_report_events: Arc<AtomicUsize>,
    /// Latency from execution-report event dequeue to fan-out completion (milliseconds)
    pub execution_report_event_to_fanout_latency_ms: Arc<Mutex<Vec<u64>>>,
    /// Total order events written to the database
    pub order_db_writes: Arc<AtomicUsize>,
    /// Latency from DB event dequeue to write completion (milliseconds)
    pub order_db_write_latency_ms: Arc<Mutex<Vec<u64>>>,
    /// Total FIX requests processed
    pub fix_requests: Arc<AtomicUsize>,
    /// Total FIX responses delivered
    pub fix_responses: Arc<AtomicUsize>,
    /// Total FIX responses dropped because the client channel was full
    pub fix_response_dropped: Arc<AtomicUsize>,
    /// Latency from FIX request receipt to response delivery (milliseconds)
    pub fix_request_to_response_latency_ms: Arc<Mutex<Vec<u64>>>,
    /// Total backend -> player service API calls
    pub player_api_calls: Arc<AtomicUsize>,
    /// Total backend -> player service API call failures
    pub player_api_errors: Arc<AtomicUsize>,
    /// Latency for backend -> player service API calls (milliseconds)
    pub player_api_latency_ms: Arc<Mutex<Vec<u64>>>,
    /// Latency from UI click to UI receiving the first response (milliseconds)
    pub ui_order_round_trip_latency_ms: Arc<Mutex<Vec<u64>>>,
}

impl MarketMetrics {
    /// Create a new MarketMetrics instance with all counters initialized to zero.
    pub fn new() -> Self {
        Self {
            login_attempts: Arc::new(AtomicUsize::new(0)),
            login_success: Arc::new(AtomicUsize::new(0)),
            login_failure: Arc::new(AtomicUsize::new(0)),
            order_events: Arc::new(AtomicUsize::new(0)),
            trades: Arc::new(AtomicUsize::new(0)),
            cancel_orders: Arc::new(AtomicUsize::new(0)),
            login_latency_ms: Arc::new(Mutex::new(Vec::new())),
            order_latency_ms: Arc::new(Mutex::new(Vec::new())),
            execution_latency_ms: Arc::new(Mutex::new(Vec::new())),
            order_book_events: Arc::new(AtomicUsize::new(0)),
            order_book_event_to_fanout_latency_ms: Arc::new(Mutex::new(Vec::new())),
            websocket_fanout_to_browser_latency_us: Arc::new(Mutex::new(Vec::new())),
            websocket_fanout_order_book_levels: Arc::new(AtomicUsize::new(0)),
            websocket_lagged_events: Arc::new(AtomicUsize::new(0)),
            websocket_send_latency_us: Arc::new(Mutex::new(Vec::new())),
            websocket_player_state_send_latency_us: Arc::new(Mutex::new(Vec::new())),
            execution_report_events: Arc::new(AtomicUsize::new(0)),
            execution_report_event_to_fanout_latency_ms: Arc::new(Mutex::new(Vec::new())),
            order_db_writes: Arc::new(AtomicUsize::new(0)),
            order_db_write_latency_ms: Arc::new(Mutex::new(Vec::new())),
            fix_requests: Arc::new(AtomicUsize::new(0)),
            fix_responses: Arc::new(AtomicUsize::new(0)),
            fix_response_dropped: Arc::new(AtomicUsize::new(0)),
            fix_request_to_response_latency_ms: Arc::new(Mutex::new(Vec::new())),
            player_api_calls: Arc::new(AtomicUsize::new(0)),
            player_api_errors: Arc::new(AtomicUsize::new(0)),
            player_api_latency_ms: Arc::new(Mutex::new(Vec::new())),
            ui_order_round_trip_latency_ms: Arc::new(Mutex::new(Vec::new())),
        }
    }
}
