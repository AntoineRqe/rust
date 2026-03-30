use axum::{
    Router,
    routing::get,
    response::{Html, IntoResponse},
    extract::State,
    http::{HeaderMap, StatusCode, header},
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use crate::state::EventBus;
use crate::players::PlayerStore;
use crate::ws::ws_handler;
use base64::{Engine as _, engine::general_purpose};

pub type FixSender = Arc<dyn Fn(Vec<u8>) + Send + Sync>;

/// Everything axum handlers need — cheap to clone, Arc-backed internally.
#[derive(Clone)]
pub struct AppState {
    pub bus: EventBus,
    /// Injects a raw FIX message into the engine, bypassing TCP.
    pub fix_sender: FixSender,
    /// Per-player state registry (tokens, pending orders, credentials).
    pub player_store: PlayerStore,
}

pub fn run_web_server(
    bus: EventBus,
    fix_sender: FixSender,
    ip: &str,
    port: u16,
    players_file: PathBuf,
) {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("web-tokio")
        .build()
        .expect("Failed to build tokio runtime")
        .block_on(serve(bus, fix_sender, ip, port, players_file))
}

async fn serve(
    bus: EventBus,
    fix_sender: FixSender,
    ip: &str,
    port: u16,
    players_file: PathBuf,
) {
    let player_store = PlayerStore::load(players_file);
    let state = AppState { bus, fix_sender, player_store };

    let app = Router::new()
        .route("/",   get(index_handler))
        .route("/ws", get(ws_handler))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr).await
        .unwrap_or_else(|e| panic!("Cannot bind to port {port}: {e}"));

    tracing::info!("Web terminal → http://{}:{}", ip, port);
    axum::serve(listener, app).await.expect("axum server error");
}

async fn index_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if authenticate(&headers, &state.player_store).is_none() {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Basic realm=\"Market Simulator\"")],
            "Authentication required",
        )
            .into_response();
    }

    Html(include_str!("../frontend/index.html")).into_response()
}

/// Extract `(username, password)` from an HTTP Basic-Auth header.
pub(crate) fn extract_credentials(headers: &HeaderMap) -> Option<(String, String)> {
    let auth = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let encoded = auth.strip_prefix("Basic ")?;
    let decoded = general_purpose::STANDARD.decode(encoded).ok()?;
    let user_pass = std::str::from_utf8(&decoded).ok()?.to_string();
    let (user, pass) = user_pass.split_once(':')?;
    Some((user.to_string(), pass.to_string()))
}

/// Authenticate (or auto-register) via the player store.
/// Returns `Some(username)` on success, `None` on missing / wrong credentials.
pub(crate) fn authenticate(headers: &HeaderMap, store: &PlayerStore) -> Option<String> {
    let (username, password) = extract_credentials(headers)?;
    store.authenticate_or_register(&username, &password).ok()
}