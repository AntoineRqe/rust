use std::collections::{HashMap, VecDeque};
use std::{collections::BTreeMap, sync::atomic::AtomicBool};
use types::{
    FixedPointArithmetic, OrderEvent, OrderId, OrderResult, OrderStatus, OrderType, Side, StopHandle, Trade, TradeId, Trades
};

use spsc::spsc_lock_free::{Consumer, Producer};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

pub struct OrderBookEngine<'a, const N: usize> {
    fifo_in: Consumer<'a, OrderEvent, N>,
    fifo_out: Producer<'a, (OrderEvent, OrderResult), N>,
    order_book: OrderBook,
    shutdown: Arc<AtomicBool>,
}

impl<'a, const N: usize> OrderBookEngine<'a, N> {
    pub fn new(fifo_in: Consumer<'a, OrderEvent, N>, fifo_out: Producer<'a, (OrderEvent, OrderResult), N>) -> Self {
        OrderBookEngine {
            fifo_in,
            fifo_out,
            order_book: OrderBook::new(),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn run(&mut self) {
        loop {
            if let Some(event) = self.fifo_in.try_pop() {
                // WARNING: Fix the clone here, we should avoid cloning the event if possible.
                // Ideally, we would want to process the event in place and then send a reference to the result back to the FIX engine.
                // However, this would require changing the design of the queues to allow for references or using some form of shared memory.
                // For now, we will clone the event to keep it simple, but this is an area that can be optimized in the future.
                let result = self.order_book.process_order(event.clone());
                let mut execution_report_input = (event, result);
                while let Err((event_er, result_er)) = self.fifo_out.try_push(execution_report_input) {
                    execution_report_input = (event_er, result_er);
                    std::hint::spin_loop(); // If the output queue is full, spin until there is space
                }
            }

            if self.shutdown.load(Ordering::Relaxed) && self.fifo_in.is_empty() {
                break;
            }
        }
    }

    pub fn stop_handle(&mut self) -> StopHandle {
        StopHandle {
            shutdown: Arc::clone(&self.shutdown),
            thread: None, // We can set this to Some(handle) if we want to join the thread later
        }
    }
}

#[derive(Debug)]
struct OrderRef {
    side: Side,
    price: FixedPointArithmetic,
    position: usize, // Position in the order queue at the given price level
}

impl OrderRef {
    fn new(side: Side, price: FixedPointArithmetic, position: usize) -> Self {
        OrderRef { side, price, position }
    }
}

/// Represents the order book, maintaining separate heaps for bids and asks.
/// Bids are stored in a max-heap (higher prices have priority), while asks are stored in a min-heap (lower prices have priority).
/// The order book processes incoming orders, matches them against existing orders, and updates the order book accordingly.
/// - `bids`: A binary heap containing buy orders, sorted by price in descending order.
/// - `asks`: A binary heap containing sell orders, sorted by price in ascending order (using `Reverse` to achieve min-heap behavior).
/// - `id_counter`: A counter used to generate unique trade IDs for matched orders.
#[derive(Debug)]
pub struct OrderBook {
    pub bids: BTreeMap<FixedPointArithmetic, VecDeque<OrderEvent>>,
    pub asks: BTreeMap<FixedPointArithmetic, VecDeque<OrderEvent>>,
    id_counter: TradeId, // Counter for generating unique trade IDs, using a fixed-size array for simplicity
    order_map: HashMap<OrderId, OrderRef>, // Map to track orders by their ID for efficient cancellation
}

impl std::fmt::Display for OrderBook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Ask")?;
        for ask in self.asks.iter() {
            write!(f, "\n{}", ask.0)?;
        }
        write!(f, "\nBid")?;
        for bid in self.bids.iter() {
            write!(f, "\n{}", bid.0)?;
        }
        Ok(())
    }
}
impl OrderBook {
    pub fn new() -> Self {
        OrderBook {
            // Arbitrary initial capacity for the heaps to avoid frequent resizing; can be adjusted based on expected order volume.
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            id_counter: TradeId::new(), // Initialize the trade ID counter to zero
            order_map: HashMap::new(), // Initialize the order map
        }
    }

    /// Processes an incoming order by determining its type (limit or market) and side (buy or sell), and then calling the appropriate processing function. The function is instrumented with tracing to provide detailed logs of the order processing steps, including the order ID, side, price, and quantity.
    /// Arguments:
    /// - `order`: The incoming order to be processed, containing details such as price, quantity, side, order type, order ID, and broker ID.
    /// Returns:
    /// - An `OrderResult` containing the details of the processed order, including any trade ID and status.
    //#[instrument(level = "debug", skip(self, order), fields(order_id = order.order_id, side = ?order.side, price = order.price, quantity = order.quantity))]
    pub fn process_order(&mut self, order: OrderEvent) -> OrderResult {
        match order.order_type {
            OrderType::LimitOrder => self.process_limit_order(order),
            OrderType::MarketOrder => self.process_market_order(order),
            OrderType::CancelOrder => self.process_cancel_order(order),
        }
    }

