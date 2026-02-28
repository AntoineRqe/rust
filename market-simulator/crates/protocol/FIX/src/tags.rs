/// Pre-defined FIX tags we care about on the hot path.
/// Using constants rather than an enum avoids match overhead.
pub mod tags {
    pub const MSG_TYPE: u32 = 35;
    pub const SENDER_COMP_ID: u32 = 49;
    pub const TARGET_COMP_ID: u32 = 56;
    pub const MSG_SEQ_NUM: u32 = 34;
    pub const SENDING_TIME: u32 = 52;
    pub const ORDER_ID: u32 = 37; // Unique order ID assigned by the exchange, for the sell side to track orders
    pub const CL_ORD_ID: u32 = 11;  // Client order ID, unique per order, for the buy side to track their orders
    pub const EXEC_ID: u32 = 17;
    pub const EXEC_TYPE: u32 = 150;
    pub const ORD_STATUS: u32 = 39;
    pub const SYMBOL: u32 = 55;
    pub const SIDE: u32 = 54;
    pub const ORDER_QTY: u32 = 38;
    pub const PRICE: u32 = 44;
    pub const LAST_QTY: u32 = 32;
    pub const LAST_PX: u32 = 31;
    pub const CUM_QTY: u32 = 14;
    pub const LEAVES_QTY: u32 = 151;
    pub const HEARTBEAT_INT: u32 = 108;
    pub const TEST_REQ_ID: u32 = 112;
    pub const BEGIN_SEQ_NO: u32 = 7;
    pub const END_SEQ_NO: u32 = 16;
    pub const CHECKSUM: u32 = 10;
}

/// Pre-defined FIX message types we care about on the hot path.
pub mod msg_types {
    pub const NEW_ORDER_SINGLE: &[u8] = b"D";
    pub const EXECUTION_REPORT: &[u8] = b"8";
    pub const ORDER_CANCEL_REQUEST: &[u8] = b"F";
    pub const ORDER_CANCEL_REPLACE_REQUEST: &[u8] = b"G";
}