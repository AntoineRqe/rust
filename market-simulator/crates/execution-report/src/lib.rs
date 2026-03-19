use types::{EntityId, OrderEvent, OrderResult};
use spsc::spsc_lock_free::{Consumer, Producer};
use std::sync::atomic::{AtomicBool, Ordering};
use fix::{engine::FixRawMsg, tags::{exec_type_code_set, msg_types, ord_status_code_set, side_code_set, tags::{self}}};
use utils::{field_str, number_to_bytes};
use std::sync::Arc;
use types::FixedPointArithmetic;

pub fn kill_execution_report_engine<const N: usize>(producer: &Producer<'_, (OrderEvent, OrderResult), N>) {
    // Send a dummy message with an empty sender_id to signal the engine to shut down
    let _ = producer.push((OrderEvent::default(), OrderResult::default()));
}

pub struct ExecutionReportEngine<'a, const N: usize> {
    fifo_in: Consumer<'a, (OrderEvent, OrderResult), N>,
    fifo_out: Producer<'a, (EntityId, FixRawMsg<N>), N>,
    shutdown: Arc<AtomicBool>,
}

impl<'a, const N: usize> ExecutionReportEngine<'a, N> {
    pub fn new(fifo_in: Consumer<'a, (OrderEvent, OrderResult), N>, fifo_out: Producer<'a, (EntityId, FixRawMsg<N>), N>) -> Self {
        Self { fifo_in, fifo_out, shutdown: Arc::new(AtomicBool::new(false)) }
    }

    pub fn run(&self) {
        while !self.shutdown.load(Ordering::Relaxed) && self.fifo_in.is_empty() {
            if let Some(exec_report) = self.fifo_in.pop() {
                if exec_report.0.sender_id == EntityId::from_ascii("") {
                    self.fifo_out.push((EntityId::default(), FixRawMsg::default())).ok(); // Push a dummy message to unblock any waiting consumers
                    self.shutdown.store(true, Ordering::Relaxed);
                } else {
                    self.process_execution_report(&exec_report);
                }
            }
        }
        println!("Execution report engine shutting down gracefully");
    }

