use std::cmp::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};
use crate::{SymbolId, EntityId, OrderId};
use crate::arithmetic::FixedPointArithmetic;
use crate::trade::{Trades};


// ---------------------------------------
// ---- Order and related types ----
// ---------------------------------------
/// Represents an order in the order book.
/// Orders are compared based on price for sorting in the order book.
/// For buy orders, higher prices have priority; for sell orders, lower prices have priority.
/// 
/// Arguments:
/// - `price`: The price of the order.
/// - `quantity`: The quantity of the order.
/// - `side`: The side of the order (buy or sell).
/// - `order_type`: The type of the order (limit or market).
/// - `id`: A unique identifier for the order.
/// - `broker_id`: The identifier of the broker placing the order.
#[derive(Debug, Clone, Copy)]
pub struct OrderEvent {
    pub price: FixedPointArithmetic,
    pub quantity: FixedPointArithmetic, // In FIX, qty is a float but we will use integer for simplicity (e.g. 100.0 -> 100)
    pub side: Side,
    pub symbol: SymbolId, // FIX Symbol can be up to 20 characters, we will use a fixed-size array for simplicity
    pub order_type: OrderType,
    pub cl_ord_id: OrderId, // FIX ClOrdID can be up to 20 characters, we will use a fixed-size array for simplicity
    pub orig_cl_ord_id: Option<OrderId>, // FIX OrigClOrdID can be up to 20 characters, we will use a fixed-size array for simplicity
    pub sender_id: EntityId, // FIX SenderCompID can be up to 20 characters, we will use a fixed-size array for simplicity
    pub target_id: EntityId, // FIX TargetCompID can be up to 20 characters, we will use a fixed-size array for simplicity
    pub timestamp_ms: u64, // Timestamp in milliseconds since epoch, added for potential future use in time-priority sorting
}

impl Default for OrderEvent {
    fn default() -> Self {
        Self {
            price: FixedPointArithmetic::ZERO,
            quantity: FixedPointArithmetic::ZERO,
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            cl_ord_id: OrderId::default(),
            orig_cl_ord_id: None,
            sender_id: EntityId::default(),
            target_id: EntityId::default(),
            symbol: SymbolId::default(),
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64, // current time in milliseconds
        }
    }
}

impl std::fmt::Display for OrderEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "\nOrderEvent:
            \tprice: {}
            \tquantity: {}
            \tside: {}
            \torder_type: {}
            \tcl_ord_id: {}
            \torig_cl_ord_id: {}
            \tsender_id: {}
            \ttarget_id: {}
            \tsymbol: {}
            \ttimestamp: {}",
            self.price.raw(),
            self.quantity,
            self.side,
            self.order_type,
            self.cl_ord_id,
            self.orig_cl_ord_id.map(|id| id.to_string()).unwrap_or("None".to_string()),
            self.sender_id,
            self.target_id,
            self.symbol,
            self.timestamp_ms
        )
    }
}

impl OrderEvent {
    pub fn new(
        price: FixedPointArithmetic,
        quantity: FixedPointArithmetic,
        side: Side,
        order_type: OrderType,
        cl_ord_id: OrderId,
        orig_cl_ord_id: Option<OrderId>,
        sender_id: EntityId,
        target_id: EntityId,
        symbol: SymbolId,
        timestamp_ms: u64,
    ) -> Self {
        Self {
            price,
            quantity,
            side,
            order_type,
            cl_ord_id,
            orig_cl_ord_id,
            sender_id,
            target_id,
            symbol,
            timestamp_ms,
        }
    }

    pub fn check_valid(&self) -> Result<(), &'static str> {
         if self.side != Side::Buy && self.side != Side::Sell {
            return Err("Invalid side");
        }

        if let Err(e) = self.check_general_valid() {
            return Err(e);
        }

