use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use order_book::engine::OrderBookEngine;
use std::sync::atomic::AtomicBool;
use std::thread;
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

const PRODUCER_CORE_OFFSET: usize = 0;
const CONSUMER_CORE_OFFSET: usize = 2;
const ENGINE_CORE_OFFSET:   usize = 4;

// For me:
// Run : cargo bench --bench ob_bench -p order-book

/// Messages per Criterion iteration for the latency benchmark.
/// Lower = faster warmup, higher = more stable histogram tails.
const LATENCY_ITERS: u64 = 10_000;

/// Messages per Criterion iteration for the throughput benchmark.
/// Higher = smooths out setup overhead in the msg/s calculation.
const THROUGHPUT_ITERS: u64 = 100_000;

const RB_SIZE: usize = 4096;

// ── helpers ───────────────────────────────────────────────────────────────────

static BENCHMARK_CORES: OnceLock<Vec<CoreId>> = OnceLock::new();

fn get_cores() -> &'static Vec<CoreId> {
    BENCHMARK_CORES.get_or_init(|| core_affinity::get_core_ids().unwrap())
}

fn make_order_event() -> OrderEvent {
    types::OrderEvent {
        order_type: types::OrderType::LimitOrder,
        cl_ord_id:  OrderId::from_ascii("CLORD12345"),
        orig_cl_ord_id: None,
        side:     types::Side::Buy,
        price:    types::FixedPointArithmetic(123_456_000),
        quantity: types::FixedPointArithmetic(1_000_000),
        sender_id: EntityId::from_ascii("SENDER"),
        target_id: EntityId::from_ascii("TARGET"),
        symbol:   SymbolId::from_ascii("TEST"),
        ..Default::default()
    }
}

fn kill_engine(tx: &Arc<spsc::spsc_lock_free::Producer<OrderEvent, RB_SIZE>>) {
    loop {
        if tx.push(OrderEvent {
            sender_id: EntityId::from_ascii(""),
            ..Default::default()
        }).is_ok() { break; }
        std::hint::spin_loop();
    }
}

// ── latency run ───────────────────────────────────────────────────────────────

/// Sends `iters` messages one-at-a-time (ping-pong) and records each
/// end-to-end latency into `histogram`. Returns wall-clock elapsed time.
fn run_latency(iters: u64, histogram: &mut Histogram<u64>) -> Duration {
    let shutdown      = Arc::new(AtomicBool::new(false));
    let ready         = Arc::new(AtomicBool::new(false));
    let producer_core = get_cores()[PRODUCER_CORE_OFFSET % get_cores().len()];
    let consumer_core = get_cores()[CONSUMER_CORE_OFFSET % get_cores().len()];
    let engine_core   = get_cores()[ENGINE_CORE_OFFSET   % get_cores().len()];

    assert!(get_cores().len() >= 2, "Need at least 2 CPU cores.");

    let mut rb_rx = RingBuffer::<OrderEvent,                RB_SIZE>::new();
    let mut rb_tx = RingBuffer::<(OrderEvent, OrderResult), RB_SIZE>::new();
    let mut ts_rb = RingBuffer::<u64,                       RB_SIZE>::new();

    let start = Instant::now();

    thread::scope(|s| {
        let (inbound_tx,  inbound_rx)  = rb_rx.split();
        let (outbound_tx, outbound_rx) = rb_tx.split();
        let (ts_tx,       ts_rx)       = ts_rb.split();

        let inbound_tx       = Arc::new(inbound_tx);
        let inbound_tx_clone = Arc::clone(&inbound_tx);

        let control_rx = crossbeam::channel::bounded::<order_book::OrderBookControl>(RB_SIZE);
        let order_book = Arc::new(RwLock::new(order_book::book::OrderBook::new("TEST".into())));
        let mut engine = OrderBookEngine::new(
            inbound_rx, [Some(outbound_tx), None, None],
            control_rx.1, order_book, Arc::clone(&shutdown),
        );

        let engine_handle = s.spawn(move || {
            core_affinity::set_for_current(engine_core);
            engine.run();
        });

        let ready_prod = Arc::clone(&ready);
        s.spawn(move || {
            core_affinity::set_for_current(producer_core);
            let mut ev = make_order_event();
            for i in 0..iters {
                if i > 0 {
                    while !ready_prod.load(std::sync::atomic::Ordering::Acquire) {
                        std::hint::spin_loop();
                    }
                }
                ready_prod.store(false, std::sync::atomic::Ordering::Release);
                let ts = UtcTimestamp::now().to_unix_ns();
                loop { if ts_tx.push(ts).is_ok() { break; } std::hint::spin_loop(); }
                ev.timestamp = ts;
                loop {
                    match inbound_tx.push(ev) {
                        Ok(()) => break,
                        Err(e) => { ev = e; std::hint::spin_loop(); }
                    }
                }
            }
        });

        // Consumer runs on this (main bench) thread.
        core_affinity::set_for_current(consumer_core);
        for _ in 0..iters {
            let send_ts = loop {
                if let Some(ts) = ts_rx.try_pop() { break ts; }
                std::hint::spin_loop();
            };
            loop {
                if outbound_rx.try_pop().is_some() { break; }
                std::hint::spin_loop();
            }
            let latency = UtcTimestamp::now().to_unix_ns().saturating_sub(send_ts);
            histogram.record(latency).unwrap();
            ready.store(true, std::sync::atomic::Ordering::Release);
        }

        shutdown.store(true, std::sync::atomic::Ordering::Release);
        kill_engine(&inbound_tx_clone);
        engine_handle.join().unwrap();
    });

    start.elapsed()
}