    fn build_cancel_report(&self, exec_report: &(OrderEvent, OrderResult)) -> FixRawMsg<N> {
        let mut report = FixRawMsg::<N>::default();
        let mut cursor = 0;
        let order = &exec_report.0;
        let order_result = &exec_report.1;

        // Build FIX message header
        self.build_field(tags::BEGIN_STRING, b"FIX.4.2", &mut report, &mut cursor);

        // Build FIX message body
        if order_result.status == types::OrderStatus::Cancelled {
            self.build_field(tags::MSG_TYPE, msg_types::EXECUTION_REPORT, &mut report, &mut cursor);
            self.build_field(tags::ORD_STATUS, ord_status_code_set::CANCELED, &mut report, &mut cursor); // OrdStatus=Cancelled
            self.build_field(tags::EXEC_TYPE, exec_type_code_set::CANCELED, &mut report, &mut cursor); // ExecType=Cancelled
        } else if order_result.status == types::OrderStatus::CancelRejected {
            self.build_field(tags::MSG_TYPE, msg_types::ORDER_CANCEL_REJECTION, &mut report, &mut cursor);
            self.build_field(tags::ORD_STATUS, ord_status_code_set::NEW, &mut report, &mut cursor); // OrdStatus=New
        }

        match order.side {
            types::Side::Buy => self.build_field(tags::SIDE, side_code_set::BUY, &mut report, &mut cursor),
            types::Side::Sell => self.build_field(tags::SIDE, side_code_set::SELL, &mut report, &mut cursor),
        }

        // Switch sender and target for the execution report since it's going back to the client
        self.build_field(tags::SENDER_COMP_ID, &order.target_id.as_ref(), &mut report, &mut cursor); 
        self.build_field(tags::TARGET_COMP_ID, &order.sender_id.as_ref(), &mut report, &mut cursor);
        self.build_field(tags::SYMBOL, &order.symbol.as_ref(), &mut report, &mut cursor);
        self.build_field(tags::SENDING_TIME, &utils::UtcTimestamp::now().to_fix_bytes(), &mut report, &mut cursor);

        self.build_field(tags::BODY_LENGTH, &number_to_bytes((cursor - 2) as u64).as_ref(), &mut report, &mut cursor); // Body length is everything after the BodyLength field (which is 2 bytes for tag and equals sign)
        self.build_field(tags::CHECK_SUM, &number_to_bytes((cursor - 2) as u64).as_ref(), &mut report, &mut cursor);

        report.len = cursor as u16;
        report
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

        let traded_qty = order_result.trades.quantity_sum();
        let remaining_qty = order.quantity - traded_qty;

        if remaining_qty > FixedPointArithmetic::ZERO {
                self.build_field(tags::ORD_STATUS, ord_status_code_set::PARTIAL_FILL, &mut report, &mut cursor); // ExecType=PartiallyFilled
        } else {
                self.build_field(tags::ORD_STATUS, ord_status_code_set::FILL, &mut report, &mut cursor); // ExecType=Filled
        }

        // At this point, we know there is a trade
        self.build_field(tags::EXEC_TYPE, exec_type_code_set::TRADE, &mut report, &mut cursor); // ExecType=PartiallyFilled

        self.build_field(tags::ORDER_ID, &order.order_id.as_ref(), &mut report, &mut cursor);
        self.build_field(tags::CL_ORD_ID, &order.cl_ord_id.as_ref(), &mut report, &mut cursor);

        match order.side {
            types::Side::Buy => self.build_field(tags::SIDE, side_code_set::BUY, &mut report, &mut cursor),
            types::Side::Sell => self.build_field(tags::SIDE, side_code_set::SELL, &mut report, &mut cursor),
        }

        self.build_field(tags::PRICE, &order.price.to_fix_bytes(), &mut report, &mut cursor);
        self.build_field(tags::LEAVES_QTY, &remaining_qty.to_fix_bytes(), &mut report, &mut cursor);
        self.build_field(tags::CUM_QTY, &traded_qty.to_fix_bytes(), &mut report, &mut cursor);

        // Switch sender and target for the execution report since it's going back to the client
        self.build_field(tags::SENDER_COMP_ID, &order.target_id.as_ref(), &mut report, &mut cursor); 
        self.build_field(tags::TARGET_COMP_ID, &order.sender_id.as_ref(), &mut report, &mut cursor);
        self.build_field(tags::SYMBOL, &order.symbol.as_ref(), &mut report, &mut cursor);
        self.build_field(tags::ORDER_QTY, &order.quantity.to_fix_bytes(), &mut report, &mut cursor);
        self.build_field(tags::SENDING_TIME, &utils::UtcTimestamp::now().to_fix_bytes(), &mut report, &mut cursor);
    
        if order_result.trades.len() > 0 {
            self.build_field(tags::LAST_PX, &order_result.trades[0].price.to_fix_bytes(), &mut report, &mut cursor);
            self.build_field(tags::LAST_QTY, &traded_qty.to_fix_bytes(), &mut report, &mut cursor);
            self.build_field(tags::AVG_PX, &order_result.trades.avg_price().to_fix_bytes(), &mut report, &mut cursor);
        }

        self.build_field(tags::BODY_LENGTH, &number_to_bytes((cursor - 2) as u64).as_ref(), &mut report, &mut cursor); // Body length is everything after the BodyLength field (which is 2 bytes for tag and equals sign)
        self.build_field(tags::CHECK_SUM, &number_to_bytes((cursor - 2) as u64).as_ref(), &mut report, &mut cursor);

        report.len = cursor as u16;
        report
    }

    fn process_execution_report(&self, exec_report: &(OrderEvent, OrderResult)) {

        let mut reports: Vec<FixRawMsg<N>> = vec![];

        if exec_report.1.status == types::OrderStatus::Cancelled || exec_report.1.status == types::OrderStatus::CancelRejected {
            reports.push(self.build_cancel_report(exec_report));
        } else {
            reports.push(self.build_new_execution_report(exec_report));

            if exec_report.1.trades.len() > 0 {
                reports.push(self.build_execution_report(exec_report));
            }
        }

        let key = exec_report.0.sender_id;

        for mut report in reports.into_iter() {
            loop {
                if let Err((_, _report)) = self.fifo_out.push((key, report)) {
                    report = _report;
                    std::hint::spin_loop();
                } else {
                    break;
                }
            }
        }
    }

