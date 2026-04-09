use snapshot::types::Snapshot;
use types::{OrderEvent, OrderResult};
use types::macros::{EntityId};
use clap::Parser;
use order_book::order_book::{OrderBook, OrderBookEngine, SnapshotGenerationEngine};
use order_book::OrderBookControl;
use server::tcp::{
    FixServer,
};
use server::multicast::{MulticastSource, spawn_market_feed_receiver};
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

struct ThreadHandles {
    fix_inbound_thread: Option<std::thread::JoinHandle<()>>,
    fix_outbound_thread: Option<std::thread::JoinHandle<()>>,
    ob_thread: Option<std::thread::JoinHandle<()>>,
    er_thread: Option<std::thread::JoinHandle<()>>,
    market_feed_thread: Option<std::thread::JoinHandle<()>>,
    multicast_receiver_thread: Option<std::thread::JoinHandle<()>>,
    db_thread: Option<std::thread::JoinHandle<()>>,
    web_thread: Option<std::thread::JoinHandle<()>>,
    tcp_thread: Option<std::thread::JoinHandle<()>>,
    grpc_thread: Option<std::thread::JoinHandle<()>>,
    snapshot_thread: Option<std::thread::JoinHandle<()>>,
    snapshot_generation_thread: Option<std::thread::JoinHandle<()>>,
}

struct MarketSimulator {
    config: MarketConfig,
    thread_handles: Arc<Mutex<ThreadHandles>>,
    entry_point: Option<Arc<channel::Sender<FixRawMsg<RB_SIZE>>>>,
    shutdown: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Multicast endpoints for order book snapshots to forward to GUI websockets.
    market_feed_sources: Vec<MulticastSource>,
    market_snapshot_sources: Vec<MulticastSource>,
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
    fn new(market_simulator: &mut MarketSimulator, market_name: &str) -> Self {
        let (net_to_fix_tx, net_to_fix_rx) = channel::bounded::<FixRawMsg<RB_SIZE>>(RB_SIZE);
        let fix_to_ob    = memory::open_shared_queue::<RB_SIZE, OrderEvent>(&format!("{market_name}_fix_to_order_book"), true);
        let ob_to_er     = memory::open_shared_queue::<RB_SIZE, (OrderEvent, OrderResult)>(&format!("{market_name}_order_book_to_execution_report"), true);
        let ob_to_db     = memory::open_shared_queue::<RB_SIZE, (OrderEvent, OrderResult)>(&format!("{market_name}_order_book_to_db"), true);
        let ob_to_md     = memory::open_shared_queue::<RB_SIZE, (OrderEvent, OrderResult)>(&format!("{market_name}_order_book_to_market_feed"), true);
        let er_to_fix     = memory::open_shared_queue::<RB_SIZE, (EntityId, FixRawMsg<RB_SIZE>)>(&format!("{market_name}_execution_report_to_fix"), true);
        let ob_to_ss     = memory::open_shared_queue::<RB_SIZE, Snapshot>(&format!("{market_name}_order_book_to_snapshot"), true);

        // Bind Fix entry point to global struct so it can be accessed by the TCP server
        market_simulator.entry_point = Some(Arc::new(net_to_fix_tx.clone()));

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
        market_simulator.thread_handles.lock().unwrap().multicast_receiver_thread = Some(_multicast_receiver_thread);
    }

