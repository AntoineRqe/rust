use criterion::{criterion_group, criterion_main, Criterion};
use std::sync::Arc;
use std::thread;
use std::time::Instant;
use hdrhistogram::Histogram;
use std::sync::OnceLock;

use spsc::spsc_lock_free::RingBuffer;
use crossbeam::queue::ArrayQueue;
use core_affinity::CoreId;
use std::time::Duration;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool};


const N: usize = 2; // Size of the ring buffer
const PRODUCER_CORE_OFFSET: usize = 0; // Offset for producer core
const CONSUMER_CORE_OFFSET: usize = 128; // Offset for consumer core
const BENCH_ITERS: u64 = 100_000; // Number of iterations for benchmarks

#[inline]
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
}

// At the top level of your benchmark file
static BENCHMARK_CORES: OnceLock<Vec<CoreId>> = OnceLock::new();

fn get_cores() -> &'static Vec<CoreId> {
    BENCHMARK_CORES.get_or_init(|| {
        let all_cores = core_affinity::get_core_ids().unwrap();
        all_cores
    })
}

fn benchmark_latency_ringbuffer_non_blocking(iters: u64, histogram: &mut Histogram<u64>) -> Duration {
    
    let cores = get_cores();

    if cores.len() < 2 {
        panic!("Need at least 2 cores available.");
    }

    let (producer_core, consumer_core) = (cores[PRODUCER_CORE_OFFSET % cores.len()], cores[CONSUMER_CORE_OFFSET % cores.len()]);
    
    let mut buffer = RingBuffer::<Instant, N>::new();
    let start = Instant::now();

    thread::scope(|s| {
        let (producer, consumer) = buffer.split();

        s.spawn(move || {
            core_affinity::set_for_current(producer_core);
            for _ in 0..iters {
                loop {
                    if producer.try_push(Instant::now()).is_ok() {
                        break;
                    }
                    std::hint::spin_loop();
                }
            }
        });

        core_affinity::set_for_current(consumer_core);

        for _ in 0..iters {
            let ts = loop {
                if let Some(ts) = consumer.try_pop() {
                    break ts;
                }
                std::hint::spin_loop();
            };

            histogram
            .record(ts.elapsed()
            .as_nanos() as u64)
            .expect("Value out of histogram range");

            // Signal producer to send the next item
        }
    });

    start.elapsed()
}

fn benchmark_throughput_ringbuffer_non_blocking(iters: u64, histogram: &mut Histogram<u64>, consumer_delay_ns: u64) -> Duration {
    
    let cores = get_cores();

    if cores.len() < 2 {
        panic!("Need at least 2 cores available.");
    }

    let (producer_core, consumer_core) = (cores[PRODUCER_CORE_OFFSET % cores.len()], cores[CONSUMER_CORE_OFFSET % cores.len()]);
    
    let mut buffer = RingBuffer::<Instant, N>::new();
    let start = Instant::now();

    thread::scope(|s| {
        let (producer, consumer) = buffer.split();

        s.spawn(move || {
            core_affinity::set_for_current(producer_core);
            for _ in 0..iters {
                loop {
                    if producer.try_push(Instant::now()).is_ok() {
                        break;
                    }
                    std::hint::spin_loop();
                }
            }
        });

        core_affinity::set_for_current(consumer_core);

        for _ in 0..iters {
            let ts = loop {
                if let Some(ts) = consumer.try_pop() {
                    break ts;
                }
                std::hint::spin_loop();
            };

            histogram
            .record(ts.elapsed()
            .as_nanos() as u64)
            .expect("Value out of histogram range");

            // Simulate consumer processing work — tune this to create backpressure
            spin_ns(consumer_delay_ns);
        }
    });

    start.elapsed()
}

