use std::collections::{HashMap, VecDeque};
use std::{collections::BTreeMap};
use types::{
    FixedPointArithmetic,
    OrderEvent,
    OrderResult,
    OrderStatus,
    OrderType,
    Side,
    Trade,
    Trades,
    macros::{
        OrderId,
    }
};

use utils::market_name;


/// Represents a reference to an order in the order book, including its side (buy or sell), price, and position in the order queue at the given price level.
/// This struct is used to efficiently track orders for cancellation and modification purposes, allowing for quick access to the order's location in the order book without needing to search through the entire book.
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
/// - `trade_id_counter`: A counter used to generate unique trade IDs for matched orders.
#[derive(Debug)]
pub struct OrderBook {
    /// Bids are stored in a BTreeMap where the key is the price and the value is a queue of orders at that price level. The BTreeMap allows us to efficiently access the best bid (highest price) and maintain the orders in price levels.
    pub bids: BTreeMap<FixedPointArithmetic, VecDeque<OrderEvent>>,
    /// Asks are stored in a BTreeMap where the key is the price and the value is a queue of orders at that price level. The BTreeMap allows us to efficiently access the best ask (lowest price) and maintain the orders in price levels.
    pub asks: BTreeMap<FixedPointArithmetic, VecDeque<OrderEvent>>,
    /// Internal counter for generating unique order IDs for incoming orders. This is used to assign an internal order ID to each order as it is processed, which can be useful for tracking and referencing orders within the order book.
    pub(crate) internal_id_counter: u64,
    /// Counter for generating unique trade IDs for matched orders. Each time a trade is executed, a new trade ID is generated using this counter to ensure that each trade can be uniquely identified and tracked.
    pub(crate) trade_id_counter: u64,
    /// Map to track orders by their ID for efficient cancellation and modification.
    order_map: HashMap<OrderId, OrderRef>,
    /// The symbol for this order book.
    pub(crate) symbol: String,
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
    pub fn new(symbol: &str) -> Self {
        OrderBook {
            // Arbitrary initial capacity for the heaps to avoid frequent resizing; can be adjusted based on expected order volume.
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            internal_id_counter: 1, // Start at 1; 0 is reserved as the sentinel "no ID" value
            trade_id_counter: 1, // Start at 1; 0 is reserved as the sentinel "no ID" value
            order_map: HashMap::new(), // Initialize the order map
            symbol: symbol.to_string(), // Set the symbol for this order book
        }
    }

    fn generate_internal_order_id(&mut self) -> u64 {
        let id = self.internal_id_counter;
        self.internal_id_counter += 1;
        id
    }

    fn generate_trade_id(&mut self) -> u64 {
        let id = self.trade_id_counter;
        self.trade_id_counter += 1;
        id
    }

    /// Processes an incoming order by determining its type (limit or market) and side (buy or sell), and then calling the appropriate processing function. The function is instrumented with tracing to provide detailed logs of the order processing steps, including the order ID, side, price, and quantity.
    /// Arguments:
    /// - `order`: The incoming order to be processed, containing details such as price, quantity, side, order type, order ID, and broker ID.
    /// Returns:
    /// - An `OrderResult` containing the details of the processed order, including any trade ID and status.
    //#[instrument(level = "debug", skip(self, order), fields(order_id = order.order_id, side = ?order.side, price = order.price, quantity = order.quantity))]
    pub fn process_order(&mut self, order: OrderEvent) -> (OrderEvent, OrderResult) {
        match order.order_type {
            OrderType::LimitOrder => self.process_limit_order(order),
            OrderType::MarketOrder => self.process_market_order(order),
            OrderType::CancelOrder => self.process_cancel_order(order),
        }
    }

    /// Processes a limit order by matching it against existing orders in the order book based on its side (buy or sell). For buy limit orders, it matches against the best available asks, and for sell limit orders, it matches against the best available bids. If the order is not fully filled after matching, it is added to the appropriate side of the order book (bids for buy orders and asks for sell orders) for future matching.
    /// Arguments:
    /// - `order`: The incoming limit order to be processed, containing details such as price, quantity, side, order ID, and broker ID.
    /// Returns:
    /// - An `OrderResult` containing the details of the processed order, including any trade ID and status. The trade ID is generated if the order was partially or fully filled, and the status is determined based on the remaining quantity of the order.
    fn process_limit_order(&mut self, order: OrderEvent) -> (OrderEvent, OrderResult) {
        match order.side {
            Side::Buy => {
                let (order, result) = self.process_buy_limit_order(order);
                (order, result)
            },
            Side::Sell => {
                let (order, result) = self.process_sell_limit_order(order);
                (order, result)
            },
        }
    }

    /// Processes a market order by treating it as a limit order with an infinitely high price for buy orders or an infinitely low price for sell orders. This ensures that market orders will match with the best available prices in the order book. The function then calls the appropriate processing function for limit orders to handle the matching and execution of the market order.
    /// Arguments:
    /// - `order`: The incoming market order to be processed, containing details such as price, quantity, side, order ID, and broker ID.
    /// Returns:
    /// - An `OrderResult` containing the details of the processed order, including any trade ID and status. The trade ID is generated if the order was partially or fully filled, and the status is determined based on the remaining quantity of the order.
    fn process_market_order(&mut self, order: OrderEvent) -> (OrderEvent, OrderResult) {
        match order.side {
            Side::Buy => {
                let (order, result) = self.process_buy_market_order(order);
                (order, result)
            },
            Side::Sell => {
                let (order, result) = self.process_sell_market_order(order);
                (order, result)
            },
        }
    }

