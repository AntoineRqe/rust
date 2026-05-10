use config::Connection;
use execution_report::{ExecutionReportEngine};
use backend::state::EventBus;
use backend::order_book::OrderBookState;

use std::sync::{Arc, Mutex, atomic::{AtomicBool}};
use types::{OrderEvent, OrderResult};
use types::consts::RB_SIZE;
use types::EntityId;
use fix::engine::FixRawMsg;

use utils::market_name;
use order_book::OrderBookControl;

// ---------------- Execution Report Engine ----------------
pub fn start_execution_report_engine(
    simulator: &mut crate::MarketSimulator,
    er_rx: spsc::Consumer<'static, (OrderEvent, OrderResult), RB_SIZE>,
    er_tx: spsc::Producer<'static, (EntityId, FixRawMsg<RB_SIZE>), RB_SIZE>,
    shutdown: Arc<AtomicBool>,
    core_id: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let execution_report_engine = ExecutionReportEngine::new(er_rx, er_tx, Arc::clone(&shutdown));

    let err_tx = Arc::clone(&simulator.err_tx);

    let _er_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: core_id });
        match execution_report_engine.run() {
            Ok(_) => (),
            Err(e) => {
                tracing::error!("[{}] Execution report engine error: {e:#}", utils::market_name());
                let _ = err_tx.send(format!("Execution report engine error: {e:#}"));
            }
        }
    });

    simulator.add_thread_handle(_er_thread);

    Ok(())
}

// ---------------- Database Engine ----------------
pub struct DbData {
    pub pending_orders: Vec<OrderEvent>,
    pub pool: Arc<sqlx::Pool<sqlx::Postgres>>,

}

pub fn start_db_engine(
    market_simulator: &mut crate::MarketSimulator,
    ob_db_rx: spsc::Consumer<'static, (OrderEvent, OrderResult), RB_SIZE>,
    database_url: String,
    global_shutdown: Arc<AtomicBool>,
    core_id: usize,
) -> Result<DbData, Box<dyn std::error::Error>> {


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

    let mut db_data = DbData {
        pending_orders: Vec::new(), // This will be populated after we start the DB engine thread and it loads the pending orders from the database.
        pool: db_engine.pool(),
    };

    db_data.pending_orders = match db_engine.get_all_pending_orders() {
        Ok(orders) => {
            tracing::info!("[{}] Loaded {} pending orders from database", market_name(), orders.len());
            orders
        },
        Err(e) => {
            return Err(Box::new(e));
        }
    };

    db_data.pool = db_engine.pool();


    let err_tx = Arc::clone(&market_simulator.err_tx);

    let _db_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: core_id });
        match db_engine.run() {
            Ok(_) => (),
            Err(e) => {
                tracing::error!("[{}] Database engine error: {e:#}", utils::market_name());
                let _ = err_tx.send(format!("Database engine error: {e:#}"));
            }
        }
    });

    {
        market_simulator.add_thread_handle(_db_thread);
    }
    Ok(db_data)
}

// ---------------- gRPC Server ----------------
pub fn start_grpc_server(
    market_simulator: &mut crate::MarketSimulator,
    ip: String,
    port: u16,
    ob_control_tx: crossbeam_channel::Sender<OrderBookControl>,
    db_pool: Arc<sqlx::Pool<sqlx::Postgres>>,
    global_shutdown: Arc<AtomicBool>,
    core_id: usize,
) -> Result<(), Box<dyn std::error::Error>> {
// gRPC MarketControl server — handles ResetMarket (order book + DB).
    let grpc_ip   = ip;
    let grpc_port = port;
    let grpc_shutdown = Arc::clone(&global_shutdown);
    let grpc_service = grpc::MarketControlService::new(ob_control_tx, db_pool);
    let err_tx = Arc::clone(&market_simulator.err_tx);
    let _grpc_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: core_id });
        match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .on_thread_start(move || {
                core_affinity::set_for_current(core_affinity::CoreId { id: core_id });
            })
            .enable_all()
            .build()
        {
            Ok(rt) => {
                let addr: std::net::SocketAddr = match format!("{grpc_ip}:{grpc_port}").parse() {
                    Ok(addr) => addr,
                    Err(e) => {
                        tracing::error!("Failed to parse gRPC server address: {e:#}");
                        let _ = err_tx.send(format!("Failed to parse gRPC server address: {e:#}"));
                        return;
                    }
                };
                    
                if let Err(e) = rt.block_on(grpc::serve(addr, grpc_service, grpc_shutdown)) {
                    tracing::error!("[{}] gRPC server error: {e:#}", utils::market_name());
                    let _ = err_tx.send(format!("gRPC server error: {e:#}"));
                }
            }
            Err(e) => {
                tracing::error!("[{}] Failed to build tokio runtime for gRPC server: {e:#}", utils::market_name());
                let _ = err_tx.send(format!("Failed to build tokio runtime for gRPC server: {e:#}"));
            }
        }
    });

    market_simulator.add_thread_handle(_grpc_thread);

    Ok(())
}


