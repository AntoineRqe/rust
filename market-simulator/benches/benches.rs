use criterion::{criterion_group, criterion_main, Criterion};
use crossbeam::channel;
use order_book::engine::OrderBookEngine;
use market_feed::engine::{kill_market_feed_engine, MarketDataFeedEngine};
use std::net::UdpSocket;
use std::sync::atomic::AtomicBool;
use std::{thread};
use std::time::Instant;
use hdrhistogram::Histogram;
use std::sync::OnceLock;

use core_affinity::CoreId;
use std::time::Duration;
use std::sync::Mutex;
use execution_report::{ExecutionReportEngine};
use spsc::spsc_lock_free::RingBuffer;
use types::{OrderEvent, OrderResult, Trades};
use types::macros::{EntityId, OrderId, SymbolId};
use fix::engine::{FixEngine, FixRawMsg, kill_fix_inbound_engine, kill_fix_outbound_engine};
use std::sync::Arc;

const PRODUCER_CORE_OFFSET: usize = 0; // Offset for producer core
const CONSUMER_CORE_OFFSET: usize = 2; // Offset for consumer core
const ENGINE_CORE_OFFSET: usize = 4; // Offset for engine core
const BENCH_ITERS: u64 = 50_000; // Number of iterations for benchmarks
const RB_SIZE: usize = 2048; // Size of the ring buffer

#[inline]
#[allow(dead_code)]
fn spin_ns(ns: u64) {
    let deadline = Instant::now() + Duration::from_nanos(ns);
    while Instant::now() < deadline {
        std::hint::spin_loop();
    }
}

// Global storage for results, populated during benchmarks
lazy_static::lazy_static! {
    static ref RESULTS: Mutex<Vec<BenchResult>> = Mutex::new(Vec::new());
}

#[derive(Clone)]
struct BenchResult {
    name: String,
    p50: Duration,
    p99: Duration,
    p999: Duration,
}

// At the top level of your benchmark file
static BENCHMARK_CORES: OnceLock<Vec<CoreId>> = OnceLock::new();

fn get_cores() -> &'static Vec<CoreId> {
    BENCHMARK_CORES.get_or_init(|| {
        let all_cores = core_affinity::get_core_ids().unwrap();
        all_cores
    })
}

