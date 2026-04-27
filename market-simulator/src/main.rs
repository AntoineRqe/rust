use snapshot::types::Snapshot;
use types::{OrderEvent, OrderResult};
use types::macros::{EntityId};
use clap::Parser;
use axum::{
    Router,
    routing::{get, post},
    extract::{Query, State},
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::{Html, IntoResponse},
    http::{HeaderMap, StatusCode},
    Json,
};
use order_book::book::{OrderBook};
use order_book::engine::OrderBookEngine;
use order_book::snapshot::SnapshotGenerationEngine;
use order_book::OrderBookControl;
use server::tcp::{
    FixServer,
};
use server::multicast::{spawn_market_feed_receiver};
use types::multicast::MulticastSource;
use utils::market_name;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use crossbeam::{channel};
use fix::engine::{FixEngine, FixRawMsg};
use memory;
use execution_report::{ExecutionReportEngine};
use std::net::TcpListener;
use web::state::{
    EventBus, OrderBookState,
};
use web::server::run_web_server;
use config::{MarketConfig, MarketsConfig};
use market_feed::engine::MarketDataFeedEngine;
use arc_swap::ArcSwap;
use serde::Deserialize;
use std::time::Duration;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite;

const RB_SIZE: usize = 1024;

#[derive(Parser, Debug)]
#[command(name = "market-simulator")]
struct Cli {
    #[arg(short = 'c', long = "config", default_value = "crates/config/default.json")]
    config_file: String,
    /// Internal: index of the market to run (used by child processes).
    #[arg(long = "market-index")]
    market_index: Option<usize>,
}

#[derive(Clone)]
struct LoginGatewayState {
    markets: Vec<web::MarketInfo>,
}

fn run_login_gateway(markets: Vec<web::MarketInfo>, ip: &str, port: u16) {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("login-gateway")
        .build()
        .expect("Failed to build tokio runtime for login gateway")
        .block_on(async move {
            let advertised = web::server::advertised_markets(&markets);
            if !markets.is_empty() {
                if advertised.is_empty() {
                    tracing::warn!(
                        "[gateway] MARKET_SIM_PUBLIC_MARKETS_ONLY=1 filtered out all configured markets; /api/markets will be empty"
                    );
                } else if advertised.len() != markets.len() {
                    tracing::info!(
                        "[gateway] MARKET_SIM_PUBLIC_MARKETS_ONLY=1 filtered {} non-public market URL(s)",
                        markets.len().saturating_sub(advertised.len())
                    );
                }
            }

            let state = LoginGatewayState { markets };
            let app = Router::new()
                .route("/", get(gateway_login_page_handler))
                .route("/app", get(gateway_app_handler))
                .route("/api/markets", get(gateway_markets_handler))
                .route("/api/login", post(gateway_login_handler))
                .route("/ws", get(gateway_ws_handler))
                .with_state(state);

            let addr: SocketAddr = format!("0.0.0.0:{port}")
                .parse()
                .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], port)));

            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .unwrap_or_else(|e| panic!("Cannot bind login gateway to port {port}: {e}"));

            tracing::info!("[gateway] Login page → http://{}:{}", ip, port);

            axum::serve(listener, app)
                .await
                .expect("login gateway server error");
        });
}

async fn gateway_login_page_handler() -> Html<&'static str> {
    Html(include_str!("../crates/web/frontend/login.html"))
}

fn gateway_public_base_url(headers: &HeaderMap) -> String {
    let host = forwarded_header(headers, "x-forwarded-host")
        .or_else(|| forwarded_header(headers, "host"))
        .unwrap_or_else(|| "127.0.0.1:9875".to_string());

    let proto = forwarded_header(headers, "x-forwarded-proto")
        .map(|p| p.to_ascii_lowercase())
        .filter(|p| p == "http" || p == "https")
        .unwrap_or_else(|| "http".to_string());

    format!("{}://{}", proto, host)
}

