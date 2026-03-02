use std::{cmp::Ordering};
use std::ops::Deref;

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
    pub quantity: u64, // In FIX, qty is a float but we will use integer for simplicity (e.g. 100.0 -> 100)
    pub side: Side,
    pub symbol: FixedString, // FIX Symbol can be up to 20 characters, we will use a fixed-size array for simplicity
    pub order_type: OrderType,
    pub cl_ord_id: OrderId, // FIX ClOrdID can be up to 20 characters, we will use a fixed-size array for simplicity
    pub order_id: OrderId, // FIX OrderID can be up to 20 characters, we will use a fixed-size array for simplicity
    pub sender_id: EntityId, // FIX SenderCompID can be up to 20 characters, we will use a fixed-size array for simplicity
    pub target_id: EntityId, // FIX TargetCompID can be up to 20 characters, we will use a fixed-size array for simplicity
    pub timestamp: u64, // Timestamp in milliseconds since epoch, added for potential future use in time-priority sorting
}

impl Default for OrderEvent {
    fn default() -> Self {
        Self {
            price: Price::PRICE_ZERO,
            quantity: 0,
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            cl_ord_id: OrderId::default(),
            order_id: OrderId::default(),
            sender_id: EntityId::default(),
            target_id: EntityId::default(),
            symbol: FixedString::default(),
            timestamp: 0,
        }
    }
}

impl OrderEvent {
    pub fn new(price: Price, quantity: u64, side: Side, order_type: OrderType, cl_ord_id: OrderId, order_id: OrderId, sender_id: EntityId, target_id: EntityId, symbol: FixedString, timestamp: u64) -> Self {
        Self {
            price,
            quantity,
            side,
            order_type,
            cl_ord_id,
            order_id,
            sender_id,
            target_id,
            symbol,
            timestamp,
        }
    }

    pub fn check_valid(&self) -> Result<(), &'static str> {
         if self.side != Side::Buy && self.side != Side::Sell {
            return Err("Invalid side");
        }
        if self.order_type != OrderType::LimitOrder && self.order_type != OrderType::MarketOrder {
            return Err("Invalid order type");
        }
        if self.quantity == 0 {
            return Err("Quantity cannot be zero");
        }
        if self.price.raw() < 0 {
            return Err("Price cannot be negative");
        }
        if self.sender_id.0.iter().all(|&b| b == 0) {
            return Err("Sender ID cannot be empty");
        }
        if self.target_id.0.iter().all(|&b| b == 0) {
            return Err("Target ID cannot be empty");
        }
        Ok(())
    }
}

impl std::fmt::Display for OrderEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "OrderEvent {{ price: {}, quantity: {}, side: {:?}, order_type: {:?}, cl_ord_id: {:?}, order_id: {:?}, sender_id: {:?}, target_id: {:?}, symbol: {:?}, timestamp: {} }}",
            self.price.raw(), self.quantity, self.side, self.order_type, self.cl_ord_id, self.order_id, self.sender_id, self.target_id, self.symbol, self.timestamp
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

impl TradeId {
    pub fn new() -> Self {
        TradeId([0u8; 20]) // In a real implementation, you would want to generate unique IDs
    }

    pub fn increment(&mut self) {
        for i in (0..self.0.len()).rev() {
            if self.0[i] < 255 {
                self.0[i] += 1;
                break;
            } else {
                self.0[i] = 0; // Reset to zero and carry over to the next byte
            }
        }
    }
}

/// Represents a trade that occurs when an order is matched in the order book.
/// Arguments:
/// - `price`: The price at which the trade occurred.
/// - `quantity`: The quantity that was traded.
/// - `id`: A unique identifier for the trade.
#[derive(Debug, Clone)]
pub struct Trade {
    pub price: Price,
    pub quantity: u64,
    pub id: TradeId, // Trade ID can be up to 20 characters, we will use a fixed-size array for simplicity
}

#[derive(Debug, Clone)]
pub struct OrderResult {
    pub trades: Vec<Trade>,
    pub status: OrderStatus,
    pub timestamp: u64, // Timestamp in milliseconds since epoch, added for potential future use in time-priority sorting
}

impl std::fmt::Display for OrderResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "OrderResult {{ status: {:?} }}\n",
            self.status
        )?;
        for trade in &self.trades {
            write!(
                f,
                "  Trade {{ price: {}, quantity: {}, id: {:?} }}\n",
                trade.price.raw(), trade.quantity, trade.id
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
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum OrderStatus {
    New,
    PartiallyFilled,
    Filled,
    Canceled,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FixedString(pub [u8; 20]);

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TradeId(pub [u8; 20]);

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Hash)]
pub struct OrderId(pub [u8; 20]);

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Hash)]
pub struct EntityId(pub [u8; 20]);

impl Deref for EntityId {
    type Target = [u8; 20];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Deref for FixedString {
    type Target = [u8; 20];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Deref for OrderId {
    type Target = [u8; 20];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Deref for TradeId {
    type Target = [u8; 20];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<[u8]> for OrderId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8]> for TradeId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8]> for EntityId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8]> for FixedString {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl EntityId {
    pub const fn from_ascii(s: &str) -> Self {
        let mut bytes = [0u8; 20];
        let s_bytes = s.as_bytes();
        let mut i = 0;
        while i < s_bytes.len() && i < 20 {
            bytes[i] = s_bytes[i];
            i += 1;
        }
        EntityId(bytes)
    }
}

impl OrderId {
    pub const fn from_ascii(s: &str) -> Self {
        let mut bytes = [0u8; 20];
        let s_bytes = s.as_bytes();
        let mut i = 0;
        while i < s_bytes.len() && i < 20 {
            bytes[i] = s_bytes[i];
            i += 1;
        }
        OrderId(bytes)
    }
}

impl FixedString {
    pub const fn from_ascii(s: &str) -> Self {
        let mut bytes = [0u8; 20];
        let s_bytes = s.as_bytes();
        let mut i = 0;
        while i < s_bytes.len() && i < 20 {
            bytes[i] = s_bytes[i];
            i += 1;
        }
        FixedString(bytes)
    }
}

/// Price represented as integer with implicit 8 decimal places
/// e.g. 123.45678900 -> 12_345_678_900
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Price(pub i64);


impl Price {
    pub const PRICE_ZERO: Price = Price(0);
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

impl std::fmt::Display for Price {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let raw = self.0;
        let integer_part = raw / Self::SCALE;
        let frac_part = (raw.abs() % Self::SCALE) as u64;
        write!(f, "{}.{:08}", integer_part, frac_part)
    }
}