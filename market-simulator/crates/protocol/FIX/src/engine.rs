use std::sync::atomic::AtomicBool;

use crate::tags::{tags, msg_types};
use crate::parser;
use order_book::types::{OrderEvent, OrderResult, Price, Side};
use spsc::spsc_lock_free::{Consumer, Producer};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use crossbeam::queue::ArrayQueue;

pub type RequestQueue<const N: usize> = Arc<ArrayQueue<FixRawMsg<N>>>;
pub type ResponseQueue<const N: usize> = Arc<ArrayQueue<FixResponse<N>>>;

#[repr(C)]
#[derive(Clone, Debug)]
pub struct FixRawMsg<const N: usize> {
    pub len: u16,
    pub data: [u8; N],
    pub queue: Option<ResponseQueue<N>>, // Optional queue for sending responses back to the network layer, added for potential future use
}

#[repr(C)]
#[derive(Clone, Debug)]
pub struct FixResponse<const N:usize> {
    pub len: u16,
    pub data: [u8; N],
} 

impl<const N: usize> Default for FixRawMsg<N> {
    fn default() -> Self {
        Self {
            len: 0,
            data: [0u8; N],
            queue: None,
        }
    }
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

pub fn spawn_scoped<'scope, 'env, const N: usize>(
    scope: &'scope std::thread::Scope<'scope, 'env>,
    request_in:  RequestQueue<N>,
    request_out: Producer<'env, OrderEvent, N>,
    response_in: Consumer<'env, OrderResult, N>,
) -> ScopedFixEngineHandle<'scope> {

    let running = Arc::new(AtomicBool::new(true));

    let mut engine = FixEngine {
        request_in:  request_in,
        request_out: request_out,
        response_in: response_in,
        counter:  0,
        running:  Arc::clone(&running),
    };

    let thread = scope.spawn(move || engine.run());
    ScopedFixEngineHandle { running, thread }
}

/// A simple FIX engine that reads raw FIX messages from an input queue, parses them, and pushes structured order events to an output queue. This is a very basic implementation that only handles New Order Single messages and extracts a few fields for demonstration purposes.
/// In a real system, you would need to handle many more message types and fields, as well as error handling and performance optimizations.
pub struct FixEngine<'a, const N: usize> {
    request_in: Arc<ArrayQueue<FixRawMsg<N>>>,
    request_out: Producer<'a, OrderEvent, N>,
    response_in: Consumer<'a, OrderResult, N>, // For future use if we want to send execution reports back to the FIX engine
    counter: usize,
    running: Arc<AtomicBool>,
}

impl<'a, const N: usize> FixEngine<'a, N> {
    
