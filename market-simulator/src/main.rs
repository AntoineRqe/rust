use order_book::types::{OrderEvent, OrderResult};
use order_book::order_book::{OrderBookEngine};
use server::tcp::FixServer;
use std::sync::Arc;
use crossbeam::queue::ArrayQueue;
use fix::engine::{FixEngine};
use memory;
use fix::engine::{RequestQueue};
use std::net::TcpListener;

fn main() {
    // inbound: network → fix engine → order book
    let net_to_fix   = RequestQueue::new(ArrayQueue::new(1024));
    let fix_to_ob    = memory::open_shared_queue::<1024, OrderEvent>("fix_to_order_book", true);

    // outbound: order book → fix engine → network
    let ob_to_fix = memory::open_shared_queue::<1024, OrderResult>("order_book_to_fix", true);

    let (fix_tx, ob_rx) = fix_to_ob.queue.split();
    let (ob_resp_tx, fix_resp_rx) = ob_to_fix.queue.split();

    // fix engine thread
    let mut fix_engine = FixEngine ::new(Arc::clone(&net_to_fix), fix_tx, fix_resp_rx);
    std::thread::spawn(move || fix_engine.run());

    // Book engine thread
    let mut order_book_engine = OrderBookEngine::new(ob_rx, ob_resp_tx);
    std::thread::spawn(move || order_book_engine.run());

    // tcp server — each client pushes directly into fifo_in
    let server: FixServer<1024> = FixServer::new(net_to_fix);
    let listener = TcpListener::bind("127.0.0.1:9876").unwrap();
    server.accept_loop(listener);
}