fn benchmark_latency_execution_report(iters: u64, histogram: &mut Histogram<u64>) -> Duration {
    let ready = Arc::new(AtomicBool::new(false));
    let shutdown = Arc::new(AtomicBool::new(false));
    let producer_core = get_cores()[(PRODUCER_CORE_OFFSET) % get_cores().len()];
    let consumer_core = get_cores()[(CONSUMER_CORE_OFFSET) % get_cores().len()];
    let engine_core = get_cores()[(ENGINE_CORE_OFFSET) % get_cores().len()];

    let cores = get_cores();

    if cores.len() < 2 {
        panic!("Need at least 2 cores available.");
    }
         
    let start = Instant::now();

    let mut rb_rx = RingBuffer::<(OrderEvent, OrderResult), RB_SIZE>::new(); // Size of the ring buffer
    let mut rb_tx = RingBuffer::<(EntityId, FixRawMsg<RB_SIZE>), RB_SIZE>::new(); // Size of the ring buffer
    let mut ts_rb = RingBuffer::<Instant, RB_SIZE>::new();

    thread::scope(|s| {
        
        let (er_inbound_tx, er_rx) = rb_rx.split();
        let (er_tx, er_outbound_rx) = rb_tx.split();
        let (ts_tx, ts_rx) = ts_rb.split();

        let er_inbound_tx = Arc::new(er_inbound_tx);
        let er_inbound_tx_clone = Arc::clone(&er_inbound_tx);

        let engine = ExecutionReportEngine::new(er_rx, er_tx, Arc::clone(&shutdown));

        let handle = s.spawn(move || {
            core_affinity::set_for_current(engine_core);
            engine.run();
        });

        let ready_prod = Arc::clone(&ready);

        s.spawn(move || {
            core_affinity::set_for_current(producer_core);

            let mut order_event = types::OrderEvent {
                        order_type: types::OrderType::LimitOrder,
                        cl_ord_id: OrderId::from_ascii("CLORD12345"),
                        orig_cl_ord_id: None,
                        side: types::Side::Buy,
                        price: types::FixedPointArithmetic(123_456_000), // 123.456 in FIX price format (8 decimal places)
                        quantity: types::FixedPointArithmetic(1_000_000),
                        sender_id: EntityId::from_ascii("SENDER"),
                        target_id: EntityId::from_ascii("TARGET"),
                        symbol: SymbolId::from_ascii("TEST"),
                        ..Default::default() // Current timestamp in milliseconds since epoch
                    };

            let order_result = types::OrderResult {
                    internal_order_id: 0,
                    trades: Trades::<4>::default(),
                    status: types::OrderStatus::New,
                    ..Default::default() // Current timestamp in milliseconds since epoch
            };
        
            for i in 0..iters {

                if i > 0 {
                    // Wait for consumer to be ready before sending next message
                    while !ready_prod.load(std::sync::atomic::Ordering::Acquire) {
                        std::hint::spin_loop();
                    }
                }
    
                ready_prod.store(false, std::sync::atomic::Ordering::Release);

                let send_ts = Instant::now();

                loop {
                    if ts_tx.push(send_ts).is_ok() {
                        break;
                    }
                    std::hint::spin_loop();
                }

                order_event.timestamp_ms = 0;

                er_inbound_tx.push((order_event, order_result)).unwrap();
            }
        });

        core_affinity::set_for_current(consumer_core);

        for _ in 0..iters {
            let send_ts = loop {
                if let Some(ts) = ts_rx.pop() {
                    break ts;
                }
                std::hint::spin_loop();
            };

            loop {
                if let Some(_) = er_outbound_rx.pop() {
                    break;
                }
                std::hint::spin_loop();
            };

            let latency = send_ts.elapsed().as_nanos() as u64;

            histogram.record(latency).unwrap();

            ready.store(true, std::sync::atomic::Ordering::Release);
        }
        

        // Send a dummy message to unblock the engine if it's waiting
        shutdown.store(true, std::sync::atomic::Ordering::Release); // Signal the engine to shut down gracefully

        er_inbound_tx_clone.push((OrderEvent {
            ..Default::default()
        },
        OrderResult {
            ..Default::default()
        })).unwrap();

        handle.join().unwrap();
    });

    start.elapsed()
}