// Inbound + Outbound FIX engine
pub fn start_fix_engine(
    market_simulator: &mut crate::MarketSimulator,
    fix_rx: Arc<crossbeam_channel::Receiver<FixRawMsg<RB_SIZE>>>,
    fix_tx: spsc::Producer<'static, OrderEvent, RB_SIZE>,
    fix_resp_rx: spsc::Consumer<'static, (EntityId, FixRawMsg<RB_SIZE>), RB_SIZE>,
    global_shutdown: Arc<AtomicBool>,
    inbound_core_id: usize,
    outbound_core_id: usize,
) -> Result<(), Box<dyn std::error::Error>> {
        // fix engine thread
    let fix_engine = fix::engine::FixEngine ::new(
        fix_rx,
        // Arc::clone(&queues.net_to_fix_rx.as_ref().unwrap()),
        fix_tx,
        fix_resp_rx,
        Arc::clone(&global_shutdown)
    );

    let (mut inbound_engine, mut outbound_engine) = fix_engine.split();

    let err_tx = Arc::clone(&market_simulator.err_tx);
    let _fix_inbound_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: inbound_core_id });
        if let Err(e) = inbound_engine.run() {
            tracing::error!("[{}] Inbound FIX engine error: {e:#}", utils::market_name());
            let _ = err_tx.send(format!("Inbound FIX engine error: {e:#}"));
        }
    });

    let err_tx = Arc::clone(&market_simulator.err_tx);

    let _fix_outbound_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: outbound_core_id });
        if let Err(e) = outbound_engine.run() {
            tracing::error!("[{}] Outbound FIX engine error: {e:#}", utils::market_name());
            let _ = err_tx.send(format!("Outbound FIX engine error: {e:#}"));
        }
    });

    market_simulator.add_thread_handle(_fix_inbound_thread);
    market_simulator.add_thread_handle(_fix_outbound_thread);
    Ok(())
}

// ---------------- Order Book ----------------
pub fn start_order_book_engine(
    market_simulator: &mut crate::MarketSimulator,
    ob_rx: spsc::Consumer<'static, OrderEvent, RB_SIZE>,
    ob_er_tx: spsc::Producer<'static, (OrderEvent, OrderResult), RB_SIZE>,
    ob_db_tx: spsc::Producer<'static, (OrderEvent, OrderResult), RB_SIZE>,
    ob_control_rx: crossbeam::channel::Receiver<OrderBookControl>,
    metrics: Arc<backend::server::Metrics>,
    global_shutdown: Arc<AtomicBool>,
    pending_orders: Vec<OrderEvent>,
    order_book_core_id: usize,
) -> Result<(), Box<dyn std::error::Error>> {

    // TODO: Handle multiple symbols per market
    let symbols = vec!["AAAPL"]; 

    for symbol in symbols {
        tracing::info!("[{}] Initializing market for symbol '{}'", market_name(), symbol);

        // Create shared order book instance.
        let order_book = order_book::book::OrderBook::new(&symbol);

        // Book engine thread
        let mut order_book_engine = order_book::engine::OrderBookEngine::new(
            ob_rx,
            
            Some(ob_er_tx),
            None,
            Some(ob_db_tx),
            ob_control_rx,
            order_book,
            None,
            Arc::clone(&global_shutdown)
        );
        order_book_engine.set_metrics(Arc::clone(&metrics));

        // Import initial order book state from the database before starting the engine.
        // This ensures that the engine starts with the correct state and can process new orders/events in the context of existing pending orders.
        order_book_engine.import_order_book(pending_orders);

        let err_tx = Arc::clone(&market_simulator.err_tx);

        let _ob_thread = std::thread::spawn(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: order_book_core_id });
            match order_book_engine.run() {
                Ok(_) => (),
                Err(e) => {
                    tracing::error!("[{}] Order book engine error: {e:#}", utils::market_name());
                    let _ = err_tx.send(format!("Order book engine error: {e:#}"));
                }
            }
        });

        {
            market_simulator.add_thread_handle(_ob_thread);
        }

        // TODO : Handle multiple symbols per market (currently we just hardcode one symbol and ignore the symbol field in the orders/events, but in a real implementation we'd want to support multiple symbols per market and route orders/events to the correct order book based on the symbol).
        break;
    }

    Ok(())
}

// ---------------- Web Server ----------------
pub fn start_web_server(
    market_simulator: &mut crate::MarketSimulator,
    order_book: Arc<Mutex<OrderBookState>>,
    market_database_url: String,
    bus: EventBus,
    metrics: Arc<backend::server::Metrics>,
    global_shutdown: Arc<AtomicBool>,
    web_addr: Connection,
    fix_tx: Arc<crossbeam_channel::Sender<FixRawMsg<RB_SIZE>>>,
    grpc_addr: Connection,
    player_service_addr: String,
    core_id: usize,
) -> Result<(), Box<dyn std::error::Error>> {

    let _web_thread = std::thread::spawn({
        let bus = bus.clone();
        let fix_tx = fix_tx.clone();
        let grpc_addr    = format!("http://127.0.0.1:{}", grpc_addr.port);
        let web_ip = web_addr.ip.clone();
        let web_port = web_addr.port;
        let web_market_database_url = market_database_url.clone();
        let metrics = Arc::clone(&metrics);
        let web_shutdown = Arc::clone(&global_shutdown);
        let order_book = Arc::clone(&order_book);
        let err_tx = Arc::clone(&market_simulator.err_tx);

        move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: core_id });
            match backend::run_web_server(
                bus,
                fix_tx,
                metrics,
                grpc_addr,
                &web_ip,
                web_port,
                web_market_database_url,
                web_shutdown,
                order_book,
                player_service_addr,
                core_id
            ) {
                Ok(_) => (),
                Err(e) => {
                    tracing::error!("[{}] Web server error: {e:#}", utils::market_name());
                    let _ = err_tx.send(format!("[{}] Web server error: {e:#}", utils::market_name()));
                }
            }
        }
    });

    market_simulator.add_thread_handle(_web_thread);

    Ok(())
}
