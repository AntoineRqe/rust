use std::{collections::HashMap};
use std::sync::atomic::AtomicBool;

use crate::tags::{tags, msg_types, side_code_set};
use types::{
    FixedPointArithmetic,
    OrderEvent,
    Side,
    macros::{
        EntityId,
        OrderId,
    }
};
use spsc::spsc_lock_free::{Consumer, Producer};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use crossbeam::queue::{ArrayQueue};
use std::cell::UnsafeCell;
use serde::Serialize;
use tokio::sync::mpsc;
use utils::market_name;

pub type RequestQueue<const N: usize> = Arc<ArrayQueue<FixRawMsg<N>>>;
pub type ResponseQueue<const N: usize> = Arc<ArrayQueue<(u64, FixRawMsg<N>)>>;


pub fn kill_fix_inbound_engine<const N: usize>(net_to_fix_tx: &crossbeam::channel::Sender<FixRawMsg<N>>) {
    // Send a special shutdown message to the FIX engine's inbound queue
    // The FIX engine should be designed to recognize this message and exit its run loop
    // For example, we could send a message with a specific tag or an empty message
    // Here we send an empty message as a simple shutdown signal
    let shutdown_msg = FixRawMsg::default();
    net_to_fix_tx.send(shutdown_msg).unwrap();
}

pub fn kill_fix_outbound_engine<const N: usize>(er_to_fix_tx: &Producer<(EntityId, FixRawMsg<N>), N>) {
    // Send a special shutdown message to the FIX engine's outbound queue
    // The FIX engine should be designed to recognize this message and exit its run loop
    // Here we send a message with an empty EntityId as a simple shutdown signal
    let shutdown_msg = (EntityId::from_ascii(""), FixRawMsg::default());
    er_to_fix_tx.push(shutdown_msg).unwrap();
}

/// A simple FIX engine that reads raw FIX messages from an input queue, parses them, and pushes structured order events to an output queue. This is a very basic implementation that only handles New Order Single messages and extracts a few fields for demonstration purposes.
#[repr(C)]
#[derive(Clone, Debug)]
pub struct FixRawMsg<const N: usize> {
    pub len: u16,
    pub data: [u8; N],
    pub resp_queue: Option<mpsc::Sender<FixRawMsg<N>>>, // Optional queue for sending responses back to the network layer, added for potential future use
}

impl<const N: usize> Serialize for FixRawMsg<N> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Convert the raw FIX message to a human-readable string by replacing SOH with " | "
        let human_readable = String::from_utf8_lossy(&self.data[..self.len as usize])
            .replace('\x01', " | ");
        serializer.serialize_str(&human_readable)
    }
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

impl <const N: usize> FixRawMsg<N> {
    pub fn new(data: &[u8], resp_queue: Option<mpsc::Sender<FixRawMsg<N>>>) -> Self {
        let mut msg = FixRawMsg::default();
        msg.len = data.len() as u16;
        msg.data[..data.len()].copy_from_slice(data);
        msg.resp_queue = resp_queue;
        msg
    }

    pub fn to_human_readable(&self) -> String {
        String::from_utf8_lossy(&self.data[..self.len as usize])
            .replace('\x01', " | ")
            .to_string()
    }
}

/// A simple FIX engine that reads raw FIX messages from an input queue, parses them, and pushes structured order events to an output queue. This is a very basic implementation that only handles New Order Single messages and extracts a few fields for demonstration purposes.
/// In a real system, you would need to handle many more message types and fields, as well as error handling and performance optimizations.
pub struct FixEngine<'a, const N: usize> {
    request_in: Arc<crossbeam_channel::Receiver<FixRawMsg<N>>>,
    request_out: Producer<'a, OrderEvent, N>,
    response_in: Consumer<'a, (EntityId, FixRawMsg<N>), N>, // For future use if we want to send execution reports back to the FIX engine
    shutdown: Arc<AtomicBool>,
    pending: Arc<FixPendingConnection<N>>, // Shared state for pending response queues, used
}