fn benchmark_latency_order_book(iters: u64, histogram: &mut Histogram<u64>) -> Duration {
    let ready = Arc::new(AtomicBool::new(false));
    let shutdown = Arc::new(AtomicBool::new(false));
    let producer_core = get_cores()[(PRODUCER_CORE_OFFSET) % get_cores().len()];
    let consumer_core = get_cores()[(CONSUMER_CORE_OFFSET) % get_cores().len()];
    let engine_core = get_cores()[(ENGINE_CORE_OFFSET) % get_cores().len()];

    let cores = get_cores();

    if cores.len() < 2 {
        panic!("Need at least 2 cores available.");
    }
         
    let start = Instant::now();

    let mut rb_rx = RingBuffer::<OrderEvent, RB_SIZE>::new(); // Size of the ring buffer
    let mut rb_tx = RingBuffer::<(OrderEvent, OrderResult), RB_SIZE>::new(); // Size of the ring buffer
    let mut ts_rb = RingBuffer::<Instant, RB_SIZE>::new(); // Use Instant for homogeneous timing

    thread::scope(|s| {
        
        let (er_inbound_tx, er_rx) = rb_rx.split();
        let (er_tx, er_outbound_rx) = rb_tx.split();
        let (ts_tx, ts_rx) = ts_rb.split();

        let er_inbound_tx = Arc::new(er_inbound_tx);
        let er_inbound_tx_clone = Arc::clone(&er_inbound_tx);

        let control_rx = crossbeam::channel::bounded::<order_book::OrderBookControl>(RB_SIZE);
        let order_book = order_book::book::OrderBook::new("TEST".into());

        let mut engine = OrderBookEngine::new(
            er_rx,
            Some(er_tx),
            None,
            None,
            control_rx.1,
            order_book,
            None,
            Arc::clone(&shutdown)
        );

        let handle = s.spawn(move || {
            core_affinity::set_for_current(engine_core);
            engine.run();
        });

        let ready_prod = Arc::clone(&ready);
        
        s.spawn(move || {
            core_affinity::set_for_current(producer_core);

            let mut order_event = types::OrderEvent {
                order_type: types::OrderType::LimitOrder,
                cl_ord_id: OrderId::from_ascii("CLORD12345"),
                orig_cl_ord_id: None,
                side: types::Side::Buy,
                price: types::FixedPointArithmetic(123_456_000), // 123.456 in FIX price format (8 decimal places)
                quantity: types::FixedPointArithmetic(1_000_000),
                sender_id: EntityId::from_ascii("SENDER"),
                target_id: EntityId::from_ascii("TARGET"),
                symbol: SymbolId::from_ascii("TEST"),
                ..Default::default() // Current timestamp in milliseconds since epoch
            };
        
            for i in 0..iters {

                if i > 0 {
                    // Wait for consumer to be ready before sending next message
                    while !ready_prod.load(std::sync::atomic::Ordering::Acquire) {
                        std::hint::spin_loop();
                    }
                }
    
                ready_prod.store(false, std::sync::atomic::Ordering::Release);

                let send_ts = Instant::now();

                loop {
                    if ts_tx.push(send_ts).is_ok() {
                        break;
                    }
                    std::hint::spin_loop();
                }

                order_event.timestamp_ms = 0;
                loop {
                    if let Err(ev_err) = er_inbound_tx.push(order_event) {
                        order_event = ev_err;
                        std::hint::spin_loop();
                    } else {
                        break;
                    }
                }
            }
        });

        core_affinity::set_for_current(consumer_core);

        for _ in 0..iters {
            let send_ts = loop {
                if let Some(ts) = ts_rx.try_pop() {
                    break ts;
                }
                std::hint::spin_loop();
            };

            loop {
                if let Some(_) = er_outbound_rx.try_pop() {
                    break;
                }
                std::hint::spin_loop();
            };

            let latency = send_ts.elapsed().as_nanos() as u64;

            histogram.record(latency).unwrap();

            ready.store(true, std::sync::atomic::Ordering::Release);
        }


        // Send a dummy message to unblock the engine if it's waiting
        shutdown.store(true, std::sync::atomic::Ordering::Release); // Signal the engine to shut down gracefully
        
        er_inbound_tx_clone.push(OrderEvent {
            sender_id: EntityId::from_ascii(""),
            ..Default::default()
        }).unwrap();

        handle.join().unwrap();
    });

    start.elapsed()
}

