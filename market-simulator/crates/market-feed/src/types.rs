use types::FixedPointArithmetic;

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct MarketFeedHeader {
    pub length: u16,         // total message length
    pub msg_type: u8,        // MessageType
    pub version: u8,
    pub seq_num: u64,
    pub timestamp_ns: u64,
    pub instrument_id: u32,
}

impl Default for MarketFeedHeader {
    fn default() -> Self {
        Self {
            length: 0,
            msg_type: 0,
            version: 1,
            seq_num: 0,
            timestamp_ns: 0,
            instrument_id: 0,
        }
    }
}

impl MarketFeedHeader {
    pub fn to_bytes(&self) -> [u8; 24] {
        let mut buf = [0u8; 24];

        buf[0..2].copy_from_slice(&self.length.to_be_bytes());
        buf[2] = self.msg_type;
        buf[3] = self.version;
        buf[4..12].copy_from_slice(&self.seq_num.to_be_bytes());
        buf[12..20].copy_from_slice(&self.timestamp_ns.to_be_bytes());
        buf[20..24].copy_from_slice(&self.instrument_id.to_be_bytes());

        buf
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug)]
pub enum MessageType {
    AddOrder    = 1,
    ModifyOrder = 2,
    DeleteOrder = 3,
    Trade       = 4,
    Snapshot    = 5,
}

#[derive(Debug)]
pub enum MarketEvent {
    Add(MarketFeedHeader, AddOrder),
    Modify(MarketFeedHeader, ModifyOrder),
    Delete(MarketFeedHeader, DeleteOrder),
    Trade(MarketFeedHeader, Trade),
    Snapshot(MarketFeedHeader, OrderBookSnapshot),
}

impl MarketEvent {
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            MarketEvent::Add(header, add_order) => {
                let mut bytes = Vec::with_capacity(24 + 49);
                bytes.extend_from_slice(&header.to_bytes());
                bytes.extend_from_slice(&add_order.to_bytes());
                bytes
            },
            MarketEvent::Modify(header, modify_order) => {
                let mut bytes = Vec::with_capacity(24 + 48);
                bytes.extend_from_slice(&header.to_bytes());
                bytes.extend_from_slice(&modify_order.to_bytes());
                bytes
            },
            MarketEvent::Delete(header, delete_order) => {
                let mut bytes = Vec::with_capacity(24 + 8);
                bytes.extend_from_slice(&header.to_bytes());
                bytes.extend_from_slice(&delete_order.order_id.to_be_bytes());
                bytes
            },
            MarketEvent::Trade(header, trade) => {
                let mut bytes = Vec::with_capacity(24 + 49);
                bytes.extend_from_slice(&header.to_bytes());
                // Serialize trade similarly to AddOrder (not implemented here for brevity)
                // You would need to implement a to_bytes method for Trade as well
                bytes
            },
            MarketEvent::Snapshot(header, snapshot) => {
                let mut bytes = Vec::with_capacity(24 + std::mem::size_of::<OrderBookSnapshot>());
                bytes.extend_from_slice(&header.to_bytes());
                // Serialize snapshot (not implemented here for brevity)
                // You would need to implement a to_bytes method for OrderBookSnapshot as well
                bytes
            },
        }
    }
}