    fn build_new_execution_report(&self, exec_report: &(OrderEvent, OrderResult)) -> FixRawMsg<N> {
        let mut report = FixRawMsg::<N>::default();
        let mut cursor = 0;
        let order = &exec_report.0;

        // Build FIX message header
        self.build_field(tags::BEGIN_STRING, b"FIX.4.2", &mut report, &mut cursor);

        // Build FIX message body
        self.build_field(tags::MSG_TYPE, msg_types::EXECUTION_REPORT, &mut report, &mut cursor);

        // Set NEW status for all new execution reports, since this is the first report being sent for a new order.
        self.build_field(tags::ORD_STATUS, ord_status_code_set::NEW, &mut report, &mut cursor); // OrdStatus=New
        self.build_field(tags::EXEC_TYPE, exec_type_code_set::NEW, &mut report, &mut cursor); // ExecType=New

        self.build_field(tags::ORDER_ID, &order.order_id.as_ref(), &mut report, &mut cursor);
        self.build_field(tags::CL_ORD_ID, &order.cl_ord_id.as_ref(), &mut report, &mut cursor);

        match order.side {
            types::Side::Buy => self.build_field(tags::SIDE, side_code_set::BUY, &mut report, &mut cursor),
            types::Side::Sell => self.build_field(tags::SIDE, side_code_set::SELL, &mut report, &mut cursor),
        }

        self.build_field(tags::PRICE, &order.price.to_fix_bytes(), &mut report, &mut cursor);
        self.build_field(tags::ORDER_QTY, &order.quantity.to_fix_bytes(), &mut report, &mut cursor);
        self.build_field(tags::LEAVES_QTY, &order.quantity.to_fix_bytes(), &mut report, &mut cursor);
        self.build_field(tags::CUM_QTY, &FixedPointArithmetic::ZERO.to_fix_bytes(), &mut report, &mut cursor);

        // Switch sender and target for the execution report since it's going back to the client
        self.build_field(tags::SENDER_COMP_ID, &order.target_id.as_ref(), &mut report, &mut cursor); 
        self.build_field(tags::TARGET_COMP_ID, &order.sender_id.as_ref(), &mut report, &mut cursor);
        self.build_field(tags::SYMBOL, &order.symbol.as_ref(), &mut report, &mut cursor);
        self.build_field(tags::SENDING_TIME, &utils::UtcTimestamp::now().to_fix_bytes(), &mut report, &mut cursor);
    
        self.build_field(tags::LAST_PX, &FixedPointArithmetic::ZERO.to_fix_bytes(), &mut report, &mut cursor);
        self.build_field(tags::LAST_QTY, &FixedPointArithmetic::ZERO.to_fix_bytes(), &mut report, &mut cursor);
        self.build_field(tags::AVG_PX, &FixedPointArithmetic::ZERO.to_fix_bytes(), &mut report, &mut cursor);

        self.build_field(tags::BODY_LENGTH, &number_to_bytes((cursor - 2) as u64).as_ref(), &mut report, &mut cursor); // Body length is everything after the BodyLength field (which is 2 bytes for tag and equals sign)
        self.build_field(tags::CHECK_SUM, &number_to_bytes((cursor - 2) as u64).as_ref(), &mut report, &mut cursor);

        report.len = cursor as u16;
        report
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
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use types::Trades;

    use super::*;

    #[test]
    fn test_execution_report_engine() {
        let mut rb_in = spsc::spsc_lock_free::RingBuffer::<(OrderEvent, OrderResult), 1024>::new();
        let mut rb_out = spsc::spsc_lock_free::RingBuffer::<(EntityId, FixRawMsg<1024>), 1024>::new();
        let (fifo_in_tx, fifo_in_rx) = rb_in.split();
        let (fifo_out_tx, fifo_out_rx) = rb_out.split();
        let engine = ExecutionReportEngine::new(fifo_in_rx, fifo_out_tx);
        
        std::thread::scope(|s| {

            let handle = s.spawn(move || {
                engine.run();
            });

            std::thread::sleep(std::time::Duration::from_millis(100)); // Give the engine some time to start

            let order_event = types::OrderEvent {
                order_type: types::OrderType::LimitOrder,
                cl_ord_id: types::OrderId::from_ascii("CLORD12345"),
                orig_cl_ord_id: None,
                order_id: types::OrderId::from_ascii("ORDERID"),
                side: types::Side::Buy,
                price: types::FixedPointArithmetic(123_456_000), // 123.456 in FIX price format (8 decimal places)
                quantity: types::FixedPointArithmetic(1_000_000),
                sender_id: types::EntityId::from_ascii("SENDER"),
                target_id: types::EntityId::from_ascii("TARGET"),
                symbol: types::FixedString::from_ascii("TEST_SYMBOL"),
                timestamp: Instant::now(), // Current timestamp in milliseconds since epoch
            };

            let order_result = types::OrderResult {
                trades: types::Trades::default(),
                status: types::OrderStatus::New,
                timestamp: Instant::now(), // Current timestamp in milliseconds since epoch
            };

            match fifo_in_tx.push((order_event.clone(), order_result)) {
                Ok(_) => {},
                Err(e) => panic!("Failed to push order event into engine: {:?}", e),
            }

            std::thread::sleep(std::time::Duration::from_millis(100)); // Give the engine some time to process

            let (_, raw_report) = fifo_out_rx.pop().expect("No execution report generated");
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
            assert_eq!(order_qty_field.value, field_str(&order_event.quantity.to_fix_bytes()));
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
                orig_cl_ord_id: None,
                order_id: types::OrderId::from_ascii("ORDERID2"),
                side: types::Side::Sell,
                price: types::FixedPointArithmetic(12_345_600_000), // 123.456 in FIX price format (8 decimal places)
                quantity: types::FixedPointArithmetic(1_000_000),
                sender_id: types::EntityId::from_ascii("SENDER2"),
                target_id: types::EntityId::from_ascii("TARGET2"),
                symbol: types::FixedString::from_ascii("TEST_SYMBOL"),
                timestamp: Instant::now(), // Current timestamp in milliseconds since epoch
            };

            let mut sell_order_result = types::OrderResult {
                trades: Trades::default(),
                status: types::OrderStatus::New,
                timestamp: Instant::now(), // Current timestamp in milliseconds since epoch
            };

            sell_order_result.trades.add_trade(types::Trade {
                price: types::FixedPointArithmetic(12_345_600_000), // 123.456 in FIX price format (8 decimal places)
                quantity: types::FixedPointArithmetic(5_000_000_000), // 50 units in FIX quantity format (6 decimal places)
                id: types::TradeId::default()   ,
                timestamp: Instant::now(), // Current timestamp in milliseconds since epoch
            }).expect("Failed to add trade to OrderResult");
            
            match fifo_in_tx.push((sell_order_event, sell_order_result)) {
                Ok(_) => {},
                Err(e) => panic!("Failed to push sell order event into engine: {:?}", e),
            }

            std::thread::sleep(std::time::Duration::from_millis(100)); // Give the engine some time to process

            // First pop will be the execution report for the new sell order, which we can ignore for this test since we're focused on validating the execution report generated for the buy order when the trade occurs.
            let (_, new_report) = fifo_out_rx.pop().expect("No execution report generated for sell order");
            let mut fix_parser = fix::parser::FixParser::new(&new_report.data[..new_report.len as usize]);
            let parsed_report = fix_parser.get_fields();
            assert_eq!(parsed_report.fields.iter().find(|f| f.tag == tags::MSG_TYPE).expect("MSG_TYPE field missing").value, msg_types::EXECUTION_REPORT);
            assert_eq!(parsed_report.fields.iter().find(|f| f.tag == tags::ORD_STATUS).expect("ORD_STATUS field missing").value, ord_status_code_set::NEW);
            assert_eq!(parsed_report.fields.iter().find(|f| f.tag == tags::EXEC_TYPE).expect("EXEC_TYPE field missing").value, exec_type_code_set::NEW);
            assert_eq!(parsed_report.fields.iter().find(|f| f.tag == tags::CL_ORD_ID).expect("CL_ORD_ID field missing").value, field_str(&sell_order_event.cl_ord_id.as_ref()));
            assert_eq!(parsed_report.fields.iter().find(|f| f.tag == tags::ORDER_ID).expect("ORDER_ID field missing").value, field_str(&sell_order_event.order_id.as_ref()));
            assert_eq!(parsed_report.fields.iter().find(|f| f.tag == tags::SIDE).expect("SIDE field missing").value, side_code_set::SELL);
            assert_eq!(parsed_report.fields.iter().find(|f| f.tag == tags::SYMBOL).expect("SYMBOL field missing").value, field_str(&sell_order_event.symbol.as_ref()));
            assert_eq!(parsed_report.fields.iter().find(|f| f.tag == tags::ORDER_QTY).expect("ORDER_QTY field missing").value, field_str(&sell_order_event.quantity.to_fix_bytes()));
            assert_eq!(parsed_report.fields.iter().find(|f| f.tag == tags::LAST_QTY).expect("LAST_QTY field missing").value, field_str(&FixedPointArithmetic::ZERO.to_fix_bytes())); // No trades executed for the sell order, so LAST_QTY should be 0
            assert_eq!(parsed_report.fields.iter().find(|f| f.tag == tags::LAST_PX).expect("LAST_PX field missing").value, field_str(&FixedPointArithmetic::ZERO.to_fix_bytes())); // No trades executed for the sell order, so LAST_PX should be 0
            assert_eq!(parsed_report.fields.iter().find(|f| f.tag == tags::AVG_PX).expect("AVG_PX field missing").value, field_str(&FixedPointArithmetic::ZERO.to_fix_bytes())); // No trades executed for the sell order, so AVG_PX should be 0

            // Pop the next report, which should be the execution report for the buy order with the trade details populated
            let (_, raw_report) = fifo_out_rx.pop().expect("No execution report generated for sell order");
            let mut fix_parser = fix::parser::FixParser::new(&raw_report.data[..raw_report.len as usize]);
            let parsed_report = fix_parser.get_fields();  

            let last_qty_field = parsed_report.fields.iter().find(|f| f.tag == tags::LAST_QTY).expect("LAST_QTY field missing");
            assert_eq!(last_qty_field.value, field_str(&FixedPointArithmetic::from_f64(50.0).to_fix_bytes())); // 50 units filled
            let last_px_field = parsed_report.fields.iter().find(|f| f.tag == tags::LAST_PX).expect("LAST_PX field missing");
            assert_eq!(last_px_field.value, field_str(&FixedPointArithmetic::from_f64(123.456).to_fix_bytes())); // 123.456 price
            let avg_px_field = parsed_report.fields.iter().find(|f| f.tag == tags::AVG_PX).expect("AVG_PX field missing");
            assert_eq!(avg_px_field.value, field_str(&FixedPointArithmetic::from_f64(123.456).to_fix_bytes())); // 123.456 price, since only one trade executed

            // Testing the Cancel scenario
            let cancel_order_result = types::OrderResult {
                trades: Trades::default(),
                status: types::OrderStatus::Cancelled,
                timestamp: Instant::now(), // Current timestamp in milliseconds since epoch
            };
    
            match fifo_in_tx.push((order_event, cancel_order_result)) {
                Ok(_) => {},
                Err(e) => panic!("Failed to push cancel order event into engine: {:?}", e),
            }

            std::thread::sleep(std::time::Duration::from_millis(100)); // Give the engine some time to process
            let (_, raw_report) = fifo_out_rx.try_pop().expect("No execution report generated for cancel order");
            let mut fix_parser = fix::parser::FixParser::new(&raw_report.data[..raw_report.len as usize]);
            let parsed_report = fix_parser.get_fields();
            let msg_type_field = parsed_report.fields.iter().find(|f| f.tag == tags::MSG_TYPE).expect("MSG_TYPE field missing");
            assert_eq!(msg_type_field.value, msg_types::EXECUTION_REPORT);
            let ord_status_field = parsed_report.fields.iter().find(|f| f.tag == tags::ORD_STATUS).expect("ORD_STATUS field missing");
            assert_eq!(ord_status_field.value, ord_status_code_set::CANCELED);
            let exec_type_field = parsed_report.fields.iter().find(|f| f.tag == tags::EXEC_TYPE).expect("EXEC_TYPE field missing");
            assert_eq!(exec_type_field.value, exec_type_code_set::CANCELED);

            // testing the cancel rejection scenario
            let cancel_reject_order_result = types::OrderResult {
                trades: Trades::default(),
                status: types::OrderStatus::CancelRejected,
                timestamp: Instant::now(), // Current timestamp in milliseconds since epoch
            };

            match fifo_in_tx.push((order_event, cancel_reject_order_result)) {
                Ok(_) => {},
                Err(e) => panic!("Failed to push cancel reject order event into engine: {:?}", e),
            }

            std::thread::sleep(std::time::Duration::from_millis(100)); // Give the engine some time to process
            let (_, raw_report) = fifo_out_rx.pop().expect("No execution report generated for cancel reject order");
            let mut fix_parser = fix::parser::FixParser::new(&raw_report.data[..raw_report.len as usize]);
            let parsed_report = fix_parser.get_fields();
            let msg_type_field = parsed_report.fields.iter().find(|f| f.tag == tags::MSG_TYPE).expect("MSG_TYPE field missing");
            assert_eq!(msg_type_field.value, msg_types::ORDER_CANCEL_REJECTION);
            let ord_status_field = parsed_report.fields.iter().find(|f| f.tag == tags::ORD_STATUS).expect("ORD_STATUS field missing");
            assert_eq!(ord_status_field.value, ord_status_code_set::NEW);
    
            // Stop the engine
            kill_execution_report_engine(&fifo_in_tx);
            let _ = fifo_out_rx.pop(); // Purge the dummy message pushed to unblock the engine
    
            handle.join().unwrap();
        });
    }
}