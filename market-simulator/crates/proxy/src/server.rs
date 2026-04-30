use axum::{extract::ws::{WebSocketUpgrade, WebSocket, Message}, routing::get, Router};
use axum::body::Bytes;
use axum::serve;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use std::net::SocketAddr;
use socket2::{Socket, Domain, Type};

pub async fn start_server(
    market_tx: broadcast::Sender<Vec<u8>>,
    snapshot_tx: broadcast::Sender<Vec<u8>>,
    ip: String,
    port: u16,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {

    let market_tx_clone = market_tx.clone();
    let snapshot_tx_clone = snapshot_tx.clone();
    let app = Router::new()
        .route("/ws/market", get(move |ws: WebSocketUpgrade| {
            let tx = market_tx_clone.clone();
            async move {
                tracing::info!("[{}] New WebSocket connection for market feed", utils::market_name());
                // Upgrade to WebSocket and handle the connection, on_upgrade spawns a new task for each connection
                ws.on_upgrade(move |socket| handle_ws(socket, tx))
            }
        }))
        .route("/ws/snapshot", get(move |ws: WebSocketUpgrade| {
            let tx = snapshot_tx_clone.clone();
            async move {
                tracing::info!("[{}] New WebSocket connection for snapshot feed", utils::market_name());
                // Upgrade to WebSocket and handle the connection, on_upgrade spawns a new task for each connection
                ws.on_upgrade(move |socket| handle_ws(socket, tx))
            }
        }))
        .route("/", get(|| async { "hello" }));


    // Bind to the specified IP and port using socket2 for better control over socket options
    // reuse_address is important for allowing quick restarts of the server without waiting for TIME_WAIT sockets to expire
    // non-blocking is required for integration with Tokio's async runtime
    let addr: SocketAddr = format!("{}:{}", ip, port).parse()?;
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, None)?;
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    let listener = TcpListener::from_std(socket.into())?;

    // Create a shutdown future that resolves when shutdown is set to true
    let shutdown_signal = async move {
        use tokio::time::{sleep, Duration};
        while !shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            sleep(Duration::from_millis(100)).await;
        }
        tracing::info!("[{}] Axum server shutdown signal received", utils::market_name());
    };

    serve(listener, app)
        .with_graceful_shutdown(shutdown_signal)
        .await?;

    tracing::info!("[{}] WebSocket server on {} has shut down", utils::market_name(), addr);

    Ok(())
}

async fn handle_ws(mut socket: WebSocket, tx: broadcast::Sender<Vec<u8>>) {
    let mut rx = tx.subscribe();
    // Listen for messages from the broadcast channel and forward them to the WebSocket client
    while let Ok(msg) = rx.recv().await {
        let bytes = Bytes::from(msg);
        tracing::debug!("[{}] Sending {} bytes to WebSocket client", utils::market_name(), bytes.len());
        if socket.send(Message::Binary(bytes)).await.is_err() {
            tracing::warn!("[{}] WebSocket client disconnected", utils::market_name());
            break;
        }
    }
}