fn benchmark_latency_market_feed(iters: u64, histogram: &mut Histogram<u64>) -> Duration {
    let ready = Arc::new(AtomicBool::new(false));
    let shutdown = Arc::new(AtomicBool::new(false));
    let producer_core = get_cores()[(PRODUCER_CORE_OFFSET) % get_cores().len()];
    let consumer_core = get_cores()[(CONSUMER_CORE_OFFSET) % get_cores().len()];
    let engine_core = get_cores()[(ENGINE_CORE_OFFSET) % get_cores().len()];

    if get_cores().len() < 2 {
        panic!("Need at least 2 cores available.");
    }

    let iters = (iters / 100).max(100); // Reduce iterations for market feed benchmark, ensure at least 100
    let start = Instant::now();

    let mut rb_in = RingBuffer::<(OrderEvent, OrderResult), RB_SIZE>::new();
    let mut ts_rb = RingBuffer::<Instant, RB_SIZE>::new();

    thread::scope(|s| {
        let (inbound_tx, inbound_rx) = rb_in.split();
        let (ts_tx, ts_rx) = ts_rb.split();

        let inbound_tx = Arc::new(inbound_tx);
        let inbound_tx_clone = Arc::clone(&inbound_tx);

        let recv_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        recv_socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let recv_port = recv_socket.local_addr().unwrap().port();

        let mut engine = MarketDataFeedEngine::new(
            inbound_rx,
            Arc::clone(&shutdown),
            "127.0.0.1".to_string(),
            recv_port,
        )
        .unwrap();

        let engine_handle = s.spawn(move || {
            core_affinity::set_for_current(engine_core);
            engine.run();
        });

        let ready_prod = Arc::clone(&ready);

        s.spawn(move || {
            core_affinity::set_for_current(producer_core);

            let order_event = OrderEvent {
                order_type: types::OrderType::LimitOrder,
                cl_ord_id: OrderId::from_ascii("CLORD12345"),
                orig_cl_ord_id: None,
                side: types::Side::Buy,
                price: types::FixedPointArithmetic(123_456_000),
                quantity: types::FixedPointArithmetic(1_000_000),
                sender_id: EntityId::from_ascii("SENDER"),
                target_id: EntityId::from_ascii("TARGET"),
                symbol: SymbolId::from_ascii("TEST"),
                ..Default::default()
            };

            let order_result = OrderResult {
                internal_order_id: 0,
                trades: Trades::<4>::default(),
                status: types::OrderStatus::New,
                ..Default::default()
            };

            for i in 0..iters {
                if i > 0 {
                    while !ready_prod.load(std::sync::atomic::Ordering::Acquire) {
                        std::hint::spin_loop();
                    }
                }

                ready_prod.store(false, std::sync::atomic::Ordering::Release);

                let send_ts = Instant::now();
                loop {
                    if ts_tx.push(send_ts).is_ok() {
                        break;
                    }
                    std::hint::spin_loop();
                }

                while let Err(_) = inbound_tx.push((order_event, order_result)) {
                    std::hint::spin_loop();
                }
            }
        });

        core_affinity::set_for_current(consumer_core);

        let mut buf = [0u8; 2048];

        for _ in 0..iters {
            let send_ts = loop {
                if let Some(ts) = ts_rx.pop() {
                    break ts;
                }
                std::hint::spin_loop();
            };

            let _ = recv_socket
                .recv_from(&mut buf)
                .expect("Expected market data UDP packet");

            let latency = send_ts.elapsed().as_nanos() as u64;
            histogram.record(latency).unwrap();

            ready.store(true, std::sync::atomic::Ordering::Release);
        }

        shutdown.store(true, std::sync::atomic::Ordering::Relaxed); // Signal the engine to shut down gracefully
        std::thread::sleep(Duration::from_millis(100)); // Give the engine some time to shut down gracefully before sending the kill signal

        kill_market_feed_engine(&inbound_tx_clone);

        engine_handle.join().unwrap();
    });

    start.elapsed()
}

