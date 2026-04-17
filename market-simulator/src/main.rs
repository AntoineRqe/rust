use snapshot::types::Snapshot;
use types::{OrderEvent, OrderResult};
use types::macros::{EntityId};
use clap::Parser;
use axum::{
    Router,
    routing::get,
    extract::State,
    response::Html,
    Json,
};
use order_book::order_book::{OrderBook, OrderBookEngine, SnapshotGenerationEngine};
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
    EventBus,
};
use web::server::run_web_server;
use config::{MarketConfig, MarketsConfig};
use market_feed::engine::MarketDataFeedEngine;
use std::sync::RwLock;


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
            let state = LoginGatewayState { markets };
            let app = Router::new()
                .route("/", get(gateway_login_page_handler))
                .route("/api/markets", get(gateway_markets_handler))
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

async fn gateway_markets_handler(State(state): State<LoginGatewayState>) -> Json<Vec<web::MarketInfo>> {
    Json(state.markets)
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
    thread_handles: Arc<Mutex<ThreadHandles>>,
    shutdown: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Multicast endpoints for order book snapshots to forward to GUI websockets.
    market_feed_sources: Vec<MulticastSource>,
    market_snapshot_sources: Vec<MulticastSource>,
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
    ob_to_ss: Option<memory::SharedQueue<RB_SIZE, Snapshot>>,
}

impl QueueHandle {
    fn new(market_name: &str) -> Self {
        let (net_to_fix_tx, net_to_fix_rx) = channel::bounded::<FixRawMsg<RB_SIZE>>(RB_SIZE);
        let fix_to_ob    = memory::open_shared_queue::<RB_SIZE, OrderEvent>(&format!("{market_name}_fix_to_order_book"), true);
        let ob_to_er     = memory::open_shared_queue::<RB_SIZE, (OrderEvent, OrderResult)>(&format!("{market_name}_order_book_to_execution_report"), true);
        let ob_to_db     = memory::open_shared_queue::<RB_SIZE, (OrderEvent, OrderResult)>(&format!("{market_name}_order_book_to_db"), true);
        let ob_to_md     = memory::open_shared_queue::<RB_SIZE, (OrderEvent, OrderResult)>(&format!("{market_name}_order_book_to_market_feed"), true);
        let er_to_fix     = memory::open_shared_queue::<RB_SIZE, (EntityId, FixRawMsg<RB_SIZE>)>(&format!("{market_name}_execution_report_to_fix"), true);
        let ob_to_ss     = memory::open_shared_queue::<RB_SIZE, Snapshot>(&format!("{market_name}_order_book_to_snapshot"), true);

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
    let market_feed_sources = market_simulator.market_feed_sources.clone();
    let bus = EventBus::new();
    let global_shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Global shutdown flag is shared across all threads and can be set by the Ctrl-C handler to signal all threads to exit.
    market_simulator.shutdown = Some(Arc::clone(&global_shutdown));

    let _multicast_receiver_thread = spawn_market_feed_receiver(
        bus.clone(),
        market_feed_sources,
        Arc::clone(&global_shutdown),
    );

    {
        market_simulator.thread_handles.lock().unwrap().add_handle(_multicast_receiver_thread);
    }

    // Initialization of shared queues for inter-thread communication
    let mut queues = QueueHandle::new(&config.name);

    let net_to_fix_tx = queues.net_to_fix_tx.as_ref().unwrap().clone();
    let (fix_tx, ob_rx) = queues.fix_to_ob.take().unwrap().queue.split();
    let (ob_tx, er_rx) = queues.ob_to_er.take().unwrap().queue.split();
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
    let database_url = config.resolve_database_url().map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, e)
    })?;


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

    
    // Create shared order book instance and pass it to the order book engine and snapshot generation engine so they can read/write it without going through the queues.
    let order_book = Arc::new(RwLock::new(OrderBook::new("AAAPL")));

    // Book engine thread
    let mut order_book_engine = OrderBookEngine::with_shutdown(ob_rx, [Some(ob_tx), Some(ob_db_tx), Some(ob_md_tx)], ob_control_rx, Arc::clone(&order_book), Some(Arc::clone(&global_shutdown)));

    order_book_engine.import_order_book(initial_orders);

    let _ob_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: config.core_mapping.order_book_core });
        order_book_engine.run();
    });

    {
        market_simulator.add_thread_handle(_ob_thread);
    }

    // Snapshot generation thread (reads from order book and pushes to multicast engine)
    let snapshot_generation_engine = SnapshotGenerationEngine::new(ob_ss_tx, Arc::clone(&global_shutdown), Arc::clone(&order_book), config.snapshot.update_interval_ms, config.snapshot.max_depth);
    let snapshot_generation_core = config.core_mapping.snapshot_core;
    let _snapshot_generation_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: snapshot_generation_core });
        snapshot_generation_engine.run();
    });

    {
        market_simulator.add_thread_handle(_snapshot_generation_thread);
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
        let web_shutdown = Arc::clone(&global_shutdown);
        let known_markets = market_simulator.known_markets.clone();
        move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: web_core });
            run_web_server(bus, fix_tcp_addr, grpc_addr, &web_ip, web_port, std::path::PathBuf::from("players.json"), known_markets, web_shutdown);
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
        server.accept_loop(listener);
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
        .with_env_filter("debug,sqlx=warn")
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
            .into_iter()
            .nth(index)
            .unwrap_or_else(|| {
                tracing::error!("Market index {index} out of range");
                std::process::exit(1);
            });

        // In single-market mode, subscribe ONLY to this market's multicast sources.
        let market_feed_sources = vec![MulticastSource::new(
            market_config.market_feed_multicast.ip.as_str(),
            market_config.market_feed_multicast.port,
            market_name(),
        )];

        let market_snapshot_sources = vec![MulticastSource::new(
            market_config.snapshot_multicast.ip.as_str(),
            market_config.snapshot_multicast.port,
            market_name(),
        )];

        let simulator = Arc::new(Mutex::new(MarketSimulator {
            config: market_config,
            thread_handles: Arc::new(Mutex::new(ThreadHandles::new())),
            shutdown: None,
            market_feed_sources,
            market_snapshot_sources,
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