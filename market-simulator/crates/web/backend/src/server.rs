use crate::fix_session::FIXSessionManager;
use crate::login::{
    api_login_handler,
};
use crate::metrics::{create_metrics_registry, metrics_handler};
use crate::order_book::{OrderBookState, stable_order_id_from_cl_ord_id};
use crate::player_client::PlayerClient;
use crate::state::EventBus;
use crate::state::TradeView;
use crate::ws::ws_handler;
use axum::{
    Router,
    routing::{get, post},
    Extension,
};
use db;
use fix::engine::FixRawMsg;
use std::collections::VecDeque;
use std::error::Error;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::signal;
use tower_http::cors::{Any, CorsLayer};
use types::consts::RB_SIZE;
use utils::market_name;

fn format_error_chain(err: &(dyn Error + 'static)) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(next) = source {
        parts.push(next.to_string());
        source = next.source();
    }
    parts.join(" | caused by: ")
}

pub type Metrics = types::MarketMetrics;

/// Everything axum handlers need — cheap to clone, Arc-backed internally.
#[derive(Clone)]
pub struct AppState {
    /// Event bus for publishing FIX messages and other server-side events to browsers.
    pub bus: EventBus,
    /// gRPC address of the MarketControl service (e.g. "http://[::1]:50051").
    pub grpc_addr: String,
    /// gRPC client for the player service (running on separate port, e.g. 50052).
    pub player_client: Arc<tokio::sync::Mutex<PlayerClient>>,
    /// Current order book state for all symbols.
    pub order_book: Arc<Mutex<OrderBookState>>,
    /// Number of currently connected browser websocket sessions.
    pub active_visitors: Arc<AtomicUsize>,
    /// Total number of websocket sessions ever opened (all-time).
    pub total_visitors: Arc<AtomicUsize>,
    /// Persistent FIX TCP session registry (one connection per player).
    pub fix_session_manager: FIXSessionManager,
    /// Recent trades for price chart initialization.
    pub trades_queue: Arc<Mutex<VecDeque<TradeView>>>,
}

