use std::sync::{Arc, atomic::{AtomicBool}};
use spsc::{Producer};
use snapshot::types::{Snapshot};
use utils::market_name;
use arc_swap::ArcSwap;

/// Snapshot generation engine that runs in a separate thread and periodically sends snapshots of the order book to the output queue.
/// It uses an `ArcSwap` to hold the latest snapshot, allowing for efficient updates without blocking the snapshot generation thread.
/// The engine checks for a shutdown signal to gracefully exit when requested.
pub struct SnapshotGenerationEngine<'a, const N: usize> {
    /// Producer for sending snapshots to the output queue.
    producer: Producer<'a, Arc<Snapshot>, N>,
    /// Atomic boolean flag to signal shutdown of the snapshot generation engine.
    shutdown: Arc<AtomicBool>,
    /// ArcSwap holding the latest snapshot of the order book, allowing for efficient updates and reads without blocking.
    snapshot_ptr: Arc<ArcSwap<Snapshot>>,
    /// Interval in milliseconds between snapshot generations, allowing for configurable snapshot frequency.
    interval_ms: u64,
}

impl <'a, const N: usize> SnapshotGenerationEngine<'a, N> {
    pub fn new(
        producer: Producer<'a, Arc<Snapshot>, N>,
        shutdown: Arc<AtomicBool>,
        snapshot_ptr: Arc<ArcSwap<Snapshot>>, 
        interval_ms: u64) -> Self
    {
        Self {  producer,
                shutdown,
                snapshot_ptr,
                interval_ms 
        }
    }

    pub fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            let wait_started = std::time::Instant::now();
            let wait_target = std::time::Duration::from_millis(self.interval_ms);

            while wait_started.elapsed() < wait_target {
                if self.shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }

                let remaining = wait_target.saturating_sub(wait_started.elapsed());
                std::thread::park_timeout(remaining);
            }
            
            if self.shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                break; // Check for shutdown signal again after waking up to avoid generating an unnecessary snapshot
            }

            let mut snapshot = self.snapshot_ptr.load_full();

            tracing::debug!("[{}] Snapshot sent with ID: {}, timestamp: {}", market_name(), snapshot.id, snapshot.timestamp_ms);
            
            while let Err(s) = self.producer.push(snapshot) {
                snapshot = s;
                std::hint::spin_loop(); // If the output queue is full, spin until there is space
            }
        }

        // Send a final snapshot with the shutdown flag set to true to signal the snapshot consumer to stop processing snapshots and exit gracefully.
        let mut snapshot = Arc::new(Snapshot::default());
    
        while let Err(s) = self.producer.push(Arc::clone(&snapshot)) {
            snapshot = s;
            std::hint::spin_loop(); // If the output queue is full, spin until there is space
        }

        tracing::info!("[{}] Snapshot generation engine shutting down gracefully", market_name());

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use types::{OrderEvent, FixedPointArithmetic, Side, OrderType};
    use std::sync::atomic::Ordering;
    use snapshot::types::OrderBookSnapshot;
    
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

        let mut snapshot = Snapshot {
            timestamp_ms: 1627846267000,
            symbol: "TEST".to_string(),
            id: 1,
            order_book: OrderBookSnapshot::default(),
        };

        snapshot.order_book.add_bid(order1).unwrap();
        snapshot.order_book.add_ask(order2).unwrap();

        let mut queue = spsc::spsc_lock_free::RingBuffer::<Arc<Snapshot>, 1024>::new();
        
        std::thread::scope(|   s| {
            let (ss_producer, ss_consumer) = queue.split();

            let snapshot_ptr = Arc::new(ArcSwap::from_pointee(snapshot));
            let shutdown = Arc::new(AtomicBool::new(false));

            let snapshot_engine = SnapshotGenerationEngine::new(
                ss_producer,
            Arc::clone(&shutdown),
            Arc::clone(&snapshot_ptr),
        1000, // Set the snapshot interval to 1000 milliseconds (1 second)
            );
            

            let _engine_handle = s.spawn(move || {
                let _ = snapshot_engine.run();
            });

            let mut updated_snapshot = Snapshot::default();
            updated_snapshot.order_book.add_bid(order1).unwrap();
            updated_snapshot.order_book.add_ask(order2).unwrap();
            snapshot_ptr.store(Arc::new(updated_snapshot));

            // Give some time for the snapshot to be sent and received
            std::thread::sleep(std::time::Duration::from_millis(1000));

            let recv_snapshot = ss_consumer.pop().unwrap();
        
            // Check ask side
            assert_eq!(recv_snapshot.order_book.bids_len, 1); // One bid should be in the snapshot
            assert_eq!(recv_snapshot.order_book.bids[0].price, FixedPointArithmetic::from_f64(100.0));
            assert_eq!(recv_snapshot.order_book.bids[0].quantity, FixedPointArithmetic::from_f64(10.0));

            // Check bid side
            assert_eq!(recv_snapshot.order_book.asks_len, 1); // One ask should be in the snapshot
            assert_eq!(recv_snapshot.order_book.asks[0].price, FixedPointArithmetic::from_f64(102.0));
            assert_eq!(recv_snapshot.order_book.asks[0].quantity, FixedPointArithmetic::from_f64(5.0));

            shutdown.store(true, Ordering::Relaxed);

            _engine_handle.join().expect("Engine thread panicked");
        });
    }
}