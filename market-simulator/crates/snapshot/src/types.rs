use types::{OrderEvent};
use std::time::{SystemTime, UNIX_EPOCH};

/// This module defines the data structures for representing order book snapshots and market data feeds.

const MAX_SNAPSHOT_DEPTH: usize = 10;


#[derive(Debug)]
/// Represents a snapshot of the order book at a specific point in time, including bids and asks.
pub struct OrderBookSnapshot {
    /// A vector of order events representing the current bids in the order book.
    pub bids: [OrderEvent; MAX_SNAPSHOT_DEPTH],
    /// A vector of order events representing the current asks in the order book.
    pub asks: [OrderEvent; MAX_SNAPSHOT_DEPTH],
    /// The number of valid bids in the `bids` array.
    pub bids_len: usize,
    /// The number of valid asks in the `asks` array.
    pub asks_len: usize,
}

impl Default for OrderBookSnapshot {
    fn default() -> Self {
        Self {
            bids: [OrderEvent::default(); MAX_SNAPSHOT_DEPTH],
            asks: [OrderEvent::default(); MAX_SNAPSHOT_DEPTH],
            bids_len: 0,
            asks_len: 0,
        }
    }
}

impl OrderBookSnapshot {
    pub fn new(bids: [OrderEvent; MAX_SNAPSHOT_DEPTH], asks: [OrderEvent; MAX_SNAPSHOT_DEPTH], bids_len: usize, asks_len: usize) -> Self {
        Self {
            bids,
            asks,
            bids_len,
            asks_len,
        }
    }

    pub fn add_bid(&mut self, bid: OrderEvent) -> Result<(), &'static str> {
        if self.bids_len < self.bids.len() {
            self.bids[self.bids_len] = bid;
            self.bids_len += 1;
            Ok(())
        } else {
            Err("Maximum snapshot depth reached")
        }
    }

    pub fn add_ask(&mut self, ask: OrderEvent) -> Result<(), &'static str> {
        if self.asks_len < self.asks.len() {
            self.asks[self.asks_len] = ask;
            self.asks_len += 1;
            Ok(())
        } else {
            Err("Maximum snapshot depth reached")
        }
    }

    pub fn clear(&mut self) {
        self.bids_len = 0;
        self.asks_len = 0;
    }
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
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            symbol: String::new(),
            id: 0,
            order_book: OrderBookSnapshot::default(),
        }
    }
}

impl Snapshot {
    pub fn new(timestamp: u64, symbol: &str, id: u64, order_book: OrderBookSnapshot) -> Self {
        Self {
            timestamp,
            symbol: symbol.to_string(),
            id,
            order_book,
        }
    }
}