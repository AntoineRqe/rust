use std::io;
use types::multicast::{MulticastSource, SourceSocket};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;

pub struct MarketDataProxy {
    market_feed_source: SourceSocket,
    snapshot_feed_source: SourceSocket,
    shutdown: Arc<AtomicBool>,
}

impl MarketDataProxy {
    pub fn new(
        market_feed_source: SourceSocket,
        snapshot_feed_source: SourceSocket,
        shutdown: Arc<AtomicBool>
    ) -> Self {
        Self {
            market_feed_source,
            snapshot_feed_source,
            shutdown,
        }
    }

    pub fn run(&self) {
        // Start threads for market feed
        let market_feed_handle = subscribe_multicast(
            self.market_feed_source.source.clone(),
            self.shutdown.clone()
        ).expect("Failed to subscribe to market feed");

        // Start threads for snapshot feed
        let snapshot_feed_handle = subscribe_multicast(
            self.snapshot_feed_source.source.clone(),
            self.shutdown.clone()
        ).expect("Failed to subscribe to snapshot feed");

        // Wait for threads to finish (in a real application, you might want to handle this more gracefully)
        market_feed_handle.join().expect("Market feed thread panicked").expect("Market feed thread failed");
        snapshot_feed_handle.join().expect("Snapshot feed thread panicked").expect("Snapshot feed thread failed");
    }
}




pub fn subscribe_multicast(
    source: MulticastSource,
    shutdown: Arc<AtomicBool>
) -> Result<std::thread::JoinHandle<Result<(), io::Error>>, io::Error> {

    let handle = std::thread::spawn(move || {
        let socket = match SourceSocket::create_multicast_receiver_socket(source.port) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to create socket for {}:{} - {}", source.ip, source.port, e);
                return Err(e);
            }
        };

        // Join multicast group
        let group_addr: std::net::Ipv4Addr = match source.ip.parse() {
            Ok(addr) => addr,
            Err(e) => {
                eprintln!("Invalid multicast IP address {}: {}", source.ip, e);
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "Invalid multicast IP"));
            }
        };
        socket.join_multicast_v4(&group_addr, &std::net::Ipv4Addr::UNSPECIFIED)?;

        // Set a read timeout to allow the thread to check for shutdown signals periodically
        let _ = socket.set_read_timeout(Some(Duration::from_millis(100)));

        tracing::info!("Subscribed to multicast group {}:{} (market: {})", source.ip, source.port, source.market);
        let mut buf = [0u8; 65536];

        while !shutdown.load(Ordering::Relaxed) {
            let (len, src) = socket.recv_from(&mut buf)?;
            tracing::debug!("Received {} bytes from {}", len, src);
            // TODO: parse and cache market data
        }

        tracing::info!("Shutting down multicast subscriber for {}:{}", source.ip, source.port);
        Ok(())
    });

    Ok(handle)
}