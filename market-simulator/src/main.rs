use backend::order_book::OrderBookState;
use backend::state::EventBus;
use clap::Parser;
use config::{MarketConfig, SingleMarketConfig};
use crossbeam::channel;
use fix::engine::FixRawMsg;
use memory;
use order_book::OrderBookControl;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use types::consts::RB_SIZE;
use types::macros::EntityId;
use types::{OrderEvent, OrderResult};
use utils::market_name;

pub mod startup;

#[derive(Parser, Debug)]
#[command(name = "market-simulator")]
struct Cli {
    #[arg(
        short = 'c',
        long = "config",
        default_value = "crates/config/markets/nasdaq.json"
    )]
    config_file: String,
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
            handle
                .join()
                .expect(&format!("Failed to join thread {}", name));
        }
    }
}

pub struct MarketSimulator {
    config: MarketConfig,
    player_service_addr: String,
    thread_handles: Arc<Mutex<ThreadHandles>>,
    shutdown: Option<Arc<AtomicBool>>,
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
    er_to_fix: Option<memory::SharedQueue<RB_SIZE, (EntityId, FixRawMsg<RB_SIZE>)>>,
}

impl QueueHandle {
    fn new(market_name: &str) -> Self {
        let (net_to_fix_tx, net_to_fix_rx) = channel::bounded::<FixRawMsg<RB_SIZE>>(RB_SIZE);
        let fix_to_ob = memory::open_shared_queue::<RB_SIZE, OrderEvent>(
            &format!("{market_name}_fix_to_order_book"),
            true,
        );
        let ob_to_er = memory::open_shared_queue::<RB_SIZE, (OrderEvent, OrderResult)>(
            &format!("{market_name}_order_book_to_execution_report"),
            true,
        );
        let ob_to_db = memory::open_shared_queue::<RB_SIZE, (OrderEvent, OrderResult)>(
            &format!("{market_name}_order_book_to_db"),
            true,
        );
        let er_to_fix = memory::open_shared_queue::<RB_SIZE, (EntityId, FixRawMsg<RB_SIZE>)>(
            &format!("{market_name}_execution_report_to_fix"),
            true,
        );

        Self {
            net_to_fix_tx: Some(Arc::new(net_to_fix_tx)),
            net_to_fix_rx: Some(Arc::new(net_to_fix_rx)),
            fix_to_ob: Some(fix_to_ob),
            ob_to_er: Some(ob_to_er),
            ob_to_db: Some(ob_to_db),
            er_to_fix: Some(er_to_fix),
        }
    }
}

fn start_market(
    market_simulator: Arc<Mutex<MarketSimulator>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut market_simulator = market_simulator.lock().unwrap();

    let config = market_simulator.config.clone();
    let player_service_addr = market_simulator.player_service_addr.clone();
    let bus = EventBus::new();
    let global_shutdown = Arc::new(AtomicBool::new(false));
    let order_book = Arc::new(std::sync::Mutex::new(OrderBookState::new()));
    let database_url = config
        .resolve_database_url()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    // Initialization of shared queues for inter-thread communication
    let mut queues = QueueHandle::new(&config.name);

    let net_to_fix_tx = queues.net_to_fix_tx.as_ref().unwrap().clone();
    let (fix_tx, ob_rx) = queues.fix_to_ob.take().unwrap().queue.split();
    let (ob_er_tx, er_rx) = queues.ob_to_er.take().unwrap().queue.split();
    let (er_tx, fix_resp_rx) = queues.er_to_fix.take().unwrap().queue.split();
    let (ob_db_tx, ob_db_rx) = queues.ob_to_db.take().unwrap().queue.split();

    // Order-book control channel (used by the gRPC reset service).
    let (ob_control_tx, ob_control_rx) = crossbeam_channel::bounded::<OrderBookControl>(32);
    let (err_tx, err_rx) = crossbeam_channel::bounded::<String>(32);
    market_simulator.err_tx = Arc::new(err_tx);
    market_simulator.err_rx = err_rx.clone();

    // Global shutdown flag is shared across all threads and can be set by the Ctrl-C handler to signal all threads to exit.
    market_simulator.shutdown = Some(Arc::clone(&global_shutdown));

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
        ob_db_rx,
        database_url.clone(),
        Arc::clone(&global_shutdown),
        config.core_mapping.db_core,
    )?;

    // Order book engine thread (must be started after the DB engine since it needs to import the initial order book state from the database before starting to process new orders/events).
    startup::start_order_book_engine(
        &mut market_simulator,
        ob_rx,
        ob_er_tx,
        ob_db_tx,
        ob_control_rx,
        Arc::clone(&global_shutdown),
        db_data.pending_orders.clone(),
        config.core_mapping.order_book_core,
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
        database_url.clone(),
        bus.clone(),
        Arc::clone(&global_shutdown),
        config.web.clone(),
        net_to_fix_tx.clone(),
        config.grpc.clone(),
        player_service_addr,
        config.core_mapping.web_core,
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
        Ok(e) => return Err(Box::new(std::io::Error::new(std::io::ErrorKind::Other, e))), // failed during startup
        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {} // still running, good
        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {} // thread exited cleanly (unlikely here)
    }

    tracing::info!(
        "[{}] Web terminal  -> http://{}:{}",
        utils::market_name(),
        config.web.ip,
        config.web.port
    );
    tracing::info!(
        "[{}] gRPC control  -> {}:{}",
        utils::market_name(),
        config.grpc.ip,
        config.grpc.port
    );
    tracing::info!(
        "[{}] Market proxy  -> {}:{}",
        utils::market_name(),
        config.proxy.ip,
        config.proxy.port
    );

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

    tracing::info!(
        "[{}] All threads stopped, market simulator exiting",
        market_name()
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let config = SingleMarketConfig::parse_from_file(&cli.config_file);
    utils::set_market_name(&config.market.name);

    tracing_subscriber::fmt()
        .with_env_filter("debug,sqlx=warn,h2=warn,tokio_util=warn")
        .init();

    let market_config = config.market.clone();
    let (err_tx, err_rx) = crossbeam_channel::bounded::<String>(32);
    let err_tx = Arc::new(err_tx);

    let player_service_addr = format!(
        "http://{}:{}",
        config.players_service.grpc.ip, config.players_service.grpc.port
    );

    let simulator = Arc::new(Mutex::new(MarketSimulator {
        config: market_config,
        player_service_addr,
        thread_handles: Arc::new(Mutex::new(ThreadHandles::new())),
        shutdown: None,
        err_tx,
        err_rx,
    }));

    match start_market(Arc::clone(&simulator)) {
        Ok(_) => {}
        Err(e) => {
            tracing::error!("Market failed to start: {e}");
            stop_market(Arc::clone(&simulator));
            return Err(e);
        }
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
