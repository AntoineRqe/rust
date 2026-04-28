use std::io;
use types::multicast::{MulticastSource, SourceSocket};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;
use tokio::sync::broadcast;
mod server;

pub struct MarketDataProxy {
    market_feed_source: MulticastSource,
    snapshot_feed_source: MulticastSource,
    shutdown: Arc<AtomicBool>,
    core_id: usize,
    ws_ip: String,
    ws_port: u16,
}

impl MarketDataProxy {
    /// Synchronous bridge for running the async server logic
    pub fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to build Tokio runtime");
        rt.block_on(self.run_async())
    }

    pub fn new(
        market_feed_source: MulticastSource,
        snapshot_feed_source: MulticastSource,
        shutdown: Arc<AtomicBool>,
        core_id: usize,
        ws_ip: String,
        ws_port: u16,
    ) -> Self {
        Self {
            market_feed_source,
            snapshot_feed_source,
            shutdown,
            core_id,
            ws_ip,
            ws_port,
        }
    }

    pub async fn run_async(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (market_tx, _) = broadcast::channel(100);
        let (snapshot_tx, _) = broadcast::channel(100);

        let market_feed_source = self.market_feed_source.clone();
        let snapshot_feed_source = self.snapshot_feed_source.clone();
        let shutdown_market = self.shutdown.clone();
        let shutdown_snapshot = self.shutdown.clone();
        let core_id = self.core_id;

        // Spawn multicast listeners that forward to broadcast channels, pinned to core_id
        let market_tx_clone = market_tx.clone();
        let snapshot_tx_clone = snapshot_tx.clone();

        let market_handle = tokio::task::spawn_blocking(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: core_id });
            listen_and_forward(market_feed_source, shutdown_market, market_tx_clone)
        });
        let snapshot_handle = tokio::task::spawn_blocking(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: core_id });
            listen_and_forward(snapshot_feed_source, shutdown_snapshot, snapshot_tx_clone)
        });

        // Start Axum server
        server::start_server(market_tx, snapshot_tx, self.ws_ip.clone(), self.ws_port).await;

        // Await multicast listener tasks and propagate errors
        let market_result = market_handle.await?;
        let snapshot_result = snapshot_handle.await?;
        market_result?;
        snapshot_result?;
        Ok(())
    }
}

fn listen_and_forward(
    source: MulticastSource,
    shutdown: Arc<AtomicBool>,
    tx: broadcast::Sender<Vec<u8>>
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {

    let socket = match SourceSocket::create_multicast_receiver_socket(source.port) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to create socket for {}:{} - {}", source.ip, source.port, e);
            return Err(Box::new(e));
        }
    };
    let group_addr: std::net::Ipv4Addr = match source.ip.parse() {
        Ok(addr) => addr,
        Err(e) => {
            tracing::error!("Invalid multicast IP address {}: {}", source.ip, e);
            return Err(Box::new(e));
        }
    };
    if let Err(e) = socket.join_multicast_v4(&group_addr, &std::net::Ipv4Addr::UNSPECIFIED) {
        tracing::error!("Failed to join multicast group {}: {} - {}", source.ip, source.port, e);
        return Err(Box::new(e));
    }
    let _ = socket.set_read_timeout(Some(Duration::from_millis(100)));

    let mut buf = [0u8; 65536];
    while !shutdown.load(Ordering::Relaxed) {
        match socket.recv_from(&mut buf) {
            Ok((len, _src)) => {
                let data = buf[..len].to_vec();
                let _ = tx.send(data);
            },
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => continue,
            Err(e) => {
                tracing::error!("Error receiving from multicast socket {}:{} - {}", source.ip, source.port, e);
                return Err(Box::new(e));
            }
        }
    }
    tracing::info!("Shutting down multicast subscriber for {}:{}", source.ip, source.port);
    Ok(())
}