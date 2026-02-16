use std::{cmp::Reverse, collections::BinaryHeap};
use crate::types::{Order, Side, OrderResult, OrderStatus};
use tracing::{instrument};


/// Represents the order book, maintaining separate heaps for bids and asks.
/// Bids are stored in a max-heap (higher prices have priority), while asks are stored in a min-heap (lower prices have priority).
/// The order book processes incoming orders, matches them against existing orders, and updates the order book accordingly.
pub struct OrderBook {
    pub bids: BinaryHeap<Order>,
    pub asks: BinaryHeap<Reverse<Order>>,
    id_counter: u64,
}

impl OrderBook {
    pub fn new() -> Self {
        OrderBook {
            bids: BinaryHeap::new(),
            asks: BinaryHeap::new(),
            id_counter: 0,
        }
    }

    #[instrument(level = "debug", skip(self, order), fields(order_id = order.id, side = ?order.side, price = order.price, quantity = order.quantity))]
    pub fn process_order(&mut self, order: Order) -> OrderResult {

        match order.side {
            Side::Buy => self.process_buy_order(order),
            Side::Sell => self.process_sell_order(order),
        }
    }

    fn generate_order_result(&mut self, order: &Order, status: Option<OrderStatus>, original_quantity: f64) -> OrderResult {
        OrderResult {
            price: order.price,
            quantity: order.quantity,
            side: order.side,
            order_type: order.order_type,
            order_id: order.id,
            trade_id: if order.quantity < original_quantity {
                self.id_counter += 1;
                Some(self.id_counter) // Example trade ID
            } else {
                None
            },
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

    fn process_sell_order(&mut self, mut order: Order) -> OrderResult {
        let original_quantity = order.quantity;

        while let Some(best_bid) = self.bids.peek() {
            if best_bid.price >= order.price {
                let mut best_bid = self.bids.pop().unwrap();
                let trade_quantity = order.quantity.min(best_bid.quantity);
                // Process the trade here (e.g., update quantities, record the trade, etc.)
                best_bid.quantity -= trade_quantity;
    
                if best_bid.quantity > 0.0 {
                    self.bids.push(best_bid);
                }
                // Update the incoming order's quantity
                order.quantity -= trade_quantity;

                if order.quantity == 0.0 {
                    return self.generate_order_result(&order, Some(OrderStatus::Filled), original_quantity);
                }
            } else {
                break;
            }
        }

        let order_result = self.generate_order_result(&order, None, original_quantity);

        if order.quantity > 0.0 {
            self.asks.push(Reverse(order));
        }

        order_result
    }

    fn process_buy_order(&mut self, mut order: Order) -> OrderResult {
        let original_quantity = order.quantity;

        while let Some(Reverse(best_ask)) = self.asks.peek() {
            if best_ask.price <= order.price {
                let mut best_ask = self.asks.pop().unwrap().0;
                let trade_quantity = order.quantity.min(best_ask.quantity);
                // Process the trade here (e.g., update quantities, record the trade, etc.)
                best_ask.quantity -= trade_quantity;
    
                if best_ask.quantity > 0.0 {
                    self.asks.push(Reverse(best_ask));
                }
                // Update the incoming order's quantity
                order.quantity -= trade_quantity;

                if order.quantity == 0.0 {
                    return self.generate_order_result(&order, Some(OrderStatus::Filled), original_quantity);
                }
            } else {
                break;
            }
        }

        let order_result = self.generate_order_result(&order, None, original_quantity);

        if order.quantity > 0.0 {
            self.bids.push(order);
        }

        order_result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::OrderType::{LimitOrder};

    #[test]
    fn test_order_book() {
        logging::init_tracing("order_book");

        tracing::debug!("TEST STARTED");

        let mut order_book = OrderBook::new();
        let order1 = Order {
            price: 100.0,
            quantity: 10.0,
            side: Side::Buy,
            order_type: LimitOrder,
            id: 1,
        };
        let order2 = Order {
            price: 99.0,
            quantity: 5.0,
            side: Side::Sell,
            order_type: LimitOrder,
            id: 2,
        };
        let order3 = Order {
            price: 98.0,
            quantity: 10.0,
            side: Side::Sell,
            order_type: LimitOrder,
            id: 3,
        };

        let result1 = order_book.process_order(order1);
        let result2 = order_book.process_order(order2);
        let result3 = order_book.process_order(order3);

        // The first order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the bids.
        assert_eq!(result1.price, 100.0);
        assert_eq!(result1.quantity, 10.0); // 5 units filled, 5 units remaining
        assert_eq!(result1.side, Side::Buy);
        assert_eq!(result1.order_type, LimitOrder);
        assert_eq!(result1.order_id, 1);
        assert!(result1.trade_id.is_none());
        assert_eq!(result1.status, OrderStatus::NotMatched);

        // The second order should be completely filled (5 units filled, 0 units remaining).
        assert_eq!(result2.price, 99.0);
        assert_eq!(result2.quantity, 0.0);
        assert_eq!(result2.side, Side::Sell);
        assert_eq!(result2.order_type, LimitOrder);
        assert_eq!(result2.order_id, 2);
        assert!(result2.trade_id.is_some());
        assert_eq!(result2.status, OrderStatus::Filled);

        // The third order should not be matched immediately, as there are no existing orders in the order book, so it should be added to the asks.
        assert_eq!(result3.price, 98.0);
        assert_eq!(result3.quantity, 5.0); // 5 units filled, 5 units remaining
        assert_eq!(result3.side, Side::Sell);
        assert_eq!(result3.order_type, LimitOrder);
        assert_eq!(result3.order_id, 3);
        assert!(result3.trade_id.is_some());
        assert_eq!(result3.status, OrderStatus::PartiallyFilled);

        std::thread::sleep(std::time::Duration::from_millis(100));

    }
}