use snapshot::types::Snapshot;
use types::{OrderEvent, OrderResult};
use types::macros::{EntityId};
use clap::Parser;
use order_book::OrderBookControl;
use types::multicast::MulticastSource;
use utils::market_name;
use std::sync::{Arc, Mutex};
use crossbeam::{channel};
use fix::engine::{FixRawMsg};
use memory;
use web::state::{
    EventBus, OrderBookState,
};
use config::{MarketConfig, MarketsConfig};
use std::sync::atomic::{AtomicBool};
use types::consts::RB_SIZE;


pub mod gateway;
pub mod startup;

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

pub struct MarketSimulator {
    config: MarketConfig,
    player_database_url: String,
    thread_handles: Arc<Mutex<ThreadHandles>>,
    shutdown: Option<Arc<AtomicBool>>,
    /// Multicast endpoints for order book snapshots to forward to GUI websockets.
    market_feed_sources: Vec<MulticastSource>,
    snapshot_feed_sources: Vec<MulticastSource>,
    /// All configured markets — passed to the web server for the login page.
    known_markets: Vec<web::MarketInfo>,
    // Error channel for threads to report startup errors back to main thread for logging.
    err_rx: crossbeam::channel::Receiver<String>,
    err_tx: Arc<crossbeam::channel::Sender<String>>,
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
    let snapshot_feed_sources = market_simulator.snapshot_feed_sources.clone();
    let bus = EventBus::new();
    let global_shutdown = Arc::new(AtomicBool::new(false));
    let order_book = Arc::new(std::sync::Mutex::new(OrderBookState::new()));
    let database_url = config.resolve_database_url().map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, e)
    })?;
    let player_store = web::players::PlayerStore::load_postgres(&player_database_url);

    // Initialization of shared queues for inter-thread communication
    let mut queues = QueueHandle::new(&config.name);

    let net_to_fix_tx = queues.net_to_fix_tx.as_ref().unwrap().clone();
    let (fix_tx, ob_rx) = queues.fix_to_ob.take().unwrap().queue.split();
    let (ob_er_tx, er_rx) = queues.ob_to_er.take().unwrap().queue.split();
    let (er_tx, fix_resp_rx) = queues.er_to_fix.take().unwrap().queue.split();
    let (ob_db_tx, ob_db_rx) = queues.ob_to_db.take().unwrap().queue.split();
    let (ob_md_tx, ob_md_rx) = queues.ob_to_md.take().unwrap().queue.split();
    let (ob_ss_tx, ob_ss_rx) = queues.ob_to_ss.take().unwrap().queue.split();

    // Order-book control channel (used by the gRPC reset service).
    let (ob_control_tx, ob_control_rx) = crossbeam_channel::bounded::<OrderBookControl>(32);
    let (err_tx, err_rx) = crossbeam_channel::bounded::<String>(32);
    market_simulator.err_tx = Arc::new(err_tx);
    market_simulator.err_rx = err_rx.clone();

    // Global shutdown flag is shared across all threads and can be set by the Ctrl-C handler to signal all threads to exit.
    market_simulator.shutdown = Some(Arc::clone(&global_shutdown));


    // Start the multicast receiver thread(s) for this market.  This thread will subscribe to the configured multicast sources for this market and push incoming market feed messages into the event bus, which other threads can subscribe to.
    startup::start_multicast_receiver(
        &mut market_simulator,
        bus.clone(),
        market_feed_sources.clone(),
        Arc::clone(&global_shutdown),
        Arc::clone(&order_book),
        player_store.clone(),
        config.core_mapping.market_feed_multicast_core,
    )?;
    
    // execution report engine thread
    startup::start_execution_report_engine(
        &mut market_simulator,
        er_rx,
        er_tx,
        Arc::clone(&global_shutdown),
        config.core_mapping.execution_report_core,
    )?;

    // DB engine thread
    let db_data = startup::start_db_engine(
        &mut market_simulator,
        ob_db_rx, database_url,
        Arc::clone(&global_shutdown),
        config.core_mapping.db_core
    )?;

    // Order book engine thread (must be started after the DB engine since it needs to import the initial order book state from the database before starting to process new orders/events).
    startup::start_order_book_engine(
        &mut market_simulator,
        ob_rx,
        ob_er_tx,
        ob_db_tx,
        ob_md_tx,
        ob_control_rx,
        ob_ss_tx,
        Arc::clone(&global_shutdown),
        db_data.pending_orders.clone(),
        config.snapshot.update_interval_ms,
        config.core_mapping.order_book_core,
        config.core_mapping.snapshot_core,
    )?;

    // Start Market proxy thread
    startup::start_market_data_proxy(
        &mut market_simulator,
        market_feed_sources[0].clone(),
        snapshot_feed_sources[0].clone(),
        Arc::clone(&global_shutdown),
        config.core_mapping.market_data_proxy_core,
            config.proxy.ip.clone(),
            config.proxy.port,
    )?;

    // Snapshot multicast engine thread
    startup::start_snapshot_multicast_engine(
        &mut market_simulator,
        ob_ss_rx,
        Arc::clone(&global_shutdown),
        config.snapshot_multicast.ip.clone(),
        config.snapshot_multicast.port,
        config.core_mapping.snapshot_multicast_core,
    )?;

    // Market data feed engine thread
    startup::start_market_feed_engine(
        &mut market_simulator,
        ob_md_rx,
        Arc::clone(&global_shutdown),
        config.market_feed_multicast.ip.clone(),
        config.market_feed_multicast.port,
        config.core_mapping.market_feed_core,
    )?;

    // FIX engine thread
    startup::start_fix_engine(
        &mut market_simulator,
        Arc::clone(&queues.net_to_fix_rx.as_ref().unwrap()),
        fix_tx,
        fix_resp_rx,
        Arc::clone(&global_shutdown),
        config.core_mapping.fix_inbound_core,
        config.core_mapping.fix_outbound_core,
    )?;

    // Web server thread
    startup::start_web_server(
        &mut market_simulator,
        Arc::clone(&order_book),
        player_database_url.clone(),    
        bus.clone(),
        Arc::clone(&global_shutdown),
        config.web.clone(),
        config.tcp.clone(),
        config.grpc.clone(),
        config.core_mapping.web_core,

    )?;

    // TCP server thread (FIX protocol)
    startup::start_tcp_server(
        &mut market_simulator,
        net_to_fix_tx.clone(),
        Arc::clone(&global_shutdown),
        config.tcp.clone(),
        config.core_mapping.tcp_core,
    )?;

    // Start the gRPC server in a separate thread, passing it the order book control channel and database pool.
    startup::start_grpc_server(
        &mut market_simulator,
        config.grpc.ip.clone(),
        config.grpc.port,
        ob_control_tx,
        Arc::clone(&db_data.pool),
        Arc::clone(&global_shutdown),
            config.core_mapping.global_core,
    )?;

    // Give the thread a moment to fail fast on init errors
    match err_rx.recv_timeout(std::time::Duration::from_millis(500)) {
        Ok(e) => return Err(Box::new(std::io::Error::new(std::io::ErrorKind::Other, e))),          // failed during startup
        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}  // still running, good
        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {} // thread exited cleanly (unlikely here)
    }

    tracing::info!("[{}] FIX server    -> {}:{}", utils::market_name(), config.tcp.ip, config.tcp.port);
    tracing::info!("[{}] Web terminal  -> http://{}:{}", utils::market_name(), config.web.ip, config.web.port);
    tracing::info!("[{}] gRPC control  -> {}:{}", utils::market_name(), config.grpc.ip, config.grpc.port);
    tracing::info!("[{}] Market feed   -> {}:{}", utils::market_name(), config.market_feed_multicast.ip, config.market_feed_multicast.port);
    tracing::info!("[{}] Snapshot feed -> {}:{}", utils::market_name(), config.snapshot_multicast.ip, config.snapshot_multicast.port);
    tracing::info!("[{}] Market proxy  -> {}:{}", utils::market_name(), config.proxy.ip, config.proxy.port);

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
            market_config.market_feed_multicast.ip.clone(),
            market_config.market_feed_multicast.port,
            market_name(),
        )];

        let snapshot_feed_sources = vec![MulticastSource::new(
            market_config.snapshot_multicast.ip.clone(),
            market_config.snapshot_multicast.port,
            market_name(),
        )];

        let (err_tx, err_rx) = crossbeam_channel::bounded::<String>(32);
        let err_tx = Arc::new(err_tx);
    
        let simulator = Arc::new(Mutex::new(MarketSimulator {
            config: market_config,
            player_database_url,
            thread_handles: Arc::new(Mutex::new(ThreadHandles::new())),
            shutdown: None,
            market_feed_sources,
            snapshot_feed_sources,
            known_markets,
            err_tx,
            err_rx,
        }));

        if let Err(e) = start_market(Arc::clone(&simulator)) {
            tracing::error!("Market failed to start: {e}");
            stop_market(Arc::clone(&simulator));
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
            gateway::run_login_gateway(gateway_markets, &login_ip, login_port);
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