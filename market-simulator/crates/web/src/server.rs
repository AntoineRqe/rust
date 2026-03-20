use axum::{
    Router,
    routing::get,
    response::{Html, IntoResponse},
    extract::State,
    http::{HeaderMap, StatusCode, header},
};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use crate::state::EventBus;
use crate::ws::ws_handler;
use base64::{Engine as _, engine::general_purpose};

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
    pub web_password: Option<String>,
}

pub fn run_web_server(bus: EventBus, fix_sender: FixSender, port: u16, web_password: Option<String>) {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("web-tokio")
        .build()
        .expect("Failed to build tokio runtime")
        .block_on(serve(bus, fix_sender, port, web_password))
}

async fn serve(bus: EventBus, fix_sender: FixSender, port: u16, web_password: Option<String>) {
    let state = AppState { bus, fix_sender, web_password };

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

async fn index_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !is_authorized(&headers, state.web_password.as_deref()) {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Basic realm=\"FIX Web Terminal\"")],
            "Authentication required",
        )
            .into_response();
    }

    Html(include_str!("../frontend/index.html")).into_response()
}

pub(crate) fn is_authorized(headers: &HeaderMap, expected_password: Option<&str>) -> bool {
    let Some(expected) = expected_password else {
        return true;
    };

    let Some(auth_header) = headers.get(header::AUTHORIZATION) else {
        return false;
    };
    let Ok(auth_str) = auth_header.to_str() else {
        return false;
    };

    let Some(encoded) = auth_str.strip_prefix("Basic ") else {
        return false;
    };
    let Ok(decoded) = general_purpose::STANDARD.decode(encoded) else {
        return false;
    };
    let Ok(user_pass) = std::str::from_utf8(&decoded) else {
        return false;
    };
    let Some((_, password)) = user_pass.split_once(':') else {
        return false;
    };

    password == expected
}