    // Initialization of shared queues for inter-thread communication
    let mut queues = QueueHandle::new(&mut market_simulator, &config.name);

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
        market_simulator.thread_handles.lock().unwrap().er_thread = Some(_er_thread);
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
        market_simulator.thread_handles.lock().unwrap().db_thread = Some(_db_thread);
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
        market_simulator.thread_handles.lock().unwrap().ob_thread = Some(_ob_thread);
    }

    // Snapshot generation thread (reads from order book and pushes to multicast engine)
    let snapshot_generation_engine = SnapshotGenerationEngine::new(ob_ss_tx, Arc::clone(&global_shutdown), Arc::clone(&order_book), config.snapshot.update_interval_ms, config.snapshot.max_depth);
    let snapshot_generation_core = config.core_mapping.snapshot_core;
    let _snapshot_generation_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: snapshot_generation_core });
        snapshot_generation_engine.run();
    });

    {
        market_simulator.thread_handles.lock().unwrap().snapshot_generation_thread = Some(_snapshot_generation_thread);
    }

    // Snapshot multicast engine thread
    let mut snapshot_engine = snapshot::engine::SnapshotMultiCastEngine::new(
        ob_ss_rx,
        Arc::clone(&global_shutdown),
        &config.snapshot_multicast.address,
        config.snapshot_multicast.port,
    );

    let snapshot_core = config.core_mapping.snapshot_core;
    let _snapshot_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: snapshot_core });
        snapshot_engine.run();
    });

    {
        market_simulator.thread_handles.lock().unwrap().snapshot_thread = Some(_snapshot_thread);
    }

    // Market feed engine thread
    let mut market_feed_engine = MarketDataFeedEngine::new(
        ob_md_rx,
        Arc::clone(&global_shutdown),
        &config.market_feed_multicast.address,
        config.market_feed_multicast.port,
    )?;

    let market_feed_core = config.core_mapping.market_feed_core;
    let _market_feed_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: market_feed_core });
        market_feed_engine.run();
    });

    {
        market_simulator.thread_handles.lock().unwrap().market_feed_thread = Some(_market_feed_thread);
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
        let mut thread_handles = market_simulator.thread_handles.lock().unwrap();
        thread_handles.fix_inbound_thread = Some(_fix_inbound_thread);
        thread_handles.fix_outbound_thread = Some(_fix_outbound_thread);
    }

    // Start the web server in a separate thread, passing it the event bus
    let _web_thread = std::thread::spawn({
        core_affinity::set_for_current(core_affinity::CoreId { id: config.core_mapping.web_core });
        let bus = bus.clone();
        let fix_tcp_addr = format!("{}:{}", config.tcp.ip, config.tcp.port);
        let grpc_addr    = format!("http://127.0.0.1:{}", config.grpc.port);
        let web_ip = config.web.ip.clone();
        let web_port = config.web.port;
        let web_shutdown = Arc::clone(&global_shutdown);
        move || {
            run_web_server(bus, fix_tcp_addr, grpc_addr, &web_ip, web_port, std::path::PathBuf::from("players.json"), web_shutdown);
        }
    });

    {
        market_simulator.thread_handles.lock().unwrap().web_thread = Some(_web_thread);
    }

    // tcp server — each client pushes directly into fifo_in
    let server: FixServer<RB_SIZE> = FixServer::new(Arc::clone(&net_to_fix_tx));
    let listener = TcpListener::bind(format!("{}:{}", config.tcp.ip, config.tcp.port)).unwrap();

    // Grab the shutdown flag before releasing the lock so the Ctrl-C handler
    // can signal the accept loop without holding any other lock.

    let tcp_core = config.core_mapping.tcp_core;
    let _tcp_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: tcp_core });
        server.accept_loop(listener);
    });

    {
        market_simulator.thread_handles.lock().unwrap().tcp_thread = Some(_tcp_thread);
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
        market_simulator.thread_handles.lock().unwrap().grpc_thread = Some(_grpc_thread);
    }

    tracing::info!("[{}] FIX server    -> {}:{}", config.name, config.tcp.ip, config.tcp.port);
    tracing::info!("[{}] Web terminal  -> http://{}:{}", config.name, config.web.ip, config.web.port);
    tracing::info!("[{}] gRPC control  -> {}:{}", config.name, config.grpc.ip, config.grpc.port);
    tracing::info!("[{}] Market feed   -> {}:{}", config.name, config.market_feed_multicast.address, config.market_feed_multicast.port);
    tracing::info!("[{}] Snapshot feed -> {}:{}", config.name, config.snapshot_multicast.address, config.snapshot_multicast.port);

    Ok(())
}

