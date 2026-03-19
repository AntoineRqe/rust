use axum::{Router, routing::get, response::Html};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use crate::state::EventBus;
use crate::ws::ws_handler;

// N must be known at compile time — use a type alias in your types crate
// or pass it as a const generic. We use a trait object here to avoid
// leaking N into the web crate.
pub type FixSender = Arc<dyn Fn(Vec<u8>) + Send + Sync>;

/// Everything axum handlers need — cheap to clone, Arc-backed internally.
#[derive(Clone)]
pub struct AppState {
    pub bus: EventBus,
    /// Injects a raw FIX message into the engine, bypassing TCP.
    /// The closure captures net_to_fix_tx from main.rs.
    pub fix_sender: FixSender,
}

pub fn run_web_server(bus: EventBus, fix_sender: FixSender, port: u16) {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("web-tokio")
        .build()
        .expect("Failed to build tokio runtime")
        .block_on(serve(bus, fix_sender, port))
}

async fn serve(bus: EventBus, fix_sender: FixSender, port: u16) {
    let state = AppState { bus, fix_sender };

    let app = Router::new()
        .route("/",   get(index_handler))
        .route("/ws", get(ws_handler))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr).await
        .unwrap_or_else(|e| panic!("Cannot bind to port {port}: {e}"));

    tracing::info!("Web terminal → http://localhost:{port}");
    axum::serve(listener, app).await.expect("axum server error");
}

async fn index_handler() -> Html<&'static str> {
    Html(include_str!("../frontend/index.html"))
}