use criterion::{criterion_group, criterion_main, Criterion};
use std::time::Instant;
use hdrhistogram::Histogram;
use protocol::fix::{FixParser};

const ITERATIONS: usize = 1_000;

const fn make_fix_64k() -> [u8; 65536] {
    let mut buf = [0u8; 65536];
    let mut i = 0;

    // Basic FIX header
    let header = b"8=FIX.4.4\x019=0000\x0135=D\x0149=SENDER\x0156=TARGET\x0134=1\x0152=20240219-12:30:00.000\x01";

    let mut h = 0;
    while h < header.len() {
        buf[i] = header[h];
        i += 1;
        h += 1;
    }

    // Repeat fields until we hit ~64 KB
    while i + 32 < 65500 {
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

pub const FIX_64K: [u8; 65536] = make_fix_64k();


const FIX_MESSAGE: &[u8] = b"8=FIX.4.2\x01\
9=512\x01\
35=D\x01\
34=999999\x01\
49=MEGA_SENDER_COMP_ID\x01\
52=20260218-12:34:56.789\x01\
56=ULTRA_LONG_TARGET_COMP_ID_FOR_MAXIMUM_PAIN\x01\
1=ACCOUNT_WITH_AN_EXCEPTIONALLY_LONG_IDENTIFIER_FOR_STRESS_TESTING_PURPOSES\x01\
11=SUPER_ULTRA_HYPER_MEGA_LONG_CLIENT_ORDER_ID_THAT_NEVER_SEEMS_TO_END_AND_JUST_KEEPS_GOING_ON_AND_ON_AND_ON_AND_ON_AND_ON_AND_ON\x01\
21=1\x01\
38=999999999\x01\
40=2\x01\
44=123456789.987654321\x01\
54=1\x01\
55=EXTREMELY_LONG_SYMBOL_NAME_DESIGNED_TO_TRIGGER_CACHE_MISSES_AND_BRANCH_PREDICTOR_FAILURES_IN_PARSING_LOGIC\x01\
59=0\x01\
60=20260218-12:34:56.789\x01\
100=VERY_LONG_EXCHANGE_IDENTIFIER_FOR_STRESSING_HASH_MAP_LOOKUPS_AND_STRING_COMPARISONS\x01\
110=999999999\x01\
111=999999999\x01\
126=20260218-23:59:59.999\x01\
140=2\x01\
141=Y\x01\
167=FUTURE_WITH_AN_OBSCENELY_VERBOSE_SECURITY_TYPE_DESCRIPTION\x01\
200=202612\x01\
205=365\x01\
207=EXCHANGE_WITH_A_NAME_SO_LONG_IT_MIGHT_CAUSE_STACK_SPILLS\x01\
223=123456789.123456789\x01\
231=1.000000000000000000\x01\
432=20260218\x01\
448=THIS_IS_A_PARTY_IDENTIFIER_THAT_IS_LONG_ENOUGH_TO_EXCEED_REASONABLE_BUFFER_SIZES_AND_FORCE_REALLOCATION\x01\
447=D\x01\
452=1\x01\
453=5\x01\
448=SECONDARY_PARTY_WITH_AN_EVEN_LONGER_IDENTIFIER_JUST_TO_BE_EXTRA_CRUEL\x01\
447=D\x01\
452=3\x01\
448=TERTIARY_PARTY_IDENTIFIER_DESIGNED_TO_ABSOLUTELY_MAX_OUT_PARSER_THROUGHPUT_AND_MEMORY_BANDWIDTH\x01\
447=D\x01\
452=12\x01\
448=QUATERNARY_PARTY_IDENTIFIER_NOW_WE_ARE_JUST_SHOWING_OFF\x01\
447=D\x01\
452=17\x01\
448=FINAL_BOSS_PARTY_IDENTIFIER_WHICH_EXISTS_SOLELY_TO_TORTURE_BENCHMARKS\x01\
447=D\x01\
452=37\x01\
10=000\x01";

/// Benchmark for the FIX message parser, measuring the latency of parsing a simple FIX message containing three fields (8, 9, and 35). The benchmark uses a single producer thread to push timestamps into a ring buffer and a single consumer thread to pop timestamps and record the latency in a histogram. The producer and consumer threads are pinned to specific CPU cores to minimize interference and ensure accurate latency measurements.
fn benchmark_fix_scalar() -> (u64, u64) {
    
    let mut histogram: Histogram<u64> = Histogram::new(3).unwrap();

    for _ in 0..ITERATIONS {
        let mut parser = FixParser::new(&FIX_64K);
        let start = Instant::now();

        while let Some(field) = parser.next_field_scalar() {
            std::hint::black_box(field);
        }

        let latency = start.elapsed().as_nanos() as u64;
        histogram.record(latency).unwrap();
    }

    (histogram.value_at_percentile(50.0), histogram.value_at_percentile(99.0))
}

/// Benchmark for the FIX message parser, measuring the latency of parsing a simple FIX message containing three fields (8, 9, and 35). The benchmark uses a single producer thread to push timestamps into a ring buffer and a single consumer thread to pop timestamps and record the latency in a histogram. The producer and consumer threads are pinned to specific CPU cores to minimize interference and ensure accurate latency measurements.
fn benchmark_fix_simd() -> (u64, u64) {
    
    let mut histogram: Histogram<u64> = Histogram::new(3).unwrap();

    for _ in 0..ITERATIONS {
        let mut parser = FixParser::new(&FIX_64K);
        let start = Instant::now();

        let fields = parser.get_fields();

        for field in fields {
            std::hint::black_box(field);
        }

        let latency = start.elapsed().as_nanos() as u64;
        histogram.record(latency).unwrap();
    }

    (histogram.value_at_percentile(50.0), histogram.value_at_percentile(99.0))
}

fn bench_fix_scalar(c: &mut Criterion) {
    c.bench_function("FIX Scalar Parser", |b| {
        b.iter(|| {
            benchmark_fix_scalar();
        });
    });
}

fn bench_fix_simd(c: &mut Criterion) {
    c.bench_function("FIX SIMD Parser", |b| {
        b.iter(|| {
            benchmark_fix_simd();
        });
    });
}

fn summary(_: &mut Criterion) {
    let (p50, p99) = benchmark_fix_scalar();
    println!("FIX Scalar Parser Latency: P50 = {} ns, P99 = {} ns", p50, p99);
    let (p50, p99) = benchmark_fix_simd();
    println!("FIX SIMD Parser Latency: P50 = {} ns, P99 = {} ns", p50, p99);
}

criterion_group!(benches, bench_fix_scalar, bench_fix_simd, summary);
criterion_main!(benches);