async fn gateway_app_handler(
    State(state): State<LoginGatewayState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let advertised = web::server::advertised_markets(&state.markets);
    if advertised.is_empty() {
        return Html("No markets available.").into_response();
    }

    let markets_for_client: Vec<web::MarketInfo> = advertised
        .iter()
        .map(|market| web::MarketInfo {
            name: market.name.clone(),
            url: adapt_market_url_for_client(&market.url, &headers),
        })
        .collect();

    let current_market_name = markets_for_client
        .first()
        .map(|market| market.name.as_str())
        .unwrap_or("unknown");
    let login_gateway_url = gateway_public_base_url(&headers);
    let markets_json = serde_json::to_string(&markets_for_client)
        .unwrap_or_else(|_| "[]".to_string());

    let html = include_str!("../crates/web/frontend/index.html")
        .replace("{{MARKET_NAME}}", "gateway")
        .replace("{{LOGIN_GATEWAY_URL}}", &login_gateway_url)
        .replace("{{CURRENT_MARKET_NAME}}", current_market_name)
        .replace("{{MARKETS_JSON}}", &markets_json);

    Html(html).into_response()
}

async fn gateway_markets_handler(State(state): State<LoginGatewayState>) -> Json<Vec<web::MarketInfo>> {
    Json(web::server::advertised_markets(&state.markets))
}

#[derive(Deserialize)]
struct GatewayWsQuery {
    token: String,
    market: Option<String>,
}

fn market_ws_url(base_url: &str, token: &str) -> String {
    let endpoint = format!("{}/ws?token={}", base_url.trim_end_matches('/'), token);
    if endpoint.starts_with("https://") {
        return endpoint.replacen("https://", "wss://", 1);
    }
    if endpoint.starts_with("http://") {
        return endpoint.replacen("http://", "ws://", 1);
    }
    endpoint
}

fn pick_market<'a>(markets: &'a [web::MarketInfo], requested: Option<&str>) -> Option<&'a web::MarketInfo> {
    if let Some(name) = requested {
        let requested_trimmed = name.trim();
        if !requested_trimmed.is_empty() {
            if let Some(found) = markets.iter().find(|m| m.name.eq_ignore_ascii_case(requested_trimmed)) {
                return Some(found);
            }
        }
    }
    markets.first()
}

async fn gateway_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<LoginGatewayState>,
    Query(query): Query<GatewayWsQuery>,
) -> impl IntoResponse {
    if query.token.trim().is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let markets = web::server::advertised_markets(&state.markets);
    let Some(target_market) = pick_market(&markets, query.market.as_deref()) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };

    let target_market_name = target_market.name.clone();
    let target_ws = market_ws_url(&target_market.url, query.token.trim());
    ws.on_upgrade(move |socket| proxy_websocket(socket, target_ws, target_market_name))
        .into_response()
}

async fn proxy_websocket(client_ws: WebSocket, target_ws_url: String, target_market_name: String) {
    let upstream = match connect_async(&target_ws_url).await {
        Ok((ws_stream, _)) => ws_stream,
        Err(e) => {
            tracing::warn!(
                "[gateway] WS proxy failed to connect to market '{}' at '{}': {}",
                target_market_name,
                target_ws_url,
                e
            );
            return;
        }
    };

    let (mut client_tx, mut client_rx) = client_ws.split();
    let (mut upstream_tx, mut upstream_rx) = upstream.split();

    let client_to_upstream = async {
        while let Some(next) = client_rx.next().await {
            let Ok(msg) = next else { break; };
            let mapped = match msg {
                Message::Text(text) => tungstenite::Message::Text(text.to_string().into()),
                Message::Binary(bin) => tungstenite::Message::Binary(bin.to_vec().into()),
                Message::Ping(data) => tungstenite::Message::Ping(data.to_vec().into()),
                Message::Pong(data) => tungstenite::Message::Pong(data.to_vec().into()),
                Message::Close(_) => {
                    let _ = upstream_tx.send(tungstenite::Message::Close(None)).await;
                    break;
                }
            };

            if upstream_tx.send(mapped).await.is_err() {
                break;
            }
        }
    };

    let upstream_to_client = async {
        while let Some(next) = upstream_rx.next().await {
            let Ok(msg) = next else { break; };
            let mapped = match msg {
                tungstenite::Message::Text(text) => Message::Text(text.to_string().into()),
                tungstenite::Message::Binary(bin) => Message::Binary(bin.into()),
                tungstenite::Message::Ping(data) => Message::Ping(data.into()),
                tungstenite::Message::Pong(data) => Message::Pong(data.into()),
                tungstenite::Message::Close(_) => {
                    let _ = client_tx.send(Message::Close(None)).await;
                    break;
                }
                tungstenite::Message::Frame(_) => continue,
            };

            if client_tx.send(mapped).await.is_err() {
                break;
            }
        }
    };

    tokio::select! {
        _ = client_to_upstream => {}
        _ = upstream_to_client => {}
    }
}

