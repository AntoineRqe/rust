use types::{EntityId, OrderEvent, OrderResult};
use order_book::order_book::{OrderBookEngine};
use server::tcp::FixServer;
use std::sync::{Arc, Mutex};
use crossbeam::{channel};
use fix::engine::{FixEngine, FixRawMsg};
use memory;
use execution_report::{ExecutionReportEngine};
use std::net::TcpListener;
use types::StopHandle;

const RB_SIZE: usize = 1024;

struct StopHandles {
    stop_fix: Option<StopHandle>,
    stop_ob: Option<StopHandle>,
    stop_er: Option<StopHandle>,
}

struct ThreadHandles {
    fix_thread: Option<std::thread::JoinHandle<()>>,
    ob_thread: Option<std::thread::JoinHandle<()>>,
    er_thread: Option<std::thread::JoinHandle<()>>,
}

struct MarketSimulator {
    stop_handles: Arc<Mutex<StopHandles>>,
    thread_handles: Arc<Mutex<ThreadHandles>>,
    entry_point: Option<Arc<channel::Sender<FixRawMsg<RB_SIZE>>>>,
}

fn start_market(market_simulator: Arc<Mutex<MarketSimulator>>) {

    let mut market_simulator = market_simulator.lock().unwrap();

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
    let er_stop_handle = execution_report_engine.stop_handle();

    let _er_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: 2 });
        execution_report_engine.run();
    });

    {
        market_simulator.stop_handles.lock().unwrap().stop_er = Some(er_stop_handle);
        market_simulator.thread_handles.lock().unwrap().er_thread = Some(_er_thread);
    }

    // Book engine thread
    let mut order_book_engine = OrderBookEngine::new(ob_rx, ob_tx);
    let ob_stop_handle = order_book_engine.stop_handle();

    let _ob_thread = std::thread::spawn(move || {
        core_affinity::set_for_current(core_affinity::CoreId { id: 4 });
        order_book_engine.run();
    });

    {
        market_simulator.stop_handles.lock().unwrap().stop_ob = Some(ob_stop_handle);
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

    
    core_affinity::set_for_current(core_affinity::CoreId { id: 0 });
    // tcp server — each client pushes directly into fifo_in
    let server: FixServer<RB_SIZE> = FixServer::new(Arc::clone(&net_to_fix_tx));
    let listener = TcpListener::bind("127.0.0.1:9876").unwrap();

    drop(market_simulator); // Release the lock before starting the server loop

    println!("Market simulator is running. Listening for FIX connections on 127.0.0.1:9876");

    server.accept_loop(listener);
}

fn stop_market(market_simulator: Arc<Mutex<MarketSimulator>>) {
    let market_simulator = market_simulator.lock().unwrap();
    let stop_handles = &mut market_simulator.stop_handles.lock().unwrap();

    if let Some(stop_handle) = stop_handles.stop_fix.take() {
        stop_handle.stop();
    }
    if let Some(stop_handle) = stop_handles.stop_ob.take() {
        stop_handle.stop();
    }
    if let Some(stop_handle) = stop_handles.stop_er.take() {
        stop_handle.stop();
    }

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

    let market_simulator = Arc::new(Mutex::new(MarketSimulator {
        stop_handles: Arc::new(Mutex::new(StopHandles {
            stop_fix: None,
            stop_ob: None,
            stop_er: None,
        })),
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