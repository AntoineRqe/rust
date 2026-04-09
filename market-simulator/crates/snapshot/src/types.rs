use types::{OrderEvent};

#[derive(Debug)]
pub struct OrderBookSnapshot {
    pub bids: Vec<OrderEvent>,
    pub asks: Vec<OrderEvent>,
}

#[derive(Debug)]
pub struct Snapshot {
    pub timestamp: u64,
    pub symbol: String,
    pub id: u64,
    pub order_book: OrderBookSnapshot,
    // pub execution_reports: Vec<ExecutionReport>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            timestamp: 0,
            symbol: String::new(),
            id: 0,
            order_book: OrderBookSnapshot { bids: Vec::new(), asks: Vec::new() },
        }
    }
}

impl Snapshot {
    pub fn new(timestamp: u64, symbol: String, id: u64, order_book: OrderBookSnapshot) -> Self {
        Self {
            timestamp,
            symbol,
            id,
            order_book,
        }
    }
}