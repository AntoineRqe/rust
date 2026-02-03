use spsc::RingBuffer;
use std::sync::Arc;
use std::thread;


fn main() {

    println!("Starting SPSC RingBuffer profile...");

    let rb = Arc::new(RingBuffer::<usize, 1024>::new());
    let rb_producer = rb.clone();

    let prod = thread::spawn({
        let rb = rb_producer;
        move || {
            for i in 0..10_000_000 {
                while rb.push(i).is_err() {}
            }
        }
    });

    let rb_consumer = rb.clone();
    let cons = thread::spawn({
        let rb = rb_consumer;
        move || {
            let mut expected = 0;
            while expected < 10_000_000 {
                loop {
                    if let Some(v) = rb.pop() {
                        assert_eq!(v, expected);
                        expected += 1;
                        break;
                    }
                }
            }
        }
    });

    prod.join().unwrap();
    cons.join().unwrap();

    println!("SPSC RingBuffer profile completed.");
}