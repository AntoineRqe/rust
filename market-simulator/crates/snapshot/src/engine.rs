use spsc::spsc_lock_free::{Consumer};
use std::sync::{Arc, atomic::{AtomicBool}};
use crate::types::Snapshot;
use types::multicast::MultiCastInfo;

pub struct SnapshotMultiCastEngine<'a, const N: usize> {
    fifo_in: Consumer<'a, Snapshot, N>,
    shutdown: Arc<AtomicBool>,
    multicast_info: MultiCastInfo,
}

impl <'a, const N: usize> SnapshotMultiCastEngine<'a, N> {
    pub fn new(fifo_in: Consumer<'a, Snapshot, N>, shutdown: Arc<AtomicBool>, ip: &str, port: u16) -> Self {
        Self { 
            fifo_in,
            shutdown,
            multicast_info: MultiCastInfo::new(ip, port),
        }
    }

    pub fn run(&mut self) {
        while !self.shutdown.load(std::sync::atomic::Ordering::Relaxed) || !self.fifo_in.is_empty() {
            if let Some(snapshot) = self.fifo_in.pop() {
                // Process the snapshot
                // For example, you can update the order book snapshot based on the snapshot
                if snapshot.timestamp == 0 {
                    tracing::info!("[{}] Received shutdown signal, stopping SnapshotMultiCastEngine", utils::market_name());
                    continue;
                }

                tracing::debug!("[{}] Received snapshot: {:?}", utils::market_name(), snapshot);

                // Process incoming order events and results from the order book engine
                // Transform the order event and result into a market data feed event
                let bytes = [0u8; 256]; // TODO: serialize the snapshot into bytes
                let _ = self.multicast_info.socket.send_to(&bytes, &self.multicast_info.addr);
            }
        }

        tracing::info!("[{}] SnapshotMultiCastEngine stopped", utils::market_name());
    }
}

