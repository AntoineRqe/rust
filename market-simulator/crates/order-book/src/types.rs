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
/// - `broker_id`: The identifier of the broker placing the order.
#[derive(Debug, Clone)]
pub struct OrderEvent {
    pub price: Price,
    pub quantity: u64,
    pub side: Side,
    pub order_type: OrderType,
    pub order_id: u64,
    pub broker_id: u64,
}

impl std::fmt::Display for OrderEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "OrderEvent {{ price: {}, quantity: {}, side: {:?}, order_type: {:?}, order_id: {}, broker_id: {} }}",
            self.price.raw(), self.quantity, self.side, self.order_type, self.order_id, self.broker_id
        )
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

pub struct Trade {
    pub traded_price: Price,
    pub traded_quantity: u64,
    pub trade_id: u64,
}
pub struct OrderResult {
    pub original_price: Price,
    pub original_quantity: u64,
    pub trades: Vec<Trade>,
    pub side: Side,
    pub order_type: OrderType,
    pub order_id: u64,
    pub status: OrderStatus,
}

impl std::fmt::Display for OrderResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "OrderResult {{ original_price: {}, original_quantity: {}, side: {:?}, order_type: {:?}, order_id: {}, status: {:?} }}",
            self.original_price.raw(), self.original_quantity, self.side, self.order_type, self.order_id, self.status
        )?;
        for trade in &self.trades {
            write!(
                f,
                "  Trade {{ traded_price: {}, traded_quantity: {}, trade_id: {} }}\n",
                trade.traded_price.raw(), trade.traded_quantity, trade.trade_id
            )?;
        }
        Ok(())
    }
}

/// Price represented as integer with implicit 8 decimal places
/// e.g. 123.45678900 -> 12_345_678_900
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Price(i64);

impl Price {
    pub const SCALE: i64 = 100_000_000; // 10^8

    pub fn from_fix_bytes(bytes: &[u8]) -> Option<Self> {
        // parse "123.45678900" without any float conversion
        let mut integer_part: i64 = 0;
        let mut frac_part: i64 = 0;
        let mut frac_digits: i64 = 0;
        let mut in_frac = false;
        let mut negative = false;
        let mut i = 0;

        if bytes.first() == Some(&b'-') {
            negative = true;
            i += 1;
        }

        while i < bytes.len() {
            match bytes[i] {
                b'0'..=b'9' => {
                    let d = (bytes[i] - b'0') as i64;
                    if in_frac {
                        if frac_digits < 8 {
                            frac_part = frac_part * 10 + d;
                            frac_digits += 1;
                        }
                        // ignore extra decimal places
                    } else {
                        integer_part = integer_part * 10 + d;
                    }
                }
                b'.' => in_frac = true,
                _ => return None,
            }
            i += 1;
        }

        // pad fractional part to 8 digits
        // e.g. "123.45" -> frac_part=45, frac_digits=2 -> pad by 10^6
        let scale = 10_i64.pow((8 - frac_digits) as u32);
        let raw = integer_part * Self::SCALE + frac_part * scale;

        Some(Price(if negative { -raw } else { raw }))
    }

    pub fn from_f64(price: f64) -> Self {
        Price((price * Self::SCALE as f64).round() as i64)
    }
    
    pub fn from_raw(raw: i64) -> Self {
        Price(raw)
    }

    pub fn raw(self) -> i64 {
        self.0
    }
}