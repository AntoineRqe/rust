use crate::fix;
use order_book::types::{OrderEvent, Price, Side};
use spsc::spsc_lock_free::{Producer, Consumer};

#[repr(C)]
#[derive(Copy, Clone)]
pub struct FixRawMsg<const N: usize> {
    pub len: u16,
    pub data: [u8; N],
}

/// A simple FIX engine that reads raw FIX messages from an input queue, parses them, and pushes structured order events to an output queue. This is a very basic implementation that only handles New Order Single messages and extracts a few fields for demonstration purposes.
/// In a real system, you would need to handle many more message types and fields, as well as error handling and performance optimizations.
pub struct FixEngine<'a> {
    fifo_in: Consumer<'a, FixRawMsg<1024>, 1024>,
    fifo_out: Producer<'a, OrderEvent, 1024>,
    counter: usize,
}

impl<'a> FixEngine<'a> {
    pub fn new(fifo_in: Consumer<'a, FixRawMsg<1024>, 1024>, fifo_out: Producer<'a, OrderEvent, 1024>) -> Self {
        Self { fifo_in, fifo_out, counter: 0 }
    }
    
    pub fn run(&mut self) {
        loop {
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
                        fix::tags::ORDER_ID => {
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

                if !order_event.check_valid() {
                    continue; // Skip invalid orders
                }
                self.counter += 1;
                // For simulator, we assume push will always succeed. In a real system, we would need to handle the case where the queue is full.
                self.fifo_out.push(order_event).unwrap();
            }
        }
    }

}