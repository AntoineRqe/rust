use std::{cmp::Reverse, collections::BinaryHeap};
use crate::types::{Order, Side, OrderResult, OrderStatus, Trade};
use tracing::{instrument};


/// Represents the order book, maintaining separate heaps for bids and asks.
/// Bids are stored in a max-heap (higher prices have priority), while asks are stored in a min-heap (lower prices have priority).
/// The order book processes incoming orders, matches them against existing orders, and updates the order book accordingly.
/// - `bids`: A binary heap containing buy orders, sorted by price in descending order.
/// - `asks`: A binary heap containing sell orders, sorted by price in ascending order (using `Reverse` to achieve min-heap behavior).
/// - `id_counter`: A counter used to generate unique trade IDs for matched orders.
pub struct OrderBook {
    pub bids: BinaryHeap<Order>,
    pub asks: BinaryHeap<Reverse<Order>>,
    id_counter: u64,
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
            bids: BinaryHeap::new(),
            asks: BinaryHeap::new(),
            id_counter: 0,
        }
    }

    #[instrument(level = "debug", skip(self, order), fields(order_id = order.order_id, side = ?order.side, price = order.price, quantity = order.quantity))]
    pub fn process_order(&mut self, order: Order) -> OrderResult {
        match order.order_type {
            crate::types::OrderType::LimitOrder => self.process_limit_order(order),
            crate::types::OrderType::MarketOrder => self.process_market_order(order),
        }
    }

    fn process_limit_order(&mut self, order: Order) -> OrderResult {
        match order.side {
            Side::Buy => self.process_buy_limit_order(order),
            Side::Sell => self.process_sell_limit_order(order),
        }
    }

    #[instrument(level = "debug", skip(self, order), fields(order_id = order.order_id, side = ?order.side, price = order.price, quantity = order.quantity))]
    fn process_market_order(&mut self, order: Order) -> OrderResult {
        match order.side {
            Side::Buy => self.process_buy_market_order(order),
            Side::Sell => self.process_sell_market_order(order),
        }
    }

    /// Generates an `OrderResult` based on the processed order, including trade ID and status.
    /// The trade ID is generated if the order was partially or fully filled, and the status is determined based on the remaining quantity of the order.
    fn generate_order_result(&mut self, order: &Order, status: Option<OrderStatus>, original_quantity: f64, trades: Vec<Trade>) -> OrderResult {
        OrderResult {
            original_price: order.price,
            original_quantity,
            trades,
            side: order.side,
            order_type: order.order_type,
            order_id: order.order_id,
            status: status.unwrap_or_else(|| {
                if order.quantity == 0.0 {
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
    fn process_sell_limit_order(&mut self, mut order: Order) -> OrderResult {
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
                    trade_id: self.id_counter, // Example trade ID
                });
                self.id_counter += 1;
        
                if best_bid.quantity > 0.0 {
                    self.bids.push(best_bid);
                }
                // Update the incoming order's quantity
                order.quantity -= trade_quantity;

                if order.quantity == 0.0 {
                    return self.generate_order_result(&order, Some(OrderStatus::Filled), original_quantity, trades);
                }
            } else {
                break;
            }
        }

        let order_result = self.generate_order_result(&order, None, original_quantity, trades);

        if order.quantity > 0.0 {
            self.asks.push(Reverse(order));
        }

        order_result
    }

    /// Processes a buy limit order by matching it against the best available asks in the order book. If the order is not fully filled, it is added to the bids heap.
    /// Arguments:
    /// - `order`: The incoming buy limit order to be processed.
    /// Returns:
    /// - An `OrderResult` containing the details of the processed order, including any trade ID and status.
    fn process_buy_limit_order(&mut self, mut order: Order) -> OrderResult {
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
                    trade_id: self.id_counter, // Example trade ID
                });
                self.id_counter += 1;

                // If the best ask still has quantity remaining after the trade, push it back onto the asks
                if best_ask.quantity > 0.0 {
                    self.asks.push(Reverse(best_ask));
                }
                // Update the incoming order's quantity
                order.quantity -= trade_quantity;

                if order.quantity == 0.0 {
                    return self.generate_order_result(&order, Some(OrderStatus::Filled), original_quantity, trades);
                }
            } else {
                break;
            }
        }

        let order_result = self.generate_order_result(&order, None, original_quantity, trades);

        if order.quantity > 0.0 {
            self.bids.push(order);
        }

        order_result
    }

    fn process_buy_market_order(&mut self, order: Order) -> OrderResult {
        unimplemented!()
    }

    fn process_sell_market_order(&mut self, order: Order) -> OrderResult {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::OrderType::{LimitOrder};

    #[test]
    fn test_limit_orders_single_trade() {
        logging::init_tracing("order_book");

        let mut order_book = OrderBook::new();
        let order1 = Order {
            price: 100.0,
            quantity: 10.0,
            side: Side::Buy,
            order_type: LimitOrder,
            order_id: 1,
            broker_id: 666,
        };
        let order2 = Order {
            price: 99.0,
            quantity: 5.0,
            side: Side::Sell,
            order_type: LimitOrder,
            order_id: 2,
            broker_id: 667,
        };
        let order3 = Order {
            price: 98.0,
            quantity: 10.0,
            side: Side::Sell,
            order_type: LimitOrder,
            order_id: 3,
            broker_id: 668,
        };

        let result1 = order_book.process_order(order1);
        let result2 = order_book.process_order(order2);
        let result3 = order_book.process_order(order3);

        // The first order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the bids.
        assert_eq!(result1.original_price, 100.0);
        assert_eq!(result1.trades.len(), 0); // No trades executed,
        assert_eq!(result1.side, Side::Buy);
        assert_eq!(result1.order_type, LimitOrder);
        assert_eq!(result1.order_id, 1);
        assert_eq!(result1.status, OrderStatus::NotMatched);

        // The second order should be completely filled (5 units filled, 0 units remaining).
        assert_eq!(result2.original_price, 99.0);
        assert_eq!(result2.trades.len(), 1); // 5 units * 99.0 price
        assert_eq!(result2.side, Side::Sell);
        assert_eq!(result2.order_type, LimitOrder);
        assert_eq!(result2.order_id, 2);
        assert_eq!(result2.trades[0].trade_id, 0); // Trade ID should be 0 for the first trade
        assert_eq!(result2.trades[0].traded_quantity, 5.0); // 5 units filled
        assert_eq!(result2.trades[0].traded_price, 100.0); // 100.0
        assert_eq!(result2.status, OrderStatus::Filled);

        // The third order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the asks.
        assert_eq!(result3.original_price, 98.0);
        assert_eq!(result3.trades.len(), 1); // 5 units * 98.0 price
        assert_eq!(result3.trades[0].traded_quantity, 5.0); // 5 units filled
        assert_eq!(result3.trades[0].traded_price, 100.0); // 5 units * 100.0 price
        assert_eq!(result3.side, Side::Sell);
        assert_eq!(result3.order_type, LimitOrder);
        assert_eq!(result3.order_id, 3);
        assert_eq!(result3.status, OrderStatus::PartiallyFilled);

        assert_eq!(order_book.bids.len(), 0); // One ask should remain in the order book
        assert_eq!(order_book.asks.len(), 1); // One ask should remain in the order book
        assert_eq!(order_book.asks.peek().unwrap().0.price, 98.0); // The remaining ask should be the one at 98.0
        assert_eq!(order_book.asks.peek().unwrap().0.quantity, 5.0); // The remaining ask should have a quantity of 5.0
        assert_eq!(order_book.asks.peek().unwrap().0.order_id, 3); // The remaining ask should have the same order ID as the third order
        assert_eq!(order_book.asks.peek().unwrap().0.broker_id, 668); // The remaining ask should have the same broker ID as the third order
        assert_eq!(order_book.asks.peek().unwrap().0.order_type, LimitOrder); // The remaining ask should have the same order type as the third order
        // Give time for the logs to be flushed before the test ends
        std::thread::sleep(std::time::Duration::from_millis(100));

    }

    #[test]
    fn test_limit_orders_multiple_trades() {
        logging::init_tracing("order_book");

        let mut order_book = OrderBook::new();

        let order1 = Order {
            price: 99.0,
            quantity: 3.0,
            side: Side::Sell,
            order_type: LimitOrder,
            order_id: 0,
            broker_id: 669,
        };
        let order2 = Order {
            price: 98.0,
            quantity: 5.0,
            side: Side::Sell,
            order_type: LimitOrder,
            order_id: 0,
            broker_id: 667,
        };
        let order3 = Order {
            price: 97.0,
            quantity: 3.0,
            side: Side::Sell,
            order_type: LimitOrder,
            order_id: 0,
            broker_id: 668,
        };
        let order4 = Order {
            price: 100.0,
            quantity: 10.0,
            side: Side::Buy,
            order_type: LimitOrder,
            order_id: 0,
            broker_id: 666,
        };

        let result1 = order_book.process_order(order1);
        let result2 = order_book.process_order(order2);
        let result3 = order_book.process_order(order3);
        let result4 = order_book.process_order(order4);

        // The first order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the bids.
        assert_eq!(result1.original_price, 99.0);
        assert_eq!(result1.trades.len(), 0); // No trades executed,
        assert_eq!(result1.side, Side::Sell);
        assert_eq!(result1.order_type, LimitOrder);
        assert_eq!(result1.order_id, 0);
        assert_eq!(result1.status, OrderStatus::NotMatched);

        // The second order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the bids.
        assert_eq!(result2.original_price, 98.0);
        assert_eq!(result2.trades.len(), 0); // No trades executed,
        assert_eq!(result2.side, Side::Sell);
        assert_eq!(result2.order_type, LimitOrder);
        assert_eq!(result2.order_id, 0);
        assert_eq!(result2.status, OrderStatus::NotMatched);

        // The third order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the bids.
        assert_eq!(result3.original_price, 97.0);
        assert_eq!(result3.trades.len(), 0); // No trades executed,
        assert_eq!(result3.side, Side::Sell);
        assert_eq!(result3.order_type, LimitOrder);
        assert_eq!(result3.order_id, 0);
        assert_eq!(result3.status, OrderStatus::NotMatched);

        // The fourth order should be completely filled (3 units filled at 97.0, 5 units filled at 98.0, and 2 units filled at 99.0).
        assert_eq!(result4.original_price, 100.0);
        assert_eq!(result4.trades.len(), 3); // 3 trades executed
        assert_eq!(result4.trades[0].trade_id, 0); // Trade ID should be 0 for the first trade
        assert_eq!(result4.trades[0].traded_quantity, 3.0); // 3 units filled
        assert_eq!(result4.trades[0].traded_price, 97.0); // 3 units * 97.0 price
        assert_eq!(result4.trades[1].trade_id, 1); // Trade ID should be 1 for the second trade
        assert_eq!(result4.trades[1].traded_quantity, 5.0); // 5 units filled
        assert_eq!(result4.trades[1].traded_price, 98.0); // 5 units * 98.0 price
        assert_eq!(result4.trades[2].trade_id, 2); // Trade ID should be 2 for the third trade
        assert_eq!(result4.trades[2].traded_quantity, 2.0); // 2 units filled
        assert_eq!(result4.trades[2].traded_price, 99.0); // 2 units * 99.0 price
        assert_eq!(result4.side, Side::Buy);
        assert_eq!(result4.order_type, LimitOrder);
        assert_eq!(result4.order_id, 0);
        assert_eq!(result4.status, OrderStatus::Filled);

        assert_eq!(order_book.asks.len(), 1); // One ask should remain in the order book
        assert_eq!(order_book.asks.peek().unwrap().0.price, 99.0); // The remaining ask should be the one at 99.0
        assert_eq!(order_book.asks.peek().unwrap().0.quantity, 1.0); // The remaining ask should have a quantity of 1.0
        assert_eq!(order_book.asks.peek().unwrap().0.order_id, 0); // The remaining ask should have the same order ID as the first order
        assert_eq!(order_book.asks.peek().unwrap().0.broker_id, 669); // The remaining ask should have the same broker ID as the first order
        assert_eq!(order_book.asks.peek().unwrap().0.order_type, LimitOrder); // The remaining ask should have the same order type as the first order

        assert!(order_book.bids.is_empty()); // No bids should remain in the order book
    
        // Give time for the logs to be flushed before the test ends
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}