use axum::{extract::ws::{WebSocketUpgrade, WebSocket, Message}, routing::get, Router};
use axum::body::Bytes;
use axum::serve;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

pub async fn start_server(
    market_tx: broadcast::Sender<Vec<u8>>,
    snapshot_tx: broadcast::Sender<Vec<u8>>,
    ip: String,
    port: u16,
) {
    let market_tx_clone = market_tx.clone();
    let snapshot_tx_clone = snapshot_tx.clone();
    let app = Router::new()
        .route("/ws/market", get(move |ws: WebSocketUpgrade| {
            let tx = market_tx_clone.clone();
            async move {
                ws.on_upgrade(move |socket| handle_ws(socket, tx))
            }
        }))
        .route("/ws/snapshot", get(move |ws: WebSocketUpgrade| {
            let tx = snapshot_tx_clone.clone();
            async move {
                ws.on_upgrade(move |socket| handle_ws(socket, tx))
            }
        }));

    let addr = format!("{}:{}", ip, port);
    tracing::info!("WebSocket server listening on {}", addr);
    let listener = TcpListener::bind(&addr).await.unwrap();
    serve(listener, app).await.unwrap();
}

async fn handle_ws(mut socket: WebSocket, tx: broadcast::Sender<Vec<u8>>) {
    let mut rx = tx.subscribe();
    while let Ok(msg) = rx.recv().await {
        let bytes = Bytes::from(msg);
        if socket.send(Message::Binary(bytes)).await.is_err() {
            break;
        }
    }
}