fn is_loopback_host(host: &str) -> bool {
    let value = host.trim().to_ascii_lowercase();
    matches!(
        value.as_str(),
        "localhost" | "127.0.0.1" | "::1" | "0.0.0.0" | "host.docker.internal"
    )
}

fn extract_market_url_parts(raw_url: &str) -> Option<(&str, &str, &str)> {
    let trimmed = raw_url.trim();
    if let Some(rest) = trimmed.strip_prefix("http://") {
        let slash = rest.find('/').unwrap_or(rest.len());
        return Some(("http", &rest[..slash], &rest[slash..]));
    }
    if let Some(rest) = trimmed.strip_prefix("https://") {
        let slash = rest.find('/').unwrap_or(rest.len());
        return Some(("https", &rest[..slash], &rest[slash..]));
    }
    None
}

fn split_host_port(host_port: &str) -> (String, Option<String>) {
    let trimmed = host_port.trim();
    if trimmed.is_empty() {
        return (String::new(), None);
    }

    if let Some(rest) = trimmed.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            let host = rest[..end].to_string();
            let remainder = &rest[end + 1..];
            let port = remainder.strip_prefix(':').map(|p| p.to_string());
            return (host, port);
        }
    }

    if let Some((host, port)) = trimmed.rsplit_once(':') {
        if host.contains(':') {
            return (trimmed.to_string(), None);
        }
        return (host.to_string(), Some(port.to_string()));
    }

    (trimmed.to_string(), None)
}

fn forwarded_header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn adapt_market_url_for_client(market_url: &str, headers: &HeaderMap) -> String {
    let Some((scheme, host_port, path)) = extract_market_url_parts(market_url) else {
        return market_url.to_string();
    };

    let (market_host, _market_port) = split_host_port(host_port);
    if !is_loopback_host(&market_host) {
        return market_url.to_string();
    }

    let request_host = forwarded_header(headers, "x-forwarded-host")
        .or_else(|| forwarded_header(headers, "host"));
    let Some(request_host_value) = request_host else {
        return market_url.to_string();
    };

    let (req_host, req_port) = split_host_port(&request_host_value);
    if req_host.is_empty() || is_loopback_host(&req_host) {
        return market_url.to_string();
    }

    let proto = forwarded_header(headers, "x-forwarded-proto")
        .map(|p| p.to_ascii_lowercase())
        .filter(|p| p == "http" || p == "https")
        .unwrap_or_else(|| scheme.to_string());

    // If the public host does not provide a port, keep it implicit (80/443)
    // instead of leaking the internal market port.
    let final_port = req_port;
    let final_host_port = if let Some(port) = final_port {
        format!("{}:{}", req_host, port)
    } else {
        req_host
    };

    format!("{}://{}{}", proto, final_host_port, path)
}

#[derive(Deserialize, serde::Serialize)]
struct GatewayLoginRequest {
    username: String,
    password: String,
    market: Option<String>,
}