    /// Processes a cancel order by looking up the original order using the `orig_cl_ord_id` and removing it from the order book if it exists. The function checks for the validity of the cancel order, including the presence of the original client order ID and the existence of the original order in the order book. If the cancellation is successful, it returns an `OrderResult` with a status of `Cancelled`. If the cancellation fails (e.g., due to missing original client order ID or order not found), it returns an `OrderResult` with a status of `CancelRejected`.
    /// Arguments:
    /// - `order`: The incoming cancel order to be processed, containing details such as the original client order ID, order ID, and broker ID.
    /// Returns:
    /// - An `OrderResult` containing the details of the processed cancel order, including any trade ID and status. The status is determined based on the success or failure of the cancellation.
    fn process_cancel_order(&mut self, order: OrderEvent) -> (OrderEvent, OrderResult) {
        let orig_cl_ord_id = if let Some(orig_cl_ord_id) = order.orig_cl_ord_id {
            orig_cl_ord_id
        } else {
            tracing::error!("[{}][{}][{}] Cancel order with ID: {} is missing original client order ID, cannot process cancellation", market_name(), order.symbol, order.cl_ord_id, order.cl_ord_id);
            return (order, OrderResult {
                internal_order_id: 0, // No internal order ID since the cancellation cannot be processed
                trades: Trades::default(),
                status: OrderStatus::CancelRejected,
                ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
            });
        };

        if let Some(order_ref) = self.order_map.get(&orig_cl_ord_id) {
            let order_queue = match order_ref.side {
                Side::Buy => self.bids.get_mut(&order_ref.price),
                Side::Sell => self.asks.get_mut(&order_ref.price),
            };

            if let Some(queue) = order_queue {
                // Fast path: use cached position if still valid and points to the same order.
                let index = if order_ref.position < queue.len()
                    && queue[order_ref.position].cl_ord_id == orig_cl_ord_id
                {
                    Some(order_ref.position)
                } else {
                    // Fallback: positions can become stale after fills/cancels at the same price level.
                    queue.iter().position(|o| o.cl_ord_id == orig_cl_ord_id)
                };

                if let Some(index) = index {
                    match queue.remove(index) {
                        Some(cancelled_order) => {
                            // Remove the price entry from the order if no more orders are left at that price level
                            if queue.is_empty() {
                                match order_ref.side {
                                    Side::Buy => self.bids.remove(&order_ref.price),
                                    Side::Sell => self.asks.remove(&order_ref.price),
                                };
                            }

                            // keep track of the original order attributes for the cancellation acknowledgment.
                            let mut cancel_ack = order;
                            cancel_ack.side = cancelled_order.side;
                            cancel_ack.price = cancelled_order.price;
                            cancel_ack.quantity = cancelled_order.quantity;

                            tracing::debug!("[{}][{}][{}] Cancelled order with ID: {}, side: {:?}, price: {}, position: {}", market_name(), order.symbol, order.cl_ord_id, orig_cl_ord_id, order_ref.side, order_ref.price, index);

                            // !!! Remove the cancelled order from the order map
                            if self.order_map.remove(&orig_cl_ord_id).is_none() {
                                tracing::error!("[{}][{}][{}] Failed to remove order with ID: {} from order map after cancellation, order not found", market_name(), order.symbol, order.cl_ord_id, orig_cl_ord_id);
                            }

                            return (cancel_ack, OrderResult {
                                internal_order_id: self.generate_internal_order_id(),
                                trades: Trades::default(),
                                status: OrderStatus::Cancelled,
                                ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
                            });
                        }
                        None => tracing::error!("[{}][{}][{}] Failed to cancel order with ID: {}, side: {:?}, price: {}, position: {}, order not found in queue", market_name(), order.symbol, order.cl_ord_id, orig_cl_ord_id, order_ref.side, order_ref.price, index),
                    }
                } else {
                    tracing::error!("[{}][{}][{}] Failed to cancel order with ID: {}, side: {:?}, price: {}, order not found in queue", market_name(), order.symbol, order.cl_ord_id, orig_cl_ord_id, order_ref.side, order_ref.price);
                }
            } else {
                tracing::error!("[{}][{}][{}] Failed to cancel order with ID: {}, side: {:?}, price: {}, order queue not found for price level", market_name(), order.symbol, order.cl_ord_id, orig_cl_ord_id, order_ref.side, order_ref.price);
            }
        }

        tracing::error!("[{}][{}][{}] Failed to cancel order with ID: {}, original client order ID: {}, order not found in order book", market_name(), order.symbol, order.cl_ord_id, order.cl_ord_id, orig_cl_ord_id);
        // If we reach this point, it means the order was not found or could not be cancelled
        (order, OrderResult {
            internal_order_id: self.generate_internal_order_id(),
            trades: Trades::default(),
            status: OrderStatus::CancelRejected,
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        })
    }