fn benchmark_latency_fix_inbound(iters: u64, histogram: &mut Histogram<u64>) -> Duration {
    let ready = Arc::new(AtomicBool::new(false));
    let shutdown = Arc::new(AtomicBool::new(false));
    let producer_core = get_cores()[(PRODUCER_CORE_OFFSET) % get_cores().len()];
    let consumer_core = get_cores()[(CONSUMER_CORE_OFFSET) % get_cores().len()];
    let engine_core = get_cores()[(ENGINE_CORE_OFFSET) % get_cores().len()];

    let cores = get_cores();
    let iters = (iters / 10).max(100); // Reduce iterations for FIX benchmark, ensure at least 100
    
    if cores.len() < 2 {
        panic!("Need at least 2 cores available.");
    }
         
    let start = Instant::now();

    let mut rb_rx = RingBuffer::<OrderEvent, RB_SIZE>::new();
    let mut rb_tx = RingBuffer::<(EntityId, FixRawMsg<RB_SIZE>), RB_SIZE>::new();
    let mut ts_rb = RingBuffer::<Instant, RB_SIZE>::new();
 
    thread::scope(|s| {
        
        let (fix_inbound_tx, fix_inbound_rx) = rb_rx.split();
        let (_, er_outbound_rx) = rb_tx.split();
        let (ts_tx, ts_rx) = ts_rb.split();
        let (net_to_fix_tx, net_to_fix_rx) = channel::bounded::<FixRawMsg<RB_SIZE>>(RB_SIZE);

        let net_to_fix_rx = Arc::new(net_to_fix_rx);
        let net_to_fix_tx = Arc::new(net_to_fix_tx);
        let net_to_fix_tx_clone = Arc::clone(&net_to_fix_tx);

        let engine = FixEngine::new(
            net_to_fix_rx,
            fix_inbound_tx,
            er_outbound_rx,
            Arc::clone(&shutdown));

        let (mut inbound_engine, _) = engine.split();

        let handle = s.spawn(move || {
            core_affinity::set_for_current(engine_core);
            inbound_engine.run();
        });

        let ready_prod = Arc::clone(&ready);
        
        s.spawn(move || {
            core_affinity::set_for_current(producer_core);

            let fix_message = b"8=FIX.4.4\x019=0000\x0135=D\x0149=SENDER\x0156=TARGET\x0134=1\x0152=20240219-12:30:00.000\x0111=12345\x0154=1\x0138=1000000\x0144=1.23456\x0155=EURUSD\x0110=123\x01";

            let raw_msg = FixRawMsg {
                len: fix_message.len() as u16,
                data: {
                    let mut data = [0u8; 2048];
                    data[..fix_message.len()].copy_from_slice(fix_message);
                    data
                },
                resp_queue: None, // Not using the response queue in this test, but could be set here if needed for future tests
            };
        
            for i in 0..iters {
                let tmp_raw_msg = raw_msg.clone(); // Create a mutable copy for this iteration

                if i > 0 {
                    // Wait for consumer to be ready before sending next message
                    while !ready_prod.load(std::sync::atomic::Ordering::Acquire) {
                        std::hint::spin_loop();
                    }
                }
    
                ready_prod.store(false, std::sync::atomic::Ordering::Release);

                let send_ts = Instant::now();

                ts_tx.push(send_ts).unwrap();
                net_to_fix_tx.send(tmp_raw_msg).unwrap();
            }
        });

        core_affinity::set_for_current(consumer_core);

        for _ in 0..iters {
            let send_ts = ts_rx.pop().unwrap();
            fix_inbound_rx.pop().unwrap();

            let latency = send_ts.elapsed().as_nanos() as u64;

            histogram.record(latency).unwrap();

            ready.store(true, std::sync::atomic::Ordering::Release);
        }


        let dummy_msg = FixRawMsg {
            len: 0,
            data: [0u8; RB_SIZE],
            resp_queue: None,
        };
    
        shutdown.store(true, std::sync::atomic::Ordering::Release); // Signal the engine to shut down gracefully
        net_to_fix_tx_clone.send(dummy_msg).unwrap(); // Send a dummy message to unblock the engine if it's waiting

        handle.join().unwrap();
    });

    start.elapsed()
}