    pub fn new(
        request_in: Arc<ArrayQueue<FixRawMsg<N>>>,
        request_out: Producer<'a, OrderEvent, N>,
        response_in: Consumer<'a, OrderResult, N>,
    ) -> Self {
        Self {
            request_in: request_in,
            request_out: request_out,
            response_in: response_in,
            counter: 0,
            running: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn run(&mut self) {
        while self.running.load(Ordering::Relaxed) {
            // The pop already handles backoff when the queue is empty, so we can just wait for messages to arrive, we can use a busy loop here without sleeping
            if let Some(msg) = self.request_in.pop() {
                println!("FixEngine: Processing message #{}", self.counter);
                let mut parser = parser::FixParser::new(&msg.data[..msg.len as usize]);
                let fields = parser.get_fields();

                let mut order_event = OrderEvent::default();

                for field in fields.fields {
                    match field.tag {
                       tags::MSG_TYPE => {
                            match field.value {
                                // Only handling New Order Single for this example, but could add more message types here
                                msg_types::NEW_ORDER_SINGLE => {},
                                _ => continue, // Skip unsupported message types
                            }
                        },
                        tags::SIDE => {
                            let side_types = parser::parse_tag_fast(field.value);
                            match side_types {
                                parser::side_types::BUY => order_event.side = Side::Buy,
                                parser::side_types::SELL => order_event.side = Side::Sell,
                                _ => continue, // Skip unsupported side types
                            }
                        },
                        tags::PRICE => {
                            if let Some(price) = Price::from_fix_bytes(field.value) {
                                order_event.price = price;
                            }
                        },
                        // For simplicity, we will treat both ORDER_ID and CL_ORD_ID as the same order ID field in our OrderEvent struct, since they both serve the purpose of uniquely identifying an order.
                        tags::ORDER_ID => {
                            utils::copy_array(&mut order_event.order_id.0, field.value);
                        },
                        tags::CL_ORD_ID => {
                            utils::copy_array(&mut order_event.order_id.0, field.value);
                        },
                        tags::SENDER_COMP_ID => {
                            utils::copy_array(&mut order_event.sender_id, field.value);
                        },
                        tags::TARGET_COMP_ID => {
                            utils::copy_array(&mut order_event.target_id, field.value);
                        },
                        tags::ORDER_QTY => {
                            if let Some(qty) = utils::parse_unsigned_ascii::<u64>(field.value) {
                                order_event.quantity = qty;
                            }
                        },
                        tags::SENDING_TIME => {
                            if let Some(timestamp) = utils::UtcTimestamp::from_fix_bytes(field.value) {
                                order_event.timestamp = timestamp.to_unix_ms() as u64;
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

                match self.request_out.push(order_event) {
                    Ok(_) => { println!("FixEngine: Successfully pushed order event #{} to order book queue", self.counter); },
                    Err(e) => {
                        println!("Failed to push order event to order book queue: {}, skipping", e);
                        continue; // In a real implementation, you would want to handle this case properly, maybe with a retry mechanism or backpressure
                    }
                }

                loop {
                    match self.response_in.pop() {
                        Some(_response) => {
                            println!("Received response from order book for order event #{}", self.counter);
                            msg.queue.unwrap().push(FixResponse {
                                len: 0, // In a real implementation, you would serialize an actual FIX message here based on the order result
                                data: [0u8; N],
                            }).expect("Failed to push response to network queue");
                            break;
                        },
                        None => continue, // No more responses to process at this time
                    }
                }
                
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
        let network_out_fix_in = Arc::new(ArrayQueue::<FixRawMsg<1024>>::new(1024));
        let mut request_queue = RingBuffer::<OrderEvent, 1024>::new();
        let (request_out, request_in) = request_queue.split();
        let mut response_queue = RingBuffer::<OrderResult, 1024>::new();
        let (_, response_in) = response_queue.split();

        std::thread::scope(|scope| {
            let handle = spawn_scoped(scope, network_out_fix_in.clone(), request_out, response_in);

            let fix_message = b"8=FIX.4.4\x019=0000\x0135=D\x0149=SENDER\x0156=TARGET\x0134=1\x0152=20240219-12:30:00.000\x0111=12345\x0154=1\x0138=1000000\x0144=1.23456\x0155=EURUSD\x0110=123\x01";

            let raw_msg = FixRawMsg {
                len: fix_message.len() as u16,
                data: {
                    let mut data = [0u8; 1024];
                    data[..fix_message.len()].copy_from_slice(fix_message);
                    data
                },
                queue: None, // Not using the response queue in this test, but could be set here if needed for future tests
            };

            network_out_fix_in.push(raw_msg).expect("Failed to push message");

            // spin wait instead of sleep
            let order_event = loop {
                if let Some(event) = request_in.pop() {
                    break event;
                }
                std::hint::spin_loop();
            };

            assert_eq!(field_str(&order_event.order_id.0),  b"12345");
            assert_eq!(field_str(&order_event.sender_id), b"SENDER");
            assert_eq!(field_str(&order_event.target_id), b"TARGET");
            assert_eq!(order_event.quantity, 1_000_000);
            assert_eq!(order_event.price, Price(123_456_000));
            assert_eq!(order_event.side, Side::Buy);

            handle.stop();
        });
    }
}
