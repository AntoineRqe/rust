use atomics_and_locks::mutex::Mutex;
use std::time::Instant;

const N_THREADS: usize = 10;
const N_ITER: usize = 1_000_000;

#[test]
fn perf_mutex() {
    let m = Mutex::new(0);
    std::thread::scope(|s| {
        for _ in 0..N_THREADS {
            let m = &m;
            s.spawn(move || {
                for _ in 0..N_ITER {
                    let mut g = m.lock();
                    *g += 1;
                }
            });
        }
    });
    assert_eq!(*m.lock(), N_THREADS * N_ITER);
}

// Add this for flamegraph profiling
#[test]
#[ignore]
fn perf_mutex_flamegraph() {
    println!("Starting mutex performance test...");
    println!("Threads: {}, Iterations: {}", N_THREADS, N_ITER);
    
    let m = Mutex::new(0);
    let start = Instant::now();
    
    std::thread::scope(|s| {
        for _ in 0..N_THREADS {
            let m = &m;
            s.spawn(move || {
                for _ in 0..N_ITER {
                    let mut g = m.lock();
                    *g += 1;
                }
            });
        }
    });
    
    let elapsed = start.elapsed();
    let total_ops = N_THREADS * N_ITER;
    
    println!("Time: {:?}", elapsed);
    println!("Total operations: {}", total_ops);
    println!("Ops/sec: {:.2}", total_ops as f64 / elapsed.as_secs_f64());
    println!("Avg time per lock: {:.2} ns", elapsed.as_nanos() as f64 / total_ops as f64);
    
    assert_eq!(*m.lock(), total_ops);
}

// Add comparison with std::sync::Mutex
#[test]
#[ignore]
fn perf_compare_mutex_implementations() {
    println!("\n=== Custom Mutex ===");
    {
        let m = Mutex::new(0);
        let start = Instant::now();
        
        std::thread::scope(|s| {
            for _ in 0..N_THREADS {
                let m = &m;
                s.spawn(move || {
                    for _ in 0..N_ITER {
                        let mut g = m.lock();
                        *g += 1;
                    }
                });
            }
        });
        
        let elapsed = start.elapsed();
        println!("Time: {:?}", elapsed);
        println!("Ops/sec: {:.2}", (N_THREADS * N_ITER) as f64 / elapsed.as_secs_f64());
    }
    
    println!("\n=== std::sync::Mutex ===");
    {
        let m = std::sync::Mutex::new(0);
        let start = Instant::now();
        
        std::thread::scope(|s| {
            for _ in 0..N_THREADS {
                let m = &m;
                s.spawn(move || {
                    for _ in 0..N_ITER {
                        let mut g = m.lock().unwrap();
                        *g += 1;
                    }
                });
            }
        });
        
        let elapsed = start.elapsed();
        println!("Time: {:?}", elapsed);
        println!("Ops/sec: {:.2}", (N_THREADS * N_ITER) as f64 / elapsed.as_secs_f64());
    }
}

// Add test with less contention to see other hotspots
#[test]
#[ignore]
fn perf_mutex_with_work() {
    println!("Testing mutex with work between locks...");
    
    let m = Mutex::new(0);
    let start = Instant::now();
    
    std::thread::scope(|s| {
        for _ in 0..N_THREADS {
            let m = &m;
            s.spawn(move || {
                for i in 0..N_ITER {
                    // Simulate some work outside the lock
                    let work = (i % 1000) as u64;
                    let _result = work.wrapping_mul(work);
                    
                    // Then acquire lock
                    let mut g = m.lock();
                    *g += 1;
                }
            });
        }
    });
    
    let elapsed = start.elapsed();
    println!("Time: {:?}", elapsed);
    println!("Ops/sec: {:.2}", (N_THREADS * N_ITER) as f64 / elapsed.as_secs_f64());
}