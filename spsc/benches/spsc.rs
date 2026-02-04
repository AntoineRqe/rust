use criterion::{criterion_group, criterion_main, Criterion};
use std::sync::Arc;
use std::thread;
use std::time::Instant;
use hdrhistogram::Histogram;

use spsc::RingBuffer;
use crossbeam::queue::ArrayQueue;

const N: usize = 1024;
const ITERATIONS: usize = 1_000_000;

/// Benchmark your SPSC RingBuffer
fn benchmark_ringbuffer() -> (u64, u64) {
    let mut buffer = RingBuffer::<Instant, N>::new();
    let (producer, consumer) = buffer.split();

    let cores = core_affinity::get_core_ids().unwrap();
    let producer_core = cores[0];
    let consumer_core = cores[1];

    let hist = thread::scope(|s| {

        let mut hist = Histogram::<u64>::new_with_bounds(1, 10_000_000, 3).unwrap();

        let prod_handle = s.spawn(|| {
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

        let cons_handle = s.spawn(|| {
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
            hist
        });

        prod_handle.join().unwrap();
        let hist = cons_handle.join().unwrap();

        hist
    });

    (hist.value_at_percentile(50.0), hist.value_at_percentile(99.0))
}

/// Benchmark Crossbeam ArrayQueue
fn benchmark_crossbeam() -> (u64, u64) {
    let buffer = Arc::new(ArrayQueue::<Instant>::new(N));
    let mut hist = Histogram::<u64>::new_with_bounds(1, 10_000_000, 3).unwrap();

    let buffer_producer = Arc::clone(&buffer);
    let buffer_consumer = Arc::clone(&buffer);

    let cores = core_affinity::get_core_ids().unwrap();
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

fn spsc_comparison_benchmark(c: &mut Criterion) {
    // Run benchmarks once to get comparison data
    let (p50_ring, p99_ring) = benchmark_ringbuffer();
    let (p50_cb, p99_cb) = benchmark_crossbeam();
    
    println!("\n╔══════════════════════════════════════════════╗");
    println!("║        SPSC Queue Latency Comparison        ║");
    println!("╠══════════════════════════════════════════════╣");
    println!("║ Metric  │ RingBuffer │ Crossbeam │ Speedup  ║");
    println!("╠═════════╪════════════╪═══════════╪══════════╣");
    println!("║ p50 (ns)│ {:>10} │ {:>9} │ {:.2}x     ║", 
             p50_ring, p50_cb, p50_cb as f64 / p50_ring as f64);
    println!("║ p99 (ns)│ {:>10} │ {:>9} │ {:.2}x     ║", 
             p99_ring, p99_cb, p99_cb as f64 / p99_ring as f64);
    println!("╚═════════╧════════════╧═══════════╧══════════╝\n");
    
    // Still run the criterion benchmark
    c.bench_function("RingBuffer vs Crossbeam", |b| {
        b.iter(|| {
            benchmark_ringbuffer();
            benchmark_crossbeam();
        });
    });
}

criterion_group!(benches, spsc_comparison_benchmark);
criterion_main!(benches);