async fn gateway_login_handler(
    State(state): State<LoginGatewayState>,
    headers: HeaderMap,
    Json(body): Json<GatewayLoginRequest>,
) -> impl IntoResponse {
    let markets = web::server::advertised_markets(&state.markets);
    if markets.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "No market login endpoint is configured.",
                "code": "NO_MARKET_LOGIN_ENDPOINT"
            })),
        )
            .into_response();
    }

    let requested_market = body
        .market
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());

    let markets_to_try: Vec<web::MarketInfo> = if let Some(name) = requested_market {
        match markets.iter().find(|market| market.name.eq_ignore_ascii_case(name)) {
            Some(found) => vec![found.clone()],
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": "Requested market is not available.",
                        "code": "MARKET_NOT_FOUND"
                    })),
                )
                    .into_response();
            }
        }
    } else {
        markets
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            tracing::error!("[gateway] Failed to build HTTP client for login proxy: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Gateway login service initialization failed.",
                    "code": "GATEWAY_LOGIN_INIT_FAILED"
                })),
            )
                .into_response();
        }
    };

    let mut last_unauthorized: Option<serde_json::Value> = None;

    for market in markets_to_try {
        let endpoint = format!("{}/api/login", market.url.trim_end_matches('/'));
        let response = match client
            .post(&endpoint)
            .json(&serde_json::json!({
                "username": body.username,
                "password": body.password
            }))
            .send()
            .await
        {
            Ok(response) => response,
            Err(e) => {
                tracing::warn!(
                    "[gateway] Login proxy could not reach market '{}' at '{}': {}",
                    market.name,
                    endpoint,
                    e
                );
                continue;
            }
        };

        let status = response.status();
        let payload = response
            .json::<serde_json::Value>()
            .await
            .unwrap_or_else(|_| serde_json::json!({}));

        if status.is_success() {
            let mut out = match payload {
                serde_json::Value::Object(map) => map,
                _ => serde_json::Map::new(),
            };
            let market_url = adapt_market_url_for_client(&market.url, &headers);
            out.insert(
                "market_url".to_string(),
                serde_json::Value::String(market_url),
            );
            out.insert(
                "market_name".to_string(),
                serde_json::Value::String(market.name.clone()),
            );
            return (StatusCode::OK, Json(serde_json::Value::Object(out))).into_response();
        }

        if status.as_u16() == StatusCode::UNAUTHORIZED.as_u16() {
            last_unauthorized = Some(payload);
            continue;
        }

        tracing::warn!(
            "[gateway] Login proxy market '{}' returned status {}",
            market.name,
            status
        );
    }

    if let Some(payload) = last_unauthorized {
        return (StatusCode::UNAUTHORIZED, Json(payload)).into_response();
    }

    (
        StatusCode::BAD_GATEWAY,
        Json(serde_json::json!({
            "error": "Could not reach any market login endpoint.",
            "code": "MARKET_LOGIN_UNREACHABLE"
        })),
    )
        .into_response()
}

struct ThreadHandles {
    handles: Vec<std::thread::JoinHandle<()>>,
}

impl ThreadHandles {
    fn new() -> Self {
        ThreadHandles {
            handles: Vec::new(),
        }
    }

    fn add_handle(&mut self, handle: std::thread::JoinHandle<()>) {
        self.handles.push(handle);
    }

    fn stop_all(&mut self) {
        for handle in self.handles.drain(..) {
            let name = handle.thread().name().unwrap_or("unknown").to_string();
            handle.thread().unpark(); // Unpark the thread in case it's parked, so it can check the shutdown flag and exit.
            handle.join().expect(&format!("Failed to join thread {}", name));
        }
    }
}

struct MarketSimulator {
    config: MarketConfig,
    player_database_url: String,
    thread_handles: Arc<Mutex<ThreadHandles>>,
    shutdown: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Multicast endpoints for order book snapshots to forward to GUI websockets.
    market_feed_sources: Vec<MulticastSource>,
    /// All configured markets — passed to the web server for the login page.
    known_markets: Vec<web::MarketInfo>,
}

impl MarketSimulator {
    fn add_thread_handle(&self, handle: std::thread::JoinHandle<()>) {
        self.thread_handles.lock().unwrap().add_handle(handle);
    }
}

struct QueueHandle {
    net_to_fix_tx: Option<Arc<channel::Sender<FixRawMsg<RB_SIZE>>>>,
    net_to_fix_rx: Option<Arc<channel::Receiver<FixRawMsg<RB_SIZE>>>>,
    fix_to_ob: Option<memory::SharedQueue<RB_SIZE, OrderEvent>>,
    ob_to_er: Option<memory::SharedQueue<RB_SIZE, (OrderEvent, OrderResult)>>,
    ob_to_db: Option<memory::SharedQueue<RB_SIZE, (OrderEvent, OrderResult)>>,
    ob_to_md: Option<memory::SharedQueue<RB_SIZE, (OrderEvent, OrderResult)>>,
    er_to_fix: Option<memory::SharedQueue<RB_SIZE, (EntityId, FixRawMsg<RB_SIZE>)>>,
    ob_to_ss: Option<memory::SharedQueue<RB_SIZE, Arc<Snapshot>>>,
}

