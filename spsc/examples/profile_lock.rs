use spsc::spsc_lock::{SpscLock};
use std::thread;
extern crate core_affinity;
use std::sync::OnceLock;
use core_affinity::CoreId;
use std::sync::Arc;

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
fn produce_single(producer: Arc<SpscLock<usize, N>>, start: usize, count: usize) {
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
fn consume_single(consumer: Arc<SpscLock<usize, N>>, total_count: usize) {
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
fn profile_single_spsc(producer: Arc<SpscLock<usize, N>>, consumer: Arc<SpscLock<usize, N>>) {

    println!("Starting SPSC Lock profile for single item operations...");

    let core_ids = get_cores();

    thread::scope(|s| {
        let prod = s.spawn({
            let _ = core_affinity::set_for_current(core_ids[0]);

            move || {
                produce_single(Arc::clone(&producer), 0, NUM_ITEMS);
            }
        });

        let _ = core_affinity::set_for_current(core_ids[1]);
        consume_single(Arc::clone(&consumer), NUM_ITEMS);

        prod.join().unwrap();
    });

    println!("SPSC Lock single item profile completed.");
}

fn main() {

    let rb = Arc::new(SpscLock::<usize, N>::new());

    profile_single_spsc(Arc::clone(&rb), Arc::clone(&rb));
    //profile_batch_spsc(producer, consumer);

    println!("SPSC Lock profile completed.");
}