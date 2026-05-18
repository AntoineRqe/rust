use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use spsc::spsc_lock_free::{Consumer, Producer};
use types::macros::SymbolId;
use types::OrderEvent;
use utils::market_name;

pub struct OrderBookAggregator<'a, const N: usize> {
    fifo_in: Consumer<'a, OrderEvent, N>,
    routes: HashMap<SymbolId, Producer<'a, OrderEvent, N>>,
    shutdown: Arc<AtomicBool>,
}

impl<'a, const N: usize> OrderBookAggregator<'a, N> {
    pub fn new(
        fifo_in: Consumer<'a, OrderEvent, N>,
        routes: HashMap<SymbolId, Producer<'a, OrderEvent, N>>,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            fifo_in,
            routes,
            shutdown,
        }
    }

    pub fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            if let Some(event) = self.fifo_in.pop_timeout(Duration::from_millis(500)) {
                if event.sender_id.0.iter().all(|&b| b == 0) {
                    tracing::info!("[{}] Aggregator shutdown signal received", market_name());
                    break;
                }

                let symbol = event.symbol;
                match self.routes.get_mut(&symbol) {
                    Some(route) => {
                        let mut routed = event;
                        while let Err(returned) = route.push(routed) {
                            routed = returned;
                            std::hint::spin_loop();
                        }
                    }
                    None => {
                        tracing::warn!(
                            "[{}] No order book route configured for symbol '{}'",
                            market_name(),
                            symbol
                        );
                    }
                }
            }

            if self.shutdown.load(Ordering::Relaxed) && self.fifo_in.is_empty() {
                tracing::info!("[{}] Aggregator shutting down", market_name());
                break;
            }
        }

        Ok(())
    }
}
