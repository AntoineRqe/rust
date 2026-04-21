use axum::{
    Router,
    routing::{get, post},
    response::{Html, IntoResponse},
    extract::State,
    http::StatusCode,
    Json,
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::signal;
use serde::{Deserialize, Serialize};
use rand_core::RngCore;
use tower_http::cors::{Any, CorsLayer};
use crate::state::{EventBus, OrderBookState};
use crate::players::PlayerStore;
use crate::ws::ws_handler;

fn market_name() -> &'static str {
    static MARKET_NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    MARKET_NAME
        .get_or_init(|| std::env::var("MARKET_NAME").unwrap_or_else(|_| "unknown".to_string()))
        .as_str()
}

/// Information about a single market, served to the login page.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarketInfo {
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub username: String,
    pub is_admin: bool,
}

/// Everything axum handlers need — cheap to clone, Arc-backed internally.
#[derive(Clone)]
pub struct AppState {
    pub bus: EventBus,
    /// Address of the FIX TCP gateway for this market instance.
    pub fix_tcp_addr: String,
    /// gRPC address of the MarketControl service (e.g. "http://[::1]:50051").
    pub grpc_addr: String,
    /// Per-player state registry (tokens, pending orders, credentials).
    pub player_store: PlayerStore,
    /// All configured markets (name + web URL), sent to the login page.
    pub known_markets: Vec<MarketInfo>,
    /// Active sessions: token → session metadata.
    pub sessions: Arc<Mutex<HashMap<String, SessionInfo>>>,
    /// Current order book state for all symbols (updated with market feed data).
    pub order_book: Arc<Mutex<OrderBookState>>,
}

/// Generate a cryptographically random 128-bit hex token.
pub(crate) fn generate_token() -> String {
    let mut bytes = [0u8; 16];
    rand_core::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Look up a session token and return the associated session metadata.
pub(crate) fn authenticate_token(state: &AppState, token: &str) -> Option<SessionInfo> {
    state.sessions.lock().unwrap().get(token).cloned()
}

fn admin_password() -> Option<String> {
    std::env::var("MARKET_SIMULATOR_ADMIN_PWD")
        .ok()
        .filter(|value| !value.trim().is_empty())
}

pub fn run_web_server(
    bus: EventBus,
    fix_tcp_addr: String,
    grpc_addr: String,
    ip: &str,
    port: u16,
    players_file: PathBuf,
    known_markets: Vec<MarketInfo>,
    shutdown: Arc<AtomicBool>,
    order_book: Arc<Mutex<OrderBookState>>,
) {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("web-tokio")
        .build()
        .expect("Failed to build tokio runtime")
        .block_on(serve(bus, fix_tcp_addr, grpc_addr, ip, port, players_file, known_markets, shutdown, order_book))
}

async fn serve(
    bus: EventBus,
    fix_tcp_addr: String,
    grpc_addr: String,
    ip: &str,
    port: u16,
    players_file: PathBuf,
    known_markets: Vec<MarketInfo>,
    shutdown: Arc<AtomicBool>,
    order_book: Arc<Mutex<OrderBookState>>,
) {
    let player_store = PlayerStore::load(players_file);
    let state = AppState {
        bus,
        fix_tcp_addr,
        grpc_addr,
        player_store,
        known_markets,
        sessions: Arc::new(Mutex::new(HashMap::new())),
        order_book,
    };

    // Load initial pending orders from gRPC at startup
    {
        let grpc_addr_clone = state.grpc_addr.clone();
        let order_book_clone = Arc::clone(&state.order_book);
        let player_store_clone = state.player_store.clone();
        
        match load_initial_order_book(&grpc_addr_clone, order_book_clone, &player_store_clone).await {
            Ok(count) => {
                tracing::info!("[{}] Loaded {} pending orders from gRPC at startup", market_name(), count);
            }
            Err(e) => {
                tracing::warn!("[{}] Failed to load pending orders at startup: {}", market_name(), e);
            }
        }
    }

    let app = Router::new()
        .route("/",            get(app_handler))
        .route("/login",       get(login_page_handler))
        .route("/app",         get(app_handler))
        .route("/api/login",   post(api_login_handler))
        .route("/api/markets", get(api_markets_handler))
        .route("/ws",          get(ws_handler))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state);

    let addr: SocketAddr = format!("0.0.0.0:{}", port)
        .parse()
        .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], port)));
    let listener = TcpListener::bind(addr).await
        .unwrap_or_else(|e| panic!("Cannot bind to port {port}: {e}"));

    tracing::info!("[{}] Web terminal → http://{}:{}", market_name(), ip, port);
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown_signal(shutdown))
        .await
        .expect("axum server error");

    tracing::info!("[{}] Web server has shut down", market_name());
}

async fn shutdown_signal(shutdown: Arc<AtomicBool>) {
    tokio::select! {
        _ = signal::ctrl_c() => {
            tracing::info!("[{}] Ctrl+C received, shutting down web server", market_name());
        }
        _ = async {
            while !shutdown.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        } => {
            tracing::info!("[{}] Stop request received, shutting down web server", market_name());
        }
    }
}

/// Serve the login page (always accessible, no auth required).
async fn login_page_handler() -> impl IntoResponse {
    Html(include_str!("../frontend/login.html"))
}