    fn process_limit_order(&mut self, order: OrderEvent) -> OrderResult {
        match order.side {
            Side::Buy => self.process_buy_limit_order(order),
            Side::Sell => self.process_sell_limit_order(order),
        }
    }

    fn process_market_order(&mut self, order: OrderEvent) -> OrderResult {
        match order.side {
            Side::Buy => self.process_buy_market_order(order),
            Side::Sell => self.process_sell_market_order(order),
        }
    }

    fn process_cancel_order(&mut self, order: OrderEvent) -> OrderResult {
        if order.orig_cl_ord_id.is_none() {
            eprintln!("Cancel order with ID: {} is missing original client order ID, cannot process cancellation", order.order_id);
            return OrderResult {
                trades: Trades::default(),
                status: OrderStatus::CancelRejected,
                original_quantity: order.quantity,
                timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
            };
        }

        let orig_cl_ord_id = order.orig_cl_ord_id.unwrap();

        if let Some(order_ref) = self.order_map.get(&orig_cl_ord_id) {
            let order_queue = match order_ref.side {
                Side::Buy => self.bids.get_mut(&order_ref.price),
                Side::Sell => self.asks.get_mut(&order_ref.price),
            };

            if let Some(queue) = order_queue {
                if order_ref.position < queue.len() {

                    match queue.remove(order_ref.position) {
                        Some(_) => {
                            // Remove the price entry from the order if no more orders are left at that price level
                            if queue.is_empty() {
                                match order_ref.side {
                                    Side::Buy => self.bids.remove(&order_ref.price),
                                    Side::Sell => self.asks.remove(&order_ref.price),
                                };
                            }
                        }
                        None => eprintln!("Failed to cancel order with ID: {}, side: {:?}, price: {}, position: {}, order not found in queue", order.order_id, order_ref.side, order_ref.price, order_ref.position),
                    }

                    self.order_map.remove(&orig_cl_ord_id); // Remove the order from the map after cancellation

                    return OrderResult {
                        trades: Trades::default(),
                        status: OrderStatus::Cancelled,
                        original_quantity: order.quantity,
                        timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
                    };
                }
            }
        }

        eprintln!("Failed to cancel order with ID: {}, original client order ID: {}, order not found in order book", order.order_id, orig_cl_ord_id);
        // If we reach this point, it means the order was not found or could not be cancelled
        OrderResult {
            trades: Trades::default(),
            status: OrderStatus::CancelRejected,
            original_quantity: order.quantity,
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        }
    }

    /// Generates an `OrderResult` based on the processed order, including trade ID and status.
    /// The trade ID is generated if the order was partially or fully filled, and the status is determined based on the remaining quantity of the order.
    fn generate_order_result(&mut self, order: &OrderEvent, status: Option<OrderStatus>, original_quantity: FixedPointArithmetic, trades: Trades<4>) -> OrderResult {                        
        OrderResult {
            trades,
            status: status.unwrap_or_else(|| {
                if order.quantity == FixedPointArithmetic::ZERO {
                    OrderStatus::Filled
                } else if order.quantity < original_quantity {
                    OrderStatus::PartiallyFilled
                } else {
                    OrderStatus::New
                }
            }),
            original_quantity,
            timestamp: Instant::now(), // Timestamp can be set to the current time in milliseconds since epoch if needed for time-priority sorting in the future
        }
    }

    fn add_order_to_map(&mut self, order: &OrderEvent) {
        let position = match order.side {
            Side::Buy => self.bids.get(&order.price).map_or(0, |queue| queue.len()),
            Side::Sell => self.asks.get(&order.price).map_or(0, |queue| queue.len()),
        };
        self.order_map.insert(order.cl_ord_id, OrderRef::new(order.side, order.price, position));
    }

