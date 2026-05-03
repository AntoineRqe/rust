use axum::{
    Router,
    routing::{get, post},
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::signal;
use tower_http::cors::{Any, CorsLayer};
use crate::state::EventBus;
use crate::order_book::OrderBookState;
use crate::players::PlayerStore;
use crate::fix_session::FIXSessionManager;
use crate::ws::ws_handler;
use crate::login::{root_handler, login_page_handler, app_handler, api_login_handler, api_markets_handler};
use crate::auth::{SessionInfo, MarketInfo};
use utils::market_name;

/// Everything axum handlers need — cheap to clone, Arc-backed internally.
#[derive(Clone)]
pub struct AppState {
    /// Event bus for publishing FIX messages and other server-side events to browsers.
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
    /// Number of currently connected browser websocket sessions.
    pub active_visitors: Arc<AtomicUsize>,
    /// Total number of websocket sessions ever opened (all-time).
    pub total_visitors: Arc<AtomicUsize>,
    /// Persistent FIX TCP session registry (one connection per player).
    pub fix_session_manager: FIXSessionManager,
}

/// Start the web server on the given port, serving the trading terminal and API endpoints. This will block the current thread until shutdown is requested.
/// Arguments:
/// - `bus`: Event bus for publishing server-side events to browsers.
/// - `fix_tcp_addr`: Address of the FIX TCP gateway for this market instance
/// - `grpc_addr`: gRPC address of the MarketControl service (e.g. "http://[::1]:50051") for loading initial pending orders at startup.
/// - `ip`: The IP address to advertise in the login page URL
/// - `port`: The port to listen on
/// - `database_url`: Postgres connection URL for player state persistence
/// - `known_markets`: List of all configured markets (name + web URL), sent to the login page
/// - `shutdown`: An atomic flag that can be set to request server shutdown from another thread
/// - `order_book`: Shared order book state to be populated at startup and kept in sync with market feed updates, so the terminal can display up-to-date pending orders and tokens.
pub fn run_web_server(
    bus: EventBus,
    fix_tcp_addr: String,
    grpc_addr: String,
    ip: &str,
    port: u16,
    database_url: String,
    known_markets: Vec<MarketInfo>,
    shutdown: Arc<AtomicBool>,
    order_book: Arc<Mutex<OrderBookState>>,
    core_id: usize,
) -> Result<(), Box<dyn std::error::Error>> {

    match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("web-tokio")
        .on_thread_start(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: core_id });
        })
        .build()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?
        .block_on(serve(bus, fix_tcp_addr, grpc_addr, ip, port, database_url, known_markets, shutdown, order_book)) {
            Ok(_) => (),
            Err(e) => {
                return Err(e);
            }
    }

    Ok(())
}

/// Main async function to set up and run the web server. Separated from `run_web_server` to allow using async/await syntax.
async fn serve(
    bus: EventBus,
    fix_tcp_addr: String,
    grpc_addr: String,
    ip: &str,
    port: u16,
    database_url: String,
    known_markets: Vec<MarketInfo>,
    shutdown: Arc<AtomicBool>,
    order_book: Arc<Mutex<OrderBookState>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Initialize player store and app state
    let player_store = PlayerStore::load_postgres(&database_url);
    let advertised = crate::auth::advertised_markets(&known_markets);
    if !known_markets.is_empty() {
        if advertised.is_empty() {
            tracing::warn!(
                "[{}] MARKET_SIM_PUBLIC_MARKETS_ONLY=1 filtered out all configured markets; /api/markets will be empty",
                market_name()
            );
        } else if advertised.len() != known_markets.len() {
            tracing::info!(
                "[{}] MARKET_SIM_PUBLIC_MARKETS_ONLY=1 filtered {} non-public market URL(s)",
                market_name(),
                known_markets.len().saturating_sub(advertised.len())
            );
        }
    }

    let initial_total_visitors = player_store.total_visitors() as usize;
    let state = AppState {
        bus,
        fix_tcp_addr,
        grpc_addr,
        player_store,
        known_markets,
        sessions: Arc::new(Mutex::new(HashMap::new())),
        order_book,
        active_visitors: Arc::new(AtomicUsize::new(0)),
        total_visitors: Arc::new(AtomicUsize::new(initial_total_visitors)),
        fix_session_manager: FIXSessionManager::new(),
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
        .route("/",            get(root_handler))
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
        .with_state(state.clone());

    // Bind the TCP listener before starting the server to fail fast if the port is unavailable.
    let addr: SocketAddr = format!("0.0.0.0:{}", port)
        .parse()
        .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], port)));
    // 
    let listener = match TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(e) => {
            return Err(Box::new(e));
        }
    };

    tracing::info!("[{}] Web terminal → http://{}:{}", market_name(), ip, port);
    match axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown_signal(shutdown))
        .await
    {
        Ok(_) => (),
        Err(e) => {
            return Err(Box::new(e));
        }
    }

    tracing::info!("[{}] Web server has shut down", market_name());
    state.fix_session_manager.shutdown_all();

    Ok(())
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

    // Each order from the dump is added to the order book state.
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