/// Benchmark your SPSC RingBuffer
fn benchmark_latency_ringbuffer_blocking(iters: u64, histogram: &mut Histogram<u64>) -> Duration {
    
    let cores = get_cores();
    let ready = Arc::new(AtomicBool::new(false)); // consumer signals "I popped, send next"

    if cores.len() < 2 {
        panic!("Need at least 2 cores available.");
    }

    let (producer_core, consumer_core) = (cores[PRODUCER_CORE_OFFSET % cores.len()], cores[CONSUMER_CORE_OFFSET % cores.len()]);
    
    let mut buffer = RingBuffer::<Instant, N>::new();
    let start = Instant::now();

    thread::scope(|s| {
        let (producer, consumer) = buffer.split();
        let ready_producer = Arc::clone(&ready);

        s.spawn(move || {
            core_affinity::set_for_current(producer_core);
            for i in 0..iters {
                if i > 0 {
                    // Wait for consumer to signal that it popped the last item
                    while !ready_producer.load(std::sync::atomic::Ordering::Acquire) {
                        std::hint::spin_loop();
                    }
                    ready_producer.store(false, std::sync::atomic::Ordering::Release);
                }

                loop {
                        if producer.push(Instant::now()).is_ok() {
                            break;
                        }
                        std::hint::spin_loop();
                }
            }
        });

        core_affinity::set_for_current(consumer_core);

        for _ in 0..iters {
            let ts = loop {
                if let Some(ts) = consumer.pop() {
                    break ts;
                }
                std::hint::spin_loop();
            };

            histogram.record(ts.elapsed().as_nanos() as u64).expect("Value out of histogram range");

            // Signal producer to send the next item
            ready.store(true, std::sync::atomic::Ordering::Release);
        }
    });

    start.elapsed()
}

fn benchmark_throughput_ringbuffer_blocking(iters: u64, histogram: &mut Histogram<u64>, consumer_delay_ns: u64) -> Duration {
    
    let cores = get_cores();

    if cores.len() < 2 {
        panic!("Need at least 2 cores available.");
    }

    let (producer_core, consumer_core) = (cores[PRODUCER_CORE_OFFSET % cores.len()], cores[CONSUMER_CORE_OFFSET % cores.len()]);
    
    let mut buffer = RingBuffer::<Instant, N>::new();
    let start = Instant::now();

    thread::scope(|s| {
        let (producer, consumer) = buffer.split();

        s.spawn(move || {
            core_affinity::set_for_current(producer_core);
            for _ in 0..iters {
                loop {
                    if producer.push(Instant::now()).is_ok() {
                        break;
                    }
                    std::hint::spin_loop();
                }
            }
        });

        core_affinity::set_for_current(consumer_core);

        for _ in 0..iters {
            let ts = loop {
                if let Some(ts) = consumer.pop() {
                    break ts;
                }
                std::hint::spin_loop();
            };

            histogram
            .record(ts.elapsed()
            .as_nanos() as u64)
            .expect("Value out of histogram range");

            // Simulate consumer processing work — tune this to create backpressure
            spin_ns(consumer_delay_ns);
        }
    });

    start.elapsed()
}

/// Benchmark Crossbeam ArrayQueue
fn benchmark_latency_crossbeam(iters: u64, histogram: &mut Histogram<u64>) -> Duration {
    let buffer = Arc::new(ArrayQueue::<Instant>::new(N));
    let ready = Arc::new(AtomicBool::new(false)); // consumer signals "I popped, send next"
    let buffer_consumer = Arc::clone(&buffer);

    let cores = get_cores();
    // Handle cases where we have fewer cores available
    if cores.len() < 2 {
        panic!("Need at least 2 cores available. Run with: taskset -c 0,1 cargo bench");
    }
    
    let producer_core = cores[PRODUCER_CORE_OFFSET % cores.len()];
    let consumer_core = cores[CONSUMER_CORE_OFFSET % cores.len()];

    let start = Instant::now();

    thread::scope(|s| {
        let buffer_producer = Arc::clone(&buffer);
        let ready_producer = Arc::clone(&ready);
        let producer = s.spawn(move || {

            core_affinity::set_for_current(producer_core);
            for i in 0..iters {
                if i > 0 {
                    // Wait for consumer to signal that it popped the last item
                    while !ready_producer.load(std::sync::atomic::Ordering::Acquire) {
                        std::hint::spin_loop();
                    }
                    ready_producer.store(false, std::sync::atomic::Ordering::Release);
                }

                loop {
                    if buffer_producer.push(Instant::now()).is_ok() {
                        break;
                    }
                    std::hint::spin_loop();
                }
            }
        });

        core_affinity::set_for_current(consumer_core);

        for _ in 0..iters {
            let ts = loop {
                if let Some(ts) = buffer_consumer.pop() {
                    break ts;
                }
                std::hint::spin_loop();
            };

            let latency = ts.elapsed();
            histogram.record(latency.as_nanos() as u64).expect("Value out of histogram");

            // Signal producer to send the next item
            ready.store(true, std::sync::atomic::Ordering::Release);
        }

        producer.join().unwrap();
    });

    start.elapsed()
}

