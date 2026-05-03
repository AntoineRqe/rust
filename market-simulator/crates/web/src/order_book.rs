use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A price level in the order book with aggregate quantity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceLevel {
    pub price: f64,
    pub quantity: f64,
}

/// One resting order (L3) in the book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L3Order {
    pub order_id: u64,
    pub price: f64,
    pub quantity: f64,
    pub cl_ord_id: Option<String>,
    pub time_priority: u64,
}

/// Serializable L3 order entry sent to the browser.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L3OrderView {
    pub order_id: String,
    pub price: f64,
    pub quantity: f64,
    pub cl_ord_id: Option<String>,
}

/// Current state of the order book for a single symbol.
/// Bid side: descending prices (highest first). Ask side: ascending prices (lowest first).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolOrderBook {
    pub symbol: String,
    /// Resting bid orders keyed by order_id.
    pub bids: HashMap<u64, L3Order>,
    /// Resting ask orders keyed by order_id.
    pub asks: HashMap<u64, L3Order>,
    /// Tracks side by order_id (1=bid, 2=ask), needed for modify/delete messages.
    pub order_side: HashMap<u64, u8>,
    /// Monotonic insertion sequence used for FIFO time priority at a given price.
    pub next_time_priority: u64,
    /// Timestamp of last update (milliseconds since epoch)
    pub last_update_ms: u64,
}

impl SymbolOrderBook {
    pub fn new(symbol: String) -> Self {
        Self {
            symbol,
            bids: HashMap::new(),
            asks: HashMap::new(),
            order_side: HashMap::new(),
            next_time_priority: 1,
            last_update_ms: 0,
        }
    }

    /// Apply a snapshot: replace all bids and asks using synthetic order IDs per level.
    pub fn apply_snapshot(&mut self, bids: Vec<PriceLevel>, asks: Vec<PriceLevel>, timestamp_ms: u64) {
        self.bids.clear();
        self.asks.clear();
        self.order_side.clear();
        self.next_time_priority = 1;

        for (idx, level) in bids.into_iter().enumerate() {
            let order_id = (idx as u64) + 1;
            let time_priority = self.next_time_priority;
            self.next_time_priority = self.next_time_priority.saturating_add(1);
            self.bids.insert(
                order_id,
                L3Order {
                    order_id,
                    price: level.price,
                    quantity: level.quantity,
                    cl_ord_id: None,
                    time_priority,
                },
            );
            self.order_side.insert(order_id, 1);
        }

        for (idx, level) in asks.into_iter().enumerate() {
            let order_id = (1_u64 << 63) | ((idx as u64) + 1);
            let time_priority = self.next_time_priority;
            self.next_time_priority = self.next_time_priority.saturating_add(1);
            self.asks.insert(
                order_id,
                L3Order {
                    order_id,
                    price: level.price,
                    quantity: level.quantity,
                    cl_ord_id: None,
                    time_priority,
                },
            );
            self.order_side.insert(order_id, 2);
        }

        self.last_update_ms = timestamp_ms;
    }

    /// Add or replace an order by order_id.
    pub fn add_or_update_order(
        &mut self,
        order_id: u64,
        side: u8,
        price: f64,
        quantity: f64,
        cl_ord_id: Option<String>,
        timestamp_ms: u64,
    ) {
        let existing_order = self
            .bids
            .get(&order_id)
            .cloned()
            .or_else(|| self.asks.get(&order_id).cloned());

        if let Some(previous_side) = self.order_side.get(&order_id).copied() {
            if previous_side == 1 {
                self.bids.remove(&order_id);
            } else {
                self.asks.remove(&order_id);
            }
        }

        let preserved_cl_ord_id = cl_ord_id.or_else(|| existing_order.as_ref().and_then(|order| order.cl_ord_id.clone()));

        let time_priority = if let Some(existing) = existing_order {
            existing.time_priority
        } else {
            let current = self.next_time_priority;
            self.next_time_priority = self.next_time_priority.saturating_add(1);
            current
        };

        let order = L3Order {
            order_id,
            price,
            quantity,
            cl_ord_id: preserved_cl_ord_id,
            time_priority,
        };

        if side == 1 {
            self.bids.insert(order_id, order);
        } else {
            self.asks.insert(order_id, order);
        }
        self.order_side.insert(order_id, side);
        self.last_update_ms = timestamp_ms;
    }

    /// Delete an order by order_id.
    pub fn delete_order(&mut self, order_id: u64, timestamp_ms: u64) {
        match self.order_side.remove(&order_id) {
            Some(1) => {
                self.bids.remove(&order_id);
            }
            Some(2) => {
                self.asks.remove(&order_id);
            }
            _ => {}
        }
        self.last_update_ms = timestamp_ms;
    }

