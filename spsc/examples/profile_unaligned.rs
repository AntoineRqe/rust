use spsc::spsc_lock_free::{Producer, Consumer, RingBuffer};
use std::thread;
extern crate core_affinity;
use std::sync::OnceLock;
use core_affinity::CoreId;

const N: usize = 4096; // Size of the ring buffer
#[allow(dead_code)]
const BATCH_SIZE: usize = 64; // Size of batch operations

const NUM_ITEMS: usize = 500_000_000;

// At the top level of your benchmark file
static BENCHMARK_CORES: OnceLock<Vec<CoreId>> = OnceLock::new();

fn get_cores() -> &'static Vec<CoreId> {
    BENCHMARK_CORES.get_or_init(|| {
        let all_cores = core_affinity::get_core_ids().unwrap();
        all_cores
    })
}
#[allow(dead_code)]
fn produce_single(producer: &Producer<usize, N>, start: usize, count: usize) {
    for i in 0..count {
        let item = start + i;

        loop {
            if producer.push(item).is_ok() {
                break; // success
            }
            std::hint::spin_loop();
        }
    }
}

#[allow(dead_code)]
fn consume_single(
    consumer: &Consumer<usize, N>,
    total_count: usize,
) {
    let mut expected: usize = 0;

    for _ in 0..total_count {
        let item;
        loop {
            if let Some(val) = consumer.pop() {
                item = val;
                break;
            } else {
                std::hint::spin_loop();
            }
        }

        assert_eq!(item, expected);
        expected += 1;
    }
}

#[allow(dead_code)]
fn consume_batch(consumer: &Consumer<usize, N>, total_count: usize) {
    let mut buffer = [0; BATCH_SIZE]; // replace 0 with the appropriate type
    let mut expected: usize = 0;
    let mut items_consumed = 0;

    while items_consumed < total_count {
        let to_consume = (total_count - items_consumed).min(BATCH_SIZE);
        let mut received = 0;

        while received < to_consume {
            let n = consumer.pop_batch(&mut buffer[received..to_consume]);
            if n == 0 {
                // Buffer empty, spin
                std::hint::spin_loop();
            } else {
                received += n;
            }
        }

        for &item in &buffer[..to_consume] {
            //println!("Consumed: {}", item);
            assert_eq!(item, expected);
            expected += 1;
        }

        items_consumed += to_consume;
    }
}

#[allow(dead_code)]
fn produce_batch(producer: &Producer<usize, N>, start: usize, count: usize) {
    let mut batch = [0; BATCH_SIZE]; // replace 0 with the appropriate type

    for batch_start in (start..start + count).step_by(BATCH_SIZE) {
        let end = (batch_start + BATCH_SIZE).min(start + count);
        let actual_batch_size = end - batch_start;

        // Fill the batch array
        for i in 0..actual_batch_size {
            batch[i] = batch_start + i;
        }

        let mut pushed = 0;

        while pushed < actual_batch_size {
            let n = producer.push_batch(&batch[pushed..actual_batch_size]);
            if n == 0 {
                // Buffer full, spin
                std::hint::spin_loop();
            } else {
                pushed += n;
            }
        }
        //println!("Produced batch starting at {}", batch_start);
    }
}

#[allow(dead_code)]
fn profile_single_spsc(producer: Producer<usize, N>, consumer: Consumer<usize, N>) {

    println!("Starting SPSC RingBuffer profile for single item operations...");

    let core_ids = get_cores();

    thread::scope(|s| {
        let prod = s.spawn({
            let _ = core_affinity::set_for_current(core_ids[0]);

            move || {
                produce_single(&producer, 0, NUM_ITEMS);
            }
        });

        let _ = core_affinity::set_for_current(core_ids[1]);
        consume_single(&consumer, NUM_ITEMS);

        prod.join().unwrap();
    });

    println!("SPSC RingBuffer single item profile completed.");
}

#[allow(dead_code)]
fn profile_batch_spsc(producer: Producer<usize, N>, consumer: Consumer<usize, N>) {

    println!("Starting SPSC RingBuffer profile for batch item operations...");

    let core_ids = get_cores();

    thread::scope(|s| {
        let prod = s.spawn({
            let _ = core_affinity::set_for_current(core_ids[0]);

            move || {
                produce_batch(&producer, 0, NUM_ITEMS);
            }
        });

        let _ = core_affinity::set_for_current(core_ids[1]);
        consume_batch(&consumer, NUM_ITEMS);

        prod.join().unwrap();
    });

    println!("SPSC RingBuffer batch item profile completed.");
}

fn main() {

    let mut rb = RingBuffer::<usize, N>::new();
    let (producer, consumer) = rb.split();

    profile_single_spsc(producer, consumer);
    //profile_batch_spsc(producer, consumer);

    println!("SPSC RingBuffer profile completed.");
}