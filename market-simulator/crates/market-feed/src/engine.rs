use spsc::spsc_lock_free::{Consumer};
use types::{
    OrderEvent,
    OrderResult,
};
use std::{sync::{Arc, atomic::{AtomicBool, Ordering}}};
use crate::types::{
    AddOrder,
    DeleteOrder,
    ModifyOrder,
    MarketDataHeader,
    MessageType,
    MarketEvent,
    Trade,
};

use utils::market_name;
use types::multicast::{SourceSocket};


pub struct MarketDataFeedEngine<'a, const N: usize> {
    fifo_in: Consumer<'a, (OrderEvent, OrderResult), N>,
    shutdown: Arc<AtomicBool>,
    seq_num: u64, // Sequence number for market data feed events
    source: SourceSocket, // Multicast configuration and socket management
}

impl <'a, const N: usize> MarketDataFeedEngine<'a, N> {
    pub fn new(fifo_in: Consumer<'a, (OrderEvent, OrderResult), N>,
               shutdown: Arc<AtomicBool>,
               ip: &str,
               port: u16) -> Result<Self, std::io::Error> {

        Ok(Self { fifo_in, shutdown, seq_num: 0, source: SourceSocket::new(ip, port, market_name())? })
    }

    fn build_add_order_event(&self, order_event: &OrderEvent) -> AddOrder {
        AddOrder::from_order_event(order_event)
    }

    fn build_modify_order_event(&self, order_event: &OrderEvent) -> ModifyOrder {
        ModifyOrder::from_order_event(order_event)
    }

    fn build_add_order_event_with_quantity(
        &self,
        order_event: &OrderEvent,
        quantity: types::FixedPointArithmetic,
    ) -> AddOrder {
        let mut add_order = AddOrder::from_order_event(order_event);
        add_order.quantity = quantity;
        add_order
    }

    fn build_delete_order_event(&self, order_event: &OrderEvent) -> DeleteOrder {
        DeleteOrder::from_order_event(order_event)
    }

    fn build_trade_events(&self, order_event: &OrderEvent, order_result: &OrderResult) -> Vec<Trade> {
        order_result
            .trades
            .iter()
            .map(|trade| Trade::from_trade(order_event.side, trade))
            .collect()
    }

    fn build_header(
        &mut self,
        order_event: &OrderEvent,
        msg_type: MessageType,
        payload_len: u16,
    ) -> MarketDataHeader {
        let mut header = crate::types::MarketDataHeader::default();
        header.length = 24 + payload_len;
        header.seq_num = self.seq_num;
        self.seq_num += 1;
        header.timestamp_ns = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64;
        header.msg_type = msg_type as u8;
        header.symbol = order_event.symbol;

        header
    }

    fn build_market_data_feed_events(&mut self, order_event: &OrderEvent, order_result: &OrderResult) -> Option<Vec<MarketEvent>> {
        if order_event.order_type == types::OrderType::CancelOrder {
            let delete_order = self.build_delete_order_event(order_event);
            let header = self.build_header(order_event, MessageType::DeleteOrder, 8);
            return Some(vec![MarketEvent::Delete(header, delete_order)]);
        }

        if order_result.status == types::OrderStatus::Unmatched {
            return None;
        }

        if order_result.status == types::OrderStatus::PartiallyFilled && order_result.trades.len() == 0 {
            let modify_order = self.build_modify_order_event(order_event);
            let header = self.build_header(order_event, MessageType::ModifyOrder, 48);
            return Some(vec![MarketEvent::Modify(header, modify_order)]);
        }

        let mut events = Vec::new();
        for trade in self.build_trade_events(order_event, order_result) {
            let header = self.build_header(order_event, MessageType::Trade, 49);
            events.push(MarketEvent::Trade(header, trade));
        }

        let traded_qty = order_result.trades.quantity_sum();
        let remaining_qty = if traded_qty >= order_event.quantity {
            types::FixedPointArithmetic::ZERO
        } else {
            order_event.quantity - traded_qty
        };

        if order_event.order_type == types::OrderType::LimitOrder
            && remaining_qty > types::FixedPointArithmetic::ZERO
        {
            let add_order = self.build_add_order_event_with_quantity(order_event, remaining_qty);
            let header = self.build_header(order_event, MessageType::AddOrder, 49);
            events.push(MarketEvent::Add(header, add_order));
        }

        if events.is_empty() {
            let add_order = self.build_add_order_event(order_event);
            let header = self.build_header(order_event, MessageType::AddOrder, 49);
            events.push(MarketEvent::Add(header, add_order));
        }

        Some(events)
    }

