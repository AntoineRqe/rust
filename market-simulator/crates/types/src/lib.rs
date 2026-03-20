use std::time::Instant;
use std::{cmp::Ordering};
use std::ops::{Deref, Index};
use std::iter::Sum;

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
    pub symbol: FixedString, // FIX Symbol can be up to 20 characters, we will use a fixed-size array for simplicity
    pub order_type: OrderType,
    pub cl_ord_id: OrderId, // FIX ClOrdID can be up to 20 characters, we will use a fixed-size array for simplicity
    pub orig_cl_ord_id: Option<OrderId>, // FIX OrigClOrdID can be up to 20 characters, we will use a fixed-size array for simplicity
    pub order_id: OrderId, // FIX OrderID can be up to 20 characters, we will use a fixed-size array for simplicity
    pub sender_id: EntityId, // FIX SenderCompID can be up to 20 characters, we will use a fixed-size array for simplicity
    pub target_id: EntityId, // FIX TargetCompID can be up to 20 characters, we will use a fixed-size array for simplicity
    pub timestamp: Instant, // Timestamp in milliseconds since epoch, added for potential future use in time-priority sorting
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
            order_id: OrderId::default(),
            sender_id: EntityId::default(),
            target_id: EntityId::default(),
            symbol: FixedString::default(),
            timestamp: Instant::now(),
        }
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
        order_id: OrderId,
        sender_id: EntityId,
        target_id: EntityId,
        symbol: FixedString,
        timestamp: Instant,
    ) -> Self {
        Self {
            price,
            quantity,
            side,
            order_type,
            cl_ord_id,
            orig_cl_ord_id,
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

impl std::fmt::Display for OrderEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "OrderEvent {{ price: {}, quantity: {}, side: {:?}, order_type: {:?}, cl_ord_id: {:?}, orig_cl_ord_id: {:?}, order_id: {:?}, sender_id: {:?}, target_id: {:?}, symbol: {:?}, timestamp: {:?} }}",
            self.price.raw(), self.quantity, self.side, self.order_type, self.cl_ord_id, self.orig_cl_ord_id, self.order_id, self.sender_id, self.target_id, self.symbol, self.timestamp
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
/// - `cl_ord_id`: Client order ID of the matched resting order involved in this fill.
/// - `timestamp`: The timestamp when the trade occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trade {
    pub price: FixedPointArithmetic,
    pub quantity: FixedPointArithmetic,
    pub id: TradeId, // Trade ID can be up to 20 characters, we will use a fixed-size array for simplicity
    pub cl_ord_id: OrderId,
    pub order_qty: FixedPointArithmetic,
    pub leaves_qty: FixedPointArithmetic,
    pub timestamp: Instant, // Timestamp in milliseconds since epoch, added for potential future use in time-priority sorting
}

/// Represents the result of processing an order, including any trades that occurred and the final status of the order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trades<const N: usize> {
    pub trades: [Trade; N], // Fixed-size array for trades, adjust size as needed
    count: usize, // Number of valid trades in the array
}

impl<const N: usize> Default for Trades<N> {
    fn default() -> Self {
        Self {
            trades: [Trade {
                price: FixedPointArithmetic::ZERO,
                quantity: FixedPointArithmetic::ZERO,
                id: TradeId::new(),
                cl_ord_id: OrderId::default(),
                order_qty: FixedPointArithmetic::ZERO,
                leaves_qty: FixedPointArithmetic::ZERO,
                timestamp: Instant::now(),
            }; N],
            count: 0,
        }
    }
}

impl<const N: usize> Trades<N> {
    pub fn new() -> Self {
        Self::default()
    }