    /// Generates an `OrderResult` based on the processed order, including trade ID and status.
    /// The trade ID is generated if the order was partially or fully filled, and the status is determined based on the remaining quantity of the order.
    /// Arguments:
    /// - `order`: The order that was processed, containing details such as price, quantity, side, order ID, and broker ID.
    /// - `trades`: The trades that were executed as a result of processing the order, which may include multiple trades if the order was matched against multiple existing orders in the order book.
    /// Returns:
    /// - An `OrderResult` containing the details of the processed order, including any trade ID and status. The status is determined based on the remaining quantity of the order after processing.
    fn generate_order_result(&mut self, order: OrderEvent, trades: Trades<4>) -> (OrderEvent, OrderResult) {                        
        let order_result = OrderResult {
            trades,
            status:  OrderStatus::New,
            internal_order_id: self.generate_internal_order_id(),
            ..Default::default() // Timestamp can be set to the current time in milliseconds since epoch if needed for time-priority sorting in the future
        };
        (order, order_result)
    }

    /// Adds an order to the order map for efficient tracking and future cancellation or modification.
    /// The function calculates the position of the order in the order queue at its price level and stores this information in the `OrderRef` struct, which is then inserted into the `order_map` using the order's client order ID as the key.
    /// Arguments:
    /// - `order`: The order to be added to the order map, containing details such as price, quantity, side, order ID, and broker ID.
    fn add_order_to_map(&mut self, order: &OrderEvent) {
        let position = match order.side {
            Side::Buy => self.bids.get(&order.price).map_or(0, |queue| queue.len()),
            Side::Sell => self.asks.get(&order.price).map_or(0, |queue| queue.len()),
        };
        self.order_map.insert(order.cl_ord_id, OrderRef::new(order.side, order.price, position));
        tracing::debug!("[{}][{}][{}] Added order with ID: {}, side: {:?}, price: {}, position: {} to order map", market_name(), order.symbol, order.cl_ord_id, order.cl_ord_id, order.side, order.price, position);
    }