// ── throughput run ────────────────────────────────────────────────────────────

/// Floods `iters` messages as fast as possible and returns elapsed wall time.
fn run_throughput(iters: u64) -> Duration {
    let shutdown      = Arc::new(AtomicBool::new(false));
    let producer_core = get_cores()[PRODUCER_CORE_OFFSET % get_cores().len()];
    let consumer_core = get_cores()[CONSUMER_CORE_OFFSET % get_cores().len()];
    let engine_core   = get_cores()[ENGINE_CORE_OFFSET   % get_cores().len()];

    assert!(get_cores().len() >= 2, "Need at least 2 CPU cores.");

    let mut rb_rx = RingBuffer::<OrderEvent,                RB_SIZE>::new();
    let mut rb_tx = RingBuffer::<(OrderEvent, OrderResult), RB_SIZE>::new();

    let start = Instant::now();
    let mut measured_elapsed = Duration::ZERO;

    thread::scope(|s| {
        let (inbound_tx,  inbound_rx)  = rb_rx.split();
        let (outbound_tx, outbound_rx) = rb_tx.split();

        let inbound_tx       = Arc::new(inbound_tx);
        let inbound_tx_clone = Arc::clone(&inbound_tx);

        let control_rx = crossbeam::channel::bounded::<order_book::OrderBookControl>(RB_SIZE);
        let order_book = Arc::new(RwLock::new(order_book::book::OrderBook::new("TEST".into())));
        let mut engine = OrderBookEngine::new(
            inbound_rx, [Some(outbound_tx), None, None],
            control_rx.1, order_book, Arc::clone(&shutdown),
        );

        let engine_handle = s.spawn(move || {
            core_affinity::set_for_current(engine_core);
            engine.run();
        });

        s.spawn(move || {
            core_affinity::set_for_current(producer_core);
            let mut ev = make_order_event();
            for _ in 0..iters {
                loop {
                    match inbound_tx.push(ev) {
                        Ok(()) => break,
                        Err(e) => { ev = e; std::hint::spin_loop(); }
                    }
                }
            }
        });

        core_affinity::set_for_current(consumer_core);
        for _ in 0..iters {
            loop {
                if outbound_rx.try_pop().is_some() { break; }
                std::hint::spin_loop();
            }
        }

        measured_elapsed = start.elapsed();

        shutdown.store(true, std::sync::atomic::Ordering::Release);
        kill_engine(&inbound_tx_clone);
        engine_handle.join().unwrap();
    });

    measured_elapsed
}

