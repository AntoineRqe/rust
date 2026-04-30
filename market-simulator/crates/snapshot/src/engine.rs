use spsc::spsc_lock_free::{Consumer};
use utils::market_name;
use std::sync::{Arc, atomic::{AtomicBool}};
use crate::types::Snapshot;
use types::multicast::{SourceSocket};

pub struct SnapshotMultiCastEngine<'a, const N: usize> {
    fifo_in: Consumer<'a, Arc<Snapshot>, N>,
    shutdown: Arc<AtomicBool>,
    source: SourceSocket,
}

impl <'a, const N: usize> SnapshotMultiCastEngine<'a, N> {
    pub fn new(fifo_in: Consumer<'a, Arc<Snapshot>, N>, shutdown: Arc<AtomicBool>, ip: String, port: u16) -> Self {
        Self { 
            fifo_in,
            shutdown,
            source: SourceSocket::new(ip, port, market_name()).expect("Failed to create multicast socket for SnapshotMultiCastEngine"),
        }
    }

    pub fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        while !self.shutdown.load(std::sync::atomic::Ordering::Relaxed) || !self.fifo_in.is_empty() {
            if let Some(snapshot) = self.fifo_in.pop() {
                if snapshot.timestamp_ms == 0 {
                    tracing::info!("[{}] Received shutdown signal, stopping SnapshotMultiCastEngine", market_name());
                    continue;
                }

                // Process incoming order events and results from the order book engine
                // Transform the order event and result into a market data feed event
                let bytes = crate::encode::encode_snapshot(&snapshot);
                if let Err(e) = self.source.socket.send_to(&bytes, &self.source.source.address) {
                    tracing::error!("[{}] Failed to send snapshot over multicast: {e:#}", market_name());
                }
            }
        }

        tracing::info!("[{}] SnapshotMultiCastEngine stopped", market_name());
        Ok(())
    }
}

