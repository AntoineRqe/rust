use config::Connection;
use execution_report::{ExecutionReportEngine};
use server::multicast::{spawn_market_feed_receiver};
use web::state::EventBus;
use web::order_book::OrderBookState;

use types::multicast::MulticastSource;
use std::sync::{Arc, Mutex, atomic::{AtomicBool}};
use types::{OrderEvent, OrderResult};
use types::consts::RB_SIZE;
use types::EntityId;
use fix::engine::FixRawMsg;

use utils::market_name;
use order_book::OrderBookControl;
use market_feed::engine::MarketDataFeedEngine;



// ---------------- Multicast Receiver ----------------
pub fn start_multicast_receiver(
    simulator: &mut crate::MarketSimulator,
    bus: EventBus,
    sources: Vec<MulticastSource>,
    shutdown: Arc<AtomicBool>,
    order_book: Arc<Mutex<OrderBookState>>,
    player_store: web::players::PlayerStore,
    core_id: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    
    let err_tx = Arc::clone(&simulator.err_tx);

    let _receiver_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: core_id });
        for source in sources {
            let err_tx = Arc::clone(&err_tx);
            if let Err(e) = spawn_market_feed_receiver(bus.clone(), vec![source], Arc::clone(&shutdown), Arc::clone(&order_book), player_store.clone()) {
                tracing::error!("[{}] Market feed receiver error: {e:#}", utils::market_name());
                let _ = err_tx.send(format!("Market feed receiver error: {e:#}"));
            }
        }
    });

    simulator.add_thread_handle(_receiver_thread);

    Ok(())
}

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

// ---------------- Market Data Feed Engine ----------------
pub fn start_market_feed_engine(
    market_simulator: &mut crate::MarketSimulator,
    ob_md_rx: spsc::Consumer<'static, (OrderEvent, OrderResult), RB_SIZE>,
    global_shutdown: Arc<AtomicBool>,
    ip: String,
    port: u16,
    core_id: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    // Market feed engine thread
    let mut market_feed_engine = MarketDataFeedEngine::new(
        ob_md_rx,
        Arc::clone(&global_shutdown),
        ip,
        port,
    ).expect("Failed to create MarketDataFeedEngine");

    let err_tx = Arc::clone(&market_simulator.err_tx);

    let _market_feed_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: core_id });
        match market_feed_engine.run() {
            Ok(_) => (),
            Err(e) => {
                tracing::error!("[{}] Market data feed engine error: {e:#}", utils::market_name());
                let _ = err_tx.send(format!("Market data feed engine error: {e:#}"));
            }
        }
    });

    market_simulator.add_thread_handle(_market_feed_thread);
    
    Ok(())
}