/// Serve the trading terminal. Auth is enforced client-side via sessionStorage
/// token; the WebSocket upgrade enforces it server-side.
async fn app_handler(State(state): State<AppState>) -> impl IntoResponse {
    let market = market_name();
    let login_gateway_url = std::env::var("LOGIN_GATEWAY_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:9875".to_string());
    let markets_json = serde_json::to_string(&state.known_markets)
        .unwrap_or_else(|_| "[]".to_string());
    let html = include_str!("../frontend/index.html")
        .replace("{{MARKET_NAME}}", market)
        .replace("{{LOGIN_GATEWAY_URL}}", &login_gateway_url)
        .replace("{{CURRENT_MARKET_NAME}}", market)
        .replace("{{MARKETS_JSON}}", &markets_json);
    Html(html)
}

/// POST /api/login — body: `{ "username": "...", "password": "..." }`
/// Returns 200 `{ token, username }` on success, 401 `{ error }` on failure.
#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

async fn api_login_handler(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> impl IntoResponse {
    let admin_login = body.username.eq_ignore_ascii_case("admin")
        && admin_password().as_deref() == Some(body.password.as_str());

    if admin_login {
        let token = generate_token();
        state.sessions.lock().unwrap().insert(
            token.clone(),
            SessionInfo {
                username: "admin".to_string(),
                is_admin: true,
            },
        );
        tracing::info!("[{}] Admin session created", market_name());
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "token": token,
                "username": "admin",
                "is_admin": true
            })),
        )
            .into_response();
    }

    match state.player_store.authenticate_or_register(&body.username, &body.password) {
        Ok(username) => {
            let token = generate_token();
            state.sessions.lock().unwrap().insert(
                token.clone(),
                SessionInfo {
                    username: username.clone(),
                    is_admin: false,
                },
            );
            tracing::info!("[{}] Session created for '{username}'", market_name());
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "token": token,
                    "username": username,
                    "is_admin": false
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": e.message(),
                "code": e.code(),
            })),
        )
            .into_response(),
    }
}

/// GET /api/markets — returns the list of all configured markets.
async fn api_markets_handler(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.known_markets.clone())
}

/// Load initial pending orders from gRPC DumpOrderBook RPC at startup.
/// Populates the order book state with all pending orders, which will then be
/// kept in sync by market feed updates.
async fn load_initial_order_book(
    grpc_addr: &str,
    order_book: Arc<Mutex<OrderBookState>>,
    player_store: &PlayerStore,
) -> Result<usize, String> {
    use grpc::proto::market_control_client::MarketControlClient;
    use grpc::proto::DumpOrderBookRequest;

    tracing::info!("[{}] Loading initial pending orders from gRPC at {}", market_name(), grpc_addr);

    let mut client = MarketControlClient::connect(grpc_addr.to_string())
        .await
        .map_err(|e| format!("Failed to connect to gRPC: {}", e))?;

    let response = client
        .dump_order_book(DumpOrderBookRequest {})
        .await
        .map_err(|e| format!("DumpOrderBook RPC failed: {}", e))?;

    let dump_response = response.into_inner();

    tracing::debug!("[{}] DumpOrderBook returned: success={}, message={}, order_count={}", 
        market_name(), dump_response.success, dump_response.message, dump_response.orders.len());

    if !dump_response.success {
        return Err(format!("DumpOrderBook returned success=false: {}", dump_response.message));
    }

    let mut book_state = order_book.lock().unwrap();
    let mut count = 0u64;
    let mut owner_hydration_entries: Vec<(String, String)> = Vec::new();
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    for pending_order in dump_response.orders {
        let symbol = pending_order.symbol.clone();
        let book = book_state.get_or_create(&symbol);

        let side = match pending_order.side.to_ascii_lowercase().as_str() {
            "1" | "buy" => 1,
            "2" | "sell" => 2,
            other => {
                tracing::warn!(
                    "[{}] Ignoring pending order with unknown side '{}' for symbol {}",
                    market_name(),
                    other,
                    symbol
                );
                continue;
            }
        };

        // Use the same 20-byte stable hash as market-feed Add/Delete events,
        // so follow-up delete updates match this bootstrapped entry exactly.
        let order_id = stable_order_id_from_cl_ord_id(&pending_order.cl_ord_id);

        tracing::trace!("[{}] Loading order: {} {} @ {} x {}", 
            market_name(), symbol, 
            if side == 1 { "BID" } else { "ASK" },
            pending_order.price,
            pending_order.quantity);

        book.add_or_update_order(
            order_id,
            side,
            pending_order.price,
            pending_order.quantity,
            Some(pending_order.cl_ord_id.clone()),
            timestamp_ms,
        );

        owner_hydration_entries.push((
            pending_order.cl_ord_id.clone(),
            pending_order.sender_id.clone(),
        ));

        count += 1;
    }

    drop(book_state);

    let hydrated = player_store.hydrate_order_owners_from_sender_ids(&owner_hydration_entries);

    tracing::info!(
        "[{}] Successfully loaded {} pending orders into order book state ({} owner mapping(s) hydrated)",
        market_name(),
        count,
        hydrated
    );

    Ok(count as usize)
}

fn stable_order_id_from_cl_ord_id(cl_ord_id: &str) -> u64 {
    let mut fixed = [0u8; 20];
    let bytes = cl_ord_id.as_bytes();
    let len = bytes.len().min(20);
    fixed[..len].copy_from_slice(&bytes[..len]);

    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in &fixed {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}