fn benchmark_latency_fix_outbound(iters: u64, histogram: &mut Histogram<u64>) -> Duration {
    use tokio::sync::mpsc;

    let ready = Arc::new(AtomicBool::new(false));
    let shutdown = Arc::new(AtomicBool::new(false));
    let producer_core = get_cores()[(PRODUCER_CORE_OFFSET) % get_cores().len()];
    let consumer_core = get_cores()[(CONSUMER_CORE_OFFSET) % get_cores().len()];
    let engine_core = get_cores()[(ENGINE_CORE_OFFSET) % get_cores().len()];

    if get_cores().len() < 2 {
        panic!("Need at least 2 cores available.");
    }

    let iters = (iters / 10).max(100); // Reduce iterations for FIX outbound benchmark, ensure at least 100
    let start = Instant::now();

    let (net_to_fix_tx, net_to_fix_rx) = channel::bounded::<FixRawMsg<RB_SIZE>>(RB_SIZE);
    let net_to_fix_rx = Arc::new(net_to_fix_rx);
    let net_to_fix_tx = Arc::new(net_to_fix_tx);
    let net_to_fix_tx_clone = Arc::clone(&net_to_fix_tx);

    let mut fix_to_ob = RingBuffer::<OrderEvent, RB_SIZE>::new();
    let mut er_to_fix = RingBuffer::<(EntityId, FixRawMsg<RB_SIZE>), RB_SIZE>::new();
    let mut ts_rb = RingBuffer::<Instant, RB_SIZE>::new();

    let (response_tx, mut response_rx) = mpsc::channel::<FixRawMsg<RB_SIZE>>(1024);

    thread::scope(|s| {
        let (fix_to_ob_tx, fix_to_ob_rx) = fix_to_ob.split();
        let (er_to_fix_tx, er_to_fix_rx) = er_to_fix.split();
        let (ts_tx, ts_rx) = ts_rb.split();

        let er_to_fix_tx = Arc::new(er_to_fix_tx);
        let er_to_fix_tx_clone = Arc::clone(&er_to_fix_tx);

        let engine = FixEngine::new(
            Arc::clone(&net_to_fix_rx),
            fix_to_ob_tx,
            er_to_fix_rx,
            Arc::clone(&shutdown),
        );

        let (mut inbound_engine, mut outbound_engine) = engine.split();

        let inbound_handle = s.spawn(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: engine_core.id });
            inbound_engine.run();
        });

        let outbound_handle = s.spawn(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: (engine_core.id + 1) % get_cores().len() });
            outbound_engine.run();
        });

        // Prime pending routes in outbound engine by sending one inbound FIX message with response queue.
        let setup_message = b"8=FIX.4.4\x019=0000\x0135=D\x0149=SENDER\x0156=TARGET\x0134=1\x0152=20240219-12:30:00.000\x0111=12345\x0154=1\x0138=1000000\x0144=1.23456\x0155=EURUSD\x0110=123\x01";
        let setup_raw = FixRawMsg {
            len: setup_message.len() as u16,
            data: {
                let mut data = [0u8; 2048];
                data[..setup_message.len()].copy_from_slice(setup_message);
                data
            },
            resp_queue: Some(response_tx.clone()),
        };

        net_to_fix_tx.send(setup_raw).unwrap();
        let _ = fix_to_ob_rx.pop().unwrap(); // ensure inbound processed and pending route is registered

        let ready_prod = Arc::clone(&ready);

        s.spawn(move || {
            core_affinity::set_for_current(producer_core);

            let response_msg = FixRawMsg {
                len: 5,
                data: {
                    let mut data = [0u8; 2048];
                    data[..5].copy_from_slice(b"8=FIX");
                    data
                },
                resp_queue: None,
            };

            for i in 0..iters {
                if i > 0 {
                    while !ready_prod.load(std::sync::atomic::Ordering::Acquire) {
                        std::hint::spin_loop();
                    }
                }

                ready_prod.store(false, std::sync::atomic::Ordering::Release);

                let send_ts = Instant::now();
                while let Err(_) = ts_tx.push(send_ts) {
                    std::hint::spin_loop();
                }

                while let Err(_) = er_to_fix_tx.push((EntityId::from_ascii("SENDER"), response_msg.clone())) {
                    std::hint::spin_loop();
                }
            }
        });

        core_affinity::set_for_current(consumer_core);

        for _ in 0..iters {
            let send_ts = ts_rx.pop().unwrap();
            let _ = response_rx
                .blocking_recv()
                .expect("Expected outbound FIX response in benchmark");

            let latency = send_ts.elapsed().as_nanos() as u64;
            histogram.record(latency).unwrap();

            ready.store(true, std::sync::atomic::Ordering::Release);
        }

        shutdown.store(true, std::sync::atomic::Ordering::Release);
        kill_fix_inbound_engine(&net_to_fix_tx_clone);
        kill_fix_outbound_engine(&er_to_fix_tx_clone);

        inbound_handle.join().unwrap();
        outbound_handle.join().unwrap();
    });

    start.elapsed()
}

