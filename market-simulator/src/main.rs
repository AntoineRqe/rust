use server::tcp::FixServer;
use std::sync::Arc;
use crossbeam::queue::ArrayQueue;
use protocol::fix_engine::{FixEngine, FixRawMsg};
use memory;
use std::net::TcpListener;

fn main() {
    let fix_engine_queue  = Arc::new(ArrayQueue::<FixRawMsg<1024>>::new(1024));
    let order_book_queue = memory::open_shared_queue::<1024>("order_book_queue", true);

    let (fix_engine_out, order_book_in) = order_book_queue.queue.split();

    let mut order_book = order_book::order_book::OrderBook::new();

    // fix engine thread
    let mut engine = FixEngine ::new(Arc::clone(&fix_engine_queue), fix_engine_out);
    std::thread::spawn(move || engine.run());

    // order book thread
    std::thread::spawn(move || {
        loop {
            if let Some(event) = order_book_in.pop() {
                order_book.process_order(event);
            } else {
                std::hint::spin_loop();
            }
        }
    });

    // tcp server — each client pushes directly into fifo_in
    let server = FixServer::new(fix_engine_queue);
    let listener = TcpListener::bind("127.0.0.1:9876").unwrap();
    server.accept_loop(listener);
}

/* ## Architecture
```
Client 1 ──┐
Client 2 ──┼──▶ Arc<ArrayQueue<FixRawMsg>>  ──▶  FixEngine  ──▶  SPCP  ──▶  OrderBook
Client 3 ──┘         (MPMC, lock-free)                         (SPSC, lock-free) */