// ── Criterion entry point ─────────────────────────────────────────────────────

fn benchmark_order_book(c: &mut Criterion) {
    // ── Latency ──
    let mut histogram = Histogram::<u64>::new_with_bounds(1, 10_000_000, 3)
        .expect("histogram");
    histogram.auto(true);

    c.bench_function("Order Book / latency", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                total += run_latency(LATENCY_ITERS, &mut histogram);
            }
            total
        });
    });

    // ── Throughput ──
    let mut tput_total_msgs: u64 = 0;
    let mut tput_total_time = Duration::ZERO;

    let mut group = c.benchmark_group("Order Book / throughput");
    group.throughput(Throughput::Elements(THROUGHPUT_ITERS));
    group.bench_function("msg/s", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let elapsed = run_throughput(THROUGHPUT_ITERS);
                total += elapsed;
                tput_total_time += elapsed;
                tput_total_msgs += THROUGHPUT_ITERS;
            }
            total
        });
    });
    group.finish();

    let msgs_per_sec = if tput_total_time.as_secs_f64() > 0.0 {
        (tput_total_msgs as f64 / tput_total_time.as_secs_f64()) as u64
    } else {
        0
    };

    // ── Combined summary ──
    let p50  = Duration::from_nanos(histogram.value_at_quantile(0.50));
    let p99  = Duration::from_nanos(histogram.value_at_quantile(0.99));
    let p999 = Duration::from_nanos(histogram.value_at_quantile(0.999));
    let n    = histogram.len();

    // Pre-format values so we can measure their widths and align columns.
    let n_str       = format!("{n}");
    let p50_str     = format!("{}", p50.as_nanos());
    let p99_str     = format!("{}", p99.as_nanos());
    let p999_str    = format!("{}", p999.as_nanos());
    let tput_str    = format!("{:.3} M msg/s", msgs_per_sec as f64 / 1_000_000.0);

    let val_w  = p50_str.len().max(p99_str.len()).max(p999_str.len());
    let col1_w = "  Latency   n=".len() + n_str.len();
    let col1_w = col1_w.max("  Throughput  ".len());
    let col2_w = "  p999  ".len() + val_w + " ns  ".len();
    let col2_w = col2_w.max(format!("  {tput_str}  ").len());
    let total   = col1_w + 1 + col2_w;

    let top   = format!("┌{}┐", "─".repeat(total));
    let title = format!("│{:^total$}│", "Order Book Benchmark Summary");
    let div1  = format!("├{}┬{}┤", "─".repeat(col1_w), "─".repeat(col2_w));
    let div2  = format!("├{}┼{}┤", "─".repeat(col1_w), "─".repeat(col2_w));
    let bot   = format!("└{}┘", "─".repeat(total));

    let row = |left: &str, right: &str| -> String {
        format!("│{:<col1_w$}│{:<col2_w$}│", left, right)
    };

    println!();
    println!("{top}");
    println!("{title}");
    println!("{div1}");
    println!("{}", row(&format!("  Latency   n={n_str}"), &format!("  p50   {p50_str:>val_w$} ns")));
    println!("{}", row("",                                 &format!("  p99   {p99_str:>val_w$} ns")));
    println!("{}", row("",                                 &format!("  p999  {p999_str:>val_w$} ns")));
    println!("{div2}");
    println!("{}", row("  Throughput",                    &format!("  {tput_str}")));
    println!("{bot}");
    println!();
}

criterion_group!(benches, benchmark_order_book);
criterion_main!(benches);
