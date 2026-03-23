use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

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
        /// Last 4 characters of the stored password hash — used by the browser
        /// as a stable, unique suffix in ClOrdID generation.
        id_suffix: String,
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
    Disconnect,
}
