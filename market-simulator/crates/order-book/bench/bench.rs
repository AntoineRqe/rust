use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use hdrhistogram::Histogram;
use order_book::engine::OrderBookEngine;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::Instant;

use core_affinity::CoreId;
use spsc::spsc_lock_free::RingBuffer;
use std::sync::Arc;
use std::time::Duration;
use types::macros::{EntityId, OrderId, SymbolId};
use types::{OrderEvent, OrderResult};
use utils::UtcTimestamp;

const PRODUCER_CORE_OFFSET: usize = 0;
const CONSUMER_CORE_OFFSET: usize = 2;
const ENGINE_CORE_OFFSET: usize = 4;

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
        cl_ord_id: OrderId::from_ascii("CLORD12345"),
        orig_cl_ord_id: None,
        side: types::Side::Buy,
        price: types::FixedPointArithmetic(123_456_000),
        quantity: types::FixedPointArithmetic(1_000_000),
        sender_id: EntityId::from_ascii("SENDER"),
        target_id: EntityId::from_ascii("TARGET"),
        symbol: SymbolId::from_ascii("TEST"),
        ..Default::default()
    }
}

fn make_cancel_event(orig_cl_ord_id: OrderId) -> OrderEvent {
    types::OrderEvent {
        order_type: types::OrderType::CancelOrder,
        cl_ord_id: OrderId::from_ascii("LATENCY-CANCEL-00001"),
        orig_cl_ord_id: Some(orig_cl_ord_id),
        side: types::Side::Buy,
        price: types::FixedPointArithmetic::ZERO,
        quantity: types::FixedPointArithmetic::ZERO,
        sender_id: EntityId::from_ascii("SENDER"),
        target_id: EntityId::from_ascii("TARGET"),
        symbol: SymbolId::from_ascii("TEST"),
        ..Default::default()
    }
}

fn kill_engine(tx: &Arc<spsc::spsc_lock_free::Producer<OrderEvent, RB_SIZE>>) {
    loop {
        if tx
            .push(OrderEvent {
                sender_id: EntityId::from_ascii(""),
                ..Default::default()
            })
            .is_ok()
        {
            break;
        }
        std::hint::spin_loop();
    }
}

// ── latency run ───────────────────────────────────────────────────────────────