// ---------------- Snapshot MultiCast Engine ----------------
pub fn start_snapshot_multicast_engine(
    market_simulator: &mut crate::MarketSimulator,
    snapshot_rx: spsc::Consumer<'static, Arc<snapshot::types::Snapshot>, RB_SIZE>,
    global_shutdown: Arc<AtomicBool>,
    ip: String,
    port: u16,
    core_id: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    // Snapshot multicast engine thread
        let mut snapshot_engine = snapshot::engine::SnapshotMultiCastEngine::new(
        snapshot_rx,
        Arc::clone(&global_shutdown),
        ip,
        port,
    );

    let err_tx = Arc::clone(&market_simulator.err_tx);

    let _snapshot_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: core_id });
        match snapshot_engine.run() {
            Ok(_) => (),
            Err(e) => {
                tracing::error!("[{}] Snapshot multicast engine error: {e:#}", utils::market_name());
                let _ = err_tx.send(format!("Snapshot multicast engine error: {e:#}"));
            }
        }
    });

    {
        market_simulator.add_thread_handle(_snapshot_thread);
    }
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
    ob_md_tx: spsc::Producer<'static, (OrderEvent, OrderResult), RB_SIZE>,
    ob_control_rx: crossbeam::channel::Receiver<OrderBookControl>,
    ob_ss_tx: spsc::Producer<'static, Arc<snapshot::types::Snapshot>, RB_SIZE>,
    global_shutdown: Arc<AtomicBool>,
    pending_orders: Vec<OrderEvent>,
    snapshot_interval_ms: u64,
    order_book_core_id: usize,
    snapshot_generation_core_id: usize,
) -> Result<(), Box<dyn std::error::Error>> {

    // TODO: Handle multiple symbols per market
    let symbols = vec!["AAAPL"]; 

    for symbol in symbols {
        tracing::info!("[{}] Initializing market for symbol '{}'", market_name(), symbol);

        // Create shared order book instance and pass it to the order book engine and snapshot generation engine so they can read/write it without going through the queues.
        let order_book = order_book::book::OrderBook::new(&symbol);
        let snapshot_ptr = Arc::new(arc_swap::ArcSwap::from_pointee(snapshot::types::Snapshot {
            symbol: symbol.to_string(),
            ..Default::default()
        }));

        // Book engine thread
        let mut order_book_engine = order_book::engine::OrderBookEngine::new(
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

        // Snapshot generation thread (reads from order book and pushes to multicast engine)
        let snapshot_generation_engine = order_book::snapshot::SnapshotGenerationEngine::new(
            ob_ss_tx, 
            Arc::clone(&global_shutdown),
            Arc::clone(&snapshot_ptr),
            snapshot_interval_ms,
        );

        let err_tx = Arc::clone(&market_simulator.err_tx);

        let _snapshot_generation_thread = std::thread::spawn(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: snapshot_generation_core_id });
            match snapshot_generation_engine.run() {
                Ok(_) => (),
                Err(e) => {
                    tracing::error!("[{}] Snapshot generation engine error: {e:#}", utils::market_name());
                    let _ = err_tx.send(format!("Snapshot generation engine error: {e:#}"));
                }
            }
        });

        {
            market_simulator.add_thread_handle(_snapshot_generation_thread);
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
    player_database_url: String,
    bus: EventBus,
    global_shutdown: Arc<AtomicBool>,
    web_addr: Connection,
    tcp_addr: Connection,
    grpc_addr: Connection,
    core_id: usize,
) -> Result<(), Box<dyn std::error::Error>> {

    let _web_thread = std::thread::spawn({
        let bus = bus.clone();
        let fix_tcp_addr = format!("{}:{}", tcp_addr.ip, tcp_addr.port);
        let grpc_addr    = format!("http://127.0.0.1:{}", grpc_addr.port);
        let web_ip = web_addr.ip.clone();
        let web_port = web_addr.port;
        let web_database_url = player_database_url.clone();
        let web_shutdown = Arc::clone(&global_shutdown);
        let known_markets = market_simulator.known_markets.clone();
        let order_book = Arc::clone(&order_book);
        let err_tx = Arc::clone(&market_simulator.err_tx);

        move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: core_id });
            match web::run_web_server(
                bus,
                fix_tcp_addr,
                grpc_addr,
                &web_ip,
                web_port,
                web_database_url,
                known_markets,
                web_shutdown,
                order_book,
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

// ---------------- TCP server ----------------
pub fn start_tcp_server(
    market_simulator: &mut crate::MarketSimulator,
    fix_tx: Arc<crossbeam_channel::Sender<FixRawMsg<RB_SIZE>>>,
    global_shutdown: Arc<AtomicBool>,
    tcp_addr: Connection,
    core_id: usize,
) -> Result<(), Box<dyn std::error::Error>> {

    let server: server::tcp::FixServer<RB_SIZE> = server::tcp::FixServer::new(fix_tx, Arc::clone(&global_shutdown));
    let listener = match std::net::TcpListener::bind(format!("{}:{}", tcp_addr.ip, tcp_addr.port)) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("[{}] Failed to bind TCP server to {}:{} - {e:#}", utils::market_name(), tcp_addr.ip, tcp_addr.port);
            return Err(Box::new(e));
        }
    };

    // Grab the shutdown flag before releasing the lock so the Ctrl-C handler
    // can signal the accept loop without holding any other lock.

    let err_tx = Arc::clone(&market_simulator.err_tx);

    let _tcp_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: core_id });
        match server.accept_loop(listener, vec![core_id]) {
            Ok(_) => (),
            Err(e) => {
                tracing::error!("[{}] TCP server error: {e:#}", utils::market_name());
                let _ = err_tx.send(format!("TCP server error: {e:#}"));
            }
        }
    });

    market_simulator.add_thread_handle(_tcp_thread);

    Ok(())
}

// ---------------- Market Data Proxy ----------------
pub fn start_market_data_proxy(
    market_simulator: &mut crate::MarketSimulator,
    market_feed_source: types::multicast::MulticastSource,
    snapshot_feed_source: types::multicast::MulticastSource,
    shutdown: Arc<AtomicBool>,
    core_id: usize,
    ws_ip: String,
    ws_port: u16,

) -> Result<(), Box<dyn std::error::Error>> {

    let mut proxy = proxy::MarketDataProxy::new(
        market_feed_source,
        snapshot_feed_source,
        Arc::clone(&shutdown),
        core_id,
        ws_ip,
        ws_port,
    );

    let err_tx = Arc::clone(&market_simulator.err_tx);

    let proxy_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: core_id });
        // TODO : Handle errors from the proxy and propagate them back to the main thread so we can log them and shut down gracefully if the proxy fails (currently if the market data proxy encounters an error, it will just panic and crash the thread, which is not ideal).
        match proxy.run() {
            Ok(_) => (),
            Err(e) => {
                tracing::error!("[{}] Market data proxy error: {e:#}", utils::market_name());
                let _ = err_tx.send(format!("Market data proxy error: {e:#}"));
            }
        }
    });

    market_simulator.add_thread_handle(proxy_thread);

    Ok(())
}