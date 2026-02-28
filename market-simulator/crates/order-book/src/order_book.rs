use std::{cmp::Reverse, collections::BinaryHeap, sync::atomic::AtomicBool};
use crate::types::{
    OrderEvent, OrderResult, OrderStatus,
    OrderType::{
        LimitOrder,
        MarketOrder
    },
    Price,
    Side::{
        self,
        Buy,
        Sell
    },
    Trade,
    TradeId,
};

use spsc::spsc_lock_free::{Consumer, Producer};
use std::sync::atomic::Ordering;

pub struct OrderBookEngine<'a, const N: usize> {
    fifo_in: Consumer<'a, OrderEvent, N>,
    fifo_out: Producer<'a, OrderResult, N>,
    order_book: OrderBook,
    running: AtomicBool,
}

impl<'a, const N: usize> OrderBookEngine<'a, N> {
    pub fn new(fifo_in: Consumer<'a, OrderEvent, N>, fifo_out: Producer<'a, OrderResult, N>) -> Self {
        OrderBookEngine {
            fifo_in,
            fifo_out,
            order_book: OrderBook::new(),
            running: AtomicBool::new(true),
        }
    }

    pub fn run(&mut self) {
        while self.running.load(Ordering::Relaxed) {
            if let Some(event) = self.fifo_in.pop() {
                println!("Received order event: {}", event);
                let mut result = self.order_book.process_order(event);
                // Here you would typically send the result back to the FIX engine or log it
                while let Err(res) = self.fifo_out.push(result) {
                    result = res;
                    std::hint::spin_loop(); // If the output queue is full, spin until there is space
                }
                println!("Processed order event");
            } else {
                std::hint::spin_loop(); // No events to process, spin until new events arrive
            }
        }
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        println!("OrderBookEngine: Stopped");
    }
}

/// Represents the order book, maintaining separate heaps for bids and asks.
/// Bids are stored in a max-heap (higher prices have priority), while asks are stored in a min-heap (lower prices have priority).
/// The order book processes incoming orders, matches them against existing orders, and updates the order book accordingly.
/// - `bids`: A binary heap containing buy orders, sorted by price in descending order.
/// - `asks`: A binary heap containing sell orders, sorted by price in ascending order (using `Reverse` to achieve min-heap behavior).
/// - `id_counter`: A counter used to generate unique trade IDs for matched orders.
pub struct OrderBook {
    pub bids: BinaryHeap<OrderEvent>,
    pub asks: BinaryHeap<Reverse<OrderEvent>>,
    id_counter: TradeId, // Counter for generating unique trade IDs, using a fixed-size array for simplicity
}

impl std::fmt::Display for OrderBook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Ask")?;
        for ask in self.asks.iter() {
            write!(f, "\n{}", ask.0)?;
        }
        write!(f, "\nBid")?;
        for bid in self.bids.iter() {
            write!(f, "\n{}", bid)?;
        }
        Ok(())
    }
}
impl OrderBook {
    pub fn new() -> Self {
        OrderBook {
            // Arbitrary initial capacity for the heaps to avoid frequent resizing; can be adjusted based on expected order volume.
            bids: BinaryHeap::with_capacity(1024),
            asks: BinaryHeap::with_capacity(1024),
            id_counter: TradeId::new(), // Initialize the trade ID counter to zero
        }
    }

    /// Processes an incoming order by determining its type (limit or market) and side (buy or sell), and then calling the appropriate processing function. The function is instrumented with tracing to provide detailed logs of the order processing steps, including the order ID, side, price, and quantity.
    /// Arguments:
    /// - `order`: The incoming order to be processed, containing details such as price, quantity, side, order type, order ID, and broker ID.
    /// Returns:
    /// - An `OrderResult` containing the details of the processed order, including any trade ID and status.
    //#[instrument(level = "debug", skip(self, order), fields(order_id = order.order_id, side = ?order.side, price = order.price, quantity = order.quantity))]
    pub fn process_order(&mut self, order: OrderEvent) -> OrderResult {
        println!("Processing order: ID={:?}, Side={:?}, Price={}, Quantity={}", order.order_id, order.side, order.price, order.quantity);
        match order.order_type {
            LimitOrder => self.process_limit_order(order),
            MarketOrder => self.process_market_order(order),
        }
    }