impl QueueHandle {
    fn new(market_name: &str) -> Self {
        let (net_to_fix_tx, net_to_fix_rx) = channel::bounded::<FixRawMsg<RB_SIZE>>(RB_SIZE);
        let fix_to_ob    = memory::open_shared_queue::<RB_SIZE, OrderEvent>(&format!("{market_name}_fix_to_order_book"), true);
        let ob_to_er     = memory::open_shared_queue::<RB_SIZE, (OrderEvent, OrderResult)>(&format!("{market_name}_order_book_to_execution_report"), true);
        let ob_to_db     = memory::open_shared_queue::<RB_SIZE, (OrderEvent, OrderResult)>(&format!("{market_name}_order_book_to_db"), true);
        let ob_to_md     = memory::open_shared_queue::<RB_SIZE, (OrderEvent, OrderResult)>(&format!("{market_name}_order_book_to_market_feed"), true);
        let er_to_fix     = memory::open_shared_queue::<RB_SIZE, (EntityId, FixRawMsg<RB_SIZE>)>(&format!("{market_name}_execution_report_to_fix"), true);
        let ob_to_ss     = memory::open_shared_queue::<RB_SIZE, Arc<Snapshot>>(&format!("{market_name}_order_book_to_snapshot"), true);

        Self {
            net_to_fix_tx: Some(Arc::new(net_to_fix_tx)),
            net_to_fix_rx: Some(Arc::new(net_to_fix_rx)),
            fix_to_ob: Some(fix_to_ob),
            ob_to_er: Some(ob_to_er),
            ob_to_db: Some(ob_to_db),
            ob_to_md: Some(ob_to_md),
            er_to_fix: Some(er_to_fix),
            ob_to_ss: Some(ob_to_ss),
        }
    } 
}