    /// Processes a sell limit order by matching it against the best available bids in the order book. If the order is not fully filled, it is added to the asks heap.
    /// Arguments:
    /// - `order`: The incoming sell limit order to be processed.
    /// Returns:
    /// - An `OrderResult` containing the details of the processed order, including any trade ID and status.
    fn process_sell_limit_order(&mut self, order: OrderEvent) -> (OrderEvent, OrderResult) {
        let mut remaining_quantity = order.quantity;
        let mut trades = Trades::default();
        while let Some((&best_bid_price, _ )) = self.bids.first_key_value() {
            if best_bid_price >= order.price {
                // There is a matching bid, so we need to process the trades against the orders in the best bid queue
                let mut best_bid_queue = self.bids.pop_first().unwrap().1;

                while let Some(mut best_bid) = best_bid_queue.pop_front() {
                    let maker_qty_before = best_bid.quantity;
                    let trade_quantity = remaining_quantity.min(best_bid.quantity);
                    // Process the trade here (e.g., update quantities, record the trade, etc.)
                    best_bid.quantity -= trade_quantity;
                    remaining_quantity -= trade_quantity;

                    // Add the trade to the list of trades for this order
                    if let Err(_) = trades.add_trade(Trade {
                        price: best_bid.price,
                        cl_ord_id: best_bid.cl_ord_id, // Include the client order ID of the matched order in the trade record
                        quantity: trade_quantity,
                        id: self.generate_trade_id(), // Generate a unique trade ID
                        order_qty: maker_qty_before,
                        leaves_qty: best_bid.quantity,
                        ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
                    }) {
                        // Maximum Trades reached
                        tracing::error!("[{}][{}][{}] Maximum number of trades reached for this order, some trades may not be recorded in the OrderResult", market_name(), order.symbol, order.cl_ord_id);
                    }
            
                    if best_bid.quantity > FixedPointArithmetic::ZERO {
                        best_bid_queue.push_front(best_bid);
                    } else {
                        // Remove the existing order from the order map if it has been completely filled
                        self.order_map.remove(&best_bid.cl_ord_id);
                    }

                    // Update the incoming order's quantity
                    if remaining_quantity == FixedPointArithmetic::ZERO {
                        if !best_bid_queue.is_empty() {
                            self.bids.insert(best_bid_price, best_bid_queue);
                        }

                        return self.generate_order_result(order, trades);
                    }
                }
            } else {
                break;
            }
        }

        let order_result = self.generate_order_result(order, trades);

        if remaining_quantity > FixedPointArithmetic::ZERO {
            let mut order = order;
            order.quantity = remaining_quantity;
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
    fn process_buy_limit_order(&mut self, order: OrderEvent) -> (OrderEvent, OrderResult) {
        let mut remaining_quantity = order.quantity;
        let mut trades = Trades::default();
        while let Some((&best_ask_price, _)) = self.asks.first_key_value() {
            if best_ask_price <= order.price {
                let mut best_ask_queue = self.asks.pop_first().unwrap().1; // Remove the best ask queue from the asks heap to process it
                
                while let Some(mut best_ask) = best_ask_queue.pop_front() {
                    let maker_qty_before = best_ask.quantity;
                    let trade_quantity = remaining_quantity.min(best_ask.quantity);
                    // Process the trade here (e.g., update quantities, record the trade, etc.)
                    best_ask.quantity -= trade_quantity;
                    remaining_quantity -= trade_quantity;

                    // Add the trade to the list of trades for this order
                    if let Err(_) = trades.add_trade(Trade {
                        price: best_ask.price,
                        cl_ord_id: best_ask.cl_ord_id,
                        quantity: trade_quantity,
                        id: self.generate_trade_id(), // Generate a unique trade ID
                        order_qty: maker_qty_before,
                        leaves_qty: best_ask.quantity,
                        ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
                    }) {
                        // Maximum Trades reached
                        tracing::error!("[{}][{}][{}] Maximum number of trades reached for this order, some trades may not be recorded in the OrderResult", market_name(), order.symbol, order.cl_ord_id);
                    }


                    // If the best ask still has quantity remaining after the trade, push it back onto the asks
                    if best_ask.quantity > FixedPointArithmetic::ZERO {
                        best_ask_queue.push_front(best_ask);
                    } else {
                        // Remove the existing order from the order map if it has been completely filled
                        self.order_map.remove(&best_ask.cl_ord_id);
                    }

                    // Update the incoming order's quantity
                    if remaining_quantity == FixedPointArithmetic::ZERO {
                        if !best_ask_queue.is_empty() {
                            self.asks.insert(best_ask_price, best_ask_queue.clone());
                        }

                        return self.generate_order_result(order, trades);
                    }
                }
            } else {
                break;
            }
        }

        let (order, order_result) = self.generate_order_result(order, trades);

        if order.quantity > FixedPointArithmetic::ZERO {
            let mut order = order;
            order.quantity = remaining_quantity;
            // Add for future cancellation/modification
            self.add_order_to_map(&order);

            // Add the remaining order to the order book if it was not fully filled
            self.bids
            .entry(order.price)
            .or_insert_with(VecDeque::new)
            .push_back(order);
        }

        (order, order_result)
    }

    fn process_buy_market_order(&mut self, mut order: OrderEvent) -> (OrderEvent, OrderResult) {
        order.price = FixedPointArithmetic::from_f64(f64::INFINITY); // Market orders are treated as having an infinitely high price to ensure they match with the best available asks
        self.process_buy_limit_order(order)
    }

    fn process_sell_market_order(&mut self, mut order: OrderEvent) -> (OrderEvent, OrderResult) {
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
    pub fn dump_order_book(&self, side: Side, depth: usize) -> Vec<OrderEvent> {
        match side {
            Side::Buy => self.bids.iter().flat_map(|(_price, queue)| queue.iter().cloned()).take(depth).collect(),
            Side::Sell => self.asks.iter().flat_map(|(_price, queue)| queue.iter().cloned()).take(depth).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use types::macros::{SymbolId, EntityId, OrderId};

    const SYMBOL_STR: &str = "TEST";
    const SYMBOL_ID: SymbolId = SymbolId::from_ascii(SYMBOL_STR);
    const SENDER: EntityId = EntityId::from_ascii("SENDER0000000000000");
    const TARGET: EntityId = EntityId::from_ascii("TARGET0000000000000");
    const CL_ORD_ID: OrderId = OrderId::from_ascii("12345");

    #[test]
    fn test_order_book_initialization() {
        let order_book = OrderBook::new(SYMBOL_STR);
        assert!(order_book.get_best_bid().is_none());
        assert!(order_book.get_best_ask().is_none());
        assert!(order_book.get_spread().is_none());
    }

    #[test]
    fn test_cancel_order() {
        let mut order_book = OrderBook::new(SYMBOL_STR);
        let order = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0), // 100.0 with 8 decimal places
            quantity: FixedPointArithmetic::from_f64(10.0), // 10.0 with 8 decimal places
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            orig_cl_ord_id: None,
            cl_ord_id: CL_ORD_ID,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID, // Set the symbol for the order
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };

        let (order, result) = order_book.process_order(order);
        assert_eq!(result.status, OrderStatus::New);
        assert!(order.price == FixedPointArithmetic::from_f64(100.0));
        assert!(order.quantity == FixedPointArithmetic::from_f64(10.0));

        let cancel_order = OrderEvent {
            price: FixedPointArithmetic::ZERO, // Price is not relevant for cancel orders
            quantity: FixedPointArithmetic::ZERO, // Quantity is not relevant for cancel orders
            side: Side::Buy, // Side is not relevant for cancel orders, but we can set it to match the original order
            order_type: OrderType::CancelOrder,
            cl_ord_id: CL_ORD_ID, // Use the same ClOrdID to identify which order to cancel
            orig_cl_ord_id: Some(CL_ORD_ID),
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID, // Set the symbol for the cancel order
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };

        let (cancel_order, cancel_result) = order_book.process_order(cancel_order);
        assert_eq!(cancel_result.status, OrderStatus::Cancelled);
        assert_eq!(cancel_order.price, order.price); // The cancel acknowledgment should reflect the original order's price
        assert_eq!(cancel_order.quantity, order.quantity); // The cancel acknowledgment should reflect the original order's quantity
        assert!(order_book.asks.is_empty()); // There should be no asks in the order book
        assert!(order_book.bids.is_empty()); // There should be no bids in the order book
        assert!(order_book.order_map.is_empty()); // There should be no asks in the order book

        // Testing cancellation of a sell order
        let order = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0), // 100.0 with 8 decimal places
            quantity: FixedPointArithmetic::from_f64(10.0), // 10.0 with 8 decimal places
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID, // Set the symbol for the order
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };

        let (order, result) = order_book.process_order(order);
        assert_eq!(result.status, OrderStatus::New);
        assert!(order.price == FixedPointArithmetic::from_f64(100.0));
        assert!(order.quantity == FixedPointArithmetic::from_f64(10.0));

        let cancel_order = OrderEvent {
            price: FixedPointArithmetic::ZERO, // Price is not relevant for cancel orders
            quantity: FixedPointArithmetic::ZERO, // Quantity is not relevant for cancel orders
            side: Side::Sell, // Side is not relevant for cancel orders, but we can set it to match the original order
            order_type: OrderType::CancelOrder,
            cl_ord_id: CL_ORD_ID, // Use the same ClOrdID to identify which order to cancel
            orig_cl_ord_id: Some(CL_ORD_ID),
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID, // Set the symbol for the cancel order
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };

        let (cancel_order, cancel_result) = order_book.process_order(cancel_order);
        assert_eq!(cancel_result.status, OrderStatus::Cancelled);
        assert!(cancel_order.price == order.price); // The cancel acknowledgment should reflect the original order's price
        assert!(cancel_order.quantity == order.quantity); // The cancel acknowledgment should reflect the original order's quantity
        assert_eq!(order_book.get_best_ask(), None); // The best ask should be removed after cancellation
        assert!(order_book.order_map.is_empty()); // There should be no asks in the order book

    }

    #[test]
    fn test_single_limit_order() {
        let mut order_book = OrderBook::new(SYMBOL_STR);
        let order = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0), // 100.0 with 8 decimal places
            quantity: FixedPointArithmetic::from_f64(10.0), // 10.0 with 8 decimal places
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID, // Set the symbol for the order
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };

        let (order, result) = order_book.process_order(order);

        assert_eq!(order.price, FixedPointArithmetic::from_f64(100.0));
        assert_eq!(order.quantity, FixedPointArithmetic::from_f64(10.0));
        assert_eq!(result.trades.len(), 0); // No trades executed
        assert_eq!(result.trades.quantity_sum(), FixedPointArithmetic::ZERO); // Total quantity should be zero since no trades were executed
        assert_eq!(result.trades.avg_price(), FixedPointArithmetic::ZERO); // Average price should be zero since no trades were executed
        assert_eq!(result.status, OrderStatus::New);
        assert_eq!(order_book.get_best_bid().unwrap().price, FixedPointArithmetic::from_f64(100.0)); // Best bid should be the price of the order
        assert_eq!(order_book.get_best_bid().unwrap().quantity, FixedPointArithmetic::from_f64(10.0)); // Best bid quantity should be the quantity of the order
    }

    #[test]
    fn test_trade_with_same_price() {
        let mut order_book = OrderBook::new(SYMBOL_STR);
        let order1 = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0), // 100.0 with 8 decimal places
            quantity: FixedPointArithmetic::from_f64(10.0), // 10.0 with 8 decimal places
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID, // Set the symbol for the order
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };
        let order2 = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0),
            quantity: FixedPointArithmetic::from_f64(5.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID, // Set the symbol for the order
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };

        let (order1, result1) = order_book.process_order(order1);
        let (order2, result2) = order_book.process_order(order2);

        assert_eq!(order1.price, FixedPointArithmetic::from_f64(100.0));
        assert_eq!(order1.quantity, FixedPointArithmetic::from_f64(10.0));
        assert_eq!(result1.trades.len(), 0); // No trades executed for the first order
        assert_eq!(result1.status, OrderStatus::New);

        assert_eq!(order2.price, FixedPointArithmetic::from_f64(100.0));
        assert_eq!(order2.quantity, FixedPointArithmetic::from_f64(5.0)); // The second order should be completely filled, so the remaining quantity should be 0
        assert_eq!(result2.trades.len(), 1); // One trade executed for the second order
        assert_eq!(result2.trades[0].quantity, FixedPointArithmetic::from_f64(5.0)); // 5 units filled
        assert_eq!(result2.trades[0].price, FixedPointArithmetic::from_f64(100.0)); // Trade price should be 100.0
        assert_eq!(result2.trades.quantity_sum(), FixedPointArithmetic::from_f64(5.0)); // Total quantity should be 5.0
        assert_eq!(result2.trades.avg_price(), FixedPointArithmetic::from_f64(100.0)); // Average price should be 100.0
        assert_eq!(result2.status, OrderStatus::New);
    }

    #[test]
    fn test_limit_orders_single_trade() {
        logging::init_tracing("order_book");

        let mut order_book = OrderBook::new(SYMBOL_STR);
        let order1 = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0), // 100.0 with 8 decimal places
            quantity: FixedPointArithmetic::from_f64(10.0), // 10.0 with 8 decimal places
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID, // Set the symbol for the order
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };
        let order2 = OrderEvent {
            price: FixedPointArithmetic::from_f64(99.0),
            quantity: FixedPointArithmetic::from_f64(5.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID, // Set the symbol for the order
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };
        let order3 = OrderEvent {
            price: FixedPointArithmetic::from_f64(98.0),
            quantity: FixedPointArithmetic::from_f64(10.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID, // Set the symbol for the order
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };

        let (order1, result1) = order_book.process_order(order1);
        let (order2, result2) = order_book.process_order(order2);
        let (order3, result3) = order_book.process_order(order3);

        // The first order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the bids.
        assert!(order1.price == FixedPointArithmetic::from_f64(100.0));
        assert!(order1.quantity == FixedPointArithmetic::from_f64(10.0));
        assert_eq!(result1.trades.len(), 0); // No trades executed,
        assert_eq!(result1.trades.quantity_sum(), FixedPointArithmetic::ZERO); // Total quantity should be zero since no trades were executed
        assert_eq!(result1.trades.avg_price(), FixedPointArithmetic::ZERO); // Average price should be zero since no trades were executed
        assert_eq!(result1.status, OrderStatus::New);

        // The second order should be completely filled (5 units filled, 0 units remaining).
        assert_eq!(order2.price, FixedPointArithmetic::from_f64(99.0));
        assert_eq!(order2.quantity, FixedPointArithmetic::from_f64(5.0));
        assert_eq!(result2.trades.len(), 1); // 5 units * 99.0 price
        assert_eq!(result2.trades[0].id, 1); // Trade ID should be 1 for the first trade (counter starts at 1)
        assert_eq!(result2.trades[0].quantity, FixedPointArithmetic::from_f64(5.0)); // 5 units filled
        assert_eq!(result2.trades[0].price, FixedPointArithmetic::from_f64(100.0)); // 100.0
        assert_eq!(result2.trades.avg_price(), FixedPointArithmetic::from_f64(100.0)); // Average price should be 100.0
        assert!(result2.trades.quantity_sum() == FixedPointArithmetic::from_f64(5.0)); // Total quantity should be 5.0
        assert_eq!(result2.status, OrderStatus::New);
        assert_eq!(result2.trades.avg_price(), FixedPointArithmetic::from_f64(100.0)); // Average price should be 100.0
        assert_eq!(result2.trades.quantity_sum(), FixedPointArithmetic::from_f64(5.0)); // Total quantity should be 5.0

        // The third order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the asks.
        assert_eq!(order3.price, FixedPointArithmetic::from_f64(98.0));
        assert_eq!(order3.quantity, FixedPointArithmetic::from_f64(10.0));
        assert_eq!(result3.trades.len(), 1); // 5 units * 98.0 price
        assert_eq!(result3.trades[0].quantity, FixedPointArithmetic::from_f64(5.0)); // 5 units filled
        assert_eq!(result3.trades[0].price, FixedPointArithmetic::from_f64(100.0)); // 5 units * 100.0 price
        assert_eq!(result3.trades.avg_price(), FixedPointArithmetic::from_f64(100.0)); // Average price should be 100.0
        assert!(result3.trades.quantity_sum() == FixedPointArithmetic::from_f64(5.0)); // Total quantity should be 5.0
        assert_eq!(result3.status, OrderStatus::New);
        assert_eq!(result3.trades.avg_price(), FixedPointArithmetic::from_f64(100.0)); // Average price should be 100.0
        assert_eq!(result3.trades.quantity_sum(), FixedPointArithmetic::from_f64(5.0)); // Total quantity should be 5.0

        assert_eq!(order_book.bids.len(), 0); // One ask should remain in the order book
        assert_eq!(order_book.asks.len(), 1); // One ask should remain in the order book
        assert_eq!(order_book.asks.first_entry().unwrap().key(), &FixedPointArithmetic::from_f64(98.0)); // The remaining ask should be the one at 98.0
        assert_eq!(order_book.asks.first_entry().unwrap().get().get(0).unwrap().quantity, FixedPointArithmetic::from_f64(5.0)); // The remaining ask should have a quantity of 5
        assert_eq!(order_book.asks.first_entry().unwrap().get().get(0).unwrap().cl_ord_id, CL_ORD_ID); // The remaining ask should have the same ClOrdID as the third order
        assert_eq!(order_book.asks.first_entry().unwrap().get().get(0).unwrap().sender_id, SENDER); // The remaining ask should have the same sender ID as the third order
        assert_eq!(order_book.asks.first_entry().unwrap().get().get(0).unwrap().target_id, TARGET); // The remaining ask should have the same target ID as the third order
        assert_eq!(order_book.asks.first_entry().unwrap().get().get(0).unwrap().order_type, OrderType::LimitOrder); // The remaining ask should have the same order type as the third order
        // Give time for the logs to be flushed before the test ends
        std::thread::sleep(std::time::Duration::from_millis(100));

    }

    #[test]
    fn test_limit_orders_multiple_trades() {
        logging::init_tracing("order_book");

        let mut order_book = OrderBook::new(SYMBOL_STR);

        let order1 = OrderEvent {
            price: FixedPointArithmetic::from_f64(99.0),
            quantity: FixedPointArithmetic::from_f64(3.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID,
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };
        let order2 = OrderEvent {
            price: FixedPointArithmetic::from_f64(98.0),
            quantity: FixedPointArithmetic::from_f64(5.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID,
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };
        let order3 = OrderEvent {
            price: FixedPointArithmetic::from_f64(97.0),
            quantity: FixedPointArithmetic::from_f64(3.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID,
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };
        let order4 = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0),
            quantity: FixedPointArithmetic::from_f64(10.0),
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID,
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };

        let (order1, result1) = order_book.process_order(order1);
        let (order2, result2) = order_book.process_order(order2);
        let (order3, result3) = order_book.process_order(order3);
        let (order4, result4) = order_book.process_order(order4);

        // The first order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the bids.
        assert_eq!(order1.price, FixedPointArithmetic::from_f64(99.0));
        assert_eq!(order1.quantity, FixedPointArithmetic::from_f64(3.0));
        assert_eq!(result1.trades.len(), 0); // No trades executed,
        assert_eq!(result1.status, OrderStatus::New);

        // The second order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the bids.
        assert_eq!(order2.price, FixedPointArithmetic::from_f64(98.0));
        assert_eq!(order2.quantity, FixedPointArithmetic::from_f64(5.0));
        assert_eq!(result2.trades.len(), 0); // No trades executed,
        assert_eq!(result2.status, OrderStatus::New);

        // The third order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the bids.
        assert_eq!(order3.price, FixedPointArithmetic::from_f64(97.0));
        assert_eq!(order3.quantity, FixedPointArithmetic::from_f64(3.0));
        assert_eq!(result3.trades.len(), 0); // No trades executed,
        assert_eq!(result3.status, OrderStatus::New);

        // The fourth order should be completely filled (3 units filled at 97.0, 5 units filled at 98.0, and 2 units filled at 99.0).
        assert_eq!(order4.price, FixedPointArithmetic::from_f64(100.0));
        assert_eq!(order4.quantity, FixedPointArithmetic::from_f64(10.0));
        assert_eq!(result4.trades.len(), 3); // 3 trades executed
        assert_eq!(result4.trades[0].quantity, FixedPointArithmetic::from_f64(3.0)); // 3 units filled
        assert_eq!(result4.trades[0].price, FixedPointArithmetic::from_f64(97.0)); // 3 units * 97.0 price
        assert_eq!(result4.trades[1].quantity, FixedPointArithmetic::from_f64(5.0)); // 5 units filled
        assert_eq!(result4.trades[1].price, FixedPointArithmetic::from_f64(98.0)); // 5 units * 98.0 price
        assert_eq!(result4.trades[2].quantity, FixedPointArithmetic::from_f64(2.0)); // 2 units filled
        assert_eq!(result4.trades[2].price, FixedPointArithmetic::from_f64(99.0)); // 2 units * 99.0 price
        assert_eq!(result4.status, OrderStatus::New);
        assert_eq!(result4.trades.avg_price(), FixedPointArithmetic::from_f64(97.9)); // Average price should be (3*97 + 5*98 + 2*99) / 10 = 98.0
        assert_eq!(result4.trades.quantity_sum(), FixedPointArithmetic::from_f64(10.0)); // Total quantity should be 10.0

        assert_eq!(order_book.asks.len(), 1); // One ask should remain in the order book
        assert_eq!(order_book.asks.first_entry().unwrap().key(), &FixedPointArithmetic::from_f64(99.0)); // The remaining ask should be the one at 99.0
        assert_eq!(order_book.asks.first_entry().unwrap().get().front().unwrap().quantity, FixedPointArithmetic::from_f64(1.0)); // The remaining ask should have a quantity of 1
        assert_eq!(order_book.asks.first_entry().unwrap().get().front().unwrap().cl_ord_id, CL_ORD_ID); // The remaining ask should have the same ClOrdID as the first order
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

        let mut order_book = OrderBook::new(SYMBOL_STR);

        let order1 = OrderEvent {
            price: FixedPointArithmetic::from_f64(99.0),
            quantity: FixedPointArithmetic::from_f64(5.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID,
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };
        let order2 = OrderEvent {
            price: FixedPointArithmetic::from_f64(98.0),
            quantity: FixedPointArithmetic::from_f64(5.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID,
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };
        let order3 = OrderEvent {
            price: FixedPointArithmetic::from_f64(98.0),
            quantity: FixedPointArithmetic::from_f64(10.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID,
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };
        let order4 = OrderEvent {
            price: FixedPointArithmetic::from_f64(0.0), // Price is ignored for market orders
            quantity: FixedPointArithmetic::from_f64(12.0),
            side: Side::Buy,
            order_type: OrderType::MarketOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID,
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };

        let (order1, result1) = order_book.process_order(order1);
        let (order2, result2) = order_book.process_order(order2);
        let (order3, result3) = order_book.process_order(order3);
        let (order4, result4) = order_book.process_order(order4);

        // The first three orders should be added to the asks heap as they are limit sell orders.
        assert_eq!(order1.price, FixedPointArithmetic::from_f64(99.0));
        assert_eq!(order1.quantity, FixedPointArithmetic::from_f64(5.0));
        assert_eq!(result1.trades.len(), 0); // No trades executed
        assert_eq!(result1.status, OrderStatus::New);

        assert_eq!(order2.price, FixedPointArithmetic::from_f64(98.0));
        assert_eq!(order2.quantity, FixedPointArithmetic::from_f64(5.0));
        assert_eq!(result2.trades.len(), 0); // No trades executed
        assert_eq!(result2.status, OrderStatus::New);

        assert_eq!(order3.price, FixedPointArithmetic::from_f64(98.0));
        assert_eq!(order3.quantity, FixedPointArithmetic::from_f64(10.0));
        assert_eq!(result3.trades.len(), 0); // No trades executed
        assert_eq!(result3.status, OrderStatus::New);

        // The fourth order should be completely filled (5 units filled at 98.0 and 7 units filled at 99.0).
        assert_eq!(order4.price, FixedPointArithmetic::from_f64(f64::MAX)); // Price is ignored for market orders
        assert_eq!(order4.quantity, FixedPointArithmetic::from_f64(12.0));
        assert_eq!(result4.trades.len(), 2); // 2 trades executed
        assert_eq!(result4.trades[0].id, 1); // Trade ID should be 1 for the first trade (counter starts at 1)
        assert_eq!(result4.trades[0].quantity, FixedPointArithmetic::from_f64(5.0)); // 5 units filled
        assert_eq!(result4.trades[0].price, FixedPointArithmetic::from_f64(98.0)); // 5 units * 98.0 price
        assert_eq!(result4.trades[1].id, 2); // Trade ID should be 2 for the second trade
        assert_eq!(result4.trades[1].quantity, FixedPointArithmetic::from_f64(7.0)); // 7 units filled
        assert_eq!(result4.trades[1].price, FixedPointArithmetic::from_f64(98.0)); // 7 units * 98.0 price
        assert_eq!(result4.status, OrderStatus::New);

        // Check the remaining orders in the order book after processing the market order
        assert_eq!(order_book.asks.len(), 2); // Two asks should remain in the order book
        assert_eq!(order_book.asks.first_entry().unwrap().key(), &FixedPointArithmetic::from_f64(98.0)); // The remaining ask should be the one at 98.0
        assert_eq!(order_book.asks.first_entry().unwrap().get().get(0).unwrap().quantity, FixedPointArithmetic::from_f64(3.0)); // The remaining ask should have a quantity of 3.0
        assert_eq!(order_book.asks.first_entry().unwrap().get().get(0).unwrap().cl_ord_id, CL_ORD_ID); // The remaining ask should have the same ClOrdID as the third order
        assert_eq!(order_book.asks.first_entry().unwrap().get().get(0).unwrap().sender_id, SENDER); // The remaining ask should have the same sender ID as the third order
        assert_eq!(order_book.asks.first_entry().unwrap().get().get(0).unwrap().target_id, TARGET); // The remaining ask should have the same target ID as the third order
        assert_eq!(order_book.asks.first_entry().unwrap().get().get(0).unwrap().order_type, OrderType::LimitOrder); // The remaining ask should have the same order type as the third order
        assert_eq!(order_book.asks.iter().nth(1).unwrap().0, &FixedPointArithmetic::from_f64(99.0)); // The second remaining ask should be the one at 99.0
        assert_eq!(order_book.asks.iter().nth(1).unwrap().1.get(0).unwrap().quantity, FixedPointArithmetic::from_f64(5.0)); // The second remaining ask should have a quantity of 5.0
        assert_eq!(order_book.asks.iter().nth(1).unwrap().1.get(0).unwrap().cl_ord_id, CL_ORD_ID); // The second remaining ask should have the same ClOrdID as the first order
    }

    #[test]
    fn test_spread_calculation() {
        logging::init_tracing("order_book");
        let mut order_book = OrderBook::new(SYMBOL_STR);
        let order1 = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0),
            quantity: FixedPointArithmetic::from_f64(10.0),
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID,
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };
        let order2 = OrderEvent {
            price: FixedPointArithmetic::from_f64(102.0),
            quantity: FixedPointArithmetic::from_f64(5.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID,
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
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
        let mut order_book = OrderBook::new(SYMBOL_STR);
        let order1 = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0),
            quantity: FixedPointArithmetic::from_f64(10.0),
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID,
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };
        let order2 = OrderEvent {
            price: FixedPointArithmetic::from_f64(102.0),
            quantity: FixedPointArithmetic::from_f64(5.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            cl_ord_id: CL_ORD_ID,
            orig_cl_ord_id: None,
            sender_id: SENDER,
            target_id: TARGET,
            symbol: SYMBOL_ID,
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };

        order_book.process_order(order1);
        order_book.process_order(order2);

        let bids = order_book.dump_order_book(Side::Buy, 10); // Dump the top 10 levels of the bid side
        let asks = order_book.dump_order_book(Side::Sell, 10); // Dump the top 10 levels of the ask side

        assert_eq!(bids.len(), 1); // One bid should be in the order book
        assert_eq!(bids[0].price, FixedPointArithmetic::from_f64(100.0)); // The bid should have the correct price
        assert_eq!(bids[0].quantity, FixedPointArithmetic::from_f64(10.0)); // The bid should have the correct quantity
        assert_eq!(bids[0].cl_ord_id, CL_ORD_ID); // The bid should have the correct client order ID
        assert_eq!(bids[0].target_id, TARGET); // The bid should have the correct target ID
        assert_eq!(bids[0].order_type, OrderType::LimitOrder); // The bid should have the correct order type

        assert_eq!(asks.len(), 1); // One ask should be in the order book
        assert_eq!(asks[0].price, FixedPointArithmetic::from_f64(102.0)); // The ask should have the correct price
        assert_eq!(asks[0].quantity, FixedPointArithmetic::from_f64(5.0)); // The ask should have the correct quantity
        assert_eq!(asks[0].cl_ord_id, CL_ORD_ID); // The ask should have the correct client order ID
        assert_eq!(asks[0].sender_id, SENDER); // The ask should have the correct sender ID
        assert_eq!(asks[0].target_id, TARGET); // The ask should have the correct target ID
        assert_eq!(asks[0].order_type, OrderType::LimitOrder); // The ask should have the correct order type
    }
}