    fn process_limit_order(&mut self, order: OrderEvent) -> OrderResult {
        match order.side {
            Buy => self.process_buy_limit_order(order),
            Sell => self.process_sell_limit_order(order),
        }
    }

    fn process_market_order(&mut self, order: OrderEvent) -> OrderResult {
        match order.side {
            Buy => self.process_buy_market_order(order),
            Sell => self.process_sell_market_order(order),
        }
    }

    /// Generates an `OrderResult` based on the processed order, including trade ID and status.
    /// The trade ID is generated if the order was partially or fully filled, and the status is determined based on the remaining quantity of the order.
    fn generate_order_result(&mut self, order: &OrderEvent, status: Option<OrderStatus>, original_quantity: u64, trades: Vec<Trade>) -> OrderResult {
        OrderResult {
            original_price: order.price,
            original_quantity,
            trades,
            side: order.side,
            order_type: order.order_type,
            sender_id: order.sender_id,
            target_id: order.target_id,
            order_id: order.order_id.clone(),
            status: status.unwrap_or_else(|| {
                if order.quantity == 0 {
                    OrderStatus::Filled
                } else if order.quantity < original_quantity {
                    OrderStatus::PartiallyFilled
                } else {
                    OrderStatus::NotMatched
                }
            }),
        }
    }

    /// Processes a sell limit order by matching it against the best available bids in the order book. If the order is not fully filled, it is added to the asks heap.
    /// Arguments:
    /// - `order`: The incoming sell limit order to be processed.
    /// Returns:
    /// - An `OrderResult` containing the details of the processed order, including any trade ID and status.
    fn process_sell_limit_order(&mut self, mut order: OrderEvent) -> OrderResult {
        let original_quantity = order.quantity;
        let mut trades: Vec<Trade> = Vec::with_capacity(10);

        while let Some(best_bid) = self.bids.peek() {
            if best_bid.price >= order.price {
                let mut best_bid = self.bids.pop().unwrap();
                let trade_quantity = order.quantity.min(best_bid.quantity);
                // Process the trade here (e.g., update quantities, record the trade, etc.)
                best_bid.quantity -= trade_quantity;

                // Add the trade to the list of trades for this order
                trades.push(Trade {
                    traded_price: best_bid.price,
                    traded_quantity: trade_quantity,
                    trade_id: self.id_counter.clone(), // Example trade ID
                });

                self.id_counter.increment(); // Increment the trade ID counter
        
                if best_bid.quantity > 0 {
                    self.bids.push(best_bid);
                }
                // Update the incoming order's quantity
                order.quantity -= trade_quantity;

                if order.quantity == 0 {
                    return self.generate_order_result(&order, Some(OrderStatus::Filled), original_quantity, trades);
                }
            } else {
                break;
            }
        }

        let order_result = self.generate_order_result(&order, None, original_quantity, trades);

        if order.quantity > 0 {
            self.asks.push(Reverse(order));
        }

        order_result
    }