    /// Processes a sell limit order by matching it against the best available bids in the order book. If the order is not fully filled, it is added to the asks heap.
    /// Arguments:
    /// - `order`: The incoming sell limit order to be processed.
    /// Returns:
    /// - An `OrderResult` containing the details of the processed order, including any trade ID and status.
    fn process_sell_limit_order(&mut self, mut order: OrderEvent) -> OrderResult {
        let original_quantity = order.quantity;
        let mut trades = Trades::default();
        while let Some((&best_bid_price, _ )) = self.bids.first_key_value() {
            if best_bid_price >= order.price {
                // There is a matching bid, so we need to process the trades against the orders in the best bid queue
                let mut best_bid_queue = self.bids.pop_first().unwrap().1;

                while let Some(mut best_bid) = best_bid_queue.pop_front() {
                    let trade_quantity = order.quantity.min(best_bid.quantity);
                    // Process the trade here (e.g., update quantities, record the trade, etc.)
                    best_bid.quantity -= trade_quantity;
                    order.quantity -= trade_quantity;

                    // Add the trade to the list of trades for this order
                    if let Err(_) = trades.add_trade(Trade {
                        price: best_bid.price,
                        quantity: trade_quantity,
                        id: self.id_counter, // Example trade ID
                        timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
                    }) {
                        // Maximum Trades reached
                        eprintln!("Maximum number of trades reached for this order, some trades may not be recorded in the OrderResult");
                    }

                    self.id_counter.increment(); // Increment the trade ID counter
            
                    if best_bid.quantity > FixedPointArithmetic::ZERO {
                        best_bid_queue.push_front(best_bid);
                    } else {
                        // Remove the existing order from the order map if it has been completely filled
                        self.order_map.remove(&best_bid.cl_ord_id);
                    }

                    // Update the incoming order's quantity
                    if order.quantity == FixedPointArithmetic::ZERO {
                        if !best_bid_queue.is_empty() {
                            self.bids.insert(best_bid_price, best_bid_queue);
                        }

                        return self.generate_order_result(&order, Some(OrderStatus::Filled), original_quantity, trades);
                    }
                }
            } else {
                break;
            }
        }

        let order_result = self.generate_order_result(&order, None, original_quantity, trades);

        if order.quantity > FixedPointArithmetic::ZERO {
            // Add for future cancellation/modification (Before adding to order book for coherent queue positioning in the order map)
            self.add_order_to_map(&order);

            self.asks
            .entry(order.price)
            .or_insert_with(VecDeque::new)
            .push_back(order);

            // Add the order to the order map for potential future cancellation
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
        let mut trades = Trades::default();
        while let Some((&best_ask_price, _)) = self.asks.first_key_value() {
            if best_ask_price <= order.price {
                let mut best_ask_queue = self.asks.pop_first().unwrap().1; // Remove the best ask queue from the asks heap to process it
                
                while let Some(mut best_ask) = best_ask_queue.pop_front() {
                    let trade_quantity = order.quantity.min(best_ask.quantity);
                    // Process the trade here (e.g., update quantities, record the trade, etc.)
                    best_ask.quantity -= trade_quantity;
                    order.quantity -= trade_quantity;

                    // Add the trade to the list of trades for this order
                    if let Err(_) = trades.add_trade(Trade {
                        price: best_ask.price,
                        quantity: trade_quantity,
                        id: self.id_counter, // Example trade ID
                        timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
                    }) {
                        // Maximum Trades reached
                        eprintln!("Maximum number of trades reached for this order, some trades may not be recorded in the OrderResult");
                    }

                    self.id_counter.increment(); // Increment the trade ID counter

                    // If the best ask still has quantity remaining after the trade, push it back onto the asks
                    if best_ask.quantity > FixedPointArithmetic::ZERO {
                        best_ask_queue.push_front(best_ask);
                    } else {
                        // Remove the existing order from the order map if it has been completely filled
                        self.order_map.remove(&best_ask.cl_ord_id);
                    }

                    // Update the incoming order's quantity
                    if order.quantity == FixedPointArithmetic::ZERO {
                        if !best_ask_queue.is_empty() {
                            self.asks.insert(best_ask_price, best_ask_queue.clone());
                        }

                        return self.generate_order_result(&order, Some(OrderStatus::Filled), original_quantity, trades);
                    }
                }
            } else {
                break;
            }
        }

        let order_result = self.generate_order_result(&order, None, original_quantity, trades);

        if order.quantity > FixedPointArithmetic::ZERO {
            // Add for future cancellation/modification
            self.add_order_to_map(&order);

            // Add the remaining order to the order book if it was not fully filled
            self.bids
            .entry(order.price)
            .or_insert_with(VecDeque::new)
            .push_back(order);
        }

        order_result
    }

    fn process_buy_market_order(&mut self, mut order: OrderEvent) -> OrderResult {
        order.price = FixedPointArithmetic::from_f64(f64::INFINITY); // Market orders are treated as having an infinitely high price to ensure they match with the best available asks
        self.process_buy_limit_order(order)
    }

    fn process_sell_market_order(&mut self, mut order: OrderEvent) -> OrderResult {
        order.price = FixedPointArithmetic::from_f64(f64::NEG_INFINITY); // Market orders are treated as having an infinitely low price to ensure they match with the best available bids
        self.process_sell_limit_order(order)
    }

    /// Gets the best bid from the order book, which is the highest-priced buy order. Since bids are stored in a max-heap, we can directly access the top element.
    /// Returns:
    /// - An `Option<&OrderEvent>` containing a reference to the best bid if it exists
    pub fn get_best_bid(&self) -> Option<&OrderEvent> {
        self.bids
        .first_key_value()
        .and_then(|(_price, queue)| queue.front())
    }

    /// Gets the best ask from the order book, which is the lowest-priced sell order. Since asks are stored in a min-heap using `Reverse`, we need to access the inner `OrderEvent` from the `Reverse` wrapper.
    /// Returns:
    /// - An `Option<&OrderEvent>` containing a reference to the best ask if it exists, or `None` if there are no asks in the order book.
    pub fn get_best_ask(&self) -> Option<&OrderEvent> {
        self.asks
        .first_key_value()
        .and_then(|(_price, queue)| queue.front())
    }

    /// Calculates the spread of the order book, which is the difference between the best ask price and the best bid price. If either the best bid or best ask is not available, it returns `None`.
    /// Returns:
    /// - An `Option<FixedPointArithmetic>` containing the spread if both best bid and best ask are available, or `None` if either is missing.
    pub fn get_spread(&self) -> Option<FixedPointArithmetic> {
        match (self.get_best_bid(), self.get_best_ask()) {
            (Some(best_bid), Some(best_ask)) => Some(FixedPointArithmetic::from_raw(best_ask.price.raw() - best_bid.price.raw())),
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
            Side::Buy => self.bids.iter().flat_map(|(_price, queue)| queue.iter().cloned()).collect(),
            Side::Sell => self.asks.iter().flat_map(|(_price, queue)| queue.iter().cloned()).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{thread, time::Instant};

    use types::{EntityId, FixedString, OrderId, Side};
    use super::*;

    const SYMBOL: FixedString = FixedString::from_ascii("TEST_SYMBOL000000000");
    const SENDER: EntityId = EntityId::from_ascii("SENDER0000000000000");
    const TARGET: EntityId = EntityId::from_ascii("TARGET0000000000000");
    const CL_ORD_ID: OrderId = OrderId::from_ascii("12345");
    const ORDER_ID: OrderId = OrderId::from_ascii("54321");

    #[test]
    fn test_order_book_initialization() {
        let order_book = OrderBook::new();
        assert!(order_book.get_best_bid().is_none());
        assert!(order_book.get_best_ask().is_none());
        assert!(order_book.get_spread().is_none());
    }

    #[test]
    fn test_cancel_order() {
        let mut order_book = OrderBook::new();
        let order = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0), // 100.0 with 8 decimal places
            quantity: FixedPointArithmetic::from_f64(10.0), // 10.0 with 8 decimal places
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            order_id: ORDER_ID,
            orig_cl_ord_id: None,
            cl_ord_id: CL_ORD_ID,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL, // Set the symbol for the order
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };

        let result = order_book.process_order(order);
        assert_eq!(result.status, OrderStatus::New);

        let cancel_order = OrderEvent {
            price: FixedPointArithmetic::ZERO, // Price is not relevant for cancel orders
            quantity: FixedPointArithmetic::ZERO, // Quantity is not relevant for cancel orders
            side: Side::Buy, // Side is not relevant for cancel orders, but we can set it to match the original order
            order_type: OrderType::CancelOrder,
            order_id: ORDER_ID, // Use the same order ID to identify which order to cancel
            cl_ord_id: CL_ORD_ID, // Use a different ClOrdID for the cancel order
            orig_cl_ord_id: Some(CL_ORD_ID),
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL, // Set the symbol for the cancel order
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };

        let cancel_result = order_book.process_order(cancel_order);
        assert_eq!(cancel_result.status, OrderStatus::Cancelled);
        assert!(order_book.asks.is_empty()); // There should be no asks in the order book
        assert!(order_book.bids.is_empty()); // There should be no bids in the order book
        assert!(order_book.order_map.is_empty()); // There should be no asks in the order book

        // Testing cancellation of a sell order
        let order = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0), // 100.0 with 8 decimal places
            quantity: FixedPointArithmetic::from_f64(10.0), // 10.0 with 8 decimal places
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            order_id: ORDER_ID,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL, // Set the symbol for the order
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };

