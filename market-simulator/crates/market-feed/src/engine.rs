use spsc::spsc_lock_free::{Consumer};
use types::{
    OrderEvent,
    OrderResult,
    macros::{
        EntityId,
    }
};
use std::{sync::{Arc, atomic::{AtomicBool, Ordering}}};
use std::net::UdpSocket;
use crate::types::{
    AddOrder,
    MarketFeedHeader,
    MessageType,
    MarketEvent,
};


pub struct MarketDataFeedEngine<'a, const N: usize> {
    fifo_in: Consumer<'a, (OrderEvent, OrderResult), N>,
    shutdown: Arc<AtomicBool>,
    socket: Option<std::net::UdpSocket>, // Optional socket for broadcasting market data feed events
    seq_num: u64, // Sequence number for market data feed events
    multicast_ip: String, // Multicast IP address for broadcasting market data feed events
    port: u16, // Port for broadcasting market data feed events
    // ...
}

fn market_name() -> &'static str {
    static MARKET_NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    MARKET_NAME
        .get_or_init(|| std::env::var("MARKET_NAME").unwrap_or_else(|_| "unknown".to_string()))
        .as_str()
}

impl <'a, const N: usize> MarketDataFeedEngine<'a, N> {
    pub fn new(fifo_in: Consumer<'a, (OrderEvent, OrderResult), N>,
               shutdown: Arc<AtomicBool>,
               addr: &str,
               port: u16) -> Result<Self, std::io::Error> {

        let socket = UdpSocket::bind(format!("0.0.0.0:0"))?;

        Ok(Self { fifo_in, shutdown, socket: Some(socket), seq_num: 0, multicast_ip: addr.to_string(), port })
    }

    fn build_add_order_event(&self, order_event: &OrderEvent) -> AddOrder {
        let side = match order_event.side {
            types::Side::Buy => 1,
            types::Side::Sell => 2,
        };

        // Build a market data feed event based on the order event
        AddOrder {
            order_id: order_event.order_id.to_numeric(),
            side: side,
            price: order_event.price,
            quantity: order_event.quantity,
        }
    }

    fn build_header(&mut self, order_event: &OrderEvent) -> MarketFeedHeader {
        let mut header = crate::types::MarketFeedHeader::default();
        header.seq_num = self.seq_num;
        self.seq_num += 1;
        header.timestamp_ns = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64;
        
        header.msg_type = match order_event.order_type {
            types::OrderType::LimitOrder => { 
                MessageType::AddOrder as u8
            },
            types::OrderType::MarketOrder => {
                MessageType::AddOrder as u8
            },
            _ => {
                // Handle other order types (e.g., market orders, cancellations) and populate the market data feed event accordingly
                0 // Placeholder for other message types   
            }
        };

        header
    }

    fn build_market_data_feed_from_order(&mut self, order_event: &OrderEvent) -> MarketEvent {
        let header = self.build_header(order_event);

        match order_event.order_type {
            types::OrderType::LimitOrder | types::OrderType::MarketOrder => {
                let add_order = self.build_add_order_event(order_event);
                MarketEvent::Add(header, add_order)
            },
            // Handle other order types (e.g., market orders, cancellations) and populate the market data feed event accordingly
            _ => {
                // Placeholder for other message types
                MarketEvent::Add(header, self.build_add_order_event(order_event)) // Default to Add for now
            }
        }        
    }


    pub fn run(&mut self) {
        while !self.shutdown.load(Ordering::Relaxed) || !self.fifo_in.is_empty() {
            // If shutdown is signaled and there are no more events to process, exit the loop
            if let Some((order_event, order_result)) = self.fifo_in.pop() {
                if order_event.sender_id == EntityId::from_ascii("") {
                    tracing::info!("[{}] Shutdown signal received. Market Data Feed Engine will shut down after processing remaining events.", market_name());
                    self.shutdown.store(true, Ordering::Relaxed);

                } else {
                    // Process incoming order events and results from the order book engine
                    // Transform the order event and result into a market data feed event
                    tracing::debug!("[{}] Received order event: {:?}, result: {:?}", market_name(), order_event, order_result);
                    let market_data_feed_event = self.build_market_data_feed_from_order(&order_event);
                    let bytes = market_data_feed_event.to_bytes();
                    if let Some(socket) = &self.socket {
                        tracing::info!("[{}] Broadcasting market data feed event: header={:?}, event={:?}", market_name(), market_data_feed_event, market_data_feed_event);
                        let _ = socket.send_to(&bytes, format!("{}:{}", self.multicast_ip, self.port));
                    }
                }
            }
        }

        tracing::info!("[{}] Market Data Feed Engine shutting down", market_name());
    }
}