fn benchmark_latency_all(iters: u64, histogram: &mut Histogram<u64>) -> Duration {
    use tokio::sync::mpsc;

    let ready = Arc::new(AtomicBool::new(false));
    let shutdown = Arc::new(AtomicBool::new(false));
    let producer_core = get_cores()[(PRODUCER_CORE_OFFSET) % get_cores().len()];
    let consumer_core = get_cores()[(CONSUMER_CORE_OFFSET) % get_cores().len()];

    let cores = get_cores();

    if cores.len() < 2 {
        panic!("Need at least 2 cores available.");
    }

    let iters = (iters / 1).max(100); // Run at full rate like ER/OB to get better stats
    let start = Instant::now();

    // inbound: network → fix engine → order book -> execution report
    let (net_to_fix_tx, net_to_fix_rx) = channel::bounded::<FixRawMsg<RB_SIZE>>(RB_SIZE);
    let net_to_fix_rx = Arc::new(net_to_fix_rx);
    let net_to_fix_tx = Arc::new(net_to_fix_tx);
    let net_to_fix_tx_clone = Arc::clone(&net_to_fix_tx);
    let mut fix_to_ob    = RingBuffer::<OrderEvent, RB_SIZE>::new();
    let mut ob_to_er     = RingBuffer::<(OrderEvent, OrderResult), RB_SIZE>::new();

    // outbound: execution report → fix engine → network
    let mut er_to_fix     = RingBuffer::<(EntityId, FixRawMsg<RB_SIZE>), RB_SIZE>::new();
    let (response_tx, mut response_rx) = mpsc::channel::<FixRawMsg<RB_SIZE>>(1024); // Channel for receiving responses from the FIX engine, if needed for future tests

    let mut ts_rb = RingBuffer::<Instant, RB_SIZE>::new();

    thread::scope(|s| {
    
        let shutdown = Arc::clone(&shutdown);
        let (fix_tx, ob_rx) = fix_to_ob.split();
        let (ob_tx, er_rx) = ob_to_er.split();
        let (er_tx, fix_resp_rx) = er_to_fix.split();
        let (ts_tx, ts_rx) = ts_rb.split();

        let control_rx = crossbeam::channel::bounded::<order_book::OrderBookControl>(RB_SIZE);

        // execution report engine thread
        let execution_report_engine = ExecutionReportEngine::new(er_rx, er_tx, Arc::clone(&shutdown));
        let er_handle = s.spawn(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: 4 });
            execution_report_engine.run();
        });

        // Book engine thread
        let order_book = order_book::book::OrderBook::new("TEST".into());
        let mut order_book_engine: OrderBookEngine<'_, 2048> = OrderBookEngine::new(
            ob_rx,
             Some(ob_tx),
             None,
             None,
             control_rx.1,
             order_book,
             None,
              Arc::clone(&shutdown)
        );

        let ob_handle = s.spawn(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: 6 });
            order_book_engine.run();
        });

        // fix engine thread
        let fix_engine = FixEngine ::new(Arc::clone(&net_to_fix_rx), fix_tx, fix_resp_rx, Arc::clone(&shutdown));
        let (mut inbound_engine, mut outbound_engine) = fix_engine.split();

        let inbound_fix_handle = s.spawn(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: 8 });
            inbound_engine.run();
        });

        let outbound_fix_handle = s.spawn(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: 9 });
            outbound_engine.run();
        });

        let ready_prod = Arc::clone(&ready);
        
        s.spawn(move || {
            core_affinity::set_for_current(producer_core);

            let fix_message = b"8=FIX.4.4\x019=0000\x0135=D\x0149=SENDER\x0156=TARGET\x0134=1\x0152=20240219-12:30:00.000\x0111=12345\x0154=1\x0138=1000000\x0144=1.23456\x0155=EURUSD\x0110=123\x01";

            let raw_msg = FixRawMsg {
                len: fix_message.len() as u16,
                data: {
                    let mut data = [0u8; 2048];
                    data[..fix_message.len()].copy_from_slice(fix_message);
                    data
                },
                resp_queue: Some(response_tx.clone()),
            };
        
            for i in 0..iters {

                if i > 0 {
                    // Wait for consumer to be ready before sending next message
                    while !ready_prod.load(std::sync::atomic::Ordering::Acquire) {
                        std::hint::spin_loop();
                    }
                }
    
                ready_prod.store(false, std::sync::atomic::Ordering::Release);

                let mut tmp_raw_msg = raw_msg.clone(); // Clone the message for this iteration
                let send_ts = Instant::now();

                ts_tx.push(send_ts).unwrap();

                while let Err(_) = net_to_fix_tx_clone.send(tmp_raw_msg) {
                    tmp_raw_msg = raw_msg.clone(); // Clone again if the previous one was consumed while we were trying to send
                    std::hint::spin_loop();
                }
            }
        });

        core_affinity::set_for_current(consumer_core);

        for _ in 0..iters {
            let send_ts = ts_rx.pop().unwrap();
            
            let _ = response_rx
                .blocking_recv()
                .expect("Expected FIX response in overall benchmark");
    
            let latency = send_ts.elapsed().as_nanos() as u64;
            histogram.record(latency).unwrap();

            ready.store(true, std::sync::atomic::Ordering::Release);
        }

        // Stop all engines
        shutdown.store(true, std::sync::atomic::Ordering::Relaxed); // Signal the engine to shut down gracefully
        
        std::thread::sleep(std::time::Duration::from_millis(100)); // Give engines a moment to shut down gracefully before forcefully killing the FIX inbound engine thread, which may be blocked on receiving from the channel
        kill_fix_inbound_engine(&net_to_fix_tx);

        ob_handle.join().unwrap();
        er_handle.join().unwrap();
        inbound_fix_handle.join().unwrap();
        outbound_fix_handle.join().unwrap();
    });

    start.elapsed()
}

