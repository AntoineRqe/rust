use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use std::collections::HashMap;

/// A single order resting in the order book on behalf of a player.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingOrder {
    pub cl_ord_id: String,
    pub symbol: String,
    /// FIX side: "1" = buy, "2" = sell.
    pub side: String,
    pub qty: f64,
    pub price: f64,
}

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

/// Every event the FIX engine produces that the browser needs to know about.
/// This is a plain serializable type — no FIX internals leak into the web layer.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsEvent {
    /// A raw FIX message (exec report, market data, etc.)
    /// We send the human-readable form — SOH replaced by " | "
    FixMessage {
        label: String, // e.g. "EXEC_REPORT", "MARKET_DATA_SNAPSHOT"
        body:  String, // e.g. "8=FIX.4.2 | 9=123 | 35=8 | ..."
        tag:   String, // e.g. "exec_report", "market_data" — useful for CSS styling in the browser
        #[serde(skip_serializing_if = "Option::is_none")]
        recipient: Option<String>, // targeted browser username; None means broadcast to all
    },

    /// Connection status changed
    Status {
        connected: bool,
    },

    /// Sent to the browser immediately after a WebSocket connection is
    /// established, and after every order / cancel to keep the UI in sync.
    PlayerState {
        username: String,
        tokens: f64,
        pending_orders: Vec<PendingOrder>,
        holdings: HashMap<String, f64>,
        order_owners: HashMap<String, String>,
        is_admin: bool,
        visitor_count: usize,
        total_visitor_count: usize,
        /// Last 4 characters of the stored password hash — used by the browser
        /// as a stable, unique suffix in ClOrdID generation.
        id_suffix: String,
    },

    /// Active number of websocket visitors currently connected.
    VisitorCount {
        count: usize,
        total_count: usize,
    },

    /// Order book snapshot for a symbol (updated with market feed data).
    OrderBook {
        symbol: String,
        bids: Vec<L3OrderView>,
        asks: Vec<L3OrderView>,
        timestamp_ms: u64,
    },
}

/// Capacity of the broadcast channel.
/// If a slow browser client falls this many events behind, it gets dropped.
pub const BROADCAST_CAPACITY: usize = 256;

/// The shared handle passed into the web server and into handle_client.
/// Cloning it is cheap — broadcast::Sender is Arc-backed internally.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<WsEvent>,
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self { tx }
    }

    /// Called from the sync FIX thread — non-blocking, never fails silently.
    /// Returns the number of active browser subscribers that received the event.
    pub fn publish(&self, event: WsEvent) -> usize {
        // send() only errors if there are zero receivers — that's fine,
        // it just means no browser is connected right now.
        self.tx.send(event).unwrap_or(0)
    }

    /// Subscribe a new browser WebSocket connection to the event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<WsEvent> {
        self.tx.subscribe()
    }
}

/// Commands the browser can send over WebSocket.
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BrowserCommand {
    Order {
        clord_id: String,
        symbol: String,
        qty:    f64,
        price:  f64,
        side:   String,   // "1" = buy, "2" = sell
        sender: Option<String>,
        target: Option<String>,
    },
    Cancel {
        clord_id: String,
        symbol:   Option<String>,
        qty:      Option<f64>,
    },
    MdRequest {
        symbol: String,
        depth:  Option<u32>,
    },
    ResetTokens,
    ResetSeq,
    ClearBook,
    Disconnect,
}
