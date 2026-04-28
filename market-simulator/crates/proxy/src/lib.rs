use std::{io};
use types::multicast::{MulticastSource, SourceSocket};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;

pub struct MarketDataProxy {
    market_feed_source: MulticastSource,
    snapshot_feed_source: MulticastSource,
    shutdown: Arc<AtomicBool>,
    core_id: usize,
}

impl MarketDataProxy {
    pub fn new(
        market_feed_source: MulticastSource,
        snapshot_feed_source: MulticastSource,
        shutdown: Arc<AtomicBool>,
        core_id: usize
    ) -> Self {
        Self {
            market_feed_source,
            snapshot_feed_source,
            shutdown,
            core_id,
        }
    }

    pub fn run(&self) {
        // Start threads for market feed
        let market_feed_handle = match subscribe_multicast(
            "market data feed".to_string(),
            self.market_feed_source.clone(),
            self.shutdown.clone(),
            self.core_id
        ) {
            Ok(handle) => handle,
            Err(e) => {
                tracing::error!("Failed to subscribe to market feed: {}", e);
                return;
            }
        };

        // Start threads for snapshot feed
        let snapshot_feed_handle = match subscribe_multicast(
            "snapshot feed".to_string(),
            self.snapshot_feed_source.clone(),
            self.shutdown.clone(),
            self.core_id
        ) {
            Ok(handle) => handle,
            Err(e) => {
                tracing::error!("Failed to subscribe to snapshot feed: {}", e);
                return;
            }
        };

        // Wait for threads to finish (in a real application, you might want to handle this more gracefully)
        market_feed_handle.join().expect("Market feed thread panicked").expect("Market feed thread failed");
        snapshot_feed_handle.join().expect("Snapshot feed thread panicked").expect("Snapshot feed thread failed");
    }
}




pub fn subscribe_multicast(
    service: String,
    source: MulticastSource,
    shutdown: Arc<AtomicBool>,
    core_id: usize
) -> Result<std::thread::JoinHandle<Result<(), io::Error>>, io::Error> {

    let handle = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: core_id });
        let socket = match SourceSocket::create_multicast_receiver_socket(source.port) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to create socket for {}:{} - {}", source.ip, source.port, e);
                return Err(e);
            }
        };

        // Join multicast group
        let group_addr: std::net::Ipv4Addr = match source.ip.parse() {
            Ok(addr) => addr,
            Err(e) => {
                tracing::error  !("Invalid multicast IP address {}: {}", source.ip, e);
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "Invalid multicast IP"));
            }
        };


        if let Err(e) = socket.join_multicast_v4(&group_addr, &std::net::Ipv4Addr::UNSPECIFIED) {
            tracing::error!("Failed to join multicast group {}: {} - {}", source.ip, source.port, e);
            return Err(e);
        }

        // Set a read timeout to allow the thread to check for shutdown signals periodically
        let _ = socket.set_read_timeout(Some(Duration::from_millis(100)));

        tracing::info!("[{}] Subscribed to multicast group {}:{} for {}", source.market, source.ip, source.port, service);
        let mut buf = [0u8; 65536];

        while !shutdown.load(Ordering::Relaxed) {
            let (len, src) = match socket.recv_from(&mut buf) {
                Ok((len, src)) => (len, src),
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    // Timeout occurred, check for shutdown signal and continue
                    continue;
                }
                Err(e) => {
                    tracing::error!("Error receiving from multicast socket {}:{} - {}", source.ip, source.port, e);
                    return Err(e);
                }
            };

            tracing::debug!("[{}] {} -> Received {} bytes from {} ", source.market, service, len, src);
            // TODO: parse and cache market data
        }

        tracing::info!("[{}] Shutting down multicast subscriber for {}:{} ({})", source.market, source.ip, source.port, service);
        Ok(())
    });

    Ok(handle)
}