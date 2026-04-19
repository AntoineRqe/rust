use crate::book::OrderBook;
use spsc::spsc_lock_free::{Consumer, Producer};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use types::{
    OrderEvent,
    OrderResult,
};

use types::macros::{EntityId};
use arc_swap::ArcSwap;
use snapshot::types::Snapshot;

use utils::market_name;

pub fn kill_order_book_engine<const N: usize>(fix_to_ob_tx: &Producer<OrderEvent, N>) {
    let order_event = OrderEvent {
        sender_id: EntityId::from_ascii(""), // An empty sender_id is used as a signal to the order book engine to shut down
        ..Default::default() // Fill the rest of the fields with default values
    };

    fix_to_ob_tx.push(order_event).unwrap();  
} 

pub enum OrderBookControl {
    Reset,
}

pub struct OrderBookEngine<'a, const N: usize> {
    fifo_in: Consumer<'a, OrderEvent, N>,
    fifo_out: [Option<Producer<'a, (OrderEvent, OrderResult), N>>; 3],
    control_rx: crossbeam_channel::Receiver<OrderBookControl>,
    order_book: OrderBook,
    snapshot_ptr: Option<Arc<ArcSwap<Snapshot>>>,
    shutdown: Arc<AtomicBool>,
}

impl<'a, const N: usize> OrderBookEngine<'a, N> {
    pub fn new(
        fifo_in: Consumer<'a, OrderEvent, N>,
        fifo_out: [Option<Producer<'a, (OrderEvent, OrderResult), N>>; 3],
        control_rx: crossbeam_channel::Receiver<OrderBookControl>,
        order_book: OrderBook,
        snapshot_ptr: Option<Arc<ArcSwap<Snapshot>>>,
        shutdown: Arc<AtomicBool>
    ) -> Self {
        OrderBookEngine {
            fifo_in,
            fifo_out,
            control_rx,
            order_book,
            snapshot_ptr,
            shutdown,
        }
    }

    pub fn import_order_book(&mut self, orders: Vec<OrderEvent>) {
        for order in orders {
            self.order_book.process_order(order);
        }
    }

    /// Reduces the quantity at a price level or removes the level if the quantity is fully reduced
    /// Arguments:
    /// - `levels`: The array of order events representing either the bid or ask side of the order book
    /// - `len`: The number of valid levels currently in the `levels` array
    /// - `price`: The price level to be reduced
    /// - `quantity`: The quantity to reduce at the specified price level
    fn reduce_level(levels: &mut [OrderEvent], len: &mut usize, price: types::FixedPointArithmetic, quantity: types::FixedPointArithmetic) {
        for index in 0..*len {
            if levels[index].price == price {
                if levels[index].quantity > quantity {
                    levels[index].quantity = levels[index].quantity - quantity;
                } else {
                    for shift_index in index..(*len - 1) {
                        levels[shift_index] = levels[shift_index + 1];
                    }
                    *len -= 1;
                }
                return;
            }
        }
    }

    /// Upserts a price level in the order book. If the price level already exists, it is updated with the new quantity.
    /// If it does not exist, it is inserted in the correct position based on price priority.
    /// Arguments:
    /// - `levels`: The array of order events representing either the bid or ask side of the order book
    /// - `len`: The number of valid levels currently in the `levels` array
    /// - `order`: The order event containing the price and quantity to be upserted
    /// - `descending`: A boolean indicating whether the levels are sorted in descending order (true for bids, false for asks)
    fn upsert_level(levels: &mut [OrderEvent], len: &mut usize, order: OrderEvent, descending: bool) {
        for index in 0..*len {
            if levels[index].price == order.price {
                levels[index] = order;
                return;
            }
        }

        let mut insert_index = *len;
        for index in 0..*len {
            let should_insert = if descending {
                order.price > levels[index].price
            } else {
                order.price < levels[index].price
            };

            if should_insert {
                insert_index = index;
                break;
            }
        }

        if *len < levels.len() {
            for shift_index in (insert_index..*len).rev() {
                levels[shift_index + 1] = levels[shift_index];
            }
            levels[insert_index] = order;
            *len += 1;
            return;
        }

        if insert_index < levels.len() {
            for shift_index in (insert_index..(levels.len() - 1)).rev() {
                levels[shift_index + 1] = levels[shift_index];
            }
            levels[insert_index] = order;
        }
    }

    /// Updates the snapshot with the latest state of the order book after processing an order event and its result
    /// Arguments:
    /// - `event`: The order event that was processed
    /// - `order_result`: The result of processing the order event, containing information about executed trades and the timestamp of the event
    pub fn incremental_update(&mut self, event: OrderEvent, order_result: OrderResult) {

        if let Some(snapshot_ptr) = &self.snapshot_ptr {
            snapshot_ptr.rcu(|current| {
                    let mut next = Snapshot {
                        timestamp: current.timestamp,
                        symbol: if current.symbol.is_empty() { self.order_book.symbol.clone() } else { current.symbol.clone() },
                        id: current.id,
                        order_book: snapshot::types::OrderBookSnapshot::default(),
                };

                next.order_book.bids_len = current.order_book.bids_len;
                next.order_book.asks_len = current.order_book.asks_len;
                next.order_book.bids[..next.order_book.bids_len].copy_from_slice(&current.order_book.bids[..current.order_book.bids_len]);
                next.order_book.asks[..next.order_book.asks_len].copy_from_slice(&current.order_book.asks[..current.order_book.asks_len]);

                match event.order_type {
                    types::OrderType::CancelOrder => {
                        if order_result.status == types::OrderStatus::Cancelled {
                            match event.side {
                                types::Side::Buy => Self::reduce_level(
                                    &mut next.order_book.bids,
                                    &mut next.order_book.bids_len,
                                    event.price,
                                    event.quantity,
                                ),
                                types::Side::Sell => Self::reduce_level(
                                    &mut next.order_book.asks,
                                    &mut next.order_book.asks_len,
                                    event.price,
                                    event.quantity,
                                ),
                            }
                        }
                    }
                    _ => {
                        for trade in order_result.trades.iter() {
                            if trade.quantity == types::FixedPointArithmetic::ZERO {
                                continue;
                            }

                            match event.side {
                                types::Side::Buy => Self::reduce_level(
                                    &mut next.order_book.asks,
                                    &mut next.order_book.asks_len,
                                    trade.price,
                                    trade.quantity,
                                ),
                                types::Side::Sell => Self::reduce_level(
                                    &mut next.order_book.bids,
                                    &mut next.order_book.bids_len,
                                    trade.price,
                                    trade.quantity,
                                ),
                            }
                        }

                        let traded_quantity = order_result.trades.quantity_sum();
                        let leaves_qty = if event.quantity > traded_quantity {
                            event.quantity - traded_quantity
                        } else {
                            types::FixedPointArithmetic::ZERO
                        };

                        if event.order_type == types::OrderType::LimitOrder && leaves_qty > types::FixedPointArithmetic::ZERO {
                            let mut resting_order = event;
                            resting_order.quantity = leaves_qty;

                            match event.side {
                                types::Side::Buy => Self::upsert_level(
                                    &mut next.order_book.bids,
                                    &mut next.order_book.bids_len,
                                    resting_order,
                                    true,
                                ),
                                types::Side::Sell => Self::upsert_level(
                                    &mut next.order_book.asks,
                                    &mut next.order_book.asks_len,
                                    resting_order,
                                    false,
                                ),
                            }
                        }
                    }
                }

                next.timestamp = order_result.timestamp;
                next.id = current.id.wrapping_add(1);

                Arc::new(next)
            });
        }
    }

    pub fn run(&mut self) {
        loop {
            // Process control messages first
            while let Ok(control) = self.control_rx.try_recv() {
                match control {
                    OrderBookControl::Reset => {
                        self.order_book = OrderBook::new(self.order_book.symbol.as_str()); // Reset the order book by creating a new instance
                        tracing::info!("[{}][{}] Order book reset completed", market_name(), self.order_book.symbol);
                    }
                }
            }

            if let Some(event) = self.fifo_in.pop() {
                // Process incoming order events from the input queue
                let (event, result) = self.order_book.process_order(event);
                let snapshot_event = event;
                let snapshot_result = result;
                // Generate the execution report and push it to the output queues
                let mut execution_report_input = (event, result);
                for producer in &self.fifo_out {
                    if let Some(producer) = producer {
                        while let Err((event_er, result_er)) = producer.push(execution_report_input) {
                            execution_report_input = (event_er, result_er);
                            std::hint::spin_loop(); // If the output queue is full, spin until there is space
                        }
                    }
                }
                // Update the snapshot with the latest state of the order book after processing the order
                if self.snapshot_ptr.is_some() {
                    self.incremental_update(snapshot_event, snapshot_result);
                }
            }

            if self.shutdown.load(Ordering::Relaxed) && self.fifo_in.is_empty() {
                tracing::info!("[{}][{}] Shutdown signal received, stopping order book engine", market_name(), self.order_book.symbol);
                break;
            }
        }

        tracing::info!("[{}][{}] Order book engine shutting down gracefully", market_name(), self.order_book.symbol);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use types::macros::{SymbolId, EntityId, OrderId};
    use types::{OrderEvent, FixedPointArithmetic, Side, OrderType, OrderStatus, Trade, Trades};
    use snapshot::types::OrderBookSnapshot;

    const SYMBOL_STR: &str = "TEST";
    const SYMBOL_ID: SymbolId = SymbolId::from_ascii(SYMBOL_STR);
    const SENDER: EntityId = EntityId::from_ascii("SENDER0000000000000");
    const TARGET: EntityId = EntityId::from_ascii("TARGET0000000000000");
    const CL_ORD_ID: OrderId = OrderId::from_ascii("12345");

    use std::thread;

    #[test]
    fn test_engine(){
        let mut inbound_queue = spsc::spsc_lock_free::RingBuffer::<OrderEvent, 1024>::new();
        let mut outbound_queue = spsc::spsc_lock_free::RingBuffer::<(OrderEvent, OrderResult), 1024>::new();

        thread::scope(|s| {
            let shutdown = Arc::new(AtomicBool::new(false));
            let order_book = OrderBook::new(SYMBOL_STR);
            let snapshot_ptr = Arc::new(ArcSwap::from_pointee(Snapshot::default()));

            let(inbound_producer, inbound_consumer) = inbound_queue.split();
            let(outbound_producer, outbound_consumer) = outbound_queue.split();

            let (_control_tx, control_rx) = crossbeam_channel::unbounded();
            let mut engine = OrderBookEngine::new(
        inbound_consumer,
        [Some(outbound_producer), None, None],
                control_rx,
                order_book,
                Some(Arc::clone(&snapshot_ptr)),
                Arc::clone(&shutdown),
            );
            
            let ob_handle = s.spawn(move || {
                engine.run();
            });

            // Send some orders to the engine
            let order = OrderEvent {
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

            inbound_producer.push(order).unwrap();
            // Give some time for the engine to process the order
            std::thread::sleep(std::time::Duration::from_millis(100));
            let (order_event, order_result) = outbound_consumer.pop().unwrap();

            assert!(order_event.price == FixedPointArithmetic::from_f64(100.0));
            assert!(order_result.trades.len() == 0);

            assert!(order_result.status == OrderStatus::New);
            
            // Send a matching sell order to the engine
            let order2 = OrderEvent {
                price: FixedPointArithmetic::from_f64(100.0),
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

            inbound_producer.push(order2).unwrap();
            // Give some time for the engine to process the order
            std::thread::sleep(std::time::Duration::from_millis(100));
            let (order_event2, order_result2) = outbound_consumer.pop().unwrap();
            
            assert!(order_event2.price == FixedPointArithmetic::from_f64(100.0));
            assert!(order_result2.trades.len() == 1); // One trade should be executed for the matching orders
            assert!(order_result2.status == OrderStatus::New); // Both orders should be filled

            // Send a dummy order to unblock the engine if it's waiting for orders
            shutdown.store(true, std::sync::atomic::Ordering::Release);
            kill_order_book_engine(&inbound_producer);

            ob_handle.join().expect("Engine thread panicked");
        });
    }

    #[test]
    fn test_incremental_snapshot_partial_fill_then_cancel() {
        const N: usize = 1024;

        let mut inbound_queue = spsc::spsc_lock_free::RingBuffer::<OrderEvent, N>::new();
        let mut outbound_queue = spsc::spsc_lock_free::RingBuffer::<(OrderEvent, OrderResult), N>::new();

        let (_inbound_producer, inbound_consumer) = inbound_queue.split();
        let (_outbound_producer, _outbound_consumer) = outbound_queue.split();

        let (_control_tx, control_rx) = crossbeam_channel::unbounded();
        let shutdown = Arc::new(AtomicBool::new(false));

        let mut initial_book = OrderBookSnapshot::default();
        let initial_ask = OrderEvent {
            price: FixedPointArithmetic::from_f64(101.0),
            quantity: FixedPointArithmetic::from_f64(7.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            symbol: SYMBOL_ID,
            ..Default::default()
        };
        let initial_bid = OrderEvent {
            price: FixedPointArithmetic::from_f64(99.0),
            quantity: FixedPointArithmetic::from_f64(5.0),
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            symbol: SYMBOL_ID,
            ..Default::default()
        };
        initial_book.add_ask(initial_ask).unwrap();
        initial_book.add_bid(initial_bid).unwrap();

        let snapshot_ptr = Arc::new(ArcSwap::from_pointee(Snapshot {
            timestamp: 1,
            symbol: SYMBOL_STR.to_string(),
            id: 1,
            order_book: initial_book,
        }));

        let mut engine = OrderBookEngine::new(
            inbound_consumer,
            [None, None, None],
            control_rx,
            OrderBook::new(SYMBOL_STR),
            Some(Arc::clone(&snapshot_ptr)),
            Arc::clone(&shutdown),
        );

        let taker_buy = OrderEvent {
            price: FixedPointArithmetic::from_f64(102.0),
            quantity: FixedPointArithmetic::from_f64(10.0),
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            symbol: SYMBOL_ID,
            ..Default::default()
        };

        let mut trades = Trades::new();
        trades.add_trade(Trade {
            price: FixedPointArithmetic::from_f64(101.0),
            quantity: FixedPointArithmetic::from_f64(4.0),
            ..Default::default()
        }).unwrap();

        engine.incremental_update(
            taker_buy,
            OrderResult {
                trades,
                status: OrderStatus::PartiallyFilled,
                timestamp: 2,
                ..Default::default()
            },
        );

        let after_fill = snapshot_ptr.load_full();
        assert_eq!(after_fill.order_book.asks_len, 1);
        assert_eq!(after_fill.order_book.asks[0].price, FixedPointArithmetic::from_f64(101.0));
        assert_eq!(after_fill.order_book.asks[0].quantity, FixedPointArithmetic::from_f64(3.0));
        assert_eq!(after_fill.order_book.bids_len, 2);
        assert_eq!(after_fill.order_book.bids[0].price, FixedPointArithmetic::from_f64(102.0));
        assert_eq!(after_fill.order_book.bids[0].quantity, FixedPointArithmetic::from_f64(6.0));
        assert_eq!(after_fill.order_book.bids[1].price, FixedPointArithmetic::from_f64(99.0));
        assert_eq!(after_fill.order_book.bids[1].quantity, FixedPointArithmetic::from_f64(5.0));

        let cancel_order = OrderEvent {
            price: FixedPointArithmetic::from_f64(102.0),
            quantity: FixedPointArithmetic::from_f64(6.0),
            side: Side::Buy,
            order_type: OrderType::CancelOrder,
            symbol: SYMBOL_ID,
            ..Default::default()
        };

        engine.incremental_update(
            cancel_order,
            OrderResult {
                status: OrderStatus::Cancelled,
                timestamp: 3,
                ..Default::default()
            },
        );

        let after_cancel = snapshot_ptr.load_full();
        assert_eq!(after_cancel.order_book.bids_len, 1);
        assert_eq!(after_cancel.order_book.bids[0].price, FixedPointArithmetic::from_f64(99.0));
        assert_eq!(after_cancel.order_book.bids[0].quantity, FixedPointArithmetic::from_f64(5.0));
        assert_eq!(after_cancel.order_book.asks_len, 1);
        assert_eq!(after_cancel.order_book.asks[0].price, FixedPointArithmetic::from_f64(101.0));
        assert_eq!(after_cancel.order_book.asks[0].quantity, FixedPointArithmetic::from_f64(3.0));
    }

    #[test]
    fn test_incremental_snapshot_partial_fill_then_cancel_sell_side() {
        const N: usize = 1024;

        let mut inbound_queue = spsc::spsc_lock_free::RingBuffer::<OrderEvent, N>::new();
        let mut outbound_queue = spsc::spsc_lock_free::RingBuffer::<(OrderEvent, OrderResult), N>::new();

        let (_inbound_producer, inbound_consumer) = inbound_queue.split();
        let (_outbound_producer, _outbound_consumer) = outbound_queue.split();

        let (_control_tx, control_rx) = crossbeam_channel::unbounded();
        let shutdown = Arc::new(AtomicBool::new(false));

        let mut initial_book = OrderBookSnapshot::default();
        let initial_bid = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0),
            quantity: FixedPointArithmetic::from_f64(8.0),
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            symbol: SYMBOL_ID,
            ..Default::default()
        };
        let initial_ask = OrderEvent {
            price: FixedPointArithmetic::from_f64(103.0),
            quantity: FixedPointArithmetic::from_f64(4.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            symbol: SYMBOL_ID,
            ..Default::default()
        };
        initial_book.add_bid(initial_bid).unwrap();
        initial_book.add_ask(initial_ask).unwrap();

        let snapshot_ptr = Arc::new(ArcSwap::from_pointee(Snapshot {
            timestamp: 10,
            symbol: SYMBOL_STR.to_string(),
            id: 10,
            order_book: initial_book,
        }));

        let mut engine = OrderBookEngine::new(
            inbound_consumer,
            [None, None, None],
            control_rx,
            OrderBook::new(SYMBOL_STR),
            Some(Arc::clone(&snapshot_ptr)),
            Arc::clone(&shutdown),
        );

        let taker_sell = OrderEvent {
            price: FixedPointArithmetic::from_f64(99.0),
            quantity: FixedPointArithmetic::from_f64(10.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            symbol: SYMBOL_ID,
            ..Default::default()
        };

        let mut trades = Trades::new();
        trades.add_trade(Trade {
            price: FixedPointArithmetic::from_f64(100.0),
            quantity: FixedPointArithmetic::from_f64(3.0),
            ..Default::default()
        }).unwrap();

        engine.incremental_update(
            taker_sell,
            OrderResult {
                trades,
                status: OrderStatus::PartiallyFilled,
                timestamp: 11,
                ..Default::default()
            },
        );

        let after_fill = snapshot_ptr.load_full();
        assert_eq!(after_fill.order_book.bids_len, 1);
        assert_eq!(after_fill.order_book.bids[0].price, FixedPointArithmetic::from_f64(100.0));
        assert_eq!(after_fill.order_book.bids[0].quantity, FixedPointArithmetic::from_f64(5.0));
        assert_eq!(after_fill.order_book.asks_len, 2);
        assert_eq!(after_fill.order_book.asks[0].price, FixedPointArithmetic::from_f64(99.0));
        assert_eq!(after_fill.order_book.asks[0].quantity, FixedPointArithmetic::from_f64(7.0));
        assert_eq!(after_fill.order_book.asks[1].price, FixedPointArithmetic::from_f64(103.0));
        assert_eq!(after_fill.order_book.asks[1].quantity, FixedPointArithmetic::from_f64(4.0));

        let cancel_order = OrderEvent {
            price: FixedPointArithmetic::from_f64(99.0),
            quantity: FixedPointArithmetic::from_f64(7.0),
            side: Side::Sell,
            order_type: OrderType::CancelOrder,
            symbol: SYMBOL_ID,
            ..Default::default()
        };

        engine.incremental_update(
            cancel_order,
            OrderResult {
                status: OrderStatus::Cancelled,
                timestamp: 12,
                ..Default::default()
            },
        );

        let after_cancel = snapshot_ptr.load_full();
        assert_eq!(after_cancel.order_book.asks_len, 1);
        assert_eq!(after_cancel.order_book.asks[0].price, FixedPointArithmetic::from_f64(103.0));
        assert_eq!(after_cancel.order_book.asks[0].quantity, FixedPointArithmetic::from_f64(4.0));
        assert_eq!(after_cancel.order_book.bids_len, 1);
        assert_eq!(after_cancel.order_book.bids[0].price, FixedPointArithmetic::from_f64(100.0));
        assert_eq!(after_cancel.order_book.bids[0].quantity, FixedPointArithmetic::from_f64(5.0));
    }

    #[test]
    fn test_incremental_snapshot_depth_boundary_keeps_best_levels() {
        const N: usize = 1024;

        let mut inbound_queue = spsc::spsc_lock_free::RingBuffer::<OrderEvent, N>::new();
        let mut outbound_queue = spsc::spsc_lock_free::RingBuffer::<(OrderEvent, OrderResult), N>::new();

        let (_inbound_producer, inbound_consumer) = inbound_queue.split();
        let (_outbound_producer, _outbound_consumer) = outbound_queue.split();

        let (_control_tx, control_rx) = crossbeam_channel::unbounded();
        let shutdown = Arc::new(AtomicBool::new(false));

        let mut initial_book = OrderBookSnapshot::default();
        for raw_price in (101..=110).rev() {
            initial_book.add_bid(OrderEvent {
                price: FixedPointArithmetic::from_f64(raw_price as f64),
                quantity: FixedPointArithmetic::from_f64(1.0),
                side: Side::Buy,
                order_type: OrderType::LimitOrder,
                symbol: SYMBOL_ID,
                ..Default::default()
            }).unwrap();
        }

        let snapshot_ptr = Arc::new(ArcSwap::from_pointee(Snapshot {
            timestamp: 20,
            symbol: SYMBOL_STR.to_string(),
            id: 20,
            order_book: initial_book,
        }));

        let mut engine = OrderBookEngine::new(
            inbound_consumer,
            [None, None, None],
            control_rx,
            OrderBook::new(SYMBOL_STR),
            Some(Arc::clone(&snapshot_ptr)),
            Arc::clone(&shutdown),
        );

        engine.incremental_update(
            OrderEvent {
                price: FixedPointArithmetic::from_f64(111.0),
                quantity: FixedPointArithmetic::from_f64(2.0),
                side: Side::Buy,
                order_type: OrderType::LimitOrder,
                symbol: SYMBOL_ID,
                ..Default::default()
            },
            OrderResult {
                status: OrderStatus::New,
                timestamp: 21,
                ..Default::default()
            },
        );

        let after_better_insert = snapshot_ptr.load_full();
        assert_eq!(after_better_insert.order_book.bids_len, 10);
        assert_eq!(after_better_insert.order_book.bids[0].price, FixedPointArithmetic::from_f64(111.0));
        assert_eq!(after_better_insert.order_book.bids[0].quantity, FixedPointArithmetic::from_f64(2.0));
        assert_eq!(after_better_insert.order_book.bids[9].price, FixedPointArithmetic::from_f64(102.0));

        engine.incremental_update(
            OrderEvent {
                price: FixedPointArithmetic::from_f64(90.0),
                quantity: FixedPointArithmetic::from_f64(3.0),
                side: Side::Buy,
                order_type: OrderType::LimitOrder,
                symbol: SYMBOL_ID,
                ..Default::default()
            },
            OrderResult {
                status: OrderStatus::New,
                timestamp: 22,
                ..Default::default()
            },
        );

        let after_worse_insert = snapshot_ptr.load_full();
        assert_eq!(after_worse_insert.order_book.bids_len, 10);
        assert_eq!(after_worse_insert.order_book.bids[0].price, FixedPointArithmetic::from_f64(111.0));
        assert_eq!(after_worse_insert.order_book.bids[9].price, FixedPointArithmetic::from_f64(102.0));
    }
}