        let result = order_book.process_order(order);
        assert_eq!(result.status, OrderStatus::New);

        let cancel_order = OrderEvent {
            price: FixedPointArithmetic::ZERO, // Price is not relevant for cancel orders
            quantity: FixedPointArithmetic::ZERO, // Quantity is not relevant for cancel orders
            side: Side::Sell, // Side is not relevant for cancel orders, but we can set it to match the original order
            order_type: OrderType::CancelOrder,
            order_id: ORDER_ID, // Use the same order ID to identify which order to cancel
            cl_ord_id: CL_ORD_ID, // Use a different ClOrdID for the cancel order
            orig_cl_ord_id: Some(CL_ORD_ID),
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL, // Set the symbol for the cancel order
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };

        let cancel_result = order_book.process_order(cancel_order);
        assert_eq!(cancel_result.status, OrderStatus::Cancelled);
        assert_eq!(order_book.get_best_ask(), None); // The best ask should be removed after cancellation
        assert!(order_book.order_map.is_empty()); // There should be no asks in the order book

    }

    #[test]
    fn test_single_limit_order() {
        let mut order_book = OrderBook::new();
        let order = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0), // 100.0 with 8 decimal places
            quantity: FixedPointArithmetic::from_f64(10.0), // 10.0 with 8 decimal places
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            order_id: ORDER_ID,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL, // Set the symbol for the order
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };

        let result = order_book.process_order(order);

        assert_eq!(result.trades.len(), 0); // No trades executed
        assert_eq!(result.status, OrderStatus::New);
        assert_eq!(order_book.get_best_bid().unwrap().price, FixedPointArithmetic::from_f64(100.0)); // Best bid should be the price of the order
        assert_eq!(order_book.get_best_bid().unwrap().quantity, FixedPointArithmetic::from_f64(10.0)); // Best bid quantity should be the quantity of the order
    }

    #[test]
    fn test_trade_with_same_price() {
        let mut order_book = OrderBook::new();
        let order1 = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0), // 100.0 with 8 decimal places
            quantity: FixedPointArithmetic::from_f64(10.0), // 10.0 with 8 decimal places
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            order_id: ORDER_ID,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL, // Set the symbol for the order
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };
        let order2 = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0),
            quantity: FixedPointArithmetic::from_f64(5.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            order_id: ORDER_ID,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL, // Set the symbol for the order
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };

        let result1 = order_book.process_order(order1);
        let result2 = order_book.process_order(order2);

        assert_eq!(result1.trades.len(), 0); // No trades executed for the first order
        assert_eq!(result1.status, OrderStatus::New);

        assert_eq!(result2.trades.len(), 1); // One trade executed for the second order
        assert_eq!(result2.trades[0].quantity, FixedPointArithmetic::from_f64(5.0)); // 5 units filled
        assert_eq!(result2.trades[0].price, FixedPointArithmetic::from_f64(100.0)); // Trade price should be 100.0
        assert_eq!(result2.status, OrderStatus::Filled);
    }

    #[test]
    fn test_limit_orders_single_trade() {
        logging::init_tracing("order_book");

        let mut order_book = OrderBook::new();
        let order1 = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0), // 100.0 with 8 decimal places
            quantity: FixedPointArithmetic::from_f64(10.0), // 10.0 with 8 decimal places
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            order_id: ORDER_ID,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL, // Set the symbol for the order
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };
        let order2 = OrderEvent {
            price: FixedPointArithmetic::from_f64(99.0),
            quantity: FixedPointArithmetic::from_f64(5.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            order_id: ORDER_ID,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL, // Set the symbol for the order
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };
        let order3 = OrderEvent {
            price: FixedPointArithmetic::from_f64(98.0),
            quantity: FixedPointArithmetic::from_f64(10.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            order_id: ORDER_ID,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL, // Set the symbol for the order
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };

        let result1 = order_book.process_order(order1);
        let result2 = order_book.process_order(order2);
        let result3 = order_book.process_order(order3);

        // The first order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the bids.
        assert_eq!(result1.trades.len(), 0); // No trades executed,
        assert_eq!(result1.status, OrderStatus::New);

        // The second order should be completely filled (5 units filled, 0 units remaining).
        assert_eq!(result2.trades.len(), 1); // 5 units * 99.0 price
        assert_eq!(result2.trades[0].id, TradeId::new()); // Trade ID should be 0 for the first trade
        assert_eq!(result2.trades[0].quantity, FixedPointArithmetic::from_f64(5.0)); // 5 units filled
        assert_eq!(result2.trades[0].price, FixedPointArithmetic::from_f64(100.0)); // 100.0
        assert_eq!(result2.status, OrderStatus::Filled);

        // The third order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the asks.
        assert_eq!(result3.trades.len(), 1); // 5 units * 98.0 price
        assert_eq!(result3.trades[0].quantity, FixedPointArithmetic::from_f64(5.0)); // 5 units filled
        assert_eq!(result3.trades[0].price, FixedPointArithmetic::from_f64(100.0)); // 5 units * 100.0 price
        assert_eq!(result3.status, OrderStatus::PartiallyFilled);

        assert_eq!(order_book.bids.len(), 0); // One ask should remain in the order book
        assert_eq!(order_book.asks.len(), 1); // One ask should remain in the order book
        assert_eq!(order_book.asks.first_entry().unwrap().key(), &FixedPointArithmetic::from_f64(98.0)); // The remaining ask should be the one at 98.0
        assert_eq!(order_book.asks.first_entry().unwrap().get().get(0).unwrap().quantity, FixedPointArithmetic::from_f64(5.0)); // The remaining ask should have a quantity of 5
        assert_eq!(order_book.asks.first_entry().unwrap().get().get(0).unwrap().order_id, ORDER_ID); // The remaining ask should have the same order ID as the third order
        assert_eq!(order_book.asks.first_entry().unwrap().get().get(0).unwrap().sender_id, SENDER); // The remaining ask should have the same sender ID as the third order
        assert_eq!(order_book.asks.first_entry().unwrap().get().get(0).unwrap().target_id, TARGET); // The remaining ask should have the same target ID as the third order
        assert_eq!(order_book.asks.first_entry().unwrap().get().get(0).unwrap().order_type, OrderType::LimitOrder); // The remaining ask should have the same order type as the third order
        // Give time for the logs to be flushed before the test ends
        std::thread::sleep(std::time::Duration::from_millis(100));

    }

    #[test]
    fn test_limit_orders_multiple_trades() {
        logging::init_tracing("order_book");

        let mut order_book = OrderBook::new();

        let order1 = OrderEvent {
            price: FixedPointArithmetic::from_f64(99.0),
            quantity: FixedPointArithmetic::from_f64(3.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            order_id: ORDER_ID,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL,
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };
        let order2 = OrderEvent {
            price: FixedPointArithmetic::from_f64(98.0),
            quantity: FixedPointArithmetic::from_f64(5.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            order_id: ORDER_ID,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL,
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };
        let order3 = OrderEvent {
            price: FixedPointArithmetic::from_f64(97.0),
            quantity: FixedPointArithmetic::from_f64(3.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            order_id: ORDER_ID,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL,
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };
        let order4 = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0),
            quantity: FixedPointArithmetic::from_f64(10.0),
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            order_id: ORDER_ID,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL,
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };

        let result1 = order_book.process_order(order1);
        let result2 = order_book.process_order(order2);
        let result3 = order_book.process_order(order3);
        let result4 = order_book.process_order(order4);

        // The first order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the bids.
        assert_eq!(result1.trades.len(), 0); // No trades executed,
        assert_eq!(result1.status, OrderStatus::New);

        // The second order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the bids.
        assert_eq!(result2.trades.len(), 0); // No trades executed,
        assert_eq!(result2.status, OrderStatus::New);

        // The third order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the bids.
        assert_eq!(result3.trades.len(), 0); // No trades executed,
        assert_eq!(result3.status, OrderStatus::New);

        // The fourth order should be completely filled (3 units filled at 97.0, 5 units filled at 98.0, and 2 units filled at 99.0).
        assert_eq!(result4.trades.len(), 3); // 3 trades executed
        assert_eq!(result4.trades[0].quantity, FixedPointArithmetic::from_f64(3.0)); // 3 units filled
        assert_eq!(result4.trades[0].price, FixedPointArithmetic::from_f64(97.0)); // 3 units * 97.0 price
        assert_eq!(result4.trades[1].quantity, FixedPointArithmetic::from_f64(5.0)); // 5 units filled
        assert_eq!(result4.trades[1].price, FixedPointArithmetic::from_f64(98.0)); // 5 units * 98.0 price
        assert_eq!(result4.trades[2].quantity, FixedPointArithmetic::from_f64(2.0)); // 2 units filled
        assert_eq!(result4.trades[2].price, FixedPointArithmetic::from_f64(99.0)); // 2 units * 99.0 price
        assert_eq!(result4.status, OrderStatus::Filled);

        assert_eq!(order_book.asks.len(), 1); // One ask should remain in the order book
        assert_eq!(order_book.asks.first_entry().unwrap().key(), &FixedPointArithmetic::from_f64(99.0)); // The remaining ask should be the one at 99.0
        assert_eq!(order_book.asks.first_entry().unwrap().get().front().unwrap().quantity, FixedPointArithmetic::from_f64(1.0)); // The remaining ask should have a quantity of 1
        assert_eq!(order_book.asks.first_entry().unwrap().get().front().unwrap().order_id, ORDER_ID); // The remaining ask should have the same order ID as the first order
        assert_eq!(order_book.asks.first_entry().unwrap().get().front().unwrap().sender_id, SENDER); // The remaining ask should have the same sender ID as the first order
        assert_eq!(order_book.asks.first_entry().unwrap().get().front().unwrap().target_id, TARGET); // The remaining ask should have the same target ID as the first order
        assert_eq!(order_book.asks.first_entry().unwrap().get().front().unwrap().order_type, OrderType::LimitOrder);
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
            price: FixedPointArithmetic::from_f64(99.0),
            quantity: FixedPointArithmetic::from_f64(5.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            order_id: ORDER_ID,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL,
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };
        let order2 = OrderEvent {
            price: FixedPointArithmetic::from_f64(98.0),
            quantity: FixedPointArithmetic::from_f64(5.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            order_id: ORDER_ID,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL,
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };
        let order3 = OrderEvent {
            price: FixedPointArithmetic::from_f64(98.0),
            quantity: FixedPointArithmetic::from_f64(10.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            order_id: ORDER_ID,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL,
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };
        let order4 = OrderEvent {
            price: FixedPointArithmetic::from_f64(0.0), // Price is ignored for market orders
            quantity: FixedPointArithmetic::from_f64(12.0),
            side: Side::Buy,
            order_type: OrderType::MarketOrder,
            order_id: ORDER_ID,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL,
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };

        let result1 = order_book.process_order(order1);
        let result2 = order_book.process_order(order2);
        let result3 = order_book.process_order(order3);
        let result4 = order_book.process_order(order4);

        // The first three orders should be added to the asks heap as they are limit sell orders.
        assert_eq!(result1.trades.len(), 0); // No trades executed
        assert_eq!(result1.status, OrderStatus::New);

        assert_eq!(result2.trades.len(), 0); // No trades executed
        assert_eq!(result2.status, OrderStatus::New);

        assert_eq!(result3.trades.len(), 0); // No trades executed
        assert_eq!(result3.status, OrderStatus::New);

        // The fourth order should be completely filled (5 units filled at 98.0 and 7 units filled at 99.0).
        assert_eq!(result4.trades.len(), 2); // 2 trades executed
        assert_eq!(result4.trades[0].id, TradeId::default()); // Trade ID should be 1 for the first two trades
        assert_eq!(result4.trades[0].quantity, FixedPointArithmetic::from_f64(5.0)); // 5 units filled
        assert_eq!(result4.trades[0].price, FixedPointArithmetic::from_f64(98.0)); // 5 units * 98.0 price
        assert_eq!(result4.trades[1].id.0[19], 1); // Trade ID should be 2 for the second trade
        assert_eq!(result4.trades[1].quantity, FixedPointArithmetic::from_f64(7.0)); // 7 units filled
        assert_eq!(result4.trades[1].price, FixedPointArithmetic::from_f64(98.0)); // 7 units * 98.0 price
        assert_eq!(result4.status, OrderStatus::Filled);

        // Check the remaining orders in the order book after processing the market order
        assert_eq!(order_book.asks.len(), 2); // Two asks should remain in the order book
        assert_eq!(order_book.asks.first_entry().unwrap().key(), &FixedPointArithmetic::from_f64(98.0)); // The remaining ask should be the one at 98.0
        assert_eq!(order_book.asks.first_entry().unwrap().get().get(0).unwrap().quantity, FixedPointArithmetic::from_f64(3.0)); // The remaining ask should have a quantity of 3.0
        assert_eq!(order_book.asks.first_entry().unwrap().get().get(0).unwrap().order_id, ORDER_ID); // The remaining ask should have the same order ID as the third order
        assert_eq!(order_book.asks.first_entry().unwrap().get().get(0).unwrap().sender_id, SENDER); // The remaining ask should have the same sender ID as the third order
        assert_eq!(order_book.asks.first_entry().unwrap().get().get(0).unwrap().target_id, TARGET); // The remaining ask should have the same target ID as the third order
        assert_eq!(order_book.asks.first_entry().unwrap().get().get(0).unwrap().order_type, OrderType::LimitOrder); // The remaining ask should have the same order type as the third order
        assert_eq!(order_book.asks.iter().nth(1).unwrap().0, &FixedPointArithmetic::from_f64(99.0)); // The second remaining ask should be the one at 99.0
        assert_eq!(order_book.asks.iter().nth(1).unwrap().1.get(0).unwrap().quantity, FixedPointArithmetic::from_f64(5.0)); // The second remaining ask should have a quantity of 5.0

    }

    #[test]
    fn test_spread_calculation() {
        logging::init_tracing("order_book");
        let mut order_book = OrderBook::new();
        let order1 = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0),
            quantity: FixedPointArithmetic::from_f64(10.0),
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            order_id: ORDER_ID,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL,
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };
        let order2 = OrderEvent {
            price: FixedPointArithmetic::from_f64(102.0),
            quantity: FixedPointArithmetic::from_f64(5.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            order_id: ORDER_ID,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL,
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };

        order_book.process_order(order1);
        order_book.process_order(order2);

        let spread = order_book.get_spread();
        assert_eq!(spread, Some(FixedPointArithmetic::from_f64(2.0))); // Spread should be 102.0 - 100.0 = 2.0

        // Give time for the logs to be flushed before the test ends
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    #[test]
    fn test_dump_order_book() {
        logging::init_tracing("order_book");
        let mut order_book = OrderBook::new();
        let order1 = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0),
            quantity: FixedPointArithmetic::from_f64(10.0),
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            order_id: ORDER_ID,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL,
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };
        let order2 = OrderEvent {
            price: FixedPointArithmetic::from_f64(102.0),
            quantity: FixedPointArithmetic::from_f64(5.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            order_id: ORDER_ID,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL,
            timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
        };

        order_book.process_order(order1);
        order_book.process_order(order2);

        let bids = order_book.dump_order_book(Side::Buy);
        let asks = order_book.dump_order_book(Side::Sell);

        assert_eq!(bids.len(), 1); // One bid should be in the order book
        assert_eq!(bids[0].price, FixedPointArithmetic::from_f64(100.0)); // The bid should have the correct price
        assert_eq!(bids[0].quantity, FixedPointArithmetic::from_f64(10.0)); // The bid should have the correct quantity
        assert_eq!(bids[0].order_id, ORDER_ID); // The bid should have the correct order ID
        assert_eq!(bids[0].cl_ord_id, CL_ORD_ID); // The bid should have the correct client order ID
        assert_eq!(bids[0].target_id, TARGET); // The bid should have the correct target ID
        assert_eq!(bids[0].order_type, OrderType::LimitOrder); // The bid should have the correct order type

        assert_eq!(asks.len(), 1); // One ask should be in the order book
        assert_eq!(asks[0].price, FixedPointArithmetic::from_f64(102.0)); // The ask should have the correct price
        assert_eq!(asks[0].quantity, FixedPointArithmetic::from_f64(5.0)); // The ask should have the correct quantity
        assert_eq!(asks[0].order_id, ORDER_ID); // The ask should have the correct order ID
        assert_eq!(asks[0].cl_ord_id, CL_ORD_ID); // The ask should have the correct client order ID
        assert_eq!(asks[0].sender_id, SENDER); // The ask should have the correct sender ID
        assert_eq!(asks[0].target_id, TARGET); // The ask should have the correct target ID
        assert_eq!(asks[0].order_type, OrderType::LimitOrder); // The ask should have the correct order type
    }

    #[test]
    fn test_engine(){
        let mut inbound_queue = spsc::spsc_lock_free::RingBuffer::<OrderEvent, 1024>::new();
        let mut outbound_queue = spsc::spsc_lock_free::RingBuffer::<(OrderEvent, OrderResult), 1024>::new();

        thread::scope(|s| {
            let(inbound_producer, inbound_consumer) = inbound_queue.split();
            let(outbound_producer, outbound_consumer) = outbound_queue.split();

            let mut engine = OrderBookEngine::new(
                inbound_consumer,
                outbound_producer,
            );

            let stop_handle = engine.stop_handle();
            
            let handle = s.spawn(move || {
                engine.run();
            });

            // Send some orders to the engine
            let order = OrderEvent {
                price: FixedPointArithmetic::from_f64(100.0),
                quantity: FixedPointArithmetic::from_f64(10.0),
                side: Side::Buy,
                order_type: OrderType::LimitOrder,
                order_id: ORDER_ID,
                cl_ord_id: CL_ORD_ID,
                orig_cl_ord_id: None,
                sender_id: SENDER,
                target_id: TARGET,
                symbol: SYMBOL,
                timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
            };

            inbound_producer.push(order).unwrap();
            // Give some time for the engine to process the order
            std::thread::sleep(std::time::Duration::from_millis(100));

            let (order_event, order_result) = loop {
                if let Some((order_event, order_result)) = outbound_consumer.try_pop() {
                    break (order_event, order_result);
                }
            };

            assert!(order_event.price == FixedPointArithmetic::from_f64(100.0));
            assert!(order_result.trades.len() == 0);

            assert!(order_result.status == OrderStatus::New);
            
            // Send a matching sell order to the engine
            let order2 = OrderEvent {
                price: FixedPointArithmetic::from_f64(100.0),
                quantity: FixedPointArithmetic::from_f64(10.0),
                side: Side::Sell,
                order_type: OrderType::LimitOrder,
                order_id: ORDER_ID,
                cl_ord_id: CL_ORD_ID,
                orig_cl_ord_id: None,
                sender_id: SENDER,
                target_id: TARGET,
                symbol: SYMBOL,
                timestamp: Instant::now(), // Set the timestamp to the current time in milliseconds since epoch
            };

            inbound_producer.push(order2).unwrap();
            // Give some time for the engine to process the order
            std::thread::sleep(std::time::Duration::from_millis(100));

            let (order_event2, order_result2) = loop {
                if let Some((order_event, order_result)) = outbound_consumer.try_pop() {
                    break (order_event, order_result);
                }
            };

            assert!(order_event2.price == FixedPointArithmetic::from_f64(100.0));
            assert!(order_result2.trades.len() == 1); // One trade should be executed for the matching orders
            assert!(order_result2.status == OrderStatus::Filled); // Both orders should be filled


            stop_handle.stop();
            handle.join().unwrap();
        });
            // We can add tests for the engine here, but since it relies on the order book, we can focus on testing the order book functionality first.
    }
}