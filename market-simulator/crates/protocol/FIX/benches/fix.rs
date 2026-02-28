use criterion::{criterion_group, criterion_main, Criterion};
use std::time::Duration;
use fix::parser::{FixParser};
use std::sync::Mutex;

// Global storage for results, populated during benchmarks
lazy_static::lazy_static! {
    static ref RESULTS: Mutex<Vec<BenchResult>> = Mutex::new(Vec::new());
}

struct BenchResult {
    name: String,
    p50: Duration,
    p99: Duration,
}

fn print_summary(results: &[BenchResult]) {
    let sizes = ["1k", "4k", "16k", "32k", "64k", "128k"];

    println!("\n{}", "─".repeat(105));
    println!(
        "{:<8} {:>14} {:>14} {:>10} {:>14} {:>14} {:>10}",
        "Size", "Scalar p50", "SIMD p50", "Δ p50", "Scalar p99", "SIMD p99", "Δ p99"
    );
    println!("{}", "─".repeat(105));

    for size in sizes {
        let scalar = results.iter().find(|r| r.name == format!("FIX Scalar {}", size));
        let simd   = results.iter().find(|r| r.name == format!("FIX SIMDS {}", size));

        match (scalar, simd) {
            (Some(s), Some(v)) => {
                println!(
                    "{:<8} {:>14} {:>14} {:>10} {:>14} {:>14} {:>10}",
                    size,
                    format!("{:.3?}", s.p50),
                    format!("{:.3?}", v.p50),
                    format!("{:>+.1}%", speedup(s.p50, v.p50)),
                    format!("{:.3?}", s.p99),
                    format!("{:.3?}", v.p99),
                    format!("{:>+.1}%", speedup(s.p99, v.p99)),
                );
            }
            _ => println!("{:<8} (missing data)", size),
        }
    }
    println!("{}", "─".repeat(105));
}

fn speedup(scalar: Duration, simd: Duration) -> f64 {
    let s = scalar.as_nanos() as f64;
    let v = simd.as_nanos() as f64;
    ((s - v) / s) * 100.0
}

const fn build_fix_message<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    let mut i = 0;

    // Basic FIX header
    let header = b"8=FIX.4.4\x019=0000\x0135=D\x0149=SENDER\x0156=TARGET\x0134=1\x0152=20240219-12:30:00.000\x01";

    let mut h = 0;
    while h < header.len() {
        buf[i] = header[h];
        i += 1;
        h += 1;
    }

    // Repeat fields until we hit ~N bytes
    while i + 32 < N {
        let field = b"55=EURUSD\x0138=1000000\x0144=1.23456\x01";

        let mut f = 0;
        while f < field.len() {
            buf[i] = field[f];
            i += 1;
            f += 1;
        }
    }

    // Trailer
    let trailer = b"10=123\x01";
    let mut t = 0;
    while t < trailer.len() {
        buf[i] = trailer[t];
        i += 1;
        t += 1;
    }

    buf
}

pub const FIX_1K: [u8; 1024] = build_fix_message::<1024>();
pub const FIX_4K: [u8; 4096] = build_fix_message::<4096>();
pub const FIX_16K: [u8; 16384] = build_fix_message::<16384>();
pub const FIX_32K: [u8; 32768] = build_fix_message::<32768>();
pub const FIX_64K: [u8; 65536] = build_fix_message::<65536>();
pub const FIX_128K: [u8; 131072] = build_fix_message::<131072>();

/// Benchmark for the FIX message parser, measuring the latency of parsing a simple FIX message containing three fields (8, 9, and 35). The benchmark uses a single producer thread to push timestamps into a ring buffer and a single consumer thread to pop timestamps and record the latency in a histogram. The producer and consumer threads are pinned to specific CPU cores to minimize interference and ensure accurate latency measurements.
fn benchmark_fix_scalar(data: &[u8]) {
    
        let mut parser = FixParser::new(data);

        while let Some(field) = parser.next_field_scalar() {
            std::hint::black_box(field);
        }

}

/// Benchmark for the FIX message parser, measuring the latency of parsing a simple FIX message containing three fields (8, 9, and 35). The benchmark uses a single producer thread to push timestamps into a ring buffer and a single consumer thread to pop timestamps and record the latency in a histogram. The producer and consumer threads are pinned to specific CPU cores to minimize interference and ensure accurate latency measurements.
fn benchmark_fix_simd(data: &[u8]) {
    let mut parser = FixParser::new(data);
    let fields = parser.get_fields();

    std::hint::black_box(fields);
}

fn bench_fix_scalar(c: &mut Criterion) {

    let sizes: &[(&str, &dyn Fn())] = &[
    ("FIX Scalar 1k",   &|| benchmark_fix_scalar(&FIX_1K)),
    ("FIX SIMDS 1k",    &|| benchmark_fix_simd(&FIX_1K)),
    ("FIX Scalar 4k",   &|| benchmark_fix_scalar(&FIX_4K)),
    ("FIX SIMDS 4k",    &|| benchmark_fix_simd(&FIX_4K)),
    ("FIX Scalar 16k",  &|| benchmark_fix_scalar(&FIX_16K)),
    ("FIX SIMDS 16k",   &|| benchmark_fix_simd(&FIX_16K)),
    ("FIX Scalar 32k",  &|| benchmark_fix_scalar(&FIX_32K)),
    ("FIX SIMDS 32k",   &|| benchmark_fix_simd(&FIX_32K)),
    ("FIX Scalar 64k",  &|| benchmark_fix_scalar(&FIX_64K)),
    ("FIX SIMDS 64k",   &|| benchmark_fix_simd(&FIX_64K)),
    ("FIX Scalar 128k", &|| benchmark_fix_scalar(&FIX_128K)),
    ("FIX SIMDS 128k",  &|| benchmark_fix_simd(&FIX_128K)),
];

    for (name, func) in sizes {
        let mut histogram =
            hdrhistogram::Histogram::<u64>::new_with_bounds(1, 10_000_000, 3)
                .expect("Failed to create histogram");
        histogram.auto(true);

        c.bench_function(name, |b| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let start = std::time::Instant::now();
                    func();
                    let elapsed = start.elapsed();
                    total += elapsed;
                    histogram
                        .record(elapsed.as_nanos() as u64)
                        .expect("Value out of histogram range");
                }
                total
            });
        });

        // Only runs once, after all samples for this benchmark are done
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

//criterion group for scalar benchmarks for different message sizes
criterion_group!(scalar_benches, bench_fix_scalar);
//criterion_group!(benches, bench_fix_scalar, bench_fix_simd, summary);
criterion_main!(scalar_benches);