fn stop_market(market_simulator: Arc<Mutex<MarketSimulator>>) {
    let (entry_point, thread_handles, shutdown) = {
        let market_simulator = market_simulator.lock().unwrap();
        (
            market_simulator.entry_point.clone(),
            Arc::clone(&market_simulator.thread_handles),
            Arc::clone(&market_simulator.shutdown.as_ref().unwrap()),
        )
    };

    // Set the global shutdown flag to signal all threads to exit.
    shutdown.store(true, std::sync::atomic::Ordering::Relaxed);

    // Send a shutdown message to the FIX engine to unblock it if it's waiting on the queue.
    if let Some(entry_point) = entry_point {
        let _ = entry_point.send(FixRawMsg::default());
    }

    let thread_handles = &mut thread_handles.lock().unwrap();

    if let Some(handle) = thread_handles.fix_inbound_thread.take() {
        handle.join().expect("Failed to join FIX inbound thread");
    }
    if let Some(handle) = thread_handles.fix_outbound_thread.take() {
        handle.join().expect("Failed to join FIX outbound thread");
    }
    if let Some(handle) = thread_handles.ob_thread.take() {
        handle.join().expect("Failed to join Order Book thread");
    }
    if let Some(handle) = thread_handles.er_thread.take() {
        handle.join().expect("Failed to join Execution Report thread");
    }
    if let Some(handle) = thread_handles.market_feed_thread.take() {
        handle.join().expect("Failed to join Market Feed thread");
    }
    if let Some(handle) = thread_handles.snapshot_thread.take() {
        handle.join().expect("Failed to join Snapshot thread");
    }
    if let Some(handle) = thread_handles.snapshot_generation_thread.take() {
        handle.join().expect("Failed to join Snapshot Generation thread");
    }
    if let Some(handle) = thread_handles.multicast_receiver_thread.take() {
        handle.join().expect("Failed to join Multicast Receiver thread");
    }
    if let Some(handle) = thread_handles.db_thread.take() {
        handle.join().expect("Failed to join Database thread");
    }
    if let Some(handle) = thread_handles.web_thread.take() {
        handle.join().expect("Failed to join Web Server thread");
    }
    if let Some(handle) = thread_handles.tcp_thread.take() {
        handle.join().expect("Failed to join TCP Server thread");
    }
    // Join gRPC server thread after signaling grpc_shutdown.
    if let Some(handle) = thread_handles.grpc_thread.take() {
        handle.join().expect("Failed to join gRPC Server thread");
    }
}

fn main() {
    let cli = Cli::parse();
    let config = MarketsConfig::parse_from_file(&cli.config_file);

    tracing_subscriber::fmt()
        .with_env_filter("debug")
        .init();

    if config.markets.is_empty() {
        eprintln!("No markets defined in config file '{}'", cli.config_file);
        std::process::exit(1);
    }

    // ── Single-market mode (child process) ──────────────────────────────────
    if let Some(index) = cli.market_index {
        let market_feed_sources = config.markets.iter().map(|market| MulticastSource {
            market: market.name.clone(),
            address: market.market_feed_multicast.address.clone(),
            port: market.market_feed_multicast.port,
        }).collect::<Vec<_>>();

        let market_snapshot_sources = config.markets.iter().map(|market| MulticastSource {
            market: market.name.clone(),
            address: market.snapshot_multicast.address.clone(),
            port: market.snapshot_multicast.port,
        }).collect::<Vec<_>>();

        let market_config = config
            .markets
            .into_iter()
            .nth(index)
            .unwrap_or_else(|| {
                eprintln!("Market index {index} out of range");
                std::process::exit(1);
            });

        let simulator = Arc::new(Mutex::new(MarketSimulator {
            config: market_config,
            thread_handles: Arc::new(Mutex::new(ThreadHandles {
                fix_inbound_thread: None,
                fix_outbound_thread: None,
                ob_thread: None,
                er_thread: None,
                market_feed_thread: None,
                multicast_receiver_thread: None,
                db_thread: None,
                web_thread: None,
                tcp_thread: None,
                grpc_thread: None,
                snapshot_thread: None,
                snapshot_generation_thread: None,
            })),
            entry_point: None,
            shutdown: None,
            market_feed_sources,
            market_snapshot_sources,
        }));

        if let Err(e) = start_market(Arc::clone(&simulator)) {
            eprintln!("Market failed to start: {e}");
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