fn start_market(market_simulator: Arc<Mutex<MarketSimulator>>) -> Result<(), Box<dyn std::error::Error>> {

    let mut market_simulator = market_simulator.lock().unwrap();

    let config = market_simulator.config.clone();
    let player_database_url = market_simulator.player_database_url.clone();
    let market_feed_sources = market_simulator.market_feed_sources.clone();
    let bus = EventBus::new();
    let global_shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let order_book = Arc::new(std::sync::Mutex::new(OrderBookState::new()));
    let database_url = config.resolve_database_url().map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, e)
    })?;
    let player_store = web::players::PlayerStore::load_postgres(&player_database_url);

    // Global shutdown flag is shared across all threads and can be set by the Ctrl-C handler to signal all threads to exit.
    market_simulator.shutdown = Some(Arc::clone(&global_shutdown));

    let _multicast_receiver_thread = spawn_market_feed_receiver(
        bus.clone(),
        market_feed_sources,
        Arc::clone(&global_shutdown),
        Arc::clone(&order_book),
        player_store,
    );

    {
        market_simulator.thread_handles.lock().unwrap().add_handle(_multicast_receiver_thread);
    }

    // Initialization of shared queues for inter-thread communication
    let mut queues = QueueHandle::new(&config.name);

    let net_to_fix_tx = queues.net_to_fix_tx.as_ref().unwrap().clone();
    let (fix_tx, ob_rx) = queues.fix_to_ob.take().unwrap().queue.split();
    let (ob_er_tx, er_rx) = queues.ob_to_er.take().unwrap().queue.split();
    let (er_tx, fix_resp_rx) = queues.er_to_fix.take().unwrap().queue.split();
    let (ob_db_tx, ob_db_rx) = queues.ob_to_db.take().unwrap().queue.split();
    let (ob_md_tx, ob_md_rx) = queues.ob_to_md.take().unwrap().queue.split();
    let (ob_ss_tx, ob_ss_rx) = queues.ob_to_ss.take().unwrap().queue.split();
    
    
    // execution report engine thread
    let execution_report_engine = ExecutionReportEngine::new(er_rx, er_tx, Arc::clone(&global_shutdown));

    let _er_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: config.core_mapping.execution_report_core });
        execution_report_engine.run();
    });

    {
        market_simulator.add_thread_handle(_er_thread);
    }

    // DB engine thread
    let db_engine = match db::DatabaseEngine::new(ob_db_rx, &database_url, Arc::clone(&global_shutdown)) {
        Ok(engine) => engine,
        Err(e) => {
            return Err(Box::new(e));
        }
    };

    match db_engine.init() {
        Ok(_) => (),
        Err(e) => {
            return Err(Box::new(e));
        }
    }

    // Import initial order book state from the database before starting the engine
    let initial_orders = match db_engine.get_all_pending_orders() {
        Ok(orders) => {
            tracing::info!("[{}] Loaded {} pending orders from database", config.name, orders.len());
            orders
        },
        Err(e) => {
            return Err(Box::new(e));
        }
    };

    // Grab the database pool for the gRPC service before moving db_engine into its thread.
    let db_pool = db_engine.pool();

    let _db_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: config.core_mapping.db_core });
        db_engine.run();
    });

    {
        market_simulator.add_thread_handle(_db_thread);
    }

    // Order-book control channel (used by the gRPC reset service).
    let (ob_control_tx, ob_control_rx) = crossbeam_channel::bounded::<OrderBookControl>(32);

    let symbols = vec!["AAAPL"]; // In a real implementation, this would come from the config or database.

    for symbol in symbols {
        tracing::info!("[{}] Initializing market for symbol '{}'", config.name, symbol);

        // Create shared order book instance and pass it to the order book engine and snapshot generation engine so they can read/write it without going through the queues.
        let order_book = OrderBook::new(&symbol);
        let snapshot_ptr = Arc::new(ArcSwap::from_pointee(Snapshot {
            symbol: symbol.to_string(),
            ..Default::default()
        }));

        // Book engine thread
        let mut order_book_engine = OrderBookEngine::new(
            ob_rx,
            
            Some(ob_er_tx),
            Some(ob_db_tx),
            Some(ob_md_tx),
            ob_control_rx,
            order_book,
            Some(Arc::clone(&snapshot_ptr)),
            Arc::clone(&global_shutdown)
        );

        // Import initial order book state from the database before starting the engine.
        // This ensures that the engine starts with the correct state and can process new orders/events in the context of existing pending orders.
        order_book_engine.import_order_book(initial_orders);

        let _ob_thread = std::thread::spawn(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: config.core_mapping.order_book_core });
            order_book_engine.run();
        });

        {
            market_simulator.add_thread_handle(_ob_thread);
        }

        // Snapshot generation thread (reads from order book and pushes to multicast engine)
        let snapshot_generation_engine = SnapshotGenerationEngine::new(
            ob_ss_tx, 
            Arc::clone(&global_shutdown),
            Arc::clone(&snapshot_ptr),
            config.snapshot.update_interval_ms, 
        );
        let snapshot_generation_core = config.core_mapping.snapshot_core;
        let _snapshot_generation_thread = std::thread::spawn(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: snapshot_generation_core });
            snapshot_generation_engine.run();
        });

        {
            market_simulator.add_thread_handle(_snapshot_generation_thread);
        }

        // TODO : Handle multiple symbols per market (currently we just hardcode one symbol and ignore the symbol field in the orders/events, but in a real implementation we'd want to support multiple symbols per market and route orders/events to the correct order book based on the symbol).
        break;
    }

    // Snapshot multicast engine thread
    let mut snapshot_engine = snapshot::engine::SnapshotMultiCastEngine::new(
        ob_ss_rx,
        Arc::clone(&global_shutdown),
        &config.snapshot_multicast.ip,
        config.snapshot_multicast.port,
    );

    let snapshot_core = config.core_mapping.snapshot_core;
    let _snapshot_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: snapshot_core });
        snapshot_engine.run();
    });

    {
        market_simulator.add_thread_handle(_snapshot_thread);
    }

    // Market feed engine thread
    let mut market_feed_engine = MarketDataFeedEngine::new(
        ob_md_rx,
        Arc::clone(&global_shutdown),
        &config.market_feed_multicast.ip,
        config.market_feed_multicast.port,
    )?;

    let market_feed_core = config.core_mapping.market_feed_core;
    let _market_feed_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: market_feed_core });
        market_feed_engine.run();
    });

    {
        market_simulator.add_thread_handle(_market_feed_thread);
    }

    // fix engine thread
    let fix_engine = FixEngine ::new(Arc::clone(&queues.net_to_fix_rx.as_ref().unwrap()), fix_tx, fix_resp_rx, Arc::clone(&global_shutdown));
    let (mut inbound_engine, mut outbound_engine) = fix_engine.split();

    let _fix_inbound_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: config.core_mapping.fix_inbound_core });
        inbound_engine.run();
    });

    let _fix_outbound_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: config.core_mapping.fix_outbound_core });
        outbound_engine.run();
    });

    {
        market_simulator.add_thread_handle(_fix_inbound_thread);
        market_simulator.add_thread_handle(_fix_outbound_thread);
    }

    // Start the web server in a separate thread, passing it the event bus
    let web_core = config.core_mapping.web_core;
    let _web_thread = std::thread::spawn({
        let bus = bus.clone();
        let fix_tcp_addr = format!("{}:{}", config.tcp.ip, config.tcp.port);
        let grpc_addr    = format!("http://127.0.0.1:{}", config.grpc.port);
        let web_ip = config.web.ip.clone();
        let web_port = config.web.port;
        let web_database_url = player_database_url.clone();
        let web_shutdown = Arc::clone(&global_shutdown);
        let known_markets = market_simulator.known_markets.clone();
        let order_book = Arc::clone(&order_book);
        move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: web_core });
            run_web_server(bus, fix_tcp_addr, grpc_addr, &web_ip, web_port, web_database_url, known_markets, web_shutdown, order_book);
        }
    });

    {
        market_simulator.add_thread_handle(_web_thread);
    }

    // tcp server — each client pushes directly into fifo_in
    let server: FixServer<RB_SIZE> = FixServer::new(Arc::clone(&net_to_fix_tx), Arc::clone(&global_shutdown));
    let listener = TcpListener::bind(format!("{}:{}", config.tcp.ip, config.tcp.port)).unwrap();

    // Grab the shutdown flag before releasing the lock so the Ctrl-C handler
    // can signal the accept loop without holding any other lock.

    let tcp_core = config.core_mapping.tcp_core;
    let _tcp_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: tcp_core });
        server.accept_loop(listener, vec![tcp_core]);
    });

    {
        market_simulator.add_thread_handle(_tcp_thread);
    }

    // gRPC MarketControl server — handles ResetMarket (order book + DB).
    let grpc_ip   = config.grpc.ip.clone();
    let grpc_port = config.grpc.port;
    let grpc_shutdown = Arc::clone(&global_shutdown);
    let grpc_service = grpc::MarketControlService::new(ob_control_tx, db_pool);
    let _grpc_thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to build tokio runtime for gRPC server");
        let addr: std::net::SocketAddr = format!("{grpc_ip}:{grpc_port}")
            .parse()
            .expect("invalid gRPC address");
        if let Err(e) = rt.block_on(grpc::serve(addr, grpc_service, grpc_shutdown)) {
            tracing::error!("gRPC server error: {e:#}");
        }
    });

    {
        market_simulator.add_thread_handle(_grpc_thread);
    }

    tracing::info!("[{}] FIX server    -> {}:{}", utils::market_name(), config.tcp.ip, config.tcp.port);
    tracing::info!("[{}] Web terminal  -> http://{}:{}", utils::market_name(), config.web.ip, config.web.port);
    tracing::info!("[{}] gRPC control  -> {}:{}", utils::market_name(), config.grpc.ip, config.grpc.port);
    tracing::info!("[{}] Market feed   -> {}:{}", utils::market_name(), config.market_feed_multicast.ip, config.market_feed_multicast.port);
    tracing::info!("[{}] Snapshot feed -> {}:{}", utils::market_name(), config.snapshot_multicast.ip, config.snapshot_multicast.port);

    Ok(())
}