/// The data struct which will be shared between the inbound and outbound engines, containing the pending response queues for each order event, and a shutdown flag to signal when the engine should stop. This allows the inbound and outbound engines to communicate with each other without needing to share the entire engine struct, which can help reduce contention and improve performance.
struct FixShared<const N: usize> {
    shutdown: Arc<AtomicBool>,
    pending: Arc<FixPendingConnection<N>>,
}

impl <const N: usize> FixShared<N> {

    fn update_pending(&self, key: EntityId, resp_queue: mpsc::Sender<FixRawMsg<N>>) {
        // loop while locked is true, then set locked to true and update the pending queue, then set locked to false. This is a very basic spinlock implementation, in a real implementation you would want to use a more robust locking mechanism or a lock-free data structure to avoid contention and improve performance.
        while self.pending.locked.swap(true, Ordering::Acquire) { std::hint::spin_loop(); }
        unsafe {
            (*self.pending.pending.get()).insert(key, resp_queue);
        }
        self.pending.locked.store(false, Ordering::Release);
    }

    fn get_pending(&self, key: &EntityId) -> Option<mpsc::Sender<FixRawMsg<N>>> {
        while self.pending.locked.swap(true, Ordering::Acquire) { std::hint::spin_loop(); }
        let result = unsafe {
            (*self.pending.pending.get()).get(key).cloned()
        };
        self.pending.locked.store(false, Ordering::Release);
        result
    }

    fn remove_pending(&self, key: &EntityId) {
        while self.pending.locked.swap(true, Ordering::Acquire) { std::hint::spin_loop(); }
        unsafe {
            (*self.pending.pending.get()).remove(key);
        }
        self.pending.locked.store(false, Ordering::Release);
    }

    fn prune_closed_pending(&self) -> usize {
        while self.pending.locked.swap(true, Ordering::Acquire) { std::hint::spin_loop(); }
        let removed = unsafe {
            let pending = &mut *self.pending.pending.get();
            let before = pending.len();
            pending.retain(|_, sender| !sender.is_closed());
            before.saturating_sub(pending.len())
        };
        self.pending.locked.store(false, Ordering::Release);
        removed
    }
}

pub struct FixInboundEngine<'a, const N: usize> {
    request_in: Arc<crossbeam_channel::Receiver<FixRawMsg<N>>>,
    request_out: Producer<'a, OrderEvent, N>,
    counter: usize,
    shared: Arc<FixShared<N>>,
}

impl<'a, const N: usize> FixInboundEngine<'a, N> {
        // Blocking wait for new inbound FIX messages. Shutdown is signaled via a
        // sentinel message (len == 0) or by disconnecting the input channel.
    pub fn run(&mut self) {
        loop {
            let mut msg = match self.request_in.recv() {
                Ok(msg) => msg,
                Err(_) => break,
            };

            if msg.len == 0 {
                tracing::info!("[{}] Inbound FIX engine received shutdown sentinel", market_name());
                break;
            }

            let resp_queue = msg.resp_queue.take(); // Take ownership of the response queue if provided, so we can use it later when sending responses back to the client

            let order_event = match self.build_order(msg) {
                Ok(event) => event,
                Err(e) => {
                    tracing::error!("[{}] Failed to parse FIX message: {}, skipping", market_name(), e);
                    continue; // Skip malformed messages
                }
            };

            // Check validity of the parsed order event before pushing to the order book queue, this is important to avoid processing invalid events downstream
            match order_event.check_valid() {
                Ok(_) => {},
                Err(e) => {
                    tracing::error!("[{}] Invalid order event parsed: {}, skipping", market_name(), e);
                    continue; // Skip invalid events
                }
            }

            self.counter += 1;

            // Store the response queue for this order event if provided, so that we can send a response back to the client after processing the order.
            if let Some(resp_queue) = resp_queue {
                self.shared.update_pending(order_event.sender_id, resp_queue);
            }

            // Push the structured order event to the order book queue.
            match self.request_out.push(order_event) {
                Ok(_) => {
                    tracing::debug!("[{}] Parsed order event pushed to order book queue successfully", market_name());
                    self.counter += 1;
                },
                Err(e) => {
                    tracing::error!("[{}] Failed to push order event to order book queue: {}, skipping", market_name(), e);
                    continue; // In a real implementation, you would want to handle this case properly, maybe with a retry mechanism or backpressure
                }
            }
        }

        // Propagate kill signal to order book engine by setting the shutdown flag, which the order book engine checks to know when to exit gracefully.
        self.request_out.push(OrderEvent::default()).unwrap(); // Send a final order event with default values to unblock any subscribers that may be waiting for order events, such as the order book engine, allowing them to check the shutdown flag and exit gracefully.

        tracing::info!("[{}] Inbound FIX engine shutting down, processed {} messages", market_name(), self.counter);
    }

