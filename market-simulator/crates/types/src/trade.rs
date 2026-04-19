use std::ops::Index;
use std::time::{SystemTime, UNIX_EPOCH};
use crate::arithmetic::FixedPointArithmetic;
use crate::macros::{OrderId};

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
    pub id: u64, // Trade ID can be up to 20 characters, we will use a fixed-size array for simplicity
    pub cl_ord_id: OrderId,
    pub order_qty: FixedPointArithmetic,
    pub leaves_qty: FixedPointArithmetic,
    pub timestamp: u64, // Timestamp in milliseconds since epoch, added for potential future use in time-priority sorting
}

impl Default for Trade {
    fn default() -> Self {
        Self {
            price: FixedPointArithmetic::ZERO,
            quantity: FixedPointArithmetic::ZERO,
            id: 0,
            cl_ord_id: OrderId::default(),
            order_qty: FixedPointArithmetic::ZERO,
            leaves_qty: FixedPointArithmetic::ZERO,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64, // current time in milliseconds
        }
    }
}

impl std::fmt::Display for Trade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Trade :
            \nprice: {}
            \nquantity: {}
            \nid: {:?}
            \ncl_ord_id: {}
            \norder_qty: {}
            \nleaves_qty: {}
            \ntimestamp: {}",
            self.price.raw(), self.quantity, self.id, self.cl_ord_id, self.order_qty, self.leaves_qty, self.timestamp
        )
    }
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
                id: 0,
                cl_ord_id: OrderId::default(),
                order_qty: FixedPointArithmetic::ZERO,
                leaves_qty: FixedPointArithmetic::ZERO,
                timestamp: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64,
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
