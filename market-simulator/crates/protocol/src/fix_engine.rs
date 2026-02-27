use std::sync::atomic::AtomicBool;

use crate::fix;
use order_book::types::{OrderEvent, Price, Side};
use spsc::spsc_lock_free::{Producer, Consumer};
use std::sync::Arc;
use std::sync::atomic::Ordering;

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct FixRawMsg<const N: usize> {
    pub len: u16,
    pub data: [u8; N],
}

/// A handle to control the FIX engine thread, allowing for graceful shutdown.
pub struct ScopedFixEngineHandle<'scope> {
    running: Arc<AtomicBool>,
    thread: std::thread::ScopedJoinHandle<'scope, ()>,
}

impl<'scope> ScopedFixEngineHandle<'scope> {
    pub fn stop(self) {
        self.running.store(false, Ordering::Relaxed);
        self.thread.join().unwrap(); // wait for clean exit
    }
}

pub fn spawn_scoped<'scope, 'env>(
    scope: &'scope std::thread::Scope<'scope, 'env>,
    fix_in:  Consumer<'env, FixRawMsg<1024>, 1024>,
    fix_out: Producer<'env, OrderEvent, 1024>,
) -> ScopedFixEngineHandle<'scope> {
    let running = Arc::new(AtomicBool::new(true));

    let mut engine = FixEngine {
        fifo_in:  fix_in,
        fifo_out: fix_out,
        counter:  0,
        running:  Arc::clone(&running),
    };

    let thread = scope.spawn(move || engine.run());
    ScopedFixEngineHandle { running, thread }
}

/// A simple FIX engine that reads raw FIX messages from an input queue, parses them, and pushes structured order events to an output queue. This is a very basic implementation that only handles New Order Single messages and extracts a few fields for demonstration purposes.
/// In a real system, you would need to handle many more message types and fields, as well as error handling and performance optimizations.
pub struct FixEngine<'a> {
    fifo_in: Consumer<'a, FixRawMsg<1024>, 1024>,
    fifo_out: Producer<'a, OrderEvent, 1024>,
    counter: usize,
    running: Arc<AtomicBool>,
}

impl<'a> FixEngine<'a> {
    
    pub fn run(&mut self) {
        while self.running.load(Ordering::Relaxed) {
            // The pop already handles backoff when the queue is empty, so we can just wait for messages to arrive, we can use a busy loop here without sleeping
            if let Some(msg) = self.fifo_in.pop() {
                let mut parser = fix::FixParser::new(&msg.data[..msg.len as usize]);
                let fields = parser.get_fields();

                let mut order_event = OrderEvent::default();

                for field in fields.fields {
                    match field.tag {
                       fix::tags::MSG_TYPE => {
                            match field.value {
                                // Only handling New Order Single for this example, but could add more message types here
                                fix::msg_types::NEW_ORDER_SINGLE => {},
                                _ => continue, // Skip unsupported message types
                            }
                        },
                        fix::tags::SIDE => {
                            let side_types = fix::parse_tag_fast(field.value);
                            match side_types {
                                fix::side_types::BUY => order_event.side = Side::Buy,
                                fix::side_types::SELL => order_event.side = Side::Sell,
                                _ => continue, // Skip unsupported side types
                            }
                        },
                        fix::tags::PRICE => {
                            if let Some(price) = Price::from_fix_bytes(field.value) {
                                order_event.price = price;
                            }
                        },
                        // For simplicity, we will treat both ORDER_ID and CL_ORD_ID as the same order ID field in our OrderEvent struct, since they both serve the purpose of uniquely identifying an order.
                        fix::tags::ORDER_ID => {
                            utils::copy_array(&mut order_event.order_id, field.value);
                        },
                        fix::tags::CL_ORD_ID => {
                            utils::copy_array(&mut order_event.order_id, field.value);
                        },
                        fix::tags::SENDER_COMP_ID => {
                            utils::copy_array(&mut order_event.sender_id, field.value);
                        },
                        fix::tags::TARGET_COMP_ID => {
                            utils::copy_array(&mut order_event.target_id, field.value);
                        },
                        fix::tags::ORDER_QTY => {
                            if let Some(qty) = utils::parse_u64_ascii(field.value) {
                                order_event.quantity = qty;
                            }
                        },
                        _ => continue, // Skip unsupported tags
                    }
                }

                match order_event.check_valid() {
                    Ok(_) => {},
                    Err(e) => {
                        println!("Invalid order event parsed: {}, skipping", e);
                        continue; // Skip invalid events
                    }
                }

                self.counter += 1;
                // For simulator, we assume push will always succeed. In a real system, we would need to handle the case where the queue is full.
                self.fifo_out.push(order_event).unwrap();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use utils::field_str;
    use spsc::spsc_lock_free::RingBuffer;


    #[test]
    fn test_fix_engine() {
        let mut network_out_fix_in = RingBuffer::<FixRawMsg<1024>, 1024>::new();
        let (network_out, fix_in) = network_out_fix_in.split();
        let mut fix_out_order_book_in = RingBuffer::<OrderEvent, 1024>::new();
        let (fix_out, order_book_in) = fix_out_order_book_in.split();

        std::thread::scope(|scope| {
            let handle = spawn_scoped(scope, fix_in, fix_out);

            let fix_message = b"8=FIX.4.4\x019=0000\x0135=D\x0149=SENDER\x0156=TARGET\x0134=1\x0152=20240219-12:30:00.000\x0111=12345\x0154=1\x0138=1000000\x0144=1.23456\x0155=EURUSD\x0110=123\x01";

            let raw_msg = FixRawMsg {
                len: fix_message.len() as u16,
                data: {
                    let mut data = [0u8; 1024];
                    data[..fix_message.len()].copy_from_slice(fix_message);
                    data
                },
            };

            network_out.push(raw_msg).expect("Failed to push message");

            // spin wait instead of sleep
            let order_event = loop {
                if let Some(event) = order_book_in.pop() {
                    break event;
                }
                std::hint::spin_loop();
            };

            assert_eq!(field_str(&order_event.order_id),  b"12345");
            assert_eq!(field_str(&order_event.sender_id), b"SENDER");
            assert_eq!(field_str(&order_event.target_id), b"TARGET");
            assert_eq!(order_event.quantity, 1_000_000);
            assert_eq!(order_event.price,    Price(123_456_000));
            assert_eq!(order_event.side,     Side::Buy);
            
            handle.stop();
        });
    }
}