fn stop_market(market_simulator: Arc<Mutex<MarketSimulator>>) {
    let (thread_handles, shutdown) = {
        let market_simulator = market_simulator.lock().unwrap();
        (
            Arc::clone(&market_simulator.thread_handles),
            Arc::clone(&market_simulator.shutdown.as_ref().unwrap()),
        )
    };

    // Set the global shutdown flag to signal all threads to exit.
    shutdown.store(true, std::sync::atomic::Ordering::Relaxed);

    let thread_handles = &mut thread_handles.lock().unwrap();
    thread_handles.stop_all();
    

     tracing::info!("[{}] All threads stopped, market simulator exiting", market_name());
}

fn main() {
    let cli = Cli::parse();
    let config = MarketsConfig::parse_from_file(&cli.config_file);

    tracing_subscriber::fmt()
        .with_env_filter("debug,sqlx=warn,h2=warn,tokio_util=warn")
        .init();

    if config.markets.is_empty() {
        tracing::error!("No markets defined in config file '{}'", cli.config_file);
        std::process::exit(1);
    }

    // ── Single-market mode (child process) ──────────────────────────────────
    if let Some(index) = cli.market_index {
        // Build the list of all markets for the login page.
        let known_markets: Vec<web::MarketInfo> = config.markets.iter().map(|m| web::MarketInfo {
            name: m.name.clone(),
            url: format!("http://{}:{}", m.web.ip, m.web.port),
        }).collect();

        let market_config = config
            .markets
            .get(index)
            .cloned()
            .unwrap_or_else(|| {
                tracing::error!("Market index {index} out of range");
                std::process::exit(1);
            });

        let player_database_url = config
            .resolve_player_database_url(&market_config)
            .unwrap_or_else(|e| {
                tracing::error!("{e}");
                std::process::exit(1);
            });

        // In single-market mode, subscribe ONLY to this market's multicast sources.
        let market_feed_sources = vec![MulticastSource::new(
            market_config.market_feed_multicast.ip.as_str(),
            market_config.market_feed_multicast.port,
            market_name(),
        )];

        let simulator = Arc::new(Mutex::new(MarketSimulator {
            config: market_config,
            player_database_url,
            thread_handles: Arc::new(Mutex::new(ThreadHandles::new())),
            shutdown: None,
            market_feed_sources,
            known_markets,
        }));

        if let Err(e) = start_market(Arc::clone(&simulator)) {
            tracing::error!("Market failed to start: {e}");
            std::process::exit(1);
        }

        ctrlc::set_handler(move || {
            stop_market(Arc::clone(&simulator));
            std::process::exit(0);
        })
        .expect("Error setting Ctrl-C handler");

        loop {
            std::thread::park();
        }
    }

    // ── Multi-market mode (parent process) ──────────────────────────────────
    let gateway_ip = config.entry_point.ip.clone();
    let gateway_port = config.entry_point.port;
    let gateway_markets: Vec<web::MarketInfo> = config
        .markets
        .iter()
        .map(|m| web::MarketInfo {
            name: m.name.clone(),
            url: format!("http://{}:{}", m.web.ip, m.web.port),
        })
        .collect();

    tracing::info!(
        "[gateway] Configured entry point → http://{}:{}",
        gateway_ip,
        gateway_port
    );

    {
        let login_ip = gateway_ip.clone();
        let login_port = gateway_port;
        std::thread::spawn(move || {
            run_login_gateway(gateway_markets, &login_ip, login_port);
        });
    }

    // Fork one child process per market entry; each child re-execs this binary
    // with --market-index <n> so it runs in single-market mode above.
    let exe = std::env::current_exe().expect("Cannot determine current executable path");

    let mut children: Vec<std::process::Child> = config
        .markets
        .iter()
        .enumerate()
        .map(|(index, market)| {
            tracing::info!("[{}] Spawning process (index {index})", market.name);
            std::process::Command::new(&exe)
                .args(["--config", &cli.config_file, "--market-index", &index.to_string()])
                .env("MARKET_NAME", &market.name)
                .env("LOGIN_GATEWAY_URL", format!("http://{}:{}", gateway_ip, gateway_port))
                .spawn()
                .unwrap_or_else(|e| panic!("Failed to spawn process for market '{}': {e}", market.name))
        })
        .collect();

    // Ctrl+C from the terminal goes to the whole process group, so every
    // child's own ctrlc handler will fire.  Just wait for them here.
    for child in &mut children {
        let _ = child.wait();
    }
}