    /// Processes a buy limit order by matching it against the best available asks in the order book. If the order is not fully filled, it is added to the bids heap.
    /// Arguments:
    /// - `order`: The incoming buy limit order to be processed.
    /// Returns:
    /// - An `OrderResult` containing the details of the processed order, including any trade ID and status.
    fn process_buy_limit_order(&mut self, mut order: OrderEvent) -> OrderResult {
        let original_quantity = order.quantity;
        let mut trades: Vec<Trade> = Vec::with_capacity(10);

        while let Some(Reverse(best_ask)) = self.asks.peek() {
            if best_ask.price <= order.price {
                let mut best_ask = self.asks.pop().unwrap().0;
                let trade_quantity = order.quantity.min(best_ask.quantity);
                // Process the trade here (e.g., update quantities, record the trade, etc.)
                best_ask.quantity -= trade_quantity;
                // Add the trade to the list of trades for this order
                trades.push(Trade {
                    traded_price: best_ask.price,
                    traded_quantity: trade_quantity,
                    trade_id: self.id_counter.clone(), // Example trade ID
                });
                self.id_counter.increment(); // Increment the trade ID counter

                // If the best ask still has quantity remaining after the trade, push it back onto the asks
                if best_ask.quantity > 0 {
                    self.asks.push(Reverse(best_ask));
                }
                // Update the incoming order's quantity
                order.quantity -= trade_quantity;

                if order.quantity == 0 {
                    return self.generate_order_result(&order, Some(OrderStatus::Filled), original_quantity, trades);
                }
            } else {
                break;
            }
        }

        let order_result = self.generate_order_result(&order, None, original_quantity, trades);

        if order.quantity > 0 {
            self.bids.push(order);
        }

        order_result
    }

    fn process_buy_market_order(&mut self, mut order: OrderEvent) -> OrderResult {
        order.price = Price::from_f64(f64::INFINITY); // Market orders are treated as having an infinitely high price to ensure they match with the best available asks
        self.process_buy_limit_order(order)
    }

    fn process_sell_market_order(&mut self, mut order: OrderEvent) -> OrderResult {
        order.price = Price::from_f64(f64::NEG_INFINITY); // Market orders are treated as having an infinitely low price to ensure they match with the best available bids
        self.process_sell_limit_order(order)
    }

    /// Gets the best bid from the order book, which is the highest-priced buy order. Since bids are stored in a max-heap, we can directly access the top element.
    /// Returns:
    /// - An `Option<&OrderEvent>` containing a reference to the best bid if it exists
    pub fn get_best_bid(&self) -> Option<&OrderEvent> {
        self.bids.peek()
    }

    /// Gets the best ask from the order book, which is the lowest-priced sell order. Since asks are stored in a min-heap using `Reverse`, we need to access the inner `OrderEvent` from the `Reverse` wrapper.
    /// Returns:
    /// - An `Option<&OrderEvent>` containing a reference to the best ask if it exists, or `None` if there are no asks in the order book.
    pub fn get_best_ask(&self) -> Option<&OrderEvent> {
        self.asks.peek().map(|reverse_order| &reverse_order.0)
    }

    /// Calculates the spread of the order book, which is the difference between the best ask price and the best bid price. If either the best bid or best ask is not available, it returns `None`.
    /// Returns:
    /// - An `Option<Price>` containing the spread if both best bid and best ask are available, or `None` if either is missing.
    pub fn get_spread(&self) -> Option<Price> {
        match (self.get_best_bid(), self.get_best_ask()) {
            (Some(best_bid), Some(best_ask)) => Some(Price::from_raw(best_ask.price.raw() - best_bid.price.raw())),
            _ => None,
        }
    }