/// Benchmark Crossbeam ArrayQueue
fn benchmark_throughput_crossbeam(iters: u64, histogram: &mut Histogram<u64>, consumer_delay_ns: u64) -> Duration {
    let buffer = Arc::new(ArrayQueue::<Instant>::new(N));
    let buffer_consumer = Arc::clone(&buffer);

    let cores = get_cores();
    // Handle cases where we have fewer cores available
    if cores.len() < 2 {
        panic!("Need at least 2 cores available. Run with: taskset -c 0,1 cargo bench");
    }
    
    let producer_core = cores[PRODUCER_CORE_OFFSET % cores.len()];
    let consumer_core = cores[CONSUMER_CORE_OFFSET % cores.len()];

    let start = Instant::now();

    thread::scope(|s| {
        let buffer_producer = Arc::clone(&buffer);
        let producer = s.spawn(move || {

            core_affinity::set_for_current(producer_core);
            for _ in 0..iters {
                loop {
                    if buffer_producer.push(Instant::now()).is_ok() {
                        break;
                    }
                    std::hint::spin_loop();
                }
            }
        });

        core_affinity::set_for_current(consumer_core);

        for _ in 0..iters {
            let ts = loop {
                if let Some(ts) = buffer_consumer.pop() {
                    break ts;
                }
                std::hint::spin_loop();
            };

            let latency = ts.elapsed();
            histogram.record(latency.as_nanos() as u64).expect("Value out of histogram");

            // Simulate consumer processing work — tune this to create backpressure
            spin_ns(consumer_delay_ns);
        }

        producer.join().unwrap();
    });

    start.elapsed()
}

fn benchmark_latency(c: &mut Criterion) {
    let functions: &[(&str, fn(u64, &mut Histogram<u64>) -> Duration); 3] = &[
        ("Latency Crossbeam ArrayQueue", benchmark_latency_crossbeam),
        ("Latency RingBuffer SPSC Blocking", benchmark_latency_ringbuffer_blocking),
        ("Latency RingBuffer SPSC Non-Blocking", benchmark_latency_ringbuffer_non_blocking),
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

        RESULTS.lock().unwrap().push(BenchResult {
            name: name.to_string(),
            p50,
            p99,
        });
    }

    // Print summary table after all benchmarks in this group complete
    let results = RESULTS.lock().unwrap();
    print_summary(&results);
}

fn benchmark_throughput(c: &mut Criterion) {
    let functions: &[(&str, fn(u64, &mut Histogram<u64>, u64) -> Duration)] = &[
        ("Crossbeam ArrayQueue", benchmark_throughput_crossbeam),
        ("RingBuffer Blocking", benchmark_throughput_ringbuffer_blocking),
        ("RingBuffer Non-Blocking", benchmark_throughput_ringbuffer_non_blocking),
    ];

    let delays = [0u64, 200, 500, 1000, 2000, 5000];
    let mut all_results: Vec<BenchResult> = Vec::new();

    for consumer_delay_ns in delays {
        let mut group_results: Vec<BenchResult> = Vec::new();

        for (name, func) in functions {
            let mut histogram = Histogram::<u64>::new_with_bounds(1, 10_000_000, 3)
                .expect("Failed to create histogram");
            histogram.auto(true);

            // Unique name per delay so Criterion tracks them separately
            let bench_name = format!("{} delay={}ns", name, consumer_delay_ns);

            c.bench_function(&bench_name, |b| {
                b.iter_custom(|_| {
                    func(BENCH_ITERS, &mut histogram, consumer_delay_ns)
                });
            });

            group_results.push(BenchResult {
                name: bench_name,
                p50: Duration::from_nanos(histogram.value_at_quantile(0.50)),
                p99: Duration::from_nanos(histogram.value_at_quantile(0.99)),
            });
        }

        // Print a mini table per delay level so you see results as they come
        // println!("\n── consumer_delay = {}ns ──", consumer_delay_ns);
        // print_summary(&group_results);
        all_results.extend(group_results);
    }

    // Final combined table
    println!("\n══ Full sweep ══");
    print_summary(&all_results);
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
        "{:<name_width$} │ {:>10} │ {:>10} │ {:>10}",
        "Name",
        "p50 (ns)",
        "p99 (ns)",
        "vs base",
        name_width = name_width
    );

    println!("{}", "─".repeat(name_width + 36));

    for r in results {
        let p50 = r.p50.as_nanos() as f64;
        let p99 = r.p99.as_nanos() as f64;

        let ratio = if base_p50 > 0.0 {
            p50 / base_p50
        } else {
            1.0
        };

        println!(
            "{:<name_width$} │ {:>10} │ {:>10} │ {:>9.2}x",
            r.name,
            p50 as u128,
            p99 as u128,
            ratio,
            name_width = name_width
        );
    }

    println!();
}

criterion_group!(benches, benchmark_throughput);
criterion_main!(benches);