    pub fn build_market_data_feed_from_order(&mut self, order_event: &OrderEvent, order_result: &OrderResult) -> Option<MarketEvent> {
        if let Some(events) = self.build_market_data_feed_events(order_event, order_result) {
            events.into_iter().next().or_else(|| {
                let add_order = self.build_add_order_event(order_event);
                let header = self.build_header(order_event, MessageType::AddOrder, 49);
                Some(MarketEvent::Add(header, add_order))
            })
        } else {
            None
        }
    }


    pub fn run(&mut self) {
        while !self.shutdown.load(Ordering::Relaxed) || !self.fifo_in.is_empty() {
            // If shutdown is signaled and there are no more events to process, exit the loop
            if let Some((order_event, order_result)) = self.fifo_in.pop() {
                // Process incoming order events and results from the order book engine
                // Transform the order event and result into a market data feed event
                if let Some(market_data_feed_events) = self.build_market_data_feed_events(&order_event, &order_result) {
                    for market_data_feed_event in market_data_feed_events {
                        let bytes = market_data_feed_event.to_bytes();
                        tracing::info!("[{}] Broadcasting market data feed event: header={}, event={}", market_name(), market_data_feed_event, market_data_feed_event);
                        let _ = self.source.socket.send_to(&bytes, &self.source.source.address);
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
        EntityId, OrderId, SymbolId
    };

    use super::*;
    use std::net::{Ipv4Addr};
    use std::sync::atomic::AtomicU16;
    use std::{thread};
    use std::sync::{Mutex, OnceLock};

    static NEXT_TEST_PORT: AtomicU16 = AtomicU16::new(18097);
    static UDP_TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

    fn udp_test_mutex() -> &'static Mutex<()> {
        UDP_TEST_MUTEX.get_or_init(|| Mutex::new(()))
    }

    fn next_test_port() -> u16 {
        NEXT_TEST_PORT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    fn build_engine_for_test<'a, const N: usize>(
        consumer_in: Consumer<'a, (OrderEvent, OrderResult), N>,
    ) -> MarketDataFeedEngine<'a, N> {
        MarketDataFeedEngine::new(
            consumer_in,
            Arc::new(AtomicBool::new(false)),
            "239.0.0.1",
            0,
        )
        .unwrap()
    }

    fn retrieve_market_data_feed_events(port: u16, multicast_ip: &str) -> Result<(MarketDataHeader, MarketEvent), Box<dyn std::error::Error>> {
        let socket = SourceSocket::create_multicast_socket(port)?;
        socket.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;

        let group = multicast_ip.parse::<Ipv4Addr>()?;
        if group.is_multicast() {
            let interface = "0.0.0.0".parse::<Ipv4Addr>()?;
            socket.join_multicast_v4(&group, &interface)?;
        }

        let mut buf = [0u8; 1024];

        let market_data_feed_event: (MarketDataHeader, MarketEvent) = loop {
            let (_, _) = socket.recv_from(&mut buf)?;

            let header = MarketDataHeader::from_bytes(&buf[0..24]).ok_or("Failed to deserialize MarketDataHeader")?;

            let header_size = 24;
            let body_bytes = &buf[header_size..];

            let event = match header.msg_type {
                x if x == MessageType::AddOrder as u8 => {
                    let add_order = AddOrder::from_bytes(body_bytes)
                        .ok_or("Failed to deserialize AddOrder")?;
                    MarketEvent::Add(header, add_order)
                },
                x if x == MessageType::ModifyOrder as u8 => {
                    let modify_order = ModifyOrder::from_bytes(body_bytes)
                        .ok_or("Failed to deserialize ModifyOrder")?;
                    MarketEvent::Modify(header, modify_order)
                },
                x if x == MessageType::DeleteOrder as u8 => {
                    let delete_order = DeleteOrder::from_bytes(body_bytes)
                        .ok_or("Failed to deserialize DeleteOrder")?;
                    MarketEvent::Delete(header, delete_order)
                },
                x if x == MessageType::Trade as u8 => {
                    let trade = Trade::from_bytes(body_bytes)
                        .ok_or("Failed to deserialize Trade")?;
                    MarketEvent::Trade(header, trade)
                },
                _ => {
                    return Err(format!("Unsupported message type: {}", header.msg_type).into());
                }
            };

            break (header, event);
        };

        Ok(market_data_feed_event)
    }

    fn run_engine_once_and_receive(order_event: OrderEvent, order_result: OrderResult) -> (MarketDataHeader, MarketEvent) {
        let _udp_test_guard = udp_test_mutex().lock().unwrap();
        let mut rb_in = spsc::spsc_lock_free::RingBuffer::<(OrderEvent, OrderResult), 16>::new();

        thread::scope(|s| {
            let (producer_in, consumer_in) = rb_in.split();

            let multicast_ip = "239.0.0.1";
            let port = next_test_port();
            let shutdown_signal = Arc::new(AtomicBool::new(false));

            let mut engine = MarketDataFeedEngine::new(
                consumer_in,
                Arc::clone(&shutdown_signal),
                multicast_ip,
                port,
            )
            .unwrap();

            let engine_handle = s.spawn(move || {
                engine.run();
            });

            let recv_handle = s.spawn(move || retrieve_market_data_feed_events(port, multicast_ip).unwrap());

            std::thread::sleep(std::time::Duration::from_millis(100));
            producer_in.push((order_event, order_result)).unwrap();

            let received = recv_handle.join().unwrap();

            shutdown_signal.store(true, Ordering::Relaxed);
            producer_in.push((order_event, order_result)).unwrap();

            engine_handle.join().unwrap();

            received
        })
    }

    fn assert_header_fields(
        header: &MarketDataHeader,
        order_event: &OrderEvent,
        msg_type: MessageType,
        payload_len: u16,
        expected_seq_num: u64,
    ) {
        let length = header.length;
        let header_msg_type = header.msg_type;
        let version = header.version;
        let seq_num = header.seq_num;
        let timestamp_ns = header.timestamp_ns;
        let symbol = header.symbol;

        assert_eq!(length, 24 + payload_len);
        assert_eq!(header_msg_type, msg_type as u8);
        assert_eq!(version, 1);
        assert_eq!(seq_num, expected_seq_num);
        assert!(timestamp_ns > 0);
        assert_eq!(symbol, order_event.symbol);
    }

    fn stable_u64_from_fixed_20(bytes: &[u8; 20]) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        for &b in bytes {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    #[test]
    fn test_build_market_data_feed_modify_event() {
        let mut rb_in = spsc::spsc_lock_free::RingBuffer::<(OrderEvent, OrderResult), 16>::new();
        let (_, consumer_in) = rb_in.split();
        let mut engine = build_engine_for_test(consumer_in);

        let order_event = OrderEvent {
            sender_id: EntityId::from_ascii("test_sender"),
            cl_ord_id: OrderId::from_ascii("order123"),
            side: types::Side::Buy,
            order_type: types::OrderType::LimitOrder,
            price: types::FixedPointArithmetic(100),
            quantity: types::FixedPointArithmetic(10),
            symbol: SymbolId::from_ascii("BTCUSD"),
            ..Default::default()
        };

        let order_result = OrderResult {
            status: types::OrderStatus::PartiallyFilled,
            ..OrderResult::default()
        };

        let event = engine.build_market_data_feed_from_order(&order_event, &order_result).unwrap();
        match event {
            MarketEvent::Modify(header, modify_order) => {
                assert_header_fields(&header, &order_event, MessageType::ModifyOrder, 48, 0);

                let order_id = modify_order.order_id;
                let new_price = modify_order.new_price;
                let new_quantity = modify_order.new_quantity;

                assert_eq!(order_id, stable_u64_from_fixed_20(&order_event.cl_ord_id.0));
                assert_eq!(new_price, order_event.price);
                assert_eq!(new_quantity, order_event.quantity);
            }
            _ => panic!("Expected ModifyOrder event"),
        }
    }

    #[test]
    fn test_build_market_data_feed_delete_event() {
        let mut rb_in = spsc::spsc_lock_free::RingBuffer::<(OrderEvent, OrderResult), 16>::new();
        let (_, consumer_in) = rb_in.split();
        let mut engine = build_engine_for_test(consumer_in);

        let order_event = OrderEvent {
            sender_id: EntityId::from_ascii("test_sender"),
            cl_ord_id: OrderId::from_ascii("cancel01"),
            orig_cl_ord_id: Some(OrderId::from_ascii("orig1234")),
            order_type: types::OrderType::CancelOrder,
            symbol: SymbolId::from_ascii("ETHUSD"),
            ..Default::default()
        };

        let order_result = OrderResult {
            status: types::OrderStatus::Cancelled,
            ..OrderResult::default()
        };

        let event = engine.build_market_data_feed_from_order(&order_event, &order_result).unwrap();

        match event {
            MarketEvent::Delete(header, delete_order) => {
                assert_header_fields(&header, &order_event, MessageType::DeleteOrder, 8, 0);
                let order_id = delete_order.order_id;
                assert_eq!(order_id, stable_u64_from_fixed_20(&OrderId::from_ascii("orig1234").0));
            }
            _ => panic!("Expected DeleteOrder event"),
        }
    }

    #[test]
    fn test_build_market_data_feed_trade_event() {
        let mut rb_in = spsc::spsc_lock_free::RingBuffer::<(OrderEvent, OrderResult), 16>::new();
        let (_, consumer_in) = rb_in.split();
        let mut engine = build_engine_for_test(consumer_in);

        let order_event = OrderEvent {
            sender_id: EntityId::from_ascii("test_sender"),
            cl_ord_id: OrderId::from_ascii("order123"),
            side: types::Side::Sell,
            order_type: types::OrderType::LimitOrder,
            symbol: SymbolId::from_ascii("AAPL"),
            ..Default::default()
        };

        let mut trades = types::Trades::default();
        trades
            .add_trade(types::Trade {
                price: types::FixedPointArithmetic(101),
                quantity: types::FixedPointArithmetic(3),
                id: 0,
                cl_ord_id: OrderId::from_ascii("maker001"),
                order_qty: types::FixedPointArithmetic(5),
                leaves_qty: types::FixedPointArithmetic(2),
                ..Default::default()
            })
            .unwrap();

        let order_result = OrderResult {
            internal_order_id: 0,
            trades,
            status: types::OrderStatus::PartiallyFilled,
            ..Default::default()
        };

        let event = engine.build_market_data_feed_from_order(&order_event, &order_result).unwrap();
        match event {
            MarketEvent::Trade(header, trade) => {
                assert_header_fields(&header, &order_event, MessageType::Trade, 49, 0);

                let side = trade.side;
                let trade_id = trade.trade_id;
                let price = trade.price;
                let quantity = trade.quantity;

                assert_eq!(side, 2);
                assert_eq!(trade_id, 0);
                assert_eq!(price, types::FixedPointArithmetic(101));
                assert_eq!(quantity, types::FixedPointArithmetic(3));
            }
            _ => panic!("Expected Trade event"),
        }
    }

    #[test]
    fn test_market_data_feed_engine() {
        let order_event = OrderEvent {
            sender_id: EntityId::from_ascii("test_sender"),
            cl_ord_id: OrderId::from_ascii("order123"),
            side: types::Side::Buy,
            price: types::FixedPointArithmetic(100),
            quantity: types::FixedPointArithmetic(10),
            symbol: SymbolId::from_ascii("EURUSD"),
            ..Default::default()
        };

        let order_result = OrderResult {
            internal_order_id: 0,
            trades: types::Trades::default(),
            status: types::OrderStatus::Filled,
            ..Default::default()
        };

        let (header, event) = run_engine_once_and_receive(order_event, order_result);

        assert_header_fields(&header, &order_event, MessageType::AddOrder, 49, 0);
        match event {
            MarketEvent::Add(_, add_order) => {
                let order_id = add_order.order_id;
                let side = add_order.side;
                let price = add_order.price;
                let quantity = add_order.quantity;

                assert_eq!(order_id, stable_u64_from_fixed_20(&order_event.cl_ord_id.0));
                assert_eq!(side, 1);
                assert_eq!(price, order_event.price);
                assert_eq!(quantity, order_event.quantity);
            },
            _ => panic!("Expected AddOrder event"),
        }
    }

    #[test]
    fn test_market_data_feed_engine_modify_event_udp() {
        let order_event = OrderEvent {
            sender_id: EntityId::from_ascii("test_sender"),
            cl_ord_id: OrderId::from_ascii("modify01"),
            side: types::Side::Sell,
            order_type: types::OrderType::LimitOrder,
            price: types::FixedPointArithmetic(250),
            quantity: types::FixedPointArithmetic(7),
            symbol: SymbolId::from_ascii("XAUUSD"),
            ..Default::default()
        };

        let order_result = OrderResult {
            status: types::OrderStatus::PartiallyFilled,
            ..OrderResult::default()
        };

        let (header, event) = run_engine_once_and_receive(order_event, order_result);
        assert_header_fields(&header, &order_event, MessageType::ModifyOrder, 48, 0);
        match event {
            MarketEvent::Modify(_, modify_order) => {
                let order_id = modify_order.order_id;
                let new_price = modify_order.new_price;
                let new_quantity = modify_order.new_quantity;

                assert_eq!(order_id, stable_u64_from_fixed_20(&order_event.cl_ord_id.0));
                assert_eq!(new_price, order_event.price);
                assert_eq!(new_quantity, order_event.quantity);
            }
            _ => panic!("Expected ModifyOrder event"),
        }
    }

    #[test]
    fn test_market_data_feed_engine_delete_event_udp() {
        let order_event = OrderEvent {
            sender_id: EntityId::from_ascii("test_sender"),
            cl_ord_id: OrderId::from_ascii("cancel01"),
            orig_cl_ord_id: Some(OrderId::from_ascii("orig1234")),
            order_type: types::OrderType::CancelOrder,
            symbol: SymbolId::from_ascii("TSLA"),
            ..Default::default()
        };

        let (header, event) = run_engine_once_and_receive(order_event, OrderResult::default());
        assert_header_fields(&header, &order_event, MessageType::DeleteOrder, 8, 0);
        match event {
            MarketEvent::Delete(_, delete_order) => {
                let order_id = delete_order.order_id;
                assert_eq!(order_id, stable_u64_from_fixed_20(&OrderId::from_ascii("orig1234").0));
            }
            _ => panic!("Expected DeleteOrder event"),
        }
    }

    #[test]
    fn test_market_data_feed_engine_trade_event_udp() {
        let order_event = OrderEvent {
            sender_id: EntityId::from_ascii("test_sender"),
            cl_ord_id: OrderId::from_ascii("tradeord1"),
            side: types::Side::Buy,
            order_type: types::OrderType::LimitOrder,
            symbol: SymbolId::from_ascii("NFLX"),
            ..Default::default()
        };

        let mut trades = types::Trades::default();
        trades
            .add_trade(types::Trade {
                price: types::FixedPointArithmetic(333),
                quantity: types::FixedPointArithmetic(9),
                id: 0,
                cl_ord_id: OrderId::from_ascii("maker001"),
                order_qty: types::FixedPointArithmetic(10),
                leaves_qty: types::FixedPointArithmetic(1),
                ..Default::default()
            })
            .unwrap();

        let order_result = OrderResult {
            internal_order_id: 0,
            trades,
            status: types::OrderStatus::Filled,
            ..Default::default()
        };

        let (header, event) = run_engine_once_and_receive(order_event, order_result);
        assert_header_fields(&header, &order_event, MessageType::Trade, 49, 0);
        match event {
            MarketEvent::Trade(_, trade) => {
                let side = trade.side;
                let trade_id = trade.trade_id;
                let price = trade.price;
                let quantity = trade.quantity;

                assert_eq!(side, 1);
                assert_eq!(trade_id, 0);
                assert_eq!(price, types::FixedPointArithmetic(333));
                assert_eq!(quantity, types::FixedPointArithmetic(9));
            }
            _ => panic!("Expected Trade event"),
        }
    }
}