fn benchmark_latency(c: &mut Criterion) {
    let functions: &[(&str, fn(u64, &mut Histogram<u64>) -> Duration); 6] = &[
        ("Execution Report", benchmark_latency_execution_report),
        ("Order Book", benchmark_latency_order_book),
        ("FIX Inbound", benchmark_latency_fix_inbound),
        ("FIX Outbound", benchmark_latency_fix_outbound),
        ("Market Feed", benchmark_latency_market_feed),
        ("Overall", benchmark_latency_all),
    ];

    for (name, func) in functions {
        let mut histogram =
            hdrhistogram::Histogram::<u64>::new_with_bounds(1, 10_000_000, 3)
                .expect("Failed to create histogram");
        histogram.auto(true);

        c.bench_function(name, |b| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    total += func(BENCH_ITERS, &mut histogram);
                }
                total
            });
        });

        let p50 = Duration::from_nanos(histogram.value_at_quantile(0.50));
        let p99 = Duration::from_nanos(histogram.value_at_quantile(0.99));
        let p999 = Duration::from_nanos(histogram.value_at_quantile(0.999));

        RESULTS.lock().unwrap().push(BenchResult {
            name: name.to_string(),
            p50,
            p99,
            p999,
        });
    }

    // Print summary table after all benchmarks in this group complete
    let results = RESULTS.lock().unwrap();
    print_summary(&results);
}

fn print_summary(results: &[BenchResult]) {
    if results.is_empty() {
        println!("No results.");
        return;
    }

    let mut results = results.to_vec();
    results.sort_by_key(|r| r.p50);

    let base = &results[0];
    let base_p50 = base.p50.as_nanos() as f64;

    // Dynamic width based on longest name
    let name_width = results
        .iter()
        .map(|r| r.name.len())
        .max()
        .unwrap()
        .max(20);

    println!();
    println!("SPSC Queue Latency Summary");
    println!("(baseline = {})", base.name);
    println!();

    println!(
        "{:<name_width$} │ {:>10} │ {:>10} │ {:>10} │ {:>9}",
        "Name",
        "p50 (ns)",
        "p99 (ns)",
        "p999 (ns)",
        "vs base",
        name_width = name_width
    );

    println!("{}", "─".repeat(name_width + 52));

    for r in results {
        let p50 = r.p50.as_nanos() as f64;
        let p99 = r.p99.as_nanos() as f64;
        let p999 = r.p999.as_nanos() as f64;

        let ratio = if base_p50 > 0.0 {
            p50 / base_p50
        } else {
            1.0
        };

        println!(
            "{:<name_width$} │ {:>10} │ {:>10} │ {:>10} │ {:>9.2}x",
            r.name,
            p50 as u128,
            p99 as u128,
            p999 as u128,
            ratio,
            name_width = name_width
        );
    }

    println!();
}

criterion_group!(benches, benchmark_latency);
criterion_main!(benches);
