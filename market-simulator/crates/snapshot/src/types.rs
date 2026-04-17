use types::{OrderEvent};

/// This module defines the data structures for representing order book snapshots and market data feeds.

#[derive(Debug)]
/// Represents a snapshot of the order book at a specific point in time, including bids and asks.
pub struct OrderBookSnapshot {
    /// A vector of order events representing the current bids in the order book.
    pub bids: Vec<OrderEvent>,
    /// A vector of order events representing the current asks in the order book.
    pub asks: Vec<OrderEvent>,
}

#[derive(Debug)]
/// Represents a market data feed event containing the current state of the order book for a specific symbol.
pub struct Snapshot {
    /// The timestamp of the snapshot, represented as a Unix timestamp in nanoseconds.
    pub timestamp: u64,
    /// The symbol for which the snapshot is taken (e.g., "BTCUSD").
    pub symbol: String,
    /// A unique identifier for the snapshot, which can be used for tracking and correlation purposes.
    pub id: u64,
    /// The order book snapshot containing the current bids and asks for the symbol.
    pub order_book: OrderBookSnapshot,
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