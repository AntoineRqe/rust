use types::{FixedPointArithmetic, macros::SymbolId};

pub const MARKET_DATA_HEADER_SIZE: usize = 24;

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct MarketDataHeader {
    pub msg_type: u8,        // MessageType
    pub version: u8,
    pub seq_num: u64,
    pub timestamp_ns: u64,
    pub symbol: SymbolId,     // null-terminated ASCII symbol
    pub length: u16,         // total message length
}

impl Default for MarketDataHeader {
    fn default() -> Self {
        Self {
            length: 0,
            msg_type: 0,
            version: 1,
            seq_num: 0,
            timestamp_ns: 0,
            symbol: SymbolId::default(),
        }
    }
}

impl MarketDataHeader {
    pub fn to_bytes(&self) -> [u8; 24] {
        let mut buf = [0u8; 24];

        buf[0..2].copy_from_slice(&self.length.to_be_bytes());
        buf[2] = self.msg_type;
        buf[3] = self.version;
        buf[4..12].copy_from_slice(&self.seq_num.to_be_bytes());
        buf[12..20].copy_from_slice(&self.timestamp_ns.to_be_bytes());
        buf[20..24].copy_from_slice(&self.symbol.0);

        buf
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 24 {
            return None;
        }

        Some(Self {
            length: u16::from_be_bytes(bytes[0..2].try_into().ok()?),
            msg_type: bytes[2],
            version: bytes[3],
            seq_num: u64::from_be_bytes(bytes[4..12].try_into().ok()?),
            timestamp_ns: u64::from_be_bytes(bytes[12..20].try_into().ok()?),
            symbol: SymbolId::from_bytes(&bytes[20..24])?,
        })
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
    Add(MarketDataHeader, AddOrder),
    Modify(MarketDataHeader, ModifyOrder),
    Delete(MarketDataHeader, DeleteOrder),
    Trade(MarketDataHeader, Trade),
    Snapshot(MarketDataHeader, OrderBookSnapshot),
}

fn stable_u64_from_fixed_20(bytes: &[u8; 20]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

impl std::fmt::Display for MarketEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MarketEvent::Add(_header, _add_order) => write!(f, "MarketEvent::Add"),
            MarketEvent::Modify(_header, _modify_order) => write!(f, "MarketEvent::Modify"),
            MarketEvent::Delete(_header, _delete_order) => write!(f, "MarketEvent::Delete"),
            MarketEvent::Trade(_header, _trade) => write!(f, "MarketEvent::Trade"),
            MarketEvent::Snapshot(_header, _snapshot) => write!(f, "MarketEvent::Snapshot"),
        }
    }
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
                bytes.extend_from_slice(&delete_order.to_bytes());
                bytes
            },
            MarketEvent::Trade(header, trade) => {
                let mut bytes = Vec::with_capacity(24 + 49);
                bytes.extend_from_slice(&header.to_bytes());
                bytes.extend_from_slice(&trade.to_bytes());
                bytes
            },
            MarketEvent::Snapshot(header, snapshot) => {
                let mut bytes = Vec::with_capacity(24 + SNAPSHOT_BYTES);
                bytes.extend_from_slice(&header.to_bytes());
                bytes.extend_from_slice(&snapshot.to_bytes());
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
pub const PRICE_LEVEL_BYTES: usize = 40;
pub const SNAPSHOT_BYTES: usize = 2 + (MAX_LEVELS * PRICE_LEVEL_BYTES * 2);

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
            order_id: stable_u64_from_fixed_20(&order_event.cl_ord_id.0),
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

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 48 {
            return None;
        }

        fn trim_nul(bytes: &[u8]) -> &[u8] {
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            &bytes[..end]
        }

        Some(Self {
            order_id: u64::from_be_bytes(bytes[0..8].try_into().ok()?),
            new_price: FixedPointArithmetic::from_fix_bytes(trim_nul(&bytes[8..28]))?,
            new_quantity: FixedPointArithmetic::from_fix_bytes(trim_nul(&bytes[28..48]))?,
        })
    }

    pub fn from_order_event(order_event: &types::OrderEvent) -> Self {
        Self {
            order_id: stable_u64_from_fixed_20(&order_event.cl_ord_id.0),
            new_price: order_event.price,
            new_quantity: order_event.quantity,
        }
    }
}

