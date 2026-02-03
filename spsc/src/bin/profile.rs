use spsc::RingBuffer;
use std::sync::Arc;
use std::thread;
extern crate core_affinity;

fn produce_single(rb: &RingBuffer<usize, 4096>, start: usize, count: usize) {
    for i in 0..count {
        let item = start + i;
        while rb.push(item).is_err() {
            // Buffer full, spin
            std::thread::yield_now();
        }
        //println!("Produced: {}", item);
    }
}

fn consume_single(rb: &RingBuffer<usize, 4096>, total_count: usize) {
    let mut expected: usize = 0;
    for _ in 0..total_count {
        let item;
        loop {
            if let Some(val) = rb.pop() {
                //println!("Consumed: {}", val);
                item = val;
                break;
            } else {
                // Buffer empty, spin
                std::thread::yield_now();
            }
        }
        assert_eq!(item, expected);
        expected += 1;
    }
}

const BATCH_SIZE: usize = 64;


fn consume_batch(rb: &RingBuffer<usize, 4096>, total_count: usize) {
    let mut buffer = [0; BATCH_SIZE]; // replace 0 with the appropriate type
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
            //println!("Consumed: {}", item);
            assert_eq!(item, expected);
            expected += 1;
        }

        items_consumed += to_consume;
    }
}

fn produce_batch(rb: &RingBuffer<usize, 4096>, start: usize, count: usize) {
    let mut batch = [0; BATCH_SIZE]; // replace 0 with the appropriate type

    for batch_start in (start..start + count).step_by(BATCH_SIZE) {
        let end = (batch_start + BATCH_SIZE).min(start + count);
        let actual_batch_size = end - batch_start;

        // Fill the batch array
        for i in 0..actual_batch_size {
            batch[i] = batch_start + i; // or whatever value you need
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

    let cons = thread::spawn({
        let _ = core_affinity::set_for_current(core_ids[1]);
        let rb = rb_consumer;
        move || {
            consume_single(&rb, NUM_ITEMS);
            // consume_batch(&rb, NUM_ITEMS);
        }
    });

    prod.join().unwrap();
    cons.join().unwrap();

    println!("SPSC RingBuffer profile completed.");
}