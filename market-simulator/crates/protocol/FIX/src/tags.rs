/// Pre-defined FIX tags we care about on the hot path.
/// Using constants rather than an enum avoids match overhead.
pub mod tags {
    pub const BEGIN_STRING: u32 = 8;
    pub const BODY_LENGTH: u32 = 9;
    pub const MSG_TYPE: u32 = 35;
    pub const SENDER_COMP_ID: u32 = 49;
    pub const TARGET_COMP_ID: u32 = 56;
    pub const MSG_SEQ_NUM: u32 = 34;
    pub const SENDING_TIME: u32 = 52;
    pub const ORDER_ID: u32 = 37; // Unique order ID assigned by the exchange, for the sell side to track orders
    pub const CL_ORD_ID: u32 = 11;  // Client order ID, unique per order, for the buy side to track their orders
    pub const ORIG_CL_ORD_ID: u32 = 41; // Original client order ID, used in cancel/replace requests to reference the original order
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
    pub const AVG_PX: u32 = 6;
    pub const CHECK_SUM: u32 = 10;
}

/// Pre-defined FIX message types we care about on the hot path.
pub mod msg_types {
    pub const NEW_ORDER_SINGLE: &[u8] = b"D";
    pub const EXECUTION_REPORT: &[u8] = b"8";
    pub const ORDER_CANCEL_REQUEST: &[u8] = b"F";
    pub const ORDER_CANCEL_REPLACE_REQUEST: &[u8] = b"G";
    pub const HEARTBEAT: &[u8] = b"0";
    pub const TEST_REQUEST: &[u8] = b"1";
    pub const RESEND_REQUEST: &[u8] = b"2";
    pub const REJECT: &[u8] = b"3";
    pub const SEQUENCE_RESET: &[u8] = b"4";
    pub const LOGOUT: &[u8] = b"5";
    pub const LOGON: &[u8] = b"A";
    pub const ORDER_CANCEL_REJECTION: &[u8] = b"9";
}
/// Pre-defined FIX side types.
pub mod side_code_set {
    pub const BUY: &[u8] = b"1";
    pub const SELL: &[u8] = b"2";
    pub const BUY_MINUS: &[u8] = b"3";
    pub const SELL_PLUS: &[u8] = b"4";
    pub const SELL_SHORT: &[u8] = b"5";
    pub const SELL_SHORT_EXEMPT: &[u8] = b"6";
    pub const UNDISCLOSED: &[u8] = b"7";
    pub const CROSS: &[u8] = b"8";
    pub const CROSS_SHORT: &[u8] = b"9";
    pub const CROSS_SHORT_EXEMPT: &[u8] = b"A";
    pub const AS_DEFINED: &[u8] = b"B";
    pub const OPPOSITE: &[u8] = b"C";
    pub const SUBSCRIBE: &[u8] = b"D";
    pub const REDEEM: &[u8] = b"E";
    pub const LEND: &[u8] = b"F";
    pub const BORROW: &[u8] = b"G";
    pub const SELL_UNDISCLOSED: &[u8] = b"H";
}

pub mod ord_status_code_set {
    pub const NEW: &[u8] = b"0";
    pub const PARTIAL_FILL: &[u8] = b"1";
    pub const FILL: &[u8] = b"2";
    pub const DONE_FOR_DAY: &[u8] = b"3";
    pub const CANCELED: &[u8] = b"4";
    pub const REPLACE: &[u8] = b"5";
    pub const PENDING_CANCEL: &[u8] = b"6";
    pub const STOPPED: &[u8] = b"7";
    pub const REJECTED: &[u8] = b"8";
    pub const SUSPENDED: &[u8] = b"9";
    pub const PENDING_NEW: &[u8] = b"A";
    pub const CALCULATED: &[u8] = b"B";
    pub const EXPIRED: &[u8] = b"C";
    pub const ACCEPTED_FOR_BIDDING: &[u8] = b"D";
    pub const PENDING_REPLACE: &[u8] = b"E";
}

pub mod exec_type_code_set {
    pub const NEW: &[u8] = b"0";
    pub const DONE_FOR_DAY: &[u8] = b"3";
    pub const CANCELED: &[u8] = b"4";
    pub const REPLACE: &[u8] = b"5";
    pub const PENDING_CANCEL: &[u8] = b"6";
    pub const STOPPED: &[u8] = b"7";
    pub const REJECTED: &[u8] = b"8";
    pub const SUSPENDED: &[u8] = b"9";
    pub const PENDING_NEW: &[u8] = b"A";
    pub const CALCULATED: &[u8] = b"B";
    pub const EXPIRED: &[u8] = b"C";
    pub const RESTATED: &[u8] = b"D";
    pub const PENDING_REPLACE: &[u8] = b"E";
    pub const TRADE: &[u8] = b"F";
    pub const TRADE_CORRECT: &[u8] = b"G";
    pub const TRADE_CANCEL: &[u8] = b"H";
    pub const ORDER_STATUS: &[u8] = b"I";
    pub const TRADE_IN_A_CLEANING_HOLD: &[u8] = b"J";
    pub const TRADE_HAS_BEEN_RELEASED_TO_CLEARING: &[u8] = b"K";
    pub const TRIGGERED_OR_ACTIVATED_BY_SYSTEM: &[u8] = b"L";
    pub const LOCKED: &[u8] = b"M";
    pub const RELEASED: &[u8] = b"N";
}