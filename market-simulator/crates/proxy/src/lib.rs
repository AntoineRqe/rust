use std::io;
use types::multicast::{MulticastSource, SourceSocket};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use tokio::sync::broadcast;
mod server;

pub struct MarketDataProxy {
    market_feed_source: Option<MulticastSource>,
    snapshot_feed_source: Option<MulticastSource>,
    shutdown: Arc<AtomicBool>,
    core_id: usize,
    ws_ip: String,
    ws_port: u16,
}

impl MarketDataProxy {
    /// Synchronous bridge for running the async server logic
    pub fn run(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Build a Tokio runtime and block on the async server logic
        // - enable_all() is required for using async features like timers and channels
        // - new_multi_thread() allows the runtime to use multiple threads for blocking tasks, which is important for our multicast listeners
        // - No worker thread counter is set, so it defaults to the number of CPU cores, which should be sufficient for our use case
        // TODO: Consider fine-tuning worker thread count and pining them to specific cores for better performance and isolation.
        let core_id = self.core_id;
        
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .thread_name("market-data-proxy-runtime")
            .worker_threads(3)
            .on_thread_start(move || {
                core_affinity::set_for_current(core_affinity::CoreId { id: core_id });
            })
            .enable_all()
            .build() {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!("Failed to build Tokio runtime: {}", e);
                    return Err(Box::new(e));
                }
            };
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
            market_feed_source: Some(market_feed_source),
            snapshot_feed_source: Some(snapshot_feed_source),
            shutdown,
            core_id,
            ws_ip,
            ws_port,
        }
    }

    pub async fn run_async(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (market_tx, _) = broadcast::channel(100);
        let (snapshot_tx, _) = broadcast::channel(100);

        let shutdown_market = self.shutdown.clone();
        let shutdown_snapshot = self.shutdown.clone();

        // Spawn multicast listeners that forward to broadcast channels
        let market_tx_clone = market_tx.clone();
        let snapshot_tx_clone = snapshot_tx.clone();

        // As listen_and_forward is a blocking function that runs an infinite loop
        // we use spawn_blocking to run it on a separate thread without blocking the async runtime.
        // This allows us to run the Axum server concurrently with the multicast listeners.
        let market_source = self.market_feed_source.take().expect("Market feed source must be provided");
        let market_handle = tokio::task::spawn(async move {
            listen_and_forward(market_source, shutdown_market, market_tx_clone).await
        });

        let snapshot_source = self.snapshot_feed_source.take().expect("Snapshot feed source must be provided");
        let snapshot_handle = tokio::task::spawn(async move {
            listen_and_forward(snapshot_source, shutdown_snapshot, snapshot_tx_clone).await
        });

        // Start Axum server with graceful shutdown
        server::start_server(
            market_tx,
            snapshot_tx,
            self.ws_ip.clone(),
            self.ws_port,
            self.shutdown.clone(),
        ).await?;

        // Await multicast listener tasks and propagate errors
        match market_handle.await {
            Ok(Ok(())) => tracing::info!("Market feed listener exited successfully"),
            Ok(Err(e)) => {
                tracing::error!("Market feed listener error: {}", e);
                return Err(e);
            },
            Err(e) => {
                tracing::error!("Market feed listener task panicked: {}", e);
                return Err(Box::new(e));
            },
        }
        match snapshot_handle.await {
            Ok(Ok(())) => tracing::info!("Snapshot feed listener exited successfully"),
            Ok(Err(e)) => {
                tracing::error!("Snapshot feed listener error: {}", e);
                return Err(e);
            },
            Err(e) => {
                tracing::error!("Snapshot feed listener task panicked: {}", e);
                return Err(Box::new(e));
            },
        }

        Ok(())
    }
}

async fn listen_and_forward(
    source: MulticastSource,
    shutdown: Arc<AtomicBool>,
    tx: broadcast::Sender<Vec<u8>>
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {

    let socket = match SourceSocket::create_multicast_receiver_socket_async(source.port).await {
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
            return Err(Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("Invalid multicast IP address {}: {}", source.ip, e))));
        }
    };

    if let Err(e) = socket.join_multicast_v4(group_addr, std::net::Ipv4Addr::UNSPECIFIED) {
        tracing::error!("Failed to join multicast group {}: {}", source.ip, e);
        return Err(Box::new(std::io::Error::new(std::io::ErrorKind::Other, format!("Failed to join multicast group: {e}"))));
    }

    let mut buf = [0u8; 65536];

    while !shutdown.load(Ordering::Relaxed) {
        use tokio::time::{sleep, Duration};
        tokio::select! {
            recv_result = socket.recv_from(&mut buf) => {
                match recv_result {
                    Ok((len, _src)) => {
                        tracing::debug!("[{}] Received {} bytes from multicast {}:{}", utils::market_name(), len, source.ip, source.port);
                        let data = buf[..len].to_vec();
                        let _ = tx.send(data);
                    },
                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {},
                    Err(e) => {
                        tracing::error!("[{}] Error receiving from multicast socket {}:{} - {}", utils::market_name(), source.ip, source.port, e);
                        return Err(Box::new(e));
                    }
                }
            }
            _ = sleep(Duration::from_millis(100)) => {
                // Timeout elapsed, just loop again to check shutdown
            }
        }
    }

    tracing::info!("[{}] Shutting down multicast subscriber for {}:{}", utils::market_name(), source.ip, source.port);

    Ok(())
}