// ------------------ Delete Order ------------------
impl DeleteOrder {
    pub fn to_bytes(&self) -> [u8; 8] {
        self.order_id.to_be_bytes()
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 8 {
            return None;
        }

        Some(Self {
            order_id: u64::from_be_bytes(bytes[0..8].try_into().ok()?),
        })
    }

    pub fn from_order_event(order_event: &types::OrderEvent) -> Self {
        let order_id = order_event
            .orig_cl_ord_id
            .map(|id| stable_u64_from_fixed_20(&id.0))
            .unwrap_or_else(|| stable_u64_from_fixed_20(&order_event.cl_ord_id.0));

        Self { order_id }
    }
}

// ------------------ Trade ------------------
impl Trade {
    pub fn to_bytes(&self) -> [u8; 49] {
        let mut buf = [0u8; 49];
        buf[0..8].copy_from_slice(&self.trade_id.to_be_bytes());
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
            trade_id: u64::from_be_bytes(bytes[0..8].try_into().ok()?),
            side: bytes[8],
            price: FixedPointArithmetic::from_fix_bytes(trim_nul(&bytes[9..29]))?,
            quantity: FixedPointArithmetic::from_fix_bytes(trim_nul(&bytes[29..49]))?,
        })
    }

    pub fn from_trade(side: types::Side, trade: &types::Trade) -> Self {
        Self {
            trade_id: trade.id,
            side: match side {
                types::Side::Buy => 1,
                types::Side::Sell => 2,
            },
            price: trade.price,
            quantity: trade.quantity,
        }
    }
}

// ------------------ Snapshot ------------------
impl PriceLevel {
    pub fn to_bytes(&self) -> [u8; PRICE_LEVEL_BYTES] {
        let mut buf = [0u8; PRICE_LEVEL_BYTES];
        buf[0..20].copy_from_slice(&self.price.to_fix_bytes());
        buf[20..40].copy_from_slice(&self.quantity.to_fix_bytes());
        buf
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < PRICE_LEVEL_BYTES {
            return None;
        }

        fn trim_nul(bytes: &[u8]) -> &[u8] {
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            &bytes[..end]
        }

        Some(Self {
            price: FixedPointArithmetic::from_fix_bytes(trim_nul(&bytes[0..20]))?,
            quantity: FixedPointArithmetic::from_fix_bytes(trim_nul(&bytes[20..40]))?,
        })
    }
}

impl OrderBookSnapshot {
    pub fn to_bytes(&self) -> [u8; SNAPSHOT_BYTES] {
        let mut buf = [0u8; SNAPSHOT_BYTES];
        buf[0] = self.num_bid_levels;
        buf[1] = self.num_ask_levels;

        let mut offset = 2;
        for level in self.bids {
            let level_bytes = level.to_bytes();
            buf[offset..offset + PRICE_LEVEL_BYTES].copy_from_slice(&level_bytes);
            offset += PRICE_LEVEL_BYTES;
        }

        for level in self.asks {
            let level_bytes = level.to_bytes();
            buf[offset..offset + PRICE_LEVEL_BYTES].copy_from_slice(&level_bytes);
            offset += PRICE_LEVEL_BYTES;
        }

        buf
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < SNAPSHOT_BYTES {
            return None;
        }

        let mut offset = 2;
        let mut bids = [PriceLevel {
            price: FixedPointArithmetic::ZERO,
            quantity: FixedPointArithmetic::ZERO,
        }; MAX_LEVELS];
        let mut asks = [PriceLevel {
            price: FixedPointArithmetic::ZERO,
            quantity: FixedPointArithmetic::ZERO,
        }; MAX_LEVELS];

        for level in bids.iter_mut().take(MAX_LEVELS) {
            *level = PriceLevel::from_bytes(&bytes[offset..offset + PRICE_LEVEL_BYTES])?;
            offset += PRICE_LEVEL_BYTES;
        }

        for level in asks.iter_mut().take(MAX_LEVELS) {
            *level = PriceLevel::from_bytes(&bytes[offset..offset + PRICE_LEVEL_BYTES])?;
            offset += PRICE_LEVEL_BYTES;
        }

        Some(Self {
            num_bid_levels: bytes[0],
            num_ask_levels: bytes[1],
            bids,
            asks,
        })
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
            cl_ord_id: OrderId::from_ascii("order123"), // market-feed uses cl_ord_id as the order identifier
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


        assert_eq!(order_id, stable_u64_from_fixed_20(&order_event.cl_ord_id.0));
        assert_eq!(side, 1);
        assert_eq!(price.to_f64(), 123.45);
        assert_eq!(quantity.to_f64(), 1000.0);
    }