    /// Dumps the current state of the order book for a given side (buy or sell) as a vector of orders. This can be useful for debugging or visualization purposes.
    /// Arguments:
    /// - `side`: The side of the order book to dump (either `Side::Buy` for bids or `Side::Sell` for asks).
    /// Returns:
    /// - A `Vec<OrderEvent>` containing the orders for the specified side of the order book. For bids, it returns the orders directly from the `bids` heap, and for asks, it extracts the inner `OrderEvent` from the `Reverse` wrapper in the `asks` heap.
    pub fn dump_order_book(&self, side: Side) -> Vec<OrderEvent> {
        match side {
            Buy => self.bids.iter().cloned().collect(),
            Sell => self.asks.iter().map(|reverse_order| reverse_order.0.clone()).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::types::OrderId;

    use super::*;

    #[test]
    fn test_limit_orders_single_trade() {
        logging::init_tracing("order_book");

        let mut order_book = OrderBook::new();
        let order1 = OrderEvent {
            price: Price::from_f64(100.0), // 100.0 with 8 decimal places
            quantity: 10,
            side: Buy,
            order_type: LimitOrder,
            order_id: OrderId::default(),
            sender_id: [0u8; 20],
            target_id: [0u8; 20],
            timestamp: 0,
        };
        let order2 = OrderEvent {
            price: Price::from_f64(99.0),
            quantity: 5,
            side: Sell,
            order_type: LimitOrder,
            order_id: OrderId::default(),
            sender_id: [0u8; 20],
            target_id: [0u8; 20],
            timestamp: 0,
        };
        let order3 = OrderEvent {
            price: Price::from_f64(98.0),
            quantity: 10,
            side: Sell,
            order_type: LimitOrder,
            order_id: OrderId::default(),
            sender_id: [0u8; 20],
            target_id: [0u8; 20],
            timestamp: 0,
        };

        let result1 = order_book.process_order(order1);
        let result2 = order_book.process_order(order2);
        let result3 = order_book.process_order(order3);

        // The first order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the bids.
        assert_eq!(result1.original_price, Price::from_f64(100.0));
        assert_eq!(result1.trades.len(), 0); // No trades executed,
        assert_eq!(result1.side, Buy);
        assert_eq!(result1.order_type, LimitOrder);
        assert_eq!(result1.order_id, OrderId::default());
        assert_eq!(result1.status, OrderStatus::NotMatched);

        // The second order should be completely filled (5 units filled, 0 units remaining).
        assert_eq!(result2.original_price, Price::from_f64(99.0));
        assert_eq!(result2.trades.len(), 1); // 5 units * 99.0 price
        assert_eq!(result2.side, Sell);
        assert_eq!(result2.order_type, LimitOrder);
        assert_eq!(result2.order_id, OrderId::default());
        assert_eq!(result2.trades[0].trade_id, TradeId::new()); // Trade ID should be 0 for the first trade
        assert_eq!(result2.trades[0].traded_quantity, 5); // 5 units filled
        assert_eq!(result2.trades[0].traded_price, Price::from_f64(100.0)); // 100.0
        assert_eq!(result2.status, OrderStatus::Filled);

        // The third order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the asks.
        assert_eq!(result3.original_price, Price::from_f64(98.0));
        assert_eq!(result3.trades.len(), 1); // 5 units * 98.0 price
        assert_eq!(result3.trades[0].traded_quantity, 5); // 5 units filled
        assert_eq!(result3.trades[0].traded_price, Price::from_f64(100.0)); // 5 units * 100.0 price
        assert_eq!(result3.side, Sell);
        assert_eq!(result3.order_type, LimitOrder);
        assert_eq!(result3.order_id, OrderId::default()); // Order ID should be 0 for the third order
        assert_eq!(result3.trades[0].trade_id.0[19], 1); // Trade ID should be 1 for the second trade
        assert_eq!(result3.status, OrderStatus::PartiallyFilled);

        assert_eq!(order_book.bids.len(), 0); // One ask should remain in the order book
        assert_eq!(order_book.asks.len(), 1); // One ask should remain in the order book
        assert_eq!(order_book.asks.peek().unwrap().0.price, Price::from_f64(98.0)); // The remaining ask should be the one at 98.0
        assert_eq!(order_book.asks.peek().unwrap().0.quantity, 5); // The remaining ask should have a quantity of 5
        assert_eq!(order_book.asks.peek().unwrap().0.order_id, OrderId::default()); // The remaining ask should have the same order ID as the third order
        assert_eq!(order_book.asks.peek().unwrap().0.sender_id, [0u8; 20]); // The remaining ask should have the same sender ID as the third order
        assert_eq!(order_book.asks.peek().unwrap().0.target_id, [0u8; 20]); // The remaining ask should have the same target ID as the third order
        assert_eq!(order_book.asks.peek().unwrap().0.target_id, [0u8; 20]); // The remaining ask should have the same target ID as the third order
        assert_eq!(order_book.asks.peek().unwrap().0.order_type, LimitOrder); // The remaining ask should have the same order type as the third order
        // Give time for the logs to be flushed before the test ends
        std::thread::sleep(std::time::Duration::from_millis(100));

    }

    #[test]
    fn test_limit_orders_multiple_trades() {
        logging::init_tracing("order_book");

        let mut order_book = OrderBook::new();

        let order1 = OrderEvent {
            price: Price::from_f64(99.0),
            quantity: 3,
            side: Sell,
            order_type: LimitOrder,
            order_id: OrderId::default(),
            sender_id: [0u8; 20],
            target_id: [0u8; 20],
            timestamp: 0,
        };
        let order2 = OrderEvent {
            price: Price::from_f64(98.0),
            quantity: 5,
            side: Sell,
            order_type: LimitOrder,
            order_id: OrderId::default(),
            sender_id: [0u8; 20],
            target_id: [0u8; 20],
            timestamp: 0,
        };
        let order3 = OrderEvent {
            price: Price::from_f64(97.0),
            quantity: 3,
            side: Sell,
            order_type: LimitOrder,
            order_id: OrderId::default(),
            sender_id: [0u8; 20],
            target_id: [0u8; 20],
            timestamp: 0,
        };
        let order4 = OrderEvent {
            price: Price::from_f64(100.0),
            quantity: 10,
            side: Buy,
            order_type: LimitOrder,
            order_id: OrderId::default(),
            sender_id: [0u8; 20],
            target_id: [0u8; 20],
            timestamp: 0,
        };

        let result1 = order_book.process_order(order1);
        let result2 = order_book.process_order(order2);
        let result3 = order_book.process_order(order3);
        let result4 = order_book.process_order(order4);

        // The first order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the bids.
        assert_eq!(result1.original_price, Price::from_f64(99.0));
        assert_eq!(result1.trades.len(), 0); // No trades executed,
        assert_eq!(result1.side, Sell);
        assert_eq!(result1.order_type, LimitOrder);

        assert_eq!(result1.order_id, OrderId::default()); // Order ID should be 0 for the first order
        for i in 0..20 {
            assert_eq!(result1.sender_id[i], 0u8); // Sender ID should be 0 for the first order
        }
         for i in 0..20 {
            assert_eq!(result1.target_id[i], 0u8); // Target ID should be 0 for the first order
        }
        assert_eq!(result1.status, OrderStatus::NotMatched);

        // The second order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the bids.
        assert_eq!(result2.original_price, Price::from_f64(98.0));
        assert_eq!(result2.trades.len(), 0); // No trades executed,
        assert_eq!(result2.side, Sell);
        assert_eq!(result2.order_type, LimitOrder);
        assert_eq!(result2.order_id, OrderId::default()); // Order ID should be 0 for the second order
        for i in 0..20 {
            assert_eq!(result2.sender_id[i], 0u8); // Sender ID should be 0 for the second order
        }
         for i in 0..20 {
            assert_eq!(result2.target_id[i], 0u8); // Target ID should be 0 for the second order
        }
        assert_eq!(result2.status, OrderStatus::NotMatched);

        // The third order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the bids.
        assert_eq!(result3.original_price, Price::from_f64(97.0));
        assert_eq!(result3.trades.len(), 0); // No trades executed,
        assert_eq!(result3.side, Sell);
        assert_eq!(result3.order_type, LimitOrder);
        assert_eq!(result3.order_id, OrderId::default()); // Order ID should be 0 for the third order
        assert_eq!(result3.sender_id, [0u8; 20]); // Sender ID should be 0 for the third order
        assert_eq!(result3.target_id, [0u8; 20]); // Target ID should be 0 for the third order
        assert_eq!(result3.status, OrderStatus::NotMatched);

        // The fourth order should be completely filled (3 units filled at 97.0, 5 units filled at 98.0, and 2 units filled at 99.0).
        assert_eq!(result4.original_price, Price::from_f64(100.0));
        assert_eq!(result4.trades.len(), 3); // 3 trades executed
        assert_eq!(result4.trades[0].traded_quantity, 3); // 3 units filled
        assert_eq!(result4.trades[0].traded_price, Price::from_f64(97.0)); // 3 units * 97.0 price
        assert_eq!(result4.trades[1].traded_quantity, 5); // 5 units filled
        assert_eq!(result4.trades[1].traded_price, Price::from_f64(98.0)); // 5 units * 98.0 price
        assert_eq!(result4.trades[2].traded_quantity, 2); // 2 units filled
        assert_eq!(result4.trades[2].traded_price, Price::from_f64(99.0)); // 2 units * 99.0 price
        assert_eq!(result4.side, Side::Buy);
        assert_eq!(result4.order_type, LimitOrder);
        assert_eq!(result4.order_id, OrderId::default()); // Order ID should be 0 for the fourth order
        assert_eq!(result4.sender_id, [0u8; 20]); // Sender ID should be 0 for the fourth order
        assert_eq!(result4.target_id, [0u8; 20]); // Target ID should be 0 for the fourth order
        assert_eq!(result4.status, OrderStatus::Filled);

        assert_eq!(order_book.asks.len(), 1); // One ask should remain in the order book
        assert_eq!(order_book.asks.peek().unwrap().0.price, Price::from_f64(99.0)); // The remaining ask should be the one at 99.0
        assert_eq!(order_book.asks.peek().unwrap().0.quantity, 1); // The remaining ask should have a quantity of 1
        assert_eq!(order_book.asks.peek().unwrap().0.order_id, OrderId::default()); // The remaining ask should have the same order ID as the first order
        assert_eq!(order_book.asks.peek().unwrap().0.sender_id, [0u8; 20]); // The remaining ask should have the same sender ID as the first order
        assert_eq!(order_book.asks.peek().unwrap().0.target_id, [0u8; 20]); // The remaining ask should have the same target ID as the first order
        assert_eq!(order_book.asks.peek().unwrap().0.order_type, LimitOrder);
// The remaining ask should have the same order type as the first order

        assert!(order_book.bids.is_empty()); // No bids should remain in the order book
    
        // Give time for the logs to be flushed before the test ends
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    #[test]
    fn test_market_orders() {
        logging::init_tracing("order_book");

        let mut order_book = OrderBook::new();

        let order1 = OrderEvent {
            price: Price::from_f64(99.0),
            quantity: 5,
            side: Side::Sell,
            order_type: LimitOrder,
            order_id: OrderId::default(),
            sender_id: [0u8; 20],
            target_id: [0u8; 20],
            timestamp: 0,
        };
        let order2 = OrderEvent {
            price: Price::from_f64(98.0),
            quantity: 5,
            side: Side::Sell,
            order_type: LimitOrder,
            order_id: OrderId::default(),
            sender_id: [0u8; 20],
            target_id: [0u8; 20],
            timestamp: 0,
        };
        let order3 = OrderEvent {
            price: Price::from_f64(98.0),
            quantity: 10,
            side: Side::Sell,
            order_type: LimitOrder,
            order_id: OrderId::default(),
            sender_id: [0u8; 20],
            target_id: [0u8; 20],
            timestamp: 0,
        };
        let order4 = OrderEvent {
            price: Price::from_f64(0.0), // Price is ignored for market orders
            quantity: 12,
            side: Side::Buy,
            order_type: MarketOrder,
            order_id: OrderId::default(),
            sender_id: [0u8; 20],
            target_id: [0u8; 20],
            timestamp: 0,
        };

        let result1 = order_book.process_order(order1);
        let result2 = order_book.process_order(order2);
        let result3 = order_book.process_order(order3);
        let result4 = order_book.process_order(order4);

        // The first three orders should be added to the asks heap as they are limit sell orders.
        assert_eq!(result1.original_price, Price::from_f64(99.0));
        assert_eq!(result1.trades.len(), 0); // No trades executed
        assert_eq!(result1.side, Sell);
        assert_eq!(result1.order_type, LimitOrder);
        assert_eq!(result1.order_id, OrderId::default());
        assert_eq!(result1.sender_id, [0u8; 20]);
        assert_eq!(result1.target_id, [0u8; 20]);
        assert_eq!(result1.status, OrderStatus::NotMatched);

        assert_eq!(result2.original_price, Price::from_f64(98.0));
        assert_eq!(result2.trades.len(), 0); // No trades executed
        assert_eq!(result2.side, Sell);
        assert_eq!(result2.order_type, LimitOrder);
         for i in 0..20 {
            assert_eq!(result2.sender_id[i], 0u8); // Sender ID should be 0 for the second order
        }
         for i in 0..20 {
            assert_eq!(result2.target_id[i], 0u8); // Target ID should be 0 for the second order
        }
        assert_eq!(result2.status, OrderStatus::NotMatched);

        assert_eq!(result3.original_price, Price::from_f64(98.0));
        assert_eq!(result3.trades.len(), 0); // No trades executed
        assert_eq!(result3.side, Sell);
        assert_eq!(result3.order_type, LimitOrder);
         for i in 0..20 {
            assert_eq!(result3.sender_id[i], 0u8); // Sender ID should be 0 for the third order
        }
         for i in 0..20 {
            assert_eq!(result3.target_id[i], 0u8); // Target ID should be 0 for the third order
        }
        assert_eq!(result3.order_id, OrderId::default());
        assert_eq!(result3.sender_id, [0u8; 20]);
        assert_eq!(result3.target_id, [0u8; 20]);
        assert_eq!(result3.status, OrderStatus::NotMatched);

        // The fourth order should be completely filled (5 units filled at 98.0 and 7 units filled at 99.0).
        assert_eq!(result4.original_price, Price::from_f64(f64::INFINITY)); // Market orders are treated as having an infinitely high price
        assert_eq!(result4.trades.len(), 2); // 2 trades executed
        assert_eq!(result4.trades[0].trade_id, TradeId::default()); // Trade ID should be 1 for the first two trades
        assert_eq!(result4.trades[0].traded_quantity, 5); // 5 units filled
        assert_eq!(result4.trades[0].traded_price, Price::from_f64(98.0)); // 5 units * 98.0 price
        assert_eq!(result4.trades[1].trade_id.0[19], 1); // Trade ID should be 2 for the second trade
        assert_eq!(result4.trades[1].traded_quantity, 7); // 7 units filled
        assert_eq!(result4.trades[1].traded_price, Price::from_f64(98.0)); // 7 units * 98.0 price
        assert_eq!(result4.side, Buy);
        assert_eq!(result4.order_type, MarketOrder);
        assert_eq!(result4.order_id, OrderId::default()); // Order ID should be 0 for the fourth order
        assert_eq!(result4.sender_id, [0u8; 20]); // Sender ID should be 0 for the fourth order
        assert_eq!(result4.target_id, [0u8; 20]); // Target ID should be 0 for the fourth order
        assert_eq!(result4.status, OrderStatus::Filled);

        // Check the remaining orders in the order book after processing the market order
        assert_eq!(order_book.asks.len(), 2); // Two asks should remain in the order book
        assert_eq!(order_book.asks.peek().unwrap().0.price, Price::from_f64(98.0)); // The remaining ask should be the one at 98.0
        assert_eq!(order_book.asks.peek().unwrap().0.quantity, 3); // The remaining ask should have a quantity of 3.0
        assert_eq!(order_book.asks.peek().unwrap().0.order_id, OrderId::default()); // The remaining ask should have the same order ID as the third order
        assert_eq!(order_book.asks.peek().unwrap().0.sender_id, [0u8; 20]); // The remaining ask should have the same sender ID as the third order
        assert_eq!(order_book.asks.peek().unwrap().0.target_id, [0u8; 20]); // The remaining ask should have the same target ID as the third order
        assert_eq!(order_book.asks.peek().unwrap().0.order_type, LimitOrder); // The remaining ask should have the same order type as the third order
        assert_eq!(order_book.asks.iter().nth(1).unwrap().0.price, Price::from_f64(99.0)); // The second remaining ask should be the one at 99.0
        assert_eq!(order_book.asks.iter().nth(1).unwrap().0.quantity, 5); // The second remaining ask should have a quantity of 5.0

    }

    #[test]
    fn test_spread_calculation() {
        logging::init_tracing("order_book");
        let mut order_book = OrderBook::new();
        let order1 = OrderEvent {
            price: Price::from_f64(100.0),
            quantity: 10,
            side: Buy,
            order_type: LimitOrder,
            order_id: OrderId::default(),
            sender_id: [0u8; 20],
            target_id: [0u8; 20],
            timestamp: 0,
        };
        let order2 = OrderEvent {
            price: Price::from_f64(102.0),
            quantity: 5,
            side: Sell,
            order_type: LimitOrder,
            order_id: OrderId::default(),
            sender_id: [0u8; 20],
            target_id: [0u8; 20],
            timestamp: 0,
        };

        order_book.process_order(order1);
        order_book.process_order(order2);

        let spread = order_book.get_spread();
        assert_eq!(spread, Some(Price::from_f64(2.0))); // Spread should be 102.0 - 100.0 = 2.0

        // Give time for the logs to be flushed before the test ends
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    #[test]
    fn test_dump_order_book() {
        logging::init_tracing("order_book");
        let mut order_book = OrderBook::new();
        let order1 = OrderEvent {
            price: Price::from_f64(100.0),
            quantity: 10,
            side: Buy,
            order_type: LimitOrder,
            order_id: OrderId::default(),
            sender_id: [0u8; 20],
            target_id: [0u8; 20],
            timestamp: 0,
        };
        let order2 = OrderEvent {
            price: Price::from_f64(102.0),
            quantity: 5,
            side: Sell,
            order_type: LimitOrder,
            order_id: OrderId::default(),
            sender_id: [0u8; 20],
            target_id: [0u8; 20],
            timestamp: 0,
        };

        order_book.process_order(order1);
        order_book.process_order(order2);

        let bids = order_book.dump_order_book(Buy);
        let asks = order_book.dump_order_book(Sell);

        assert_eq!(bids.len(), 1); // One bid should be in the order book
        assert_eq!(bids[0].price, Price::from_f64(100.0)); // The bid should have the correct price
        assert_eq!(bids[0].quantity, 10); // The bid should have the correct quantity
        assert_eq!(bids[0].order_id, OrderId::default()); // The bid should have the correct order ID
        assert_eq!(bids[0].sender_id, [0u8; 20]); // The bid should have the correct sender ID
        assert_eq!(bids[0].target_id, [0u8; 20]); // The bid should have the correct target ID
        assert_eq!(bids[0].order_type, LimitOrder); // The bid should have the correct order type

        assert_eq!(asks.len(), 1); // One ask should be in the order book
        assert_eq!(asks[0].price, Price::from_f64(102.0)); // The ask should have the correct price
        assert_eq!(asks[0].quantity, 5); // The ask should have the correct quantity
        assert_eq!(asks[0].order_id, OrderId::default()); // The ask should have the correct order ID
        assert_eq!(asks[0].sender_id, [0u8; 20]); // The ask should have the correct sender ID
        assert_eq!(asks[0].target_id, [0u8; 20]); // The ask should have the correct target ID
        assert_eq!(asks[0].order_type, LimitOrder); // The ask should have the correct order type
    }
}