    fn build_order(
        &self,
        msg: FixRawMsg<N>,
    ) -> Result<OrderEvent, &'static str> {
        let mut order_event = OrderEvent::default();

        let mut parser = crate::parser::FixParser::new(&msg.data[..msg.len as usize]);
        let fields = parser.get_fields();

        for field in fields.fields {
            match field.tag {
                tags::MSG_TYPE => {
                    match field.value {
                        // Only handling New Order Single for this example, but could add more message types here
                        msg_types::NEW_ORDER_SINGLE => {
                        },
                        msg_types::ORDER_CANCEL_REQUEST=> {
                            order_event.order_type = types::OrderType::CancelOrder;
                        },
                        _ => return Err("Unsupported message type"), // Unsupported message type
                    }
                },
                tags::SIDE => {
                    match field.value {
                        side_code_set::BUY => order_event.side = Side::Buy,
                        side_code_set::SELL => order_event.side = Side::Sell,
                        _ => return Err("Unsupported side code"), // Unsupported side code
                    }
                },
                tags::PRICE => {
                    if let Some(price) = FixedPointArithmetic::from_fix_bytes(field.value) {
                        order_event.price = price;
                    } else {
                        return Err("Invalid price format"); // Invalid price format
                    }
                },
                tags::CL_ORD_ID => {
                    utils::copy_array(&mut order_event.cl_ord_id.0, field.value);
                },
                tags::ORIG_CL_ORD_ID => {
                    let mut orig_cl_ord_id = OrderId::default();
                    utils::copy_array(&mut orig_cl_ord_id.0, field.value);
                    order_event.orig_cl_ord_id = Some(orig_cl_ord_id);
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
                    if let Some(qty) = FixedPointArithmetic::from_fix_bytes(field.value) {
                        order_event.quantity = qty;
                    } else {
                        return Err("Invalid quantity format"); // Invalid quantity format
                    }
                },
                tags::SENDING_TIME => {
                    if let Some(timestamp) = utils::UtcTimestamp::from_fix_bytes(field.value) {
                        order_event.timestamp_ms = timestamp.to_unix_ms();
                    } else {
                        return Err("Invalid timestamp format"); // Invalid timestamp format
                    }
                },
                _ => continue, // Skip unsupported tags
            }
        }

        Ok(order_event)
    }
}


pub struct FixOutboundEngine<'a, const N: usize> {
    response_in: Consumer<'a, (EntityId, FixRawMsg<N>), N>,
    counter: usize,
    shared: Arc<FixShared<N>>,
}

