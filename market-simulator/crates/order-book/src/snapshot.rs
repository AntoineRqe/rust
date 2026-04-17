use std::sync::{Arc, RwLock, atomic::{AtomicBool}};
use spsc::spsc_lock_free::{Producer};
use crate::book::OrderBook;
use snapshot::types::{Snapshot};
use utils::market_name;
use types::Side;

pub struct SnapshotGenerationEngine<'a, const N: usize> {
    producer: Producer<'a, Snapshot, N>,
    shutdown: Arc<AtomicBool>,
    order_book: Arc<RwLock<OrderBook>>,
    interval_ms: u64,
    depth: usize,
}

impl <'a, const N: usize> SnapshotGenerationEngine<'a, N> {
    pub fn new(
        producer: Producer<'a, Snapshot, N>,
        shutdown: Arc<AtomicBool>,
        order_book: Arc<RwLock<OrderBook>>, 
        interval_ms: u64,
        depth: usize)
        -> Self {
        Self { producer, shutdown, order_book, interval_ms, depth }
    }

    fn generate_snapshot(&self) -> Snapshot {
        let order_book = self.order_book.read().unwrap();

        let bids = order_book.dump_order_book(Side::Buy, self.depth);
        let asks = order_book.dump_order_book(Side::Sell, self.depth);

        let id = order_book.internal_id_counter; // Use the internal order ID counter as a simple way to track the snapshot ID, it will increment with each new order processed and can serve as a unique identifier for the snapshot. In a more advanced implementation, you might want to use a separate counter or timestamp-based ID for snapshots.
        let order_book_snapshot = snapshot::types::OrderBookSnapshot { bids, asks };

        use std::time::{SystemTime, UNIX_EPOCH};

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();

        Snapshot {
            timestamp: timestamp as u64, // Use the elapsed time since the order book engine started as the timestamp for the snapshot
            symbol: order_book.symbol.clone(),
            id: id, // The ID can be set to a unique value if needed, for now we can set it to 0 or use a counter if we want to track multiple snapshots
            order_book: order_book_snapshot,
        }
    }

    pub fn run(&self) {
        while !self.shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(self.interval_ms)); // Sleep for the configured interval before generating the next snapshot
            
            if self.shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                break; // Check for shutdown signal again after waking up to avoid generating an unnecessary snapshot
            }

            let mut snapshot = self.generate_snapshot();
            tracing::debug!("[{}] Snapshot sent with ID: {}, timestamp: {}", market_name(), snapshot.id, snapshot.timestamp);
            
            while let Err(s) = self.producer.push(snapshot) {
                snapshot = s;
                std::hint::spin_loop(); // If the output queue is full, spin until there is space
            }
        }

        // Send a final snapshot with the shutdown flag set to true to signal the snapshot consumer to stop processing snapshots and exit gracefully.
        let mut snapshot = Snapshot::default();
    
        while let Err(s) = self.producer.push(snapshot) {
            snapshot = s;
            std::hint::spin_loop(); // If the output queue is full, spin until there is space
        }

        tracing::info!("[{}] Snapshot generation engine shutting down gracefully", market_name());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::OrderBook;
    use types::{OrderEvent, FixedPointArithmetic, Side, OrderType};
    use std::sync::atomic::Ordering;
    
    #[test]
    fn test_send_snapshot() {
        let order1 = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0),
            quantity: FixedPointArithmetic::from_f64(10.0),
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };
        let order2 = OrderEvent {
            price: FixedPointArithmetic::from_f64(102.0),
            quantity: FixedPointArithmetic::from_f64(5.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            ..Default::default() // Set the timestamp to the current time in milliseconds since epoch
        };

        let mut queue = spsc::spsc_lock_free::RingBuffer::<Snapshot, 1024>::new();
        
        std::thread::scope(|   s| {
            let (ss_producer, ss_consumer) = queue.split();

            let order_book = Arc::new(RwLock::new(OrderBook::new("TEST")));
            let shutdown = Arc::new(AtomicBool::new(false));

            let snapshot_engine = SnapshotGenerationEngine::new(
                ss_producer,
                Arc::clone(&shutdown),
                Arc::clone(&order_book),
                1000, // Set the snapshot interval to 1000 milliseconds (1 second)
                10, // Set the maximum depth of the order book to include in the snapshot
            );
            

            let _engine_handle = s.spawn(move || {
                snapshot_engine.run();
            });

            order_book.write().unwrap().process_order(order1);
            order_book.write().unwrap().process_order(order2);

            // Give some time for the snapshot to be sent and received
            std::thread::sleep(std::time::Duration::from_millis(1000));

            let recv_snapshot = ss_consumer.pop().unwrap();
        
            // Check ask side
            assert_eq!(recv_snapshot.order_book.bids.len(), 1); // One bid should be in the snapshot
            assert_eq!(recv_snapshot.order_book.bids[0].price, FixedPointArithmetic::from_f64(100.0));
            assert_eq!(recv_snapshot.order_book.bids[0].quantity, FixedPointArithmetic::from_f64(10.0));

            // Check bid side
            assert_eq!(recv_snapshot.order_book.asks.len(), 1); // One ask should be in the snapshot
            assert_eq!(recv_snapshot.order_book.asks[0].price, FixedPointArithmetic::from_f64(102.0));
            assert_eq!(recv_snapshot.order_book.asks[0].quantity, FixedPointArithmetic::from_f64(5.0));

            shutdown.store(true, Ordering::Relaxed);

            _engine_handle.join().expect("Engine thread panicked");
        });
    }
}