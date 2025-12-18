use std::sync::Arc;
use std::thread;
use criterion::{criterion_group, criterion_main, Criterion};
use crossbeam::queue::ArrayQueue;
use spsc::RingBuffer;

const N: usize = 1024;
const ITERS: usize = 100;

fn ringbuffer_spsc(c: &mut Criterion) {
    c.bench_function("ringbuffer SPSC", |b| {
        b.iter(|| {
            let rb = Arc::new(RingBuffer::<usize, N>::new());

            let prod = {
                let rb = rb.clone();
                thread::spawn(move || {
                    for i in 0..ITERS {
                        while rb.push(i).is_err() {}
                    }
                })
            };

            let cons = {
                let rb = rb.clone();
                thread::spawn(move || {
                    for _ in 0..ITERS {
                        while rb.pop().is_none() {}
                    }
                })
            };

            prod.join().unwrap();
            cons.join().unwrap();
        });
    });
}

fn crossbeam_spsc(c: &mut Criterion) {
    c.bench_function("crossbeam ArrayQueue SPSC", |b| {
        b.iter(|| {
            let q = Arc::new(ArrayQueue::<usize>::new(N));

            let prod = {
                let q = q.clone();
                thread::spawn(move || {
                    for i in 0..ITERS {
                        while q.push(i).is_err() {}
                    }
                })
            };

            let cons = {
                let q = q.clone();
                thread::spawn(move || {
                    for _ in 0..ITERS {
                        while q.pop().is_none() {}
                    }
                })
            };

            prod.join().unwrap();
            cons.join().unwrap();
        });
    });
}

criterion_group!(spsc, ringbuffer_spsc, crossbeam_spsc);
criterion_main!(spsc);