    /// Modify an existing order by order_id (side is inferred from order_side).
    pub fn modify_order(&mut self, order_id: u64, new_price: f64, new_quantity: f64, timestamp_ms: u64) {
        match self.order_side.get(&order_id).copied() {
            Some(1) => {
                if let Some(order) = self.bids.get_mut(&order_id) {
                    order.price = new_price;
                    order.quantity = new_quantity;
                }
            }
            Some(2) => {
                if let Some(order) = self.asks.get_mut(&order_id) {
                    order.price = new_price;
                    order.quantity = new_quantity;
                }
            }
            _ => {}
        }
        self.last_update_ms = timestamp_ms;
    }

    /// Record a trade (does not directly modify book, but can be used for stats)
    pub fn record_trade(&mut self, _side: u8, _price: f64, _quantity: f64, timestamp_ms: u64) {
        self.last_update_ms = timestamp_ms;
    }

    /// Apply a trade to the passive side of the book at the traded price.
    ///
    /// `aggressor_side`: 1=buy, 2=sell.
    /// A buy aggressor consumes asks, a sell aggressor consumes bids.
    pub fn apply_trade(&mut self, aggressor_side: u8, trade_price: f64, trade_qty: f64, timestamp_ms: u64) {
        if !(trade_qty.is_finite() && trade_qty > 0.0 && trade_price.is_finite()) {
            self.last_update_ms = timestamp_ms;
            return;
        }

        let passive_side = if aggressor_side == 1 { 2 } else { 1 };
        let mut remaining = trade_qty;
        let mut to_remove: Vec<u64> = Vec::new();

        let mut candidates: Vec<(u64, u64)> = match passive_side {
            1 => self
                .bids
                .iter()
                .filter_map(|(order_id, order)| {
                    if (order.price - trade_price).abs() <= 1e-9 {
                        Some((*order_id, order.time_priority))
                    } else {
                        None
                    }
                })
                .collect(),
            _ => self
                .asks
                .iter()
                .filter_map(|(order_id, order)| {
                    if (order.price - trade_price).abs() <= 1e-9 {
                        Some((*order_id, order.time_priority))
                    } else {
                        None
                    }
                })
                .collect(),
        };

        candidates.sort_by_key(|(order_id, time_priority)| (*time_priority, *order_id));

        for (order_id, _) in candidates {
            if remaining <= 1e-9 {
                break;
            }

            let maybe_order = if passive_side == 1 {
                self.bids.get_mut(&order_id)
            } else {
                self.asks.get_mut(&order_id)
            };

            let Some(order) = maybe_order else {
                continue;
            };

            let consumed = order.quantity.min(remaining);
            order.quantity -= consumed;
            remaining -= consumed;

            if order.quantity <= 1e-9 {
                to_remove.push(order_id);
            }
        }

        for order_id in to_remove {
            if passive_side == 1 {
                self.bids.remove(&order_id);
            } else {
                self.asks.remove(&order_id);
            }
            self.order_side.remove(&order_id);
        }

        self.last_update_ms = timestamp_ms;
    }

    pub fn l3_bids_sorted(&self) -> Vec<L3OrderView> {
        let mut orders: Vec<&L3Order> = self.bids.values().collect();

        orders.sort_by(|a, b| {
            b.price
                .partial_cmp(&a.price)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.time_priority.cmp(&b.time_priority))
                .then_with(|| a.order_id.cmp(&b.order_id))
        });

        orders
            .into_iter()
            .map(|order| L3OrderView {
                order_id: order.order_id.to_string(),
                price: order.price,
                quantity: order.quantity,
                cl_ord_id: order.cl_ord_id.clone(),
            })
            .collect()
    }

    pub fn l3_asks_sorted(&self) -> Vec<L3OrderView> {
        let mut orders: Vec<&L3Order> = self.asks.values().collect();

        orders.sort_by(|a, b| {
            a.price
                .partial_cmp(&b.price)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.time_priority.cmp(&b.time_priority))
                .then_with(|| a.order_id.cmp(&b.order_id))
        });

        orders
            .into_iter()
            .map(|order| L3OrderView {
                order_id: order.order_id.to_string(),
                price: order.price,
                quantity: order.quantity,
                cl_ord_id: order.cl_ord_id.clone(),
            })
            .collect()
    }
}

/// Global order book state, indexed by symbol.
#[derive(Debug, Clone)]
pub struct OrderBookState {
    pub books: HashMap<String, SymbolOrderBook>,
}

impl OrderBookState {
    pub fn new() -> Self {
        Self {
            books: HashMap::new(),
        }
    }

    pub fn get_or_create(&mut self, symbol: &str) -> &mut SymbolOrderBook {
        self.books
            .entry(symbol.to_string())
            .or_insert_with(|| SymbolOrderBook::new(symbol.to_string()))
    }

    pub fn get(&self, symbol: &str) -> Option<&SymbolOrderBook> {
        self.books.get(symbol)
    }
}