    #[test]
    fn test_modify_order_serialization() {
        let order = ModifyOrder {
            order_id: 123456789,
            new_price: FixedPointArithmetic::from_f64(124.45),
            new_quantity: FixedPointArithmetic::from_f64(900.0),
        };

        let bytes = order.to_bytes();
        let deserialized = ModifyOrder::from_bytes(&bytes).unwrap();

        let expected_order_id = order.order_id;
        let expected_new_price = order.new_price;
        let expected_new_quantity = order.new_quantity;

        let order_id = deserialized.order_id;
        let new_price = deserialized.new_price;
        let new_quantity = deserialized.new_quantity;

        assert_eq!(order_id, expected_order_id);
        assert_eq!(new_price, expected_new_price);
        assert_eq!(new_quantity, expected_new_quantity);
    }

    #[test]
    fn test_delete_order_serialization() {
        let order = DeleteOrder { order_id: 123456789 };
        let bytes = order.to_bytes();
        let deserialized = DeleteOrder::from_bytes(&bytes).unwrap();

        let expected_order_id = order.order_id;
        let order_id = deserialized.order_id;

        assert_eq!(order_id, expected_order_id);
    }

    #[test]
    fn test_trade_serialization() {
        let trade = Trade {
            trade_id: 0,
            side: 1,
            price: FixedPointArithmetic::from_f64(123.45),
            quantity: FixedPointArithmetic::from_f64(10.0),
        };

        let bytes = trade.to_bytes();
        let deserialized = Trade::from_bytes(&bytes).unwrap();

        let expected_trade_id = trade.trade_id;
        let expected_side = trade.side;
        let expected_price = trade.price;
        let expected_quantity = trade.quantity;

        let trade_id = deserialized.trade_id;
        let side = deserialized.side;
        let price = deserialized.price;
        let quantity = deserialized.quantity;

        assert_eq!(trade_id, expected_trade_id);
        assert_eq!(side, expected_side);
        assert_eq!(price, expected_price);
        assert_eq!(quantity, expected_quantity);
    }

    #[test]
    fn test_snapshot_serialization() {
        let level = PriceLevel {
            price: FixedPointArithmetic::from_f64(100.0),
            quantity: FixedPointArithmetic::from_f64(5.0),
        };

        let snapshot = OrderBookSnapshot {
            num_bid_levels: 1,
            num_ask_levels: 1,
            bids: [level; MAX_LEVELS],
            asks: [level; MAX_LEVELS],
        };

        let bytes = snapshot.to_bytes();
        let deserialized = OrderBookSnapshot::from_bytes(&bytes).unwrap();

        let expected_num_bid_levels = snapshot.num_bid_levels;
        let expected_num_ask_levels = snapshot.num_ask_levels;
        let expected_bid_price = snapshot.bids[0].price;
        let expected_bid_quantity = snapshot.bids[0].quantity;
        let expected_ask_price = snapshot.asks[0].price;
        let expected_ask_quantity = snapshot.asks[0].quantity;

        let num_bid_levels = deserialized.num_bid_levels;
        let num_ask_levels = deserialized.num_ask_levels;
        let bid_price = deserialized.bids[0].price;
        let bid_quantity = deserialized.bids[0].quantity;
        let ask_price = deserialized.asks[0].price;
        let ask_quantity = deserialized.asks[0].quantity;

        assert_eq!(num_bid_levels, expected_num_bid_levels);
        assert_eq!(num_ask_levels, expected_num_ask_levels);
        assert_eq!(bid_price, expected_bid_price);
        assert_eq!(bid_quantity, expected_bid_quantity);
        assert_eq!(ask_price, expected_ask_price);
        assert_eq!(ask_quantity, expected_ask_quantity);
    }
}