impl<'a, const N: usize> FixOutboundEngine<'a, N> {
    pub fn run(&mut self) {
        let mut sweep_ticks = 0usize;

        loop {
            if let Some((key, response)) = self.response_in.pop() {
                if key == EntityId::from_ascii("") {
                    tracing::info!("[{}] Outbound FIX engine received shutdown sentinel", market_name());
                    break;
                }

                if let Some(resp_queue) = self.shared.get_pending(&key) {
                    match resp_queue.try_send(response) {
                        Ok(_) => {
                            self.counter += 1;
                        }
                        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                            self.shared.remove_pending(&key);
                            tracing::warn!("[{}] Client response channel closed, removing pending route", market_name());
                        }
                        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                            tracing::error!("[{}] Client response channel full, dropping response", market_name());
                        }
                    }
                }
            } else {
                if self.shared.shutdown.load(Ordering::Acquire) {
                    break;
                }
                std::hint::spin_loop();
                continue;
            }

            // Periodically prune closed pending response queues to prevent memory leaks from clients that have disconnected without properly cleaning up their pending routes.
            sweep_ticks = sweep_ticks.saturating_add(1);
            if sweep_ticks >= 10_000 {
                let removed = self.shared.prune_closed_pending();
                if removed > 0 {
                    tracing::debug!("[{}] Pruned {} stale pending FIX response route(s)", market_name(), removed);
                }
                sweep_ticks = 0;
            }
        }

        tracing::info!("[{}] Outbound FIX engine shutting down, processed {} messages", market_name(), self.counter);
    }
}

struct FixPendingConnection<const N: usize> {
    locked: AtomicBool,
    pending: UnsafeCell<HashMap<EntityId, mpsc::Sender<FixRawMsg<N>>>>,
}

unsafe impl<const N: usize> Send for FixPendingConnection<N> {}
unsafe impl<const N: usize> Sync for FixPendingConnection<N> {}

impl<'a, const N: usize> FixEngine<'a, N> {
    
