use types::{OrderEvent, OrderResult};
use order_book::order_book::{OrderBookEngine};
use server::tcp::FixServer;
use std::sync::Arc;
use crossbeam::queue::ArrayQueue;
use fix::engine::{FixEngine, FixRawMsg};
use memory;
use fix::engine::{RequestQueue};
use execution_report::{ExecutionReportEngine};
use std::net::TcpListener;

fn main() {
    // inbound: network → fix engine → order book -> exection report
    let net_to_fix   = RequestQueue::new(ArrayQueue::new(1024));
    let fix_to_ob    = memory::open_shared_queue::<1024, OrderEvent>("fix_to_order_book", true);
    let ob_to_er     = memory::open_shared_queue::<1024, (OrderEvent, OrderResult)>("order_book_to_execution_report", true);

    // outbound: execution report → fix engine → network
    let er_to_fix     = memory::open_shared_queue::<1024, (u64, FixRawMsg<1024>)>("execution_report_to_fix", true);

    let (fix_tx, ob_rx) = fix_to_ob.queue.split();
    let (ob_tx, er_rx) = ob_to_er.queue.split();
    let (er_tx, fix_resp_rx) = er_to_fix.queue.split();


    // execution report engine thread
    let execution_report_engine = ExecutionReportEngine::new(er_rx, er_tx);
    std::thread::spawn(move || execution_report_engine.run());

    // Book engine thread
    let mut order_book_engine = OrderBookEngine::new(ob_rx, ob_tx);
    std::thread::spawn(move || order_book_engine.run());

    // fix engine thread
    let mut fix_engine = FixEngine ::new(Arc::clone(&net_to_fix), fix_tx, fix_resp_rx);
    std::thread::spawn(move || fix_engine.run());

    // tcp server — each client pushes directly into fifo_in
    let server: FixServer<1024> = FixServer::new(net_to_fix);
    let listener = TcpListener::bind("127.0.0.1:9876").unwrap();
    server.accept_loop(listener);
}