use std::{collections::HashMap};
use std::sync::atomic::AtomicBool;

use crate::tags::{tags, msg_types, side_code_set};
use types::{OrderEvent, Price, Side, StopHandle};
use spsc::spsc_lock_free::{Consumer, Producer};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use crossbeam::queue::{ArrayQueue};

pub type RequestQueue<const N: usize> = Arc<ArrayQueue<FixRawMsg<N>>>;
pub type ResponseQueue<const N: usize> = Arc<ArrayQueue<(u64, FixRawMsg<N>)>>;

/// A simple FIX engine that reads raw FIX messages from an input queue, parses them, and pushes structured order events to an output queue. This is a very basic implementation that only handles New Order Single messages and extracts a few fields for demonstration purposes.
#[repr(C)]
#[derive(Clone, Debug)]
pub struct FixRawMsg<const N: usize> {
    pub len: u16,
    pub data: [u8; N],
    pub resp_queue: Option<crossbeam_channel::Sender<FixRawMsg<N>>>, // Optional queue for sending responses back to the network layer, added for potential future use
}

impl<const N: usize> Default for FixRawMsg<N> {
    fn default() -> Self {
        Self {
            len: 0,
            data: [0u8; N],
            resp_queue: None,
        }
    }
}

/// A simple FIX engine that reads raw FIX messages from an input queue, parses them, and pushes structured order events to an output queue. This is a very basic implementation that only handles New Order Single messages and extracts a few fields for demonstration purposes.
/// In a real system, you would need to handle many more message types and fields, as well as error handling and performance optimizations.
pub struct FixEngine<'a, const N: usize> {
    request_in: Arc<ArrayQueue<FixRawMsg<N>>>,
    request_out: Producer<'a, OrderEvent, N>,
    response_in: Consumer<'a, (u64, FixRawMsg<N>), N>, // For future use if we want to send execution reports back to the FIX engine
    counter: usize,
    shutdown: Arc<AtomicBool>,
    pending: HashMap<u64, crossbeam_channel::Sender<FixRawMsg<N>>>, // Map of pending order events waiting for responses, keyed by a unique identifier (e.g. order ID or a generated correlation ID)
}

impl<'a, const N: usize> FixEngine<'a, N> {
    
    pub fn new(
        request_in: Arc<ArrayQueue<FixRawMsg<N>>>,
        request_out: Producer<'a, OrderEvent, N>,
        response_in: Consumer<'a, (u64, FixRawMsg<N>), N>,
    ) -> Self {
        Self {
            request_in: request_in,
            request_out: request_out,
            response_in: response_in,
            counter: 0,
            shutdown: Arc::new(AtomicBool::new(false)),
            pending: HashMap::new(),
        }
    }

    fn build_order(
        &self,
        msg: FixRawMsg<N>,
    ) -> Option<OrderEvent> {
        let mut order_event = OrderEvent::default();

        let mut parser = crate::parser::FixParser::new(&msg.data[..msg.len as usize]);
        let fields = parser.get_fields();

        for field in fields.fields {
            match field.tag {
                tags::MSG_TYPE => {
                    match field.value {
                        // Only handling New Order Single for this example, but could add more message types here
                        msg_types::NEW_ORDER_SINGLE => {},
                        _ => return None, // Unsupported message type
                    }
                },
                tags::SIDE => {
                    match field.value {
                        side_code_set::BUY => order_event.side = Side::Buy,
                        side_code_set::SELL => order_event.side = Side::Sell,
                        _ => return None, // Unsupported side code
                    }
                },
                tags::PRICE => {
                    if let Some(price) = Price::from_fix_bytes(field.value) {
                        order_event.price = price;
                    } else {
                        return None; // Invalid price format
                    }
                },
                tags::ORDER_ID => {
                    utils::copy_array(&mut order_event.order_id.0, field.value);
                },
                tags::CL_ORD_ID => {
                    utils::copy_array(&mut order_event.cl_ord_id.0, field.value);
                },
                tags::SYMBOL => {
                    utils::copy_array(&mut order_event.symbol.0, field.value);
                },
                tags::SENDER_COMP_ID => {
                    utils::copy_array(&mut order_event.sender_id.0, field.value);
                },
                tags::TARGET_COMP_ID => {
                    utils::copy_array(&mut order_event.target_id.0, field.value);
                },
                tags::ORDER_QTY => {
                    if let Some(qty) = utils::bytes_to_number::<u64>(field.value) {
                        order_event.quantity = qty;
                    } else {
                        return None; // Invalid quantity format
                    }
                },
                tags::SENDING_TIME => {
                    if let Some(timestamp) = utils::UtcTimestamp::from_fix_bytes(field.value) {
                        order_event.timestamp = timestamp.to_unix_ms() as u64;
                    } else {
                        return None; // Invalid timestamp format
                    }
                },
                _ => continue, // Skip unsupported tags
            }
        }

        Some(order_event)
    }