// ----------------- L3 order book event structure -----------------

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct AddOrder {
    pub order_id: u64,
    pub side: u8,       // 1 = bid, 2 = ask
    pub price: FixedPointArithmetic,
    pub quantity: FixedPointArithmetic,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct ModifyOrder {
    pub order_id: u64,
    pub new_price: FixedPointArithmetic,
    pub new_quantity: FixedPointArithmetic,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct DeleteOrder {
    pub order_id: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct Trade {
    pub trade_id: u64,
    pub side: u8,
    pub price: FixedPointArithmetic,
    pub quantity: FixedPointArithmetic,
}

// ----------------- L2 order book snapshot structure -----------------

pub const MAX_LEVELS: usize = 10;

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct PriceLevel {
    pub price: FixedPointArithmetic,
    pub quantity: FixedPointArithmetic,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct OrderBookSnapshot {
    pub num_bid_levels: u8,
    pub num_ask_levels: u8,
    pub bids: [PriceLevel; MAX_LEVELS],
    pub asks: [PriceLevel; MAX_LEVELS],
}

// ----------------- Add Order ------------------
impl AddOrder {
    pub fn to_bytes(&self) -> [u8; 49] {
        let mut buf = [0u8; 49];

        buf[0..8].copy_from_slice(&self.order_id.to_be_bytes());
        buf[8] = self.side;
        buf[9..29].copy_from_slice(&self.price.to_fix_bytes());
        buf[29..49].copy_from_slice(&self.quantity.to_fix_bytes());

        buf
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 49 {
            return None;
        }

        fn trim_nul(bytes: &[u8]) -> &[u8] {
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            &bytes[..end]
        }

        Some(Self {
            order_id: u64::from_be_bytes(bytes[0..8].try_into().ok()?),
            side: bytes[8],
            price: FixedPointArithmetic::from_fix_bytes(trim_nul(&bytes[9..29]))?,
            quantity: FixedPointArithmetic::from_fix_bytes(trim_nul(&bytes[29..49]))?,
        })
    }

    pub fn from_order_event(order_event: &types::OrderEvent) -> Self {
        Self {
            order_id: order_event.order_id.to_numeric().into(),
            side: match order_event.side {
                types::Side::Buy => 1,
                types::Side::Sell => 2,
            },
            price: order_event.price,
            quantity: order_event.quantity,
        }
    }
}

// ------------------ Modify Order ------------------
impl ModifyOrder {
    pub fn to_bytes(&self) -> [u8; 48] {
        let mut buf = [0u8; 48];
        buf[0..8].copy_from_slice(&self.order_id.to_be_bytes());
        buf[8..28].copy_from_slice(&self.new_price.to_fix_bytes());
        buf[28..48].copy_from_slice(&self.new_quantity.to_fix_bytes());

        buf
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FixedPointArithmetic;
    use types::macros::{OrderId};

    #[test]
    fn test_add_order_serialization() {
        let order = AddOrder {
            order_id: 123456789,
            side: 1,
            price: FixedPointArithmetic::from_f64(123.45),
            quantity: FixedPointArithmetic::from_f64(1000.0),
        };

        let bytes = order.to_bytes();
        let deserialized = AddOrder::from_bytes(&bytes).unwrap();

        // Copy packed fields to aligned locals before using assert_eq!/method calls
        let expected_order_id = order.order_id;
        let expected_side = order.side;
        let expected_price = order.price;
        let expected_quantity = order.quantity;

        let order_id = deserialized.order_id;
        let side = deserialized.side;
        let price = deserialized.price;
        let quantity = deserialized.quantity;

        assert_eq!(expected_order_id, order_id);
        assert_eq!(expected_side, side);
        assert_eq!(expected_price.to_f64(), price.to_f64());
        assert_eq!(expected_quantity.to_f64(), quantity.to_f64());
    }

    #[test]
    fn test_add_order_from_order_event() {

        let order_event = types::OrderEvent {
            order_id: OrderId::from_ascii("order123"),
            side: types::Side::Buy,
            price: FixedPointArithmetic::from_f64(123.45),
            quantity: FixedPointArithmetic::from_f64(1000.0),
            ..types::OrderEvent::default()
        };

        let add_order = AddOrder::from_order_event(&order_event);

        let order_id = add_order.order_id;
        let side = add_order.side;
        let price = add_order.price;
        let quantity = add_order.quantity;


        assert_eq!(order_id, 0x6f72646572313233); // "order123" in hex
        assert_eq!(side, 1);
        assert_eq!(price.to_f64(), 123.45);
        assert_eq!(quantity.to_f64(), 1000.0);
    }
}