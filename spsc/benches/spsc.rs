use criterion::{criterion_group, criterion_main, Criterion};
use std::sync::Arc;
use std::thread;
use std::time::Instant;
use hdrhistogram::Histogram;
use std::sync::OnceLock;

use spsc::RingBuffer;
use crossbeam::queue::ArrayQueue;
use core_affinity::CoreId;

const N: usize = 4096; // Size of the ring buffer
const ITERATIONS: usize = 1_000_000;

// At the top level of your benchmark file
static BENCHMARK_CORES: OnceLock<Vec<CoreId>> = OnceLock::new();

fn get_cores() -> &'static Vec<CoreId> {
    BENCHMARK_CORES.get_or_init(|| {
        let all_cores = core_affinity::get_core_ids().unwrap();
        all_cores
    })
}

/// Benchmark your SPSC RingBuffer
fn benchmark_ringbuffer() -> (u64, u64) {
    let mut buffer = RingBuffer::<Instant, N>::new();
    let (producer, consumer) = buffer.split();

    let cores = get_cores();

    // Handle cases where we have fewer cores available
    if cores.len() < 2 {
        panic!("Need at least 2 cores available. Run with: taskset -c 0,1 cargo bench");
    }
    
    let producer_core = cores[0];
    let consumer_core = cores[1];
    
    let hist = thread::scope(|s| {
        let mut hist: Histogram<u64> = Histogram::<u64>::new_with_bounds(1, 10_000_000, 3).unwrap();

        let prod_handle = s.spawn(move || {
            core_affinity::set_for_current(producer_core);
            for _ in 0..ITERATIONS {
                let ts = Instant::now();
                loop {
                    if producer.push(ts).is_ok() {
                        break;
                    }
                    std::hint::spin_loop();
                }
            }
        });


        core_affinity::set_for_current(consumer_core);

        for _ in 0..ITERATIONS {
            loop {
                if let Some(ts) = consumer.pop() {
                    let latency_ns = ts.elapsed().as_nanos() as u64;
                    hist.record(latency_ns).unwrap();
                    break;
                }
                std::hint::spin_loop();
            }
        }

        prod_handle.join().unwrap();
        hist
    });
    
    (hist.value_at_percentile(50.0), hist.value_at_percentile(99.0))
}

/// Benchmark Crossbeam ArrayQueue
fn benchmark_crossbeam() -> (u64, u64) {
    let buffer = Arc::new(ArrayQueue::<Instant>::new(N));

    let buffer_producer = Arc::clone(&buffer);
    let buffer_consumer = Arc::clone(&buffer);

    let cores = get_cores();
    // Handle cases where we have fewer cores available
    if cores.len() < 2 {
        panic!("Need at least 2 cores available. Run with: taskset -c 0,1 cargo bench");
    }
    
    let producer_core = cores[0];
    let consumer_core = cores[1];

    let producer = thread::spawn(move || {
        core_affinity::set_for_current(producer_core);
        for _ in 0..ITERATIONS {
            let ts = Instant::now();
            loop {
                if buffer_producer.push(ts).is_ok() {
                    break;
                }
                std::hint::spin_loop();
            }
        }
    });

    let consumer = thread::spawn(move || {
        let mut hist = Histogram::<u64>::new_with_bounds(1, 10_000_000, 3).unwrap();
        core_affinity::set_for_current(consumer_core);

        for _ in 0..ITERATIONS {
            loop {
                if let Some(ts) = buffer_consumer.pop() {
                    let latency_ns = ts.elapsed().as_nanos() as u64;
                    hist.record(latency_ns).unwrap();
                    break;
                }
                std::hint::spin_loop();
            }
        }
        hist
    });

    producer.join().unwrap();
    let hist = consumer.join().unwrap();

    (hist.value_at_percentile(50.0), hist.value_at_percentile(99.0))
}

fn bench_ringbuffer(c: &mut Criterion) {
    c.bench_function("RingBuffer SPSC", |b| {
        b.iter(|| {
            benchmark_ringbuffer()
        });
    });
}

fn bench_crossbeam(c: &mut Criterion) {
    c.bench_function("Crossbeam ArrayQueue", |b| {
        b.iter(|| {
            benchmark_crossbeam()
        });
    });
}

fn comparison_summary(_: &mut Criterion) {
    // Run once to get actual latency numbers
    let (p50_ring, p99_ring) = benchmark_ringbuffer();
    let (p50_cb, p99_cb) = benchmark_crossbeam();
    
    println!("\n╔══════════════════════════════════════════════════════╗");
    println!("║          SPSC Queue Latency Comparison               ║");
    println!("║          ({} iterations per benchmark)          ║", ITERATIONS);
    println!("╠══════════════════════════════════════════════════════╣");
    println!("║ Metric      │ RingBuffer │ Crossbeam   │ Difference  ║");
    println!("╠═════════════╪════════════╪═════════════╪═════════════╣");
    println!("║ p50 latency │ {:>7} ns │ {:>8} ns │ {:>6.2}x     ║", 
             p50_ring, p50_cb, p50_cb as f64 / p50_ring as f64);
    println!("║ p99 latency │ {:>7} ns │ {:>8} ns │ {:>6.2}x     ║", 
             p99_ring, p99_cb, p99_cb as f64 / p99_ring as f64);
    println!("╚═════════════╧════════════╧═════════════╧═════════════╝");
    
    let speedup_p50 = p50_cb as f64 / p50_ring as f64;
    if speedup_p50 > 1.0 {
        println!("  ✓ RingBuffer is {:.2}x faster at p50", speedup_p50);
    } else {
        println!("  ✗ Crossbeam is {:.2}x faster at p50", 1.0 / speedup_p50);
    }
    
    let speedup_p99 = p99_cb as f64 / p99_ring as f64;
    if speedup_p99 > 1.0 {
        println!("  ✓ RingBuffer is {:.2}x faster at p99", speedup_p99);
    } else {
        println!("  ✗ Crossbeam is {:.2}x faster at p99", 1.0 / speedup_p99);
    }
    println!();
}

criterion_group!(benches, bench_ringbuffer, bench_crossbeam, comparison_summary);
criterion_main!(benches);