        if self.order_type == OrderType::LimitOrder || self.order_type == OrderType::MarketOrder {
            return self.check_order_valid();
        } else if self.order_type == OrderType::CancelOrder{
            return self.check_cancel_valid();
        } else {
            return Err("Invalid order type");
        }

    }

    fn check_general_valid(&self) -> Result<(), &'static str> {
        if self.symbol.0.iter().all(|&b| b == 0) {
            return Err("Symbol cannot be empty");
        }
        if self.sender_id.0.iter().all(|&b| b == 0) {
            return Err("Sender ID cannot be empty");
        }
        if self.target_id.0.iter().all(|&b| b == 0) {
            return Err("Target ID cannot be empty");
        }
        Ok(())
    }

    fn check_cancel_valid(&self) -> Result<(), &'static str> {
        if self.order_type != OrderType::CancelOrder {
            return Err("Invalid order type for cancel order");
        }
        if self.orig_cl_ord_id.is_none() {
            return Err("OrigClOrdID is required for cancel orders");
        }
        Ok(())
    }

    fn check_order_valid(&self) -> Result<(), &'static str> {
        if self.quantity == FixedPointArithmetic::ZERO {
            return Err("Quantity cannot be zero");
        }
        if self.price == FixedPointArithmetic::ZERO {
            return Err("Price cannot be zero");
        }
        Ok(())
    }
}

impl PartialEq for OrderEvent {
    fn eq(&self, other: &Self) -> bool {
        self.price == other.price
    }
}

impl Eq for OrderEvent {}

impl PartialOrd for OrderEvent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderEvent {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher price first
        self.price
            .cmp(&other.price)
    }
}

// ---------------------------------------
// ---- OrderResult and related types ----
// ---------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderResult {
    pub internal_order_id: u64, // Internal order ID assigned by the engine, can be used for tracking and debugging
    pub trades: Trades<4>, // Fixed-size array for trades, adjust size as needed
    pub status: OrderStatus,
    pub timestamp_ms: u64, // Timestamp in milliseconds since epoch, added for potential future use in time-priority sorting
}

impl Default for OrderResult {
    fn default() -> Self {
        Self {
            internal_order_id: 0,
            trades: Trades::new(),
            status: OrderStatus::Unmatched,
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        }
    }
}

impl std::fmt::Display for OrderResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "\nOrderResult
            \tinternal_order_id: {}
             \tstatus: {}
             \ttimestamp_ms: {}",
             self.internal_order_id,
            self.status,
            self.timestamp_ms
        )?;
        for i in 0..self.trades.len() {
            let trade = self.trades[i];
            write!(
                f,
                "  Trade {{ price: {}, quantity: {}, id: {:?}, cl_ord_id: {} }}\n",
                trade.price.raw(), trade.quantity, trade.id, trade.cl_ord_id
            )?;
        }
        Ok(())
    }
}

/// Represents the side of an order (buy or sell).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Side::Buy => write!(f, "Buy"),
            Side::Sell => write!(f, "Sell"),
        }
    }
}

/// Represents the type of an order (limit or market).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderType {
    LimitOrder,
    MarketOrder,
    CancelOrder,
}

impl std::fmt::Display for OrderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderType::LimitOrder => write!(f, "Limit Order"),
            OrderType::MarketOrder => write!(f, "Market Order"),
            OrderType::CancelOrder => write!(f, "Cancel Order"),
        }
    }
}

/// Represents the status of an order after processing.
/// - `New`: The order is new and has not been processed yet.
/// - `PartiallyFilled`: The order has been partially filled, meaning some quantity has been matched, but there is still remaining quantity in the order book.
/// - `Filled`: The order has been completely filled, meaning all quantity has been matched and there is no remaining quantity in the order book.
/// - `NotMatched`: The order could not be matched with any existing orders in the order book, and remains in the order book as a new order.
/// - `Canceled`: The order has been canceled and removed from the order book.
#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum OrderStatus {
    New,
    PartiallyFilled,
    Filled,
    Cancelled,
    CancelRejected,
    Unmatched,
}

impl std::fmt::Display for OrderStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderStatus::New => write!(f, "New"),
            OrderStatus::PartiallyFilled => write!(f, "Partially Filled"),
            OrderStatus::Filled => write!(f, "Filled"),
            OrderStatus::Cancelled => write!(f, "Cancelled"),
            OrderStatus::CancelRejected => write!(f, "Cancel Rejected"),
            OrderStatus::Unmatched => write!(f, "Unmatched"),
        }
    }
}