    // Non blocking check for new inbound FIX messages, if there is a message, parse it and push the corresponding order event to the order book queue. In a real implementation, you would want to have more robust error handling and also handle different message types and fields.
    fn run_inbound_request(&mut self) {
        if let Some(mut msg) = self.request_in.pop() {

            let resp_queue = msg.resp_queue.take(); // Take ownership of the response queue if provided, so we can use it later when sending responses back to the client

            let order_event = match self.build_order(msg) {
                Some(event) => event,
                None => {
                    return; // Skip malformed messages
                }
            };

            // Check validity of the parsed order event before pushing to the order book queue, this is important to avoid processing invalid events downstream
            match order_event.check_valid() {
                Ok(_) => {},
                Err(e) => {
                    println!("Invalid order event parsed: {}, skipping", e);
                    return; // Skip invalid events
                }
            }

            self.counter += 1;

            let key = utils::make_key(
                u32::from_le_bytes(order_event.sender_id.0[0..4].try_into().unwrap_or_default()), 
                u32::from_le_bytes(order_event.order_id.0[0..4].try_into().unwrap_or_default())
            );

            // Store the response queue for this order event if provided, so that we can send a response back to the client after processing the order.
            if let Some(resp_queue) = resp_queue {
                self.pending.insert(key, resp_queue); // Store the response queue for this order event, using the combined key as the key
            }
            
            // Push the structured order event to the order book queue.
            match self.request_out.try_push(order_event) {
                Ok(_) => { println!("FixEngine: Successfully pushed order event #{} to order book queue", self.counter); },
                Err(e) => {
                    println!("Failed to push order event to order book queue: {}, skipping", e);
                    return; // In a real implementation, you would want to handle this case properly, maybe with a retry mechanism or backpressure
                }
            }
        }
    }

    /// Non blocking check for outbound responses from the order book engine, and if there is a response, send it back to the client using the stored response queue. This is a very basic implementation that assumes the response contains the same key as the original order event, in a real implementation you would want to have a more robust way to correlate responses with requests, and also handle cases where the client has disconnected or the response queue is full.
    fn run_outbound_response(&mut self) {
        if let Some((key, _response)) = self.response_in.try_pop() {
            if let Some(resp_queue) = self.pending.remove(&key) {
                match resp_queue.send(_response) {
                    Ok(_) => { println!("Successfully pushed response back to client for order event #{}", self.counter); },
                    Err(_) => { println!("Failed to push response back to client, dropping response"); },
                }
            }
        }
    }

    pub fn run(&mut self) {
        loop {
            self.run_inbound_request();
            self.run_outbound_response();

            if self.shutdown.load(Ordering::Relaxed) && self.pending.is_empty() {
                break;
            }
        }
    }

    pub fn stop_handle(&self) -> StopHandle{
        StopHandle {
            shutdown: Arc::clone(&self.shutdown),
            thread: None,
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
        // Inbound : net -> FIX -> order book
        let net_to_fix = Arc::new(ArrayQueue::<FixRawMsg<1024>>::new(1024));
        let mut fix_to_ob = RingBuffer::<OrderEvent, 1024>::new();
        // Outbound : order book -> FIX -> net
        let mut ob_to_fix = RingBuffer::<(u64, FixRawMsg<1024>), 1024>::new();

        std::thread::scope(|scope| {

            let (fix_to_ob_tx, fix_to_ob_rx) = fix_to_ob.split();
            let (_, ob_to_fix_rx) = ob_to_fix.split();

            let mut handle = FixEngine::new(
                Arc::clone(&net_to_fix),
                fix_to_ob_tx,
                ob_to_fix_rx,
            );
    
            let stop_handle = handle.stop_handle();

            let engine = scope.spawn(move || {
                handle.run();
            });
            
            let fix_message = b"8=FIX.4.4\x019=0000\x0135=D\x0149=SENDER\x0156=TARGET\x0134=1\x0152=20240219-12:30:00.000\x0111=12345\x0154=1\x0138=1000000\x0144=1.23456\x0155=EURUSD\x0110=123\x01";

            let raw_msg = FixRawMsg {
                len: fix_message.len() as u16,
                data: {
                    let mut data = [0u8; 1024];
                    data[..fix_message.len()].copy_from_slice(fix_message);
                    data
                },
                resp_queue: None, // Not using the response queue in this test, but could be set here if needed for future tests
            };

            net_to_fix.push(raw_msg).expect("Failed to push message");

            // spin wait instead of sleep
            let order_event = loop {
                if let Some(event) = fix_to_ob_rx.try_pop() {
                    break event;
                }
                std::hint::spin_loop();
            };
    
            assert_eq!(field_str(order_event.cl_ord_id.as_ref()),  b"12345");
            assert_eq!(field_str(order_event.sender_id.as_ref()), b"SENDER");
            assert_eq!(field_str(order_event.target_id.as_ref()), b"TARGET");
            assert_eq!(order_event.quantity, 1_000_000);
            assert_eq!(order_event.price, Price(123_456_000));
            assert_eq!(order_event.side, Side::Buy);

            // Stop the FIX engine thread
            stop_handle.stop();

            // Wait a bit to ensure the FIX engine has stopped before ending the test, in a real implementation you would want a more robust way to ensure the thread has stopped
            engine.join().expect("Failed to join FIX engine thread");
        });
    }
}
