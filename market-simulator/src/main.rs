use types::{EntityId, OrderEvent, OrderResult};
use order_book::order_book::{OrderBookEngine};
use server::tcp::{
    FixServer,
    pretty_fix,
    classify_fix_msg,
};
use std::sync::{Arc, Mutex};
use crossbeam::{channel};
use fix::engine::{FixEngine, FixRawMsg};
use memory;
use execution_report::{ExecutionReportEngine};
use std::net::TcpListener;
use web::state::{
    EventBus,
    WsEvent,
};
use web::server::run_web_server;


const RB_SIZE: usize = 1024;

struct ThreadHandles {
    fix_thread: Option<std::thread::JoinHandle<()>>,
    ob_thread: Option<std::thread::JoinHandle<()>>,
    er_thread: Option<std::thread::JoinHandle<()>>,
}

struct MarketSimulator {
    thread_handles: Arc<Mutex<ThreadHandles>>,
    entry_point: Option<Arc<channel::Sender<FixRawMsg<RB_SIZE>>>>,
}

fn start_market(market_simulator: Arc<Mutex<MarketSimulator>>) {

    let mut market_simulator = market_simulator.lock().unwrap();

    let bus = EventBus::new();

    // inbound: network → fix engine → order book -> exection report
    let (net_to_fix_tx, net_to_fix_rx) = channel::bounded::<FixRawMsg<RB_SIZE>>(RB_SIZE);
    let fix_to_ob    = memory::open_shared_queue::<RB_SIZE, OrderEvent>("fix_to_order_book", true);
    let ob_to_er     = memory::open_shared_queue::<RB_SIZE, (OrderEvent, OrderResult)>("order_book_to_execution_report", true);

    // outbound: execution report → fix engine → network
    let er_to_fix     = memory::open_shared_queue::<RB_SIZE, (EntityId, FixRawMsg<RB_SIZE>)>("execution_report_to_fix", true);

    let (fix_tx, ob_rx) = fix_to_ob.queue.split();
    let (ob_tx, er_rx) = ob_to_er.queue.split();
    let (er_tx, fix_resp_rx) = er_to_fix.queue.split();

    // execution report engine thread
    let execution_report_engine = ExecutionReportEngine::new(er_rx, er_tx);

    let _er_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: 2 });
        execution_report_engine.run();
    });

    {
        market_simulator.thread_handles.lock().unwrap().er_thread = Some(_er_thread);
    }

    // Book engine thread
    let mut order_book_engine = OrderBookEngine::new(ob_rx, ob_tx);

    let _ob_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: 4 });
        order_book_engine.run();
    });

    {
        market_simulator.thread_handles.lock().unwrap().ob_thread = Some(_ob_thread);
    }

    // fix engine thread
    let fix_engine = FixEngine ::new(Arc::new(net_to_fix_rx), fix_tx, fix_resp_rx);
    let (mut inbound_engine, mut outbound_engine) = fix_engine.split();

    let _fix_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: 6 });
        inbound_engine.run();
    });

    let _fix_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: 7 });
        outbound_engine.run();
    });

    {
        market_simulator.thread_handles.lock().unwrap().fix_thread = Some(_fix_thread);
    }

    let net_to_fix_tx = Arc::new(net_to_fix_tx);
    // Bind Fix entry point to global struct so it can be accessed by the TCP server
    market_simulator.entry_point = Some(Arc::clone(&net_to_fix_tx));

    
    let net_to_fix_tx_web = Arc::clone(&net_to_fix_tx);
    let bus_for_sender = bus.clone();
    // Build the closure that converts raw bytes into a FixRawMsg
    // and injects it into the engine — same path as tcp.rs
    let fix_sender: web::server::FixSender = Arc::new(move |bytes: Vec<u8>| {
        let (response_tx, response_rx) = crossbeam_channel::unbounded();

        let n = bytes.len().min(RB_SIZE);
        let mut msg = FixRawMsg::default();
        msg.len = n as u16;
        msg.data[..n].copy_from_slice(&bytes[..n]);
        msg.resp_queue = Some(response_tx);
        
        if let Err(e) = net_to_fix_tx_web.send(msg) {
            tracing::warn!("Browser order injection failed: {e}");
        }

        // Spawn a thread to collect the response and publish to browser
        let bus = bus_for_sender.clone();
        std::thread::spawn(move || {
            // collect all responses for this order
            // (could be multiple exec reports for partial fills)
            loop {
                match response_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    Ok(response) => {
                        let body  = pretty_fix(&response.data[..response.len as usize]);
                        let label = classify_fix_msg(&response.data[..response.len as usize]);
                        bus.publish(WsEvent::FixMessage {
                            label,
                            body,
                            tag: "feed".into(),
                        });
                        // keep listening for more responses (partial fills)
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                        // no more responses — done
                        break;
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        break;
                    }
                }
            }
        });
    });

    // Start the web server in a separate thread, passing it the event bus
    std::thread::spawn({
        core_affinity::set_for_current(core_affinity::CoreId { id: 0 });
        let bus = bus.clone();
        move || {
            run_web_server(bus, fix_sender, 7654, std::path::PathBuf::from("players.json"));
        }
    });


    core_affinity::set_for_current(core_affinity::CoreId { id: 0 });
    // tcp server — each client pushes directly into fifo_in
    let server: FixServer<RB_SIZE> = FixServer::new(Arc::clone(&net_to_fix_tx), bus.clone());
    let listener = TcpListener::bind("127.0.0.1:9876").unwrap();

    drop(market_simulator); // Release the lock before starting the server loop

    println!("FIX server    -> localhost:9876");
    println!("Web terminal  -> http://localhost:7654");

    server.accept_loop(listener);
}

fn stop_market(market_simulator: Arc<Mutex<MarketSimulator>>) {
    let market_simulator = market_simulator.lock().unwrap();

    // Send a shutdown message to the FIX engine to unblock it if it's waiting on the queue, in a real implementation you would want a more robust way to ensure the thread has stopped
    if let Some(entry_point) = &market_simulator.entry_point {
        entry_point.send(FixRawMsg::default()).expect("Failed to push shutdown message");
    }

    let thread_handles = &mut market_simulator.thread_handles.lock().unwrap();

    if let Some(handle) = thread_handles.ob_thread.take() {
        handle.join().expect("Failed to join Order Book engine thread");
    }
    if let Some(handle) = thread_handles.er_thread.take() {
        handle.join().expect("Failed to join Execution Report engine thread");
    }
    if let Some(handle) = thread_handles.fix_thread.take() {
        handle.join().expect("Failed to join FIX engine thread");
    }


}

fn main() {


    tracing_subscriber::fmt()
        .with_env_filter("info,web=debug,server=debug,fix=debug,order_book=debug,execution_report=debug")
        .init();

    let market_simulator = Arc::new(Mutex::new(MarketSimulator {
        thread_handles: Arc::new(Mutex::new(ThreadHandles {
            fix_thread: None,
            ob_thread: None,
            er_thread: None,
        })),
        entry_point: None,
    }));

    let market_simulator_clone = Arc::clone(&market_simulator);

     std::thread::spawn(move || {
         core_affinity::set_for_current(core_affinity::CoreId { id: 0 });
         start_market(market_simulator_clone);
     });
    
    // Add the CTRL-C handler to stop the market simulator gracefully
    ctrlc::set_handler( move || {
        stop_market(Arc::clone(&market_simulator));
        std::process::exit(0);
    }).expect("Error setting Ctrl-C handler");

    loop {
        std::thread::park();
    }

}