/// Measures order creation latency only (insert into order book).
fn run_latency_create(iters: u64, histogram: &mut Histogram<u64>) -> Duration {
    let shutdown = Arc::new(AtomicBool::new(false));
    let ready = Arc::new(AtomicBool::new(false));
    let producer_core = get_cores()[PRODUCER_CORE_OFFSET % get_cores().len()];
    let consumer_core = get_cores()[CONSUMER_CORE_OFFSET % get_cores().len()];
    let engine_core = get_cores()[ENGINE_CORE_OFFSET % get_cores().len()];

    assert!(get_cores().len() >= 2, "Need at least 2 CPU cores.");

    let mut rb_rx = RingBuffer::<OrderEvent, RB_SIZE>::new();
    let mut rb_tx = RingBuffer::<(OrderEvent, OrderResult), RB_SIZE>::new();
    let mut ts_rb = RingBuffer::<u64, RB_SIZE>::new();

    let start = Instant::now();

    thread::scope(|s| {
        let (inbound_tx, inbound_rx) = rb_rx.split();
        let (outbound_tx, outbound_rx) = rb_tx.split();
        let (ts_tx, ts_rx) = ts_rb.split();

        let inbound_tx = Arc::new(inbound_tx);
        let inbound_tx_clone = Arc::clone(&inbound_tx);

        let control_rx = crossbeam::channel::bounded::<order_book::OrderBookControl>(RB_SIZE);
        let order_book = order_book::book::OrderBook::new("TEST".into());
        let mut engine = OrderBookEngine::new(
            inbound_rx,
            Some(outbound_tx),
            None,
            None,
            control_rx.1,
            order_book,
            None,
            Arc::clone(&shutdown),
        );

        let engine_handle = s.spawn(move || {
            core_affinity::set_for_current(engine_core);
            let _ = engine.run();
        });

        let ready_prod = Arc::clone(&ready);
        s.spawn(move || {
            core_affinity::set_for_current(producer_core);
            let mut ev = make_order_event();
            let mut next_order_id = OrderId::from_ascii("CREATE-ORDER-00001");

            for i in 0..iters {
                if i > 0 {
                    while !ready_prod.load(std::sync::atomic::Ordering::Acquire) {
                        std::hint::spin_loop();
                    }
                }
                ready_prod.store(false, std::sync::atomic::Ordering::Release);

                ev.cl_ord_id = next_order_id;
                next_order_id.increment();

                let ts = UtcTimestamp::now().to_unix_ns();
                loop {
                    if ts_tx.push(ts).is_ok() {
                        break;
                    }
                    std::hint::spin_loop();
                }
                loop {
                    match inbound_tx.push(ev) {
                        Ok(()) => break,
                        Err(e) => {
                            ev = e;
                            std::hint::spin_loop();
                        }
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
                if outbound_rx.try_pop().is_some() {
                    break;
                }
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

/// Measures order deletion (cancellation) latency (unlink from order book).
/// First creates all orders without measuring, then measures deletion time.
fn run_latency_delete(iters: u64, histogram: &mut Histogram<u64>) -> Duration {
    let shutdown = Arc::new(AtomicBool::new(false));
    let ready = Arc::new(AtomicBool::new(false));
    let created = Arc::new(AtomicBool::new(false));
    let producer_core = get_cores()[PRODUCER_CORE_OFFSET % get_cores().len()];
    let consumer_core = get_cores()[CONSUMER_CORE_OFFSET % get_cores().len()];
    let engine_core = get_cores()[ENGINE_CORE_OFFSET % get_cores().len()];

    assert!(get_cores().len() >= 2, "Need at least 2 CPU cores.");

    let mut rb_rx = RingBuffer::<OrderEvent, RB_SIZE>::new();
    let mut rb_tx = RingBuffer::<(OrderEvent, OrderResult), RB_SIZE>::new();
    let mut ts_rb = RingBuffer::<u64, RB_SIZE>::new();

    let start = Instant::now();

    thread::scope(|s| {
        let (inbound_tx, inbound_rx) = rb_rx.split();
        let (outbound_tx, outbound_rx) = rb_tx.split();
        let (ts_tx, ts_rx) = ts_rb.split();

        let inbound_tx = Arc::new(inbound_tx);
        let inbound_tx_clone = Arc::clone(&inbound_tx);

        let control_rx = crossbeam::channel::bounded::<order_book::OrderBookControl>(RB_SIZE);
        let order_book = order_book::book::OrderBook::new("TEST".into());
        let mut engine = OrderBookEngine::new(
            inbound_rx,
            Some(outbound_tx),
            None,
            None,
            control_rx.1,
            order_book,
            None,
            Arc::clone(&shutdown),
        );

        let engine_handle = s.spawn(move || {
            core_affinity::set_for_current(engine_core);
            let _ = engine.run();
        });

        let ready_prod = Arc::clone(&ready);
        let created_prod = Arc::clone(&created);
        s.spawn(move || {
            core_affinity::set_for_current(producer_core);
            let mut create_ev = make_order_event();
            let mut cancel_ev = make_cancel_event(create_ev.cl_ord_id);
            let mut next_order_id = OrderId::from_ascii("DELETE-ORDER-00001");
            let mut next_cancel_id = OrderId::from_ascii("DELETE-CANCEL-00001");

            for i in 0..iters {
                if i > 0 {
                    while !ready_prod.load(std::sync::atomic::Ordering::Acquire) {
                        std::hint::spin_loop();
                    }
                }
                ready_prod.store(false, std::sync::atomic::Ordering::Release);
                created_prod.store(false, std::sync::atomic::Ordering::Release);

                let current_order_id = next_order_id;
                next_order_id.increment();

                create_ev.cl_ord_id = current_order_id;
                create_ev.orig_cl_ord_id = None;
                create_ev.order_type = types::OrderType::LimitOrder;
                loop {
                    match inbound_tx.push(create_ev) {
                        Ok(()) => break,
                        Err(e) => {
                            create_ev = e;
                            std::hint::spin_loop();
                        }
                    }
                }

                while !created_prod.load(std::sync::atomic::Ordering::Acquire) {
                    std::hint::spin_loop();
                }

                let ts = UtcTimestamp::now().to_unix_ns();
                loop {
                    if ts_tx.push(ts).is_ok() {
                        break;
                    }
                    std::hint::spin_loop();
                }

                cancel_ev.cl_ord_id = next_cancel_id;
                next_cancel_id.increment();
                cancel_ev.orig_cl_ord_id = Some(current_order_id);
                cancel_ev.timestamp_ms = ts;
                loop {
                    match inbound_tx.push(cancel_ev) {
                        Ok(()) => break,
                        Err(e) => {
                            cancel_ev = e;
                            std::hint::spin_loop();
                        }
                    }
                }
            }
        });

        core_affinity::set_for_current(consumer_core);
        for _ in 0..iters {
            loop {
                if outbound_rx.try_pop().is_some() {
                    break;
                }
                std::hint::spin_loop();
            }
            created.store(true, std::sync::atomic::Ordering::Release);

            let send_ts = loop {
                if let Some(ts) = ts_rx.try_pop() {
                    break ts;
                }
                std::hint::spin_loop();
            };
            loop {
                if outbound_rx.try_pop().is_some() {
                    break;
                }
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

/// Measures deletion latency with a populated order book.
/// Builds 10000 orders at various prices, then measures cancellation latency at different depths.
fn run_latency_delete_with_depth(iters: u64, histogram: &mut Histogram<u64>) -> Duration {
    let shutdown = Arc::new(AtomicBool::new(false));
    let producer_core = get_cores()[PRODUCER_CORE_OFFSET % get_cores().len()];
    let consumer_core = get_cores()[CONSUMER_CORE_OFFSET % get_cores().len()];
    let engine_core = get_cores()[ENGINE_CORE_OFFSET % get_cores().len()];

    assert!(get_cores().len() >= 2, "Need at least 2 CPU cores.");

    let mut rb_rx = RingBuffer::<OrderEvent, RB_SIZE>::new();
    let mut rb_tx = RingBuffer::<(OrderEvent, OrderResult), RB_SIZE>::new();
    let mut ts_rb = RingBuffer::<u64, RB_SIZE>::new();

    let start = Instant::now();

    thread::scope(|s| {
        let (inbound_tx, inbound_rx) = rb_rx.split();
        let (outbound_tx, outbound_rx) = rb_tx.split();
        let (ts_tx, ts_rx) = ts_rb.split();

        let inbound_tx = Arc::new(inbound_tx);
        let inbound_tx_clone = Arc::new(inbound_tx.clone());

        let control_rx = crossbeam::channel::bounded::<order_book::OrderBookControl>(RB_SIZE);
        let order_book = order_book::book::OrderBook::new("TEST".into());
        let mut engine = OrderBookEngine::new(
            inbound_rx,
            Some(outbound_tx),
            None,
            None,
            control_rx.1,
            order_book,
            None,
            Arc::clone(&shutdown),
        );

        let engine_handle = s.spawn(move || {
            core_affinity::set_for_current(engine_core);
            let _ = engine.run();
        });

        s.spawn(move || {
            core_affinity::set_for_current(producer_core);
            let mut create_ev = make_order_event();
            let mut cancel_ev = make_cancel_event(create_ev.cl_ord_id);
            let mut next_order_id = OrderId::from_ascii("DEPTH-ORDER-00001");
            let mut next_cancel_id = OrderId::from_ascii("DEPTH-CANCEL-00001");
            let mut order_ids = Vec::new();

            // Phase 1: Build up 10000 orders at various prices (no latency measurement)
            const BOOK_SIZE: u64 = 10000;
            for i in 0..BOOK_SIZE {
                let current_order_id = next_order_id;
                order_ids.push(current_order_id);
                next_order_id.increment();

                create_ev.cl_ord_id = current_order_id;
                create_ev.price = types::FixedPointArithmetic(123_456_000 - (i as i64 * 100));
                create_ev.order_type = types::OrderType::LimitOrder;

                loop {
                    match inbound_tx.push(create_ev) {
                        Ok(()) => break,
                        Err(e) => {
                            create_ev = e;
                            std::hint::spin_loop();
                        }
                    }
                }
            }

            // Phase 2: Measure cancellation latency for iters iterations
            for iter in 0..iters {
                let ts = UtcTimestamp::now().to_unix_ns();
                loop {
                    if ts_tx.push(ts).is_ok() {
                        break;
                    }
                    std::hint::spin_loop();
                }

                cancel_ev.cl_ord_id = next_cancel_id;
                next_cancel_id.increment();
                let order_to_cancel = order_ids[(iter as usize) % order_ids.len()];
                cancel_ev.orig_cl_ord_id = Some(order_to_cancel);
                cancel_ev.timestamp_ms = ts;

                loop {
                    match inbound_tx.push(cancel_ev) {
                        Ok(()) => break,
                        Err(e) => {
                            cancel_ev = e;
                            std::hint::spin_loop();
                        }
                    }
                }
            }
        });

        core_affinity::set_for_current(consumer_core);

        // Consume acks from initial build-up phase
        const BOOK_SIZE: u64 = 10000;
        for _ in 0..BOOK_SIZE {
            loop {
                if outbound_rx.try_pop().is_some() {
                    break;
                }
                std::hint::spin_loop();
            }
        }

        // Measure cancellation latency only (not create)
        for _ in 0..iters {
            let send_ts = loop {
                if let Some(ts) = ts_rx.try_pop() {
                    break ts;
                }
                std::hint::spin_loop();
            };

            // Consume cancel ack
            loop {
                if outbound_rx.try_pop().is_some() {
                    break;
                }
                std::hint::spin_loop();
            }

            let latency = UtcTimestamp::now().to_unix_ns().saturating_sub(send_ts);
            histogram.record(latency).unwrap();
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
    let shutdown = Arc::new(AtomicBool::new(false));
    let producer_core = get_cores()[PRODUCER_CORE_OFFSET % get_cores().len()];
    let consumer_core = get_cores()[CONSUMER_CORE_OFFSET % get_cores().len()];
    let engine_core = get_cores()[ENGINE_CORE_OFFSET % get_cores().len()];

    assert!(get_cores().len() >= 2, "Need at least 2 CPU cores.");

    let mut rb_rx = RingBuffer::<OrderEvent, RB_SIZE>::new();
    let mut rb_tx = RingBuffer::<(OrderEvent, OrderResult), RB_SIZE>::new();

    let start = Instant::now();
    let mut measured_elapsed = Duration::ZERO;

    thread::scope(|s| {
        let (inbound_tx, inbound_rx) = rb_rx.split();
        let (outbound_tx, outbound_rx) = rb_tx.split();

        let inbound_tx = Arc::new(inbound_tx);
        let inbound_tx_clone = Arc::clone(&inbound_tx);

        let control_rx = crossbeam::channel::bounded::<order_book::OrderBookControl>(RB_SIZE);
        let order_book = order_book::book::OrderBook::new("TEST".into());
        let mut engine = OrderBookEngine::new(
            inbound_rx,
            Some(outbound_tx),
            None,
            None,
            control_rx.1,
            order_book,
            None,
            Arc::clone(&shutdown),
        );

        let engine_handle = s.spawn(move || {
            core_affinity::set_for_current(engine_core);
            let _ = engine.run();
        });

        s.spawn(move || {
            core_affinity::set_for_current(producer_core);
            let mut ev = make_order_event();
            for _ in 0..iters {
                loop {
                    match inbound_tx.push(ev) {
                        Ok(()) => break,
                        Err(e) => {
                            ev = e;
                            std::hint::spin_loop();
                        }
                    }
                }
            }
        });

        core_affinity::set_for_current(consumer_core);
        for _ in 0..iters {
            loop {
                if outbound_rx.try_pop().is_some() {
                    break;
                }
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
    // ── Creation Latency ──
    let mut create_histogram = Histogram::<u64>::new_with_bounds(1, 10_000_000, 3).expect("histogram");
    create_histogram.auto(true);

    c.bench_function("Order Book / latency / create", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                total += run_latency_create(LATENCY_ITERS, &mut create_histogram);
            }
            total
        });
    });

    // ── Deletion Latency ──
    let mut delete_histogram = Histogram::<u64>::new_with_bounds(1, 10_000_000, 3).expect("histogram");
    delete_histogram.auto(true);

    c.bench_function("Order Book / latency / delete", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                total += run_latency_delete(LATENCY_ITERS, &mut delete_histogram);
            }
            total
        });
    });

    // ── Deletion with Populated Book ──
    let mut delete_depth_histogram = Histogram::<u64>::new_with_bounds(1, 10_000_000, 3).expect("histogram");
    delete_depth_histogram.auto(true);

    c.bench_function("Order Book / latency / delete_with_depth", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                total += run_latency_delete_with_depth(LATENCY_ITERS, &mut delete_depth_histogram);
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
    let create_p50 = Duration::from_nanos(create_histogram.value_at_quantile(0.50));
    let create_p99 = Duration::from_nanos(create_histogram.value_at_quantile(0.99));
    let create_p999 = Duration::from_nanos(create_histogram.value_at_quantile(0.999));
    let create_n = create_histogram.len();

    let delete_p50 = Duration::from_nanos(delete_histogram.value_at_quantile(0.50));
    let delete_p99 = Duration::from_nanos(delete_histogram.value_at_quantile(0.99));
    let delete_p999 = Duration::from_nanos(delete_histogram.value_at_quantile(0.999));
    let delete_n = delete_histogram.len();

    let delete_depth_p50 = Duration::from_nanos(delete_depth_histogram.value_at_quantile(0.50));
    let delete_depth_p99 = Duration::from_nanos(delete_depth_histogram.value_at_quantile(0.99));
    let delete_depth_p999 = Duration::from_nanos(delete_depth_histogram.value_at_quantile(0.999));
    let delete_depth_n = delete_depth_histogram.len();

    // Pre-format values so we can measure their widths and align columns.
    let create_n_str = format!("{create_n}");
    let delete_n_str = format!("{delete_n}");
    let delete_depth_n_str = format!("{delete_depth_n}");
    let create_p50_str = format!("{}", create_p50.as_nanos());
    let create_p99_str = format!("{}", create_p99.as_nanos());
    let create_p999_str = format!("{}", create_p999.as_nanos());
    let delete_p50_str = format!("{}", delete_p50.as_nanos());
    let delete_p99_str = format!("{}", delete_p99.as_nanos());
    let delete_p999_str = format!("{}", delete_p999.as_nanos());
    let delete_depth_p50_str = format!("{}", delete_depth_p50.as_nanos());
    let delete_depth_p99_str = format!("{}", delete_depth_p99.as_nanos());
    let delete_depth_p999_str = format!("{}", delete_depth_p999.as_nanos());
    let tput_str = format!("{:.3} M msg/s", msgs_per_sec as f64 / 1_000_000.0);

    let val_w = create_p50_str.len()
        .max(create_p99_str.len())
        .max(create_p999_str.len())
        .max(delete_p50_str.len())
        .max(delete_p99_str.len())
        .max(delete_p999_str.len())
        .max(delete_depth_p50_str.len())
        .max(delete_depth_p99_str.len())
        .max(delete_depth_p999_str.len());
    let col1_w = "  Create Latency  n=".len() + create_n_str.len();
    let col1_w = col1_w
        .max("  Delete Latency  n=".len() + delete_n_str.len())
        .max("  Delete+Depth  n=".len() + delete_depth_n_str.len())
        .max("  Throughput      ".len());
    let col2_w = "  p999  ".len() + val_w + " ns  ".len();
    let col2_w = col2_w.max(format!("  {tput_str}  ").len());
    let total = col1_w + 1 + col2_w;

    let top = format!("┌{}┐", "─".repeat(total));
    let title = format!("│{:^total$}│", "Order Book Benchmark Summary");
    let div1 = format!("├{}┬{}┤", "─".repeat(col1_w), "─".repeat(col2_w));
    let div2 = format!("├{}┼{}┤", "─".repeat(col1_w), "─".repeat(col2_w));
    let bot = format!("└{}┘", "─".repeat(total));

    let row = |left: &str, right: &str| -> String {
        format!("│{:<col1_w$}│{:<col2_w$}│", left, right)
    };

    println!();
    println!("{top}");
    println!("{title}");
    println!("{div1}");
    println!(
        "{}",
        row(
            &format!("  Create Latency  n={create_n_str}"),
            &format!("  p50   {create_p50_str:>val_w$} ns")
        )
    );
    println!("{}", row("", &format!("  p99   {create_p99_str:>val_w$} ns")));
    println!("{}", row("", &format!("  p999  {create_p999_str:>val_w$} ns")));
    println!("{div2}");
    println!(
        "{}",
        row(
            &format!("  Delete Latency  n={delete_n_str}"),
            &format!("  p50   {delete_p50_str:>val_w$} ns")
        )
    );
    println!("{}", row("", &format!("  p99   {delete_p99_str:>val_w$} ns")));
    println!("{}", row("", &format!("  p999  {delete_p999_str:>val_w$} ns")));
    println!("{div2}");
    println!(
        "{}",
        row(
            &format!("  Delete+Depth  n={delete_depth_n_str}"),
            &format!("  p50   {delete_depth_p50_str:>val_w$} ns")
        )
    );
    println!("{}", row("", &format!("  p99   {delete_depth_p99_str:>val_w$} ns")));
    println!("{}", row("", &format!("  p999  {delete_depth_p999_str:>val_w$} ns")));
    println!("{div2}");
    println!("{}", row("  Throughput", &format!("  {tput_str}")));
    println!("{bot}");
    println!();
}

criterion_group!(benches, benchmark_order_book);
criterion_main!(benches);