/// Start the web server on the given port, serving the trading terminal and API endpoints. This will block the current thread until shutdown is requested.
/// Arguments:
/// - `bus`: Event bus for publishing server-side events to browsers.
/// - `fix_tx`: In-process ingress queue for FIX engine.
/// - `grpc_addr`: gRPC address of the MarketControl service (e.g. "http://[::1]:50051") for loading initial pending orders at startup.
/// - `ip`: The IP address to advertise in the login page URL
/// - `port`: The port to listen on
/// - `database_url`: Postgres connection URL for player state persistence
/// - `shutdown`: An atomic flag that can be set to request server shutdown from another thread
/// - `order_book`: Shared order book state populated at startup and updated by execution/report processing.
pub fn run_web_server(
    bus: EventBus,
    fix_tx: Arc<crossbeam_channel::Sender<FixRawMsg<RB_SIZE>>>,
    metrics: Arc<Metrics>,
    grpc_addr: String,
    ip: &str,
    port: u16,
    market_database_url: String,
    shutdown: Arc<AtomicBool>,
    order_book: Arc<Mutex<OrderBookState>>,
    player_service_addr: String,
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
        .block_on(serve(
            bus,
            fix_tx,
            metrics,
            grpc_addr,
            ip,
            port,
            market_database_url,
            shutdown,
            order_book,
            player_service_addr,
        )) {
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
    fix_tx: Arc<crossbeam_channel::Sender<FixRawMsg<RB_SIZE>>>,
    metrics: Arc<Metrics>,
    grpc_addr: String,
    ip: &str,
    port: u16,
    market_database_url: String,
    shutdown: Arc<AtomicBool>,
    order_book: Arc<Mutex<OrderBookState>>,
    player_service_addr: String,
) -> Result<(), Box<dyn std::error::Error>> {
    // Initialize player client connecting to the player-server gRPC service
    // The PlayerClient uses tonic::transport::Channel which automatically:
    // - Maintains HTTP/2 connection pooling
    // - Implements keepalive (15s interval, 5s timeout)
    // - Reuses connections across all requests
    // - Handles reconnection on failure
    let player_client = Arc::new(tokio::sync::Mutex::new(
        PlayerClient::connect(&player_service_addr).await.map_err(|err| {
            let detail = format_error_chain(err.as_ref());
            Box::new(std::io::Error::other(format!(
                "failed to connect to player service at '{}': {}",
                player_service_addr, detail
            ))) as Box<dyn std::error::Error>
        })?,
    ));
    {
        let mut client = player_client.lock().await;
        client.set_metrics(Arc::clone(&metrics));
    }
    tracing::info!(
        "[{}] Connected to player service at {}",
        market_name(),
        player_service_addr
    );

    // Load trades for price chart initialization
    let trades_queue = Arc::new(Mutex::new(VecDeque::new()));
    match db::connect(&market_database_url).await {
        Ok(pool) => match db::collect_last_n_trades(&pool, 10).await {
            Ok(trades) => {
                let mut queue = trades_queue.lock().unwrap();
                for trade in trades {
                    queue.push_back(TradeView {
                        id: trade.id,
                        price: trade.price.to_f64(),
                        quantity: trade.quantity.to_f64(),
                        cl_ord_id: trade.cl_ord_id.to_string(),
                    });
                }
                tracing::info!(
                    "[{}] Loaded {} trades from database for price chart",
                    market_name(),
                    queue.len()
                );
            }
            Err(e) => {
                tracing::warn!(
                    "[{}] Failed to query trades from database: {}",
                    market_name(),
                    e
                );
            }
        },
        Err(e) => {
            tracing::error!(
                "[{}] Failed to connect to database for trades: {}",
                market_name(),
                e
            );
        }
    }

    let state = AppState {
        bus,
        grpc_addr,
        player_client: player_client,
        order_book,
        active_visitors: Arc::new(AtomicUsize::new(0)),
        total_visitors: Arc::new(AtomicUsize::new(0)),
        fix_session_manager: FIXSessionManager::new(fix_tx),
        trades_queue,
    };

    // Initialize Prometheus metrics registry
    let metrics_registry = Arc::new(create_metrics_registry());
    tracing::info!(
        "[{}] Prometheus metrics enabled at http://{}:{}/metrics",
        market_name(),
        ip,
        port
    );

    // Load initial pending orders from gRPC at startup
    {
        let grpc_addr_clone = state.grpc_addr.clone();
        let order_book_clone = Arc::clone(&state.order_book);

        match load_initial_order_book(&grpc_addr_clone, order_book_clone).await {
            Ok(count) => {
                tracing::info!(
                    "[{}] Loaded {} pending orders from gRPC at startup",
                    market_name(),
                    count
                );
            }
            Err(e) => {
                tracing::warn!(
                    "[{}] Failed to load pending orders at startup: {}",
                    market_name(),
                    e
                );
            }
        }
    }

    let app = Router::new()
        .route("/api/login", post(api_login_handler))
        .route("/api/trades", get(crate::login::api_trades_handler))
        .route("/ws", get(ws_handler))
        .route("/metrics", get(metrics_handler))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(Extension(metrics))
        .layer(Extension(metrics_registry))
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
    match axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
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
/// Populates the order book state with all pending orders.
async fn load_initial_order_book(
    grpc_addr: &str,
    order_book: Arc<Mutex<OrderBookState>>,
) -> Result<usize, String> {
    use grpc::proto::DumpOrderBookRequest;
    use grpc::proto::market_control_client::MarketControlClient;

    tracing::info!(
        "[{}] Loading initial pending orders from gRPC at {}",
        market_name(),
        grpc_addr
    );

    let mut client = MarketControlClient::connect(grpc_addr.to_string())
        .await
        .map_err(|e| format!("Failed to connect to gRPC: {}", e))?;

    let response = client
        .dump_order_book(DumpOrderBookRequest {})
        .await
        .map_err(|e| format!("DumpOrderBook RPC failed: {}", e))?;

    let dump_response = response.into_inner();

    tracing::debug!(
        "[{}] DumpOrderBook returned: success={}, message={}, order_count={}",
        market_name(),
        dump_response.success,
        dump_response.message,
        dump_response.orders.len()
    );

    if !dump_response.success {
        return Err(format!(
            "DumpOrderBook returned success=false: {}",
            dump_response.message
        ));
    }

    let mut book_state = order_book.lock().unwrap();
    let mut count = 0u64;
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

        tracing::trace!(
            "[{}] Loading order: {} {} @ {} x {}",
            market_name(),
            symbol,
            if side == 1 { "BID" } else { "ASK" },
            pending_order.price,
            pending_order.quantity
        );

        book.add_or_update_order(
            order_id,
            side,
            pending_order.price,
            pending_order.quantity,
            Some(pending_order.cl_ord_id.clone()),
            timestamp_ms,
        );

        count += 1;
    }

    drop(book_state);

    tracing::info!(
        "[{}] Successfully loaded {} pending orders into order book state",
        market_name(),
        count
    );

    Ok(count as usize)
}