#[cfg(test)]
mod tests {
    use types::macros::{
        OrderId,
        EntityId,
    };

    use super::*;
    use spsc::spsc_lock_free::Producer;
    use std::net::{Ipv4Addr};
    use std::{thread};

    fn kill_market_data_feed_engine<const N: usize>(fix_to_ob_tx: &Producer<(OrderEvent, OrderResult), N>) {
        let order_event = OrderEvent {
            sender_id: EntityId::from_ascii(""), // An empty sender_id is used as a signal to the order book engine to shut down
            ..Default::default() // Fill the rest of the fields with default values
        };

        fix_to_ob_tx.push((order_event, OrderResult::default())).unwrap();  
    } 

        
    fn retrieve_market_data_feed_events(port: u16, multicast_ip: &str) -> Result<(MarketFeedHeader, MarketEvent), Box<dyn std::error::Error>> {
        // 0.0.0.0 is used to listen on all interfaces
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", port))?;

        let group = multicast_ip.parse::<Ipv4Addr>()?;
        let interface = "0.0.0.0".parse::<Ipv4Addr>()?; // Listen on all interfaces
    
        socket.join_multicast_v4(
            &group,
            &interface,
        )?;

        let mut buf = [0u8; 1024];

        let market_data_feed_event: (MarketFeedHeader, MarketEvent) = loop {
            let (_, _) = socket.recv_from(&mut buf)?;

            let header: MarketFeedHeader = unsafe {
                std::ptr::read_unaligned(buf.as_ptr() as *const _)
            };

            let header_size = std::mem::size_of::<MarketFeedHeader>();
            let body_bytes = &buf[header_size..];

            let event = match header.msg_type {
                x if x == MessageType::AddOrder as u8 => {
                    let add_order = AddOrder::from_bytes(body_bytes)
                        .ok_or("Failed to deserialize AddOrder")?;
                    MarketEvent::Add(header, add_order)
                },
                _ => {
                    return Err(format!("Unsupported message type: {}", header.msg_type).into());
                }
            };

            break (header, event);
        };

        Ok(market_data_feed_event)
    }

    #[test]
    fn test_market_data_feed_engine() {
        let mut rb_in = spsc::spsc_lock_free::RingBuffer::<(OrderEvent, OrderResult), 16>::new();

        thread::scope( |s| {

            let (producer_in, consumer_in) = rb_in.split();

            let multicast_ip = "239.0.0.1"; // Example multicast IP address
            let port = 8097;

            let mut engine = MarketDataFeedEngine::new(consumer_in, Arc::new(AtomicBool::new(false)), multicast_ip, port).unwrap();
        
            let engine_handle = s.spawn(move || {
                engine.run();
            });
    
            let recv_handle = s.spawn(move || {
                retrieve_market_data_feed_events(port, multicast_ip).unwrap()
            });

            // Simulate sending an order event to the engine
            let order_event = OrderEvent {
                sender_id: EntityId::from_ascii("test_sender"),
                order_id: OrderId::from_ascii("order123"),
                side: types::Side::Buy,
                price: types::FixedPointArithmetic(100),
                quantity: types::FixedPointArithmetic(10),
                ..Default::default()
            };

            let order_result = OrderResult {
                trades: types::Trades::default(),
                status: types::OrderStatus::Filled,
                timestamp: std::time::Instant::now(), // Set the timestamp to the current time
            };
        
            // Wait to make sure the receiver is ready before sending the order event
            std::thread::sleep(std::time::Duration::from_millis(100));
            producer_in.push((order_event, order_result)).unwrap();

            // Wait for the receiver to get the market data feed event
            let (header, event) = recv_handle.join().unwrap();

            // Validate the received market data feed event
            assert_eq!(header.msg_type, MessageType::AddOrder as u8);
            match event {
                MarketEvent::Add(_, add_order) => {
                    let order_id = add_order.order_id;
                    let side = add_order.side;
                    let price = add_order.price;
                    let quantity = add_order.quantity;

                    assert_eq!(order_id, order_event.order_id.to_numeric());
                    assert_eq!(side, 1); // Buy side should be 1
                    assert_eq!(price, order_event.price);
                    assert_eq!(quantity, order_event.quantity);
                },
                _ => panic!("Expected AddOrder event"),
            }

            kill_market_data_feed_engine(&producer_in);
            engine_handle.join().unwrap();
        });
    }
}