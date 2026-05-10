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
        }
    }
}
