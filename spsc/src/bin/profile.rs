use spsc::{RingBuffer, Timed};
use std::sync::Arc;
use std::thread;
extern crate core_affinity;
use hdrhistogram::Histogram;

fn produce_single(rb: &RingBuffer<usize, 4096>, start: usize, count: usize) {
    for i in 0..count {
        let item = start + i;

        let mut spins = 0;

        loop {
            if rb.push(item).is_ok() {
                break; // success
            }

            // Buffer full, spin for a while
            if spins < 1_000 {
                std::hint::spin_loop();
                spins += 1;
            } else {
                std::thread::yield_now();
                spins = 0; // reset after yielding
            }
        }

        //println!("Produced: {}", item);
    }
}


fn consume_single(
    rb: &RingBuffer<usize, 4096>,
    mut hist: Option<&mut Histogram<u64>>,
    total_count: usize,
) {
    let mut expected: usize = 0;

    match hist.as_deref_mut() {
        Some(hist) => {
            for count in 0..total_count {
                let mut spins = 0;
                loop {
                    if let Some(_) = rb.pop_and_record(hist, count) {
                        break;
                    }

                    if spins < 1_000 {
                        std::hint::spin_loop();
                        spins += 1;
                    } else {
                        std::thread::yield_now();
                        spins = 0;
                    }
                }
            }
        }

        None => {
            for _ in 0..total_count {
                let item;
                loop {
                    if let Some(val) = rb.pop() {
                        item = val.value;
                        break;
                    } else {
                        std::thread::yield_now();
                    }
                }

                assert_eq!(item, expected);
                expected += 1;
            }
        }
    }
}


#[allow(dead_code)]
const BATCH_SIZE: usize = 64;

#[allow(dead_code)]
fn consume_batch(rb: &RingBuffer<usize, 4096>, total_count: usize) {
    let mut buffer = [Timed { value: 0, timestamp: std::time::Instant::now() }; BATCH_SIZE]; // replace 0 with the appropriate type
    let mut expected: usize = 0;
    let mut items_consumed = 0;

    while items_consumed < total_count {
        let to_consume = (total_count - items_consumed).min(BATCH_SIZE);
        let mut received = 0;

        while received < to_consume {
            let n = rb.pop_batch(&mut buffer[received..to_consume]);
            if n == 0 {
                // Buffer empty, spin
                std::thread::yield_now();
            } else {
                received += n;
            }
        }

        for &item in &buffer[..to_consume] {
            //println!("Consumed: {}", item.value);
            assert_eq!(item.value, expected);
            expected += 1;
        }

        items_consumed += to_consume;
    }
}

#[allow(dead_code)]
fn produce_batch(rb: &RingBuffer<usize, 4096>, start: usize, count: usize) {
    let mut batch = [0; BATCH_SIZE]; // replace 0 with the appropriate type

    for batch_start in (start..start + count).step_by(BATCH_SIZE) {
        let end = (batch_start + BATCH_SIZE).min(start + count);
        let actual_batch_size = end - batch_start;

        // Fill the batch array
        for i in 0..actual_batch_size {
            batch[i] = start + batch_start + i;
        }

        let mut pushed = 0;

        while pushed < actual_batch_size {
            let n = rb.push_batch(&batch[pushed..actual_batch_size]);
            if n == 0 {
                // Buffer full, spin
                std::thread::yield_now();
            } else {
                pushed += n;
            }
        }
        //println!("Produced batch starting at {}", batch_start);
    }
}

fn main() {

    println!("Starting SPSC RingBuffer profile...");
    let core_ids = core_affinity::get_core_ids().unwrap();


    const BUFFER_SIZE: usize = 4096;
    const NUM_ITEMS: usize = 500_000_000;

    let rb = Arc::new(RingBuffer::<usize, BUFFER_SIZE>::new());
    let rb_producer = rb.clone();
    let rb_consumer = rb.clone();

    let prod = thread::spawn({
        let _ = core_affinity::set_for_current(core_ids[0]);

        let rb = rb_producer;
        move || {
            produce_single(&rb, 0, NUM_ITEMS);
            // produce_batch(&rb, 0, NUM_ITEMS);
        }
    });

    // let cons = thread::spawn({
    //     let _ = core_affinity::set_for_current(core_ids[1]);
    //     let rb = rb_consumer;
    //     move || {
    //         consume_single(&rb, NUM_ITEMS);
    //         // consume_batch(&rb, NUM_ITEMS);
    //     }
    // });

    let hist = thread::scope(|s| {
        let cons = s.spawn(move || {
            let _ = core_affinity::set_for_current(core_ids[1]);

            let mut hist = Histogram::<u64>::new_with_bounds(
                1,
                10_000_000,
                3,
            ).unwrap();

            consume_single(&rb_consumer, Some(&mut hist), NUM_ITEMS);
            hist
        });

        // producer can run concurrently here if needed

        cons.join().unwrap()
    });

    prod.join().unwrap();

    println!("Latency profile (nanoseconds):");
    println!("p50 = {} ns", hist.value_at_percentile(50.0));
    println!("p99 = {} ns", hist.value_at_percentile(99.0));
    println!("max = {} ns", hist.max());

    println!("SPSC RingBuffer profile completed.");
}