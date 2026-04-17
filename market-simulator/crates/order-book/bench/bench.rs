use criterion::{criterion_group, criterion_main, Criterion};
use order_book::engine::OrderBookEngine;
use std::sync::atomic::AtomicBool;
use std::{thread};
use std::time::Instant;
use hdrhistogram::Histogram;
use std::sync::OnceLock;

use utils::UtcTimestamp;
use core_affinity::CoreId;
use std::time::Duration;
use spsc::spsc_lock_free::RingBuffer;
use types::{OrderEvent, OrderResult};
use types::macros::{EntityId, OrderId, SymbolId};
use std::sync::Arc;
use std::sync::RwLock;

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
    static ref RESULTS: std::sync::Mutex<Vec<BenchResult>> = std::sync::Mutex::new(Vec::new());
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
    let mut ts_rb = RingBuffer::<u64, RB_SIZE>::new(); // Use u64 for nanoseconds

    thread::scope(|s| {
        
        let (er_inbound_tx, er_rx) = rb_rx.split();
        let (er_tx, er_outbound_rx) = rb_tx.split();
        let (ts_tx, ts_rx) = ts_rb.split();

        let er_inbound_tx = Arc::new(er_inbound_tx);
        let er_inbound_tx_clone = Arc::clone(&er_inbound_tx);

        let control_rx = crossbeam::channel::bounded::<order_book::OrderBookControl>(RB_SIZE);
        let order_book = Arc::new(RwLock::new(order_book::book::OrderBook::new("TEST".into())));

        let mut engine = OrderBookEngine::new(er_rx, [Some(er_tx), None, None], control_rx.1, order_book, Arc::clone(&shutdown));

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

                let send_ts = UtcTimestamp::now().to_unix_ns();

                loop {
                    if ts_tx.push(send_ts).is_ok() {
                        break;
                    }
                    std::hint::spin_loop();
                }

                order_event.timestamp = send_ts;
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

            let latency = (UtcTimestamp::now().to_unix_ns() - send_ts) as u64;

            histogram.record(latency).unwrap();

            ready.store(true, std::sync::atomic::Ordering::Release);
        }


        shutdown.store(true, std::sync::atomic::Ordering::Release);
        // Send a dummy message to unblock the engine if it's waiting
        er_inbound_tx_clone.push(OrderEvent {
            sender_id: EntityId::from_ascii(""),
            ..Default::default()
        }).unwrap();

        handle.join().unwrap();
    });

    start.elapsed()
}


fn benchmark_latency(c: &mut Criterion) {
    let functions: &[(&str, fn(u64, &mut Histogram<u64>) -> Duration); 1] = &[
        ("Order Book", benchmark_latency_order_book),
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
