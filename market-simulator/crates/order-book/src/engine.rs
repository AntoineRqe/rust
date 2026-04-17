use crate::book::OrderBook;
use spsc::spsc_lock_free::{Consumer, Producer};
use std::sync::{Arc, RwLock, atomic::{AtomicBool, Ordering}};
use types::{
    OrderEvent,
    OrderResult,
};

use types::macros::{EntityId};

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
    order_book: Arc<RwLock<OrderBook>>,
    shutdown: Arc<AtomicBool>,
}

impl<'a, const N: usize> OrderBookEngine<'a, N> {
    pub fn new(
        fifo_in: Consumer<'a, OrderEvent, N>,
        fifo_out: [Option<Producer<'a, (OrderEvent, OrderResult), N>>; 3],
        control_rx: crossbeam_channel::Receiver<OrderBookControl>,
        order_book: Arc<RwLock<OrderBook>>,
        shutdown: Arc<AtomicBool>
    ) -> Self {
        OrderBookEngine {
            fifo_in,
            fifo_out,
            control_rx,
            order_book,
            shutdown,
        }
    }

    pub fn import_order_book(&mut self, orders: Vec<OrderEvent>) {
        for order in orders {
            self.order_book.write().unwrap().process_order(order);
        }
    }

    pub fn run(&mut self) {
        loop {
            // Process control messages first
            while let Ok(control) = self.control_rx.try_recv() {
                match control {
                    OrderBookControl::Reset => {
                        *self.order_book.write().unwrap() = OrderBook::new(self.order_book.read().unwrap().symbol.as_str()); // Reset the order book by creating a new instance
                        tracing::info!("[{}][{}] Order book reset completed", market_name(), self.order_book.read().unwrap().symbol);
                    }
                }
            }

            if let Some(event) = self.fifo_in.pop() {
                let (event, result) = self.order_book.write().unwrap().process_order(event);
                let mut execution_report_input = (event, result);
                for producer in &self.fifo_out {
                    if let Some(producer) = producer {
                        while let Err((event_er, result_er)) = producer.push(execution_report_input) {
                            execution_report_input = (event_er, result_er);
                            std::hint::spin_loop(); // If the output queue is full, spin until there is space
                        }
                    }
                }
            }

            if self.shutdown.load(Ordering::Relaxed) && self.fifo_in.is_empty() {
                tracing::info!("[{}][{}] Shutdown signal received, stopping order book engine", market_name(), self.order_book.read().unwrap().symbol);
                break;
            }
        }

        tracing::info!("[{}][{}] Order book engine shutting down gracefully", market_name(), self.order_book.read().unwrap().symbol);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use types::macros::{SymbolId, EntityId, OrderId};
    use types::{OrderEvent, FixedPointArithmetic, Side, OrderType, OrderStatus};

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
            let order_book = Arc::new(RwLock::new(OrderBook::new(SYMBOL_STR)));
            let(inbound_producer, inbound_consumer) = inbound_queue.split();
            let(outbound_producer, outbound_consumer) = outbound_queue.split();

            let (_control_tx, control_rx) = crossbeam_channel::unbounded();
            let mut engine = OrderBookEngine::new(
        inbound_consumer,
        [Some(outbound_producer), None, None],
                control_rx,
                Arc::clone(&order_book),
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
}