use types::{OrderEvent, OrderResult, StopHandle};
use spsc::spsc_lock_free::{Consumer, Producer};
use std::sync::atomic::{AtomicBool, Ordering};
use fix::{engine::FixRawMsg, tags::{exec_type_code_set, msg_types, ord_status_code_set, side_code_set, tags::{self}}};
use utils::{field_str, number_to_bytes};
use std::sync::Arc;


pub struct ExecutionReportEngine<'a, const N: usize> {
    fifo_in: Consumer<'a, (OrderEvent, OrderResult), N>,
    fifo_out: Producer<'a, (u64, FixRawMsg<N>), N>,
    shutdown: Arc<AtomicBool>,
}

impl<'a, const N: usize> ExecutionReportEngine<'a, N> {
    pub fn new(fifo_in: Consumer<'a, (OrderEvent, OrderResult), N>, fifo_out: Producer<'a, (u64, FixRawMsg<N>), N>) -> Self {
        Self { fifo_in, fifo_out, shutdown: Arc::new(AtomicBool::new(false)) }
    }

    pub fn run(&self) {
        loop {
            if let Some(exec_report) = self.fifo_in.try_pop() {
                self.process_execution_report(&exec_report);
            }

            if self.shutdown.load(Ordering::Relaxed) && self.fifo_in.is_empty() {
                break;
            }
        }
    }

    fn build_execution_report(&self, exec_report: &(OrderEvent, OrderResult)) -> FixRawMsg<N> {
        let mut report = FixRawMsg::<N>::default();
        let mut cursor = 0;
        let order = &exec_report.0;
        let order_result = &exec_report.1;


        // Build FIX message header
        self.build_field(tags::BEGIN_STRING, b"FIX.4.2", &mut report, &mut cursor);

        // Build FIX message body
        self.build_field(tags::MSG_TYPE, msg_types::EXECUTION_REPORT, &mut report, &mut cursor);

        match order_result.status {
            types::OrderStatus::New => {
                self.build_field(tags::ORD_STATUS, ord_status_code_set::NEW, &mut report, &mut cursor); // ExecType=New
            },
            types::OrderStatus::PartiallyFilled => {
                self.build_field(tags::ORD_STATUS, ord_status_code_set::PARTIAL_FILL, &mut report, &mut cursor); // ExecType=PartiallyFilled
            },
            types::OrderStatus::Filled => {
                self.build_field(tags::ORD_STATUS, ord_status_code_set::FILL, &mut report, &mut cursor); // ExecType=Filled
            },
            types::OrderStatus::Canceled => {
                self.build_field(tags::ORD_STATUS, ord_status_code_set::CANCELED, &mut report, &mut cursor); // ExecType=Canceled
            },
        }

        match order_result.status {
            types::OrderStatus::New => {
                self.build_field(tags::EXEC_TYPE, exec_type_code_set::NEW, &mut report, &mut cursor); // ExecType=New
            },
            types::OrderStatus::PartiallyFilled => {
                self.build_field(tags::EXEC_TYPE, exec_type_code_set::TRADE, &mut report, &mut cursor); // ExecType=PartiallyFilled
            },
            types::OrderStatus::Filled => {
                self.build_field(tags::EXEC_TYPE, exec_type_code_set::TRADE, &mut report, &mut cursor); // ExecType=Filled
            },
            types::OrderStatus::Canceled => {
                self.build_field(tags::EXEC_TYPE, exec_type_code_set::CANCELED, &mut report, &mut cursor); // ExecType=Canceled
            },
        }

        self.build_field(tags::ORDER_ID, &order.order_id.as_ref(), &mut report, &mut cursor);
        self.build_field(tags::CL_ORD_ID, &order.cl_ord_id.as_ref(), &mut report, &mut cursor);

        match order.side {
            types::Side::Buy => self.build_field(tags::SIDE, side_code_set::BUY, &mut report, &mut cursor),
            types::Side::Sell => self.build_field(tags::SIDE, side_code_set::SELL, &mut report, &mut cursor),
        }

        // Switch sender and target for the execution report since it's going back to the client
        self.build_field(tags::SENDER_COMP_ID, &order.target_id.as_ref(), &mut report, &mut cursor); 
        self.build_field(tags::TARGET_COMP_ID, &order.sender_id.as_ref(), &mut report, &mut cursor);
        self.build_field(tags::SYMBOL, &order.symbol.as_ref(), &mut report, &mut cursor);
        self.build_field(tags::ORDER_QTY, &number_to_bytes(order.quantity).as_ref(), &mut report, &mut cursor);
        self.build_field(tags::SENDING_TIME, &utils::UtcTimestamp::now().to_fix_bytes(), &mut report, &mut cursor);
    
        if !order_result.trades.is_empty() {
            self.build_field(tags::LAST_PX, &number_to_bytes(order_result.trades.iter().map(|t| t.price.raw() as u64).sum::<u64>()).as_ref(), &mut report, &mut cursor);
            self.build_field(tags::LAST_QTY, &number_to_bytes(order_result.trades.iter().map(|t| t.quantity).sum::<u64>()).as_ref(), &mut report, &mut cursor);
            self.build_field(tags::AVG_PX, &number_to_bytes(order_result.trades.iter().map(|t| t.price.raw() as u64).sum::<u64>() / order_result.trades.len() as u64).as_ref(), &mut report, &mut cursor);
        }

        self.build_field(tags::BODY_LENGTH, &number_to_bytes((cursor - 2) as u64).as_ref(), &mut report, &mut cursor); // Body length is everything after the BodyLength field (which is 2 bytes for tag and equals sign)
        report.len = cursor as u16;
        report
    }