    pub fn new(
        request_in: Arc<crossbeam_channel::Receiver<FixRawMsg<N>>>,
        request_out: Producer<'a, OrderEvent, N>,
        response_in: Consumer<'a, (EntityId, FixRawMsg<N>), N>,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            request_in: request_in,
            request_out: request_out,
            response_in: response_in,
            shutdown: shutdown,
            pending: Arc::new(FixPendingConnection {
                locked: AtomicBool::new(false),
                pending: UnsafeCell::new(HashMap::new()),
            }),
        }
    }

    pub fn split(self) -> (FixInboundEngine<'a, N>, FixOutboundEngine<'a, N>) {
            
        let shared = Arc::new(FixShared {
            shutdown: Arc::clone(&self.shutdown),
            pending: Arc::clone(&self.pending),
        });


        let inbound = FixInboundEngine {
            request_in: self.request_in,
            request_out: self.request_out,
            shared: Arc::clone(&shared),
            counter: 0,
        };

        let outbound = FixOutboundEngine {
            response_in: self.response_in,
            shared: Arc::clone(&shared),
            counter: 0,
        };

        (inbound, outbound)

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
        let (net_to_fix_tx, net_to_fix_rx) = crossbeam_channel::bounded::<FixRawMsg<1024>>(1024);
        let mut fix_to_ob = RingBuffer::<OrderEvent, 1024>::new();
        // Outbound : order book -> FIX -> net
        let mut er_to_fix = RingBuffer::<(EntityId, FixRawMsg<1024>), 1024>::new();

        std::thread::scope(|scope| {

            let shutdown = Arc::new(AtomicBool::new(false));
            let (fix_to_ob_tx, fix_to_ob_rx) = fix_to_ob.split();
            let (er_to_fix_tx, er_to_fix_rx) = er_to_fix.split();

            let handle = FixEngine::new(
                Arc::new(net_to_fix_rx),
                fix_to_ob_tx,
                er_to_fix_rx,
                Arc::clone(&shutdown),
            );

            let (mut inbound_engine, mut outbound_engine) = handle.split();

             // Spawn a thread to run the FIX engine
    
            let inbound_handle = scope.spawn(move || {
                inbound_engine.run();
            });
            
            let outbound_handle = scope.spawn(move || {
                outbound_engine.run();
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

            net_to_fix_tx.send(raw_msg).expect("Failed to push message");

            std::thread::sleep(std::time::Duration::from_millis(100)); // Give the engine some time to process
            assert_eq!(fix_to_ob_rx.len(), 1); // We should have received one order event in the order book queue

            let order_event = fix_to_ob_rx.pop().unwrap();
    
            assert_eq!(field_str(order_event.cl_ord_id.as_ref()),  b"12345");
            assert_eq!(field_str(order_event.sender_id.as_ref()), b"SENDER");
            assert_eq!(field_str(order_event.target_id.as_ref()), b"TARGET");
            assert_eq!(order_event.quantity, FixedPointArithmetic::from_number(1000000));
            assert_eq!(order_event.price, FixedPointArithmetic::from_f64(1.23456));
            assert_eq!(order_event.side, Side::Buy);

            // Stop the FIX engine thread
            shutdown.store(true, std::sync::atomic::Ordering::Release);
            kill_fix_inbound_engine(&net_to_fix_tx);
            kill_fix_outbound_engine(&er_to_fix_tx);

            // Wait a bit to ensure the FIX engine has stopped before ending the test, in a real implementation you would want a more robust way to ensure the thread has stopped
            inbound_handle.join().expect("Failed to join inbound FIX engine thread");
            outbound_handle.join().expect("Failed to join outbound FIX engine thread");
        });
    }

    #[test]
    fn test_fix_engine_cancel_request_with_orig_cl_ord_id() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let (net_to_fix_tx, net_to_fix_rx) = crossbeam_channel::bounded::<FixRawMsg<1024>>(1024);
        let mut fix_to_ob = RingBuffer::<OrderEvent, 1024>::new();
        let mut er_to_fix = RingBuffer::<(EntityId, FixRawMsg<1024>), 1024>::new();

        std::thread::scope(|scope| {
            let (fix_to_ob_tx, fix_to_ob_rx) = fix_to_ob.split();
            let (er_to_fix_tx, er_to_fix_rx) = er_to_fix.split();

            let handle = FixEngine::new(
                Arc::new(net_to_fix_rx),
                fix_to_ob_tx,
                er_to_fix_rx,
                Arc::clone(&shutdown),
            );

            let (mut inbound_engine, mut outbound_engine) = handle.split();

             // Spawn a thread to run the FIX engine

            let inbound_handle = scope.spawn(move || {
                inbound_engine.run();
            });

            let outbound_handle = scope.spawn(move || {
                outbound_engine.run();
            });

            let fix_message = b"8=FIX.4.4\x019=0000\x0135=F\x0149=SENDER\x0156=TARGET\x0134=2\x0152=20240219-12:31:00.000\x0111=CXL-1\x0141=ORD-12345\x0154=1\x0138=100\x0144=1.23456\x0155=EURUSD\x0110=123\x01";

            let raw_msg = FixRawMsg {
                len: fix_message.len() as u16,
                data: {
                    let mut data = [0u8; 1024];
                    data[..fix_message.len()].copy_from_slice(fix_message);
                    data
                },
                resp_queue: None,
            };

            net_to_fix_tx.send(raw_msg).expect("Failed to push message");

            let order_event = loop {
                if let Some(event) = fix_to_ob_rx.try_pop() {
                    break event;
                }
                std::hint::spin_loop();
            };

            assert_eq!(order_event.order_type, types::OrderType::CancelOrder);
            assert!(order_event.orig_cl_ord_id.is_some());
            assert_eq!(
                field_str(order_event.orig_cl_ord_id.unwrap().as_ref()),
                b"ORD-12345"
            );

            shutdown.store(true, std::sync::atomic::Ordering::Release);

            kill_fix_inbound_engine(&net_to_fix_tx);
            kill_fix_outbound_engine(&er_to_fix_tx);

            inbound_handle.join().expect("Failed to join inbound FIX engine thread");
            outbound_handle.join().expect("Failed to join outbound FIX engine thread");
        });
    }
}
