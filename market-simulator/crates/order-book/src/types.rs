use std::cmp::Ordering;

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
///   
#[derive(Debug, Clone)]
pub struct Order {
    pub price: f64,
    pub quantity: f64,
    pub side: Side,
    pub order_type: OrderType,
    pub id: u64,
}

impl std::fmt::Display for Order {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Order {{ price: {}, quantity: {}, side: {:?}, order_type: {:?}, id: {} }}",
            self.price, self.quantity, self.side, self.order_type, self.id
        )
    }
}

impl PartialEq for Order {
    fn eq(&self, other: &Self) -> bool {
        self.price == other.price
    }
}

impl Eq for Order {}

impl PartialOrd for Order {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Order {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher price first
        self.price
            .partial_cmp(&other.price)
            .unwrap_or(Ordering::Equal)
    }
}

/// Represents the side of an order (buy or sell).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

/// Represents the type of an order (limit or market).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderType {
    LimitOrder,
    MarketOrder,
}

/// Represents the status of an order after processing.
/// - `New`: The order is new and has not been processed yet.
/// - `PartiallyFilled`: The order has been partially filled, meaning some quantity has been matched, but there is still remaining quantity in the order book.
/// - `Filled`: The order has been completely filled, meaning all quantity has been matched and there is no remaining quantity in the order book.
/// - `NotMatched`: The order could not be matched with any existing orders in the order book, and remains in the order book as a new order.
/// - `Canceled`: The order has been canceled and removed from the order book.
#[derive(PartialEq, Eq, Debug)]
pub enum OrderStatus {
    New,
    PartiallyFilled,
    Filled,
    NotMatched,
    Canceled,
}

pub struct OrderResult {
    pub price: f64,
    pub quantity: f64,
    pub side: Side,
    pub order_type: OrderType,
    pub order_id: u64,
    pub trade_id: Option<u64>,
    pub status: OrderStatus,
}

impl std::fmt::Display for OrderResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "OrderResult {{ price: {}, quantity: {}, side: {:?}, order_type: {:?}, order_id: {}, trade_id: {:?}, status: {:?} }}",
            self.price, self.quantity, self.side, self.order_type, self.order_id, self.trade_id, self.status
        )
    }
}