    pub fn add_trade(&mut self, trade: Trade) -> Result<(), &'static str> {
        if self.count < self.trades.len() {
            self.trades[self.count] = trade;
            self.count += 1;
            Ok(())
        } else {
            Err("Trade array is full")
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Trade> {
        self.trades[..self.count].iter()
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn quantity_sum(&self) -> FixedPointArithmetic {
        self.iter().fold(FixedPointArithmetic::ZERO, |acc, trade| acc + trade.quantity)
    }

    pub fn avg_price(&self) -> FixedPointArithmetic {
        if self.count == 0 {
            return FixedPointArithmetic::ZERO;
        }

        let total_quantity = self.quantity_sum();
        if total_quantity == FixedPointArithmetic::ZERO {
            return FixedPointArithmetic::ZERO;
        }

        let total_value = self.iter().fold(FixedPointArithmetic::ZERO, |acc, trade| acc + (trade.price * trade.quantity));
        total_value / total_quantity
    }
}

impl<const N: usize> Index<usize> for Trades<N> {
    type Output = Trade;

    fn index(&self, index: usize) -> &Self::Output {
        &self.trades[index]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderResult {
    pub trades: Trades<4>, // Fixed-size array for trades, adjust size as needed
    pub status: OrderStatus,
    pub timestamp: Instant, // Timestamp in milliseconds since epoch, added for potential future use in time-priority sorting
}

impl Default for OrderResult {
    fn default() -> Self {
        Self {
            trades: Trades::new(),
            status: OrderStatus::New,
            timestamp: Instant::now(),
        }
    }
}

impl std::fmt::Display for OrderResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "OrderResult {{ status: {:?} }}\n",
            self.status,
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

/// Represents the type of an order (limit or market).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderType {
    LimitOrder,
    MarketOrder,
    CancelOrder,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FixedString(pub [u8; 20]);

#[derive(Debug, Clone, Copy,PartialEq, Eq, Default)]
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

impl std::fmt::Display for EntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = std::str::from_utf8(&self.0).unwrap_or("<invalid utf-8>");
        write!(f, "{}", s.trim_matches(char::from(0)))
    }
}

impl std::fmt::Display for OrderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = std::str::from_utf8(&self.0).unwrap_or("<invalid utf-8>");
        write!(f, "{}", s.trim_matches(char::from(0)))
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

    pub fn new() -> Self {
        OrderId([0u8; 20]) // In a real implementation, you would want to generate unique IDs
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

impl std::ops::Add for OrderId {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        let mut result = [0u8; 20];
        for i in 0..20 {
            result[i] = self.0[i] ^ other.0[i]; // Simple XOR for demonstration, not a real ID generation strategy
        }
        OrderId(result)
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
pub struct FixedPointArithmetic(pub i64);


impl FixedPointArithmetic {
    pub const ZERO: FixedPointArithmetic = FixedPointArithmetic(0);
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

        Some(FixedPointArithmetic(if negative { -raw } else { raw }))
    }

    pub fn to_fix_bytes(self) -> [u8; 20] {
        let mut buf = [0u8; 20];
        let raw = self.0;
        let negative = raw < 0;
        let abs_raw = raw.abs();
        let integer_part = abs_raw / Self::SCALE;
        let frac_part = abs_raw % Self::SCALE;

        let mut i = 0;
        if negative {
            buf[i] = b'-';
            i += 1;
        }

        // Write integer part
        let int_str = integer_part.to_string();
        for b in int_str.as_bytes() {
            buf[i] = *b;
            i += 1;
        }

        buf[i] = b'.';
        i += 1;

        // Write fractional part, zero-padded to 8 digits
        let frac_str = format!("{:08}", frac_part);
        for b in frac_str.as_bytes() {
            buf[i] = *b;
            i += 1;
        }

        buf
    }

    pub fn from_f64(number: f64) -> Self {
        FixedPointArithmetic((number * Self::SCALE as f64).round() as i64)
    }
    
    pub fn from_raw(raw: i64) -> Self {
        FixedPointArithmetic(raw)
    }

    pub fn raw(self) -> i64 {
        self.0
    }

    pub fn from_number<T: Into<i64>>(num: T) -> Self {
        FixedPointArithmetic::from_raw(num.into() * Self::SCALE)
    }
}

impl Sum for FixedPointArithmetic {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        let mut total: i64 = 0;
        for price in iter {
            total += price.0;
        }
        FixedPointArithmetic::from_raw(total)
    }
}

impl std::ops::Add for FixedPointArithmetic {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        FixedPointArithmetic::from_raw(self.0 + other.0)
    }
}

impl std::ops::Sub for FixedPointArithmetic {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        FixedPointArithmetic::from_raw(self.0 - other.0)
    }
}

impl std::ops::Mul for FixedPointArithmetic {
    type Output = Self;

    fn mul(self, other: Self) -> Self {
        // (a * b) / SCALE to maintain the fixed-point representation
        FixedPointArithmetic::from_raw(((self.0 as i128 * other.0 as i128) / Self::SCALE as i128) as i64)
    }
}

impl std::ops::Div for FixedPointArithmetic {
    type Output = Self;

    fn div(self, other: Self) -> Self {
        // (a * SCALE) / b to maintain the fixed-point representation
        FixedPointArithmetic::from_raw(((self.0 as i128 * Self::SCALE as i128) / other.0 as i128) as i64)
    }
}

impl std::ops::AddAssign for FixedPointArithmetic {
    fn add_assign(&mut self, other: Self) {
        self.0 += other.0;
    }
}

impl std::ops::SubAssign for FixedPointArithmetic {
    fn sub_assign(&mut self, other: Self) {
        self.0 -= other.0;
    }
}

impl std::fmt::Display for FixedPointArithmetic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let raw = self.0;
        let integer_part = raw / Self::SCALE;
        let frac_part = (raw.abs() % Self::SCALE) as u64;
        write!(f, "{}.{:08}", integer_part, frac_part)
    }
}