    fn process_execution_report(&self, exec_report: &(OrderEvent, OrderResult)) {
        let raw_report = self.build_execution_report(exec_report);

        let key = utils::make_key(
            u32::from_le_bytes(exec_report.0.sender_id.0[0..4].try_into().unwrap_or_default()), 
            u32::from_le_bytes(exec_report.0.order_id.0[0..4].try_into().unwrap_or_default())
        );

        match self.fifo_out.push((key, raw_report)) {
            Ok(_) => println!("Pushed execution report into output queue"),
            Err(e) => panic!("Failed to push execution report into output queue: {:?}", e),
        }
    }

    fn build_field(&self, tag: u32, value: &[u8], report: &mut FixRawMsg<N>, cursor: &mut usize) {
        let mut buf = itoa::Buffer::new();
        let tag_str = buf.format(tag);
        let value = field_str(value);
        report.data[*cursor..*cursor + tag_str.len()].copy_from_slice(tag_str.as_bytes());
        *cursor += tag_str.len();
        report.data[*cursor] = fix::parser::EQUALS; // EQUAL
        *cursor += 1;
        report.data[*cursor..*cursor + value.len()].copy_from_slice(value);
        *cursor += value.len();
        report.data[*cursor] = fix::parser::SOH; // SOH
        *cursor += 1;
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
    use utils::UtcTimestamp;

    use super::*;

    #[test]
    fn test_execution_report_engine() {
        let mut rb_in = spsc::spsc_lock_free::RingBuffer::<(OrderEvent, OrderResult), 1024>::new();
        let mut rb_out = spsc::spsc_lock_free::RingBuffer::<(u64, FixRawMsg<1024>), 1024>::new();
        let (fifo_in_tx, fifo_in_rx) = rb_in.split();
        let (fifo_out_tx, fifo_out_rx) = rb_out.split();
        let engine = ExecutionReportEngine::new(fifo_in_rx, fifo_out_tx);
        
        std::thread::scope(|s| {

            let stop_handle = engine.stop_handle();

            let handle = s.spawn(move || {
                engine.run();
            });

            std::thread::sleep(std::time::Duration::from_millis(100)); // Give the engine some time to start

            let order_event = types::OrderEvent {
                order_type: types::OrderType::LimitOrder,
                cl_ord_id: types::OrderId::from_ascii("CLORD12345"),
                order_id: types::OrderId::from_ascii("ORDERID"),
                side: types::Side::Buy,
                price: types::Price(123_456_000), // 123.456 in FIX price format (8 decimal places)
                quantity: 100,
                sender_id: types::EntityId::from_ascii("SENDER"),
                target_id: types::EntityId::from_ascii("TARGET"),
                symbol: types::FixedString::from_ascii("TEST_SYMBOL"),
                timestamp: UtcTimestamp::now().to_unix_ms(), // Current timestamp in milliseconds since epoch
            };

            let order_result = types::OrderResult {
                trades: vec![],
                status: types::OrderStatus::New,
                timestamp: UtcTimestamp::now().to_unix_ms(), // Current timestamp in milliseconds since epoch
            };

            match fifo_in_tx.push((order_event.clone(), order_result)) {
                Ok(_) => println!("Pushed order event into engine"),
                Err(e) => panic!("Failed to push order event into engine: {:?}", e),
            }

            std::thread::sleep(std::time::Duration::from_millis(100)); // Give the engine some time to process

            let (_, raw_report) = fifo_out_rx.try_pop().expect("No execution report generated");
            let mut fix_parser = fix::parser::FixParser::new(&raw_report.data[..raw_report.len as usize]);
            let parsed_report = fix_parser.get_fields();
            
            let msg_type_field = parsed_report.fields.iter().find(|f| f.tag == tags::MSG_TYPE).expect("MSG_TYPE field missing");
            assert_eq!(msg_type_field.value, msg_types::EXECUTION_REPORT);
            let ord_status_field = parsed_report.fields.iter().find(|f| f.tag == tags::ORD_STATUS).expect("ORD_STATUS field missing");
            assert_eq!(ord_status_field.value, ord_status_code_set::NEW);
            let exec_type_field = parsed_report.fields.iter().find(|f| f.tag == tags::EXEC_TYPE).expect("EXEC_TYPE field missing");
            assert_eq!(exec_type_field.value, exec_type_code_set::NEW);
            let cl_ord_id_field = parsed_report.fields.iter().find(|f| f.tag == tags::CL_ORD_ID).expect("CL_ORD_ID field missing");
            assert_eq!(cl_ord_id_field.value, field_str(&order_event.cl_ord_id.as_ref()));
            let order_id_field = parsed_report.fields.iter().find(|f| f.tag == tags::ORDER_ID).expect("ORDER_ID field missing");
            assert_eq!(order_id_field.value, field_str(&order_event.order_id.as_ref()));
            let side_field = parsed_report.fields.iter().find(|f| f.tag == tags::SIDE).expect("SIDE field missing");
            assert_eq!(side_field.value, side_code_set::BUY);
            let symbol_field = parsed_report.fields.iter().find(|f| f.tag == tags::SYMBOL).expect("SYMBOL field missing");
            assert_eq!(symbol_field.value, field_str(&order_event.symbol.as_ref()));
            let order_qty_field = parsed_report.fields.iter().find(|f| f.tag == tags::ORDER_QTY).expect("ORDER_QTY field missing");
            assert_eq!(order_qty_field.value, number_to_bytes(order_event.quantity).as_ref());
            // let last_qty_field = parsed_report.fields.iter().find(|f| f.tag == tags::LAST_QTY).expect("LAST_QTY field missing");
            // assert_eq!(last_qty_field.value, number_to_bytes(0u64).as_ref()); // No trades executed, so LAST_QTY should be 0
            // let last_px_field = parsed_report.fields.iter().find(|f| f.tag == tags::LAST_PX).expect("LAST_PX field missing");
            // assert_eq!(last_px_field.value, number_to_bytes(0u64).as_ref()); // No trades executed, so LAST_PX should be 0
            // let avg_px_field = parsed_report.fields.iter().find(|f| f.tag == tags::AVG_PX).expect("AVG_PX field missing");
            // assert_eq!(avg_px_field.value, number_to_bytes(0u64).as_ref()); // No trades executed, so AVG_PX should be 0
    
            // Add a SELL order to generate a trade and test that LAST_QTY, LAST_PX, and AVG_PX are populated correctly in the execution report
            let sell_order_event = types::OrderEvent {
                order_type: types::OrderType::LimitOrder,
                cl_ord_id: types::OrderId::from_ascii("CLORD54321"),
                order_id: types::OrderId::from_ascii("ORDERID2"),
                side: types::Side::Sell,
                price: types::Price(123_456_000), // 123.456 in FIX price format (8 decimal places)
                quantity: 50,
                sender_id: types::EntityId::from_ascii("SENDER2"),
                target_id: types::EntityId::from_ascii("TARGET2"),
                symbol: types::FixedString::from_ascii("TEST_SYMBOL"),
                timestamp: utils::UtcTimestamp::now().to_unix_ms(), // Current timestamp in milliseconds since epoch
            };

            let sell_order_result = types::OrderResult {
                trades: vec![types::Trade { id: types::TradeId::default(), quantity: 50, price: types::Price(123_456_000) }],
                status: types::OrderStatus::PartiallyFilled,
                timestamp: utils::UtcTimestamp::now().to_unix_ms(), // Current timestamp in milliseconds since epoch
            };

            match fifo_in_tx.push((sell_order_event, sell_order_result)) {
                Ok(_) => println!("Pushed sell order event into engine"),
                Err(e) => panic!("Failed to push sell order event into engine: {:?}", e),
            }

            std::thread::sleep(std::time::Duration::from_millis(100)); // Give the engine some time to process

            let (_, raw_report) = fifo_out_rx.try_pop().expect("No execution report generated for sell order");
            let mut fix_parser = fix::parser::FixParser::new(&raw_report.data[..raw_report.len as usize]);
            let parsed_report = fix_parser.get_fields();    

            let last_qty_field = parsed_report.fields.iter().find(|f| f.tag == tags::LAST_QTY).expect("LAST_QTY field missing");
            assert_eq!(last_qty_field.value, number_to_bytes(50u64).as_ref()); // 50 units filled
            let last_px_field = parsed_report.fields.iter().find(|f| f.tag == tags::LAST_PX).expect("LAST_PX field missing");
            assert_eq!(last_px_field.value, number_to_bytes(123_456_000u64).as_ref()); // 123.456 price
            let avg_px_field = parsed_report.fields.iter().find(|f| f.tag == tags::AVG_PX).expect("AVG_PX field missing");
            assert_eq!(avg_px_field.value, number_to_bytes(123_456_000u64).as_ref()); // 123.456 price, since only one trade executed

            stop_handle.shutdown.store(true, Ordering::Relaxed);
            handle.join().unwrap();
        });
    }
}