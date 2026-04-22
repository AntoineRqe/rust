
use crate::types::Snapshot;
use types::OrderEvent;
use types::FixedPointArithmetic;
use types::macros::{EntityId, OrderId, SymbolId};
use types::Side;
use types::OrderType;

// Encoding format for snapshots
//
// The encoding format is designed to be compact and efficient for transmission over the network.
// It consists of a header followed by a sequence of order entries.
// Header format:
// - 8 bytes: timestamp (u64)
// - 4 bytes: symbol length (u32)
// - N bytes: symbol (UTF-8 string)
// - 4 bytes: snapshot ID (u32)
// - 4 bytes: number of bids (u32)
// - List of bid entries (see below)
// - 4 bytes: number of asks (u32)
// - List of ask entries (see below)
// Order entry format (for both bids and asks):
// - 20 bytes: price (custom 20-byte fixed-point format, e.g. 12 bytes for integer part, 8 bytes for fractional part)
// - 20 bytes: quantity (custom 20-byte fixed-point format, e.g. 12 bytes for integer part, 8 bytes for fractional part)
// - 1 byte: side (0 for bid, 1 for ask)

pub fn encode_snapshot(snapshot: &Snapshot) -> Vec<u8> {
    let mut bytes = Vec::new();

    // Encode header
    bytes.extend_from_slice(&snapshot.timestamp_ms.to_be_bytes());
    bytes.extend_from_slice(&(snapshot.symbol.len() as u32).to_be_bytes());
    bytes.extend_from_slice(snapshot.symbol.as_bytes());
    bytes.extend_from_slice(&(snapshot.id as u32).to_be_bytes());
    bytes.extend_from_slice(&(snapshot.order_book.bids_len as u32).to_be_bytes());

    // Encode bids
    for i in 0..snapshot.order_book.bids_len {
        let bid = &snapshot.order_book.bids[i];
        bytes.extend_from_slice(&bid.price.to_fix_bytes());
        bytes.extend_from_slice(&bid.quantity.to_fix_bytes());
        bytes.push(0); // Side: 0 for bid
    }

    bytes.extend_from_slice(&(snapshot.order_book.asks_len as u32).to_be_bytes());

    // Encode asks
    for i in 0..snapshot.order_book.asks_len {
        let ask = &snapshot.order_book.asks[i];
        bytes.extend_from_slice(&ask.price.to_fix_bytes());
        bytes.extend_from_slice(&ask.quantity.to_fix_bytes());
        bytes.push(1); // Side: 1 for ask
    }

    bytes
}

pub fn decode_snapshot(bytes: &[u8]) -> Snapshot {
    // Decoding logic would go here, but is not implemented in this example
    let mut snapshot = Snapshot::default();
    let mut offset = 0;
    // Parse header and order entries from bytes to populate the snapshot struct
    snapshot.timestamp_ms = u64::from_be_bytes(bytes[offset..offset + 8].try_into().unwrap());
    offset += 8;
    let symbol_len = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
    offset += 4;
    snapshot.symbol = String::from_utf8(bytes[offset..offset + symbol_len].to_vec()).unwrap();
    offset += symbol_len;
    snapshot.id = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as u64;
    offset += 4;


    let bids_len = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
    offset += 4;

    for _ in 0..bids_len {

        let price = FixedPointArithmetic::from_fix_bytes(&bytes[offset..offset + 20]).unwrap();
        offset += 20;
        let quantity = FixedPointArithmetic::from_fix_bytes(&bytes[offset..offset + 20]).unwrap();
        offset += 20;

        let _ = snapshot.order_book.add_bid(OrderEvent {
            price,
            quantity,
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            cl_ord_id: OrderId::default(),
            orig_cl_ord_id: None,
            sender_id: EntityId::default(),
            target_id: EntityId::default(),
            symbol: SymbolId::default(),
            timestamp_ms: snapshot.timestamp_ms,
        });
        let _side = bytes[offset];
        offset += 1;
    }

    let asks_len = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
    offset += 4;

    for _ in 0..asks_len {
        let price = FixedPointArithmetic::from_fix_bytes(&bytes[offset..offset + 20]).unwrap();
        offset += 20;
        let quantity = FixedPointArithmetic::from_fix_bytes(&bytes[offset..offset + 20]).unwrap();
        offset += 20;
        
        let _ = snapshot.order_book.add_ask(OrderEvent {
            price,
            quantity,
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            cl_ord_id: OrderId::default(),
            orig_cl_ord_id: None,
            sender_id: EntityId::default(),
            target_id: EntityId::default(),
            symbol: SymbolId::default(),
            timestamp_ms: snapshot.timestamp_ms,
        });
        let _side = bytes[offset];
        offset += 1;
    }

    snapshot
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::types::OrderBookSnapshot;
    use types::{OrderEvent, FixedPointArithmetic, Side, OrderType};
    use types::macros::{EntityId, OrderId, SymbolId};

    #[test]
    fn test_encode_snapshot() {
        let order1 = OrderEvent {
            price: FixedPointArithmetic::from_f64(100.0),
            quantity: FixedPointArithmetic::from_f64(10.0),
            side: Side::Buy,
            order_type: OrderType::LimitOrder,
            cl_ord_id: OrderId::from_ascii("bid1"),
            orig_cl_ord_id: None,
            sender_id: EntityId::from_ascii("trader1"),
            target_id: EntityId::from_ascii("exchange"),
            symbol: SymbolId::from_ascii("TEST"),
            timestamp_ms: 1627846267000,
        };
        let order2 = OrderEvent {
            price: FixedPointArithmetic::from_f64(102.0),
            quantity: FixedPointArithmetic::from_f64(5.0),
            side: Side::Sell,
            order_type: OrderType::LimitOrder,
            cl_ord_id: OrderId::from_ascii("ask1"),
            orig_cl_ord_id: None,
            sender_id: EntityId::from_ascii("trader2"),
            target_id: EntityId::from_ascii("exchange"),
            symbol: SymbolId::from_ascii("TEST"),
            timestamp_ms: 1627846267000,
        };

        let mut snapshot = Snapshot {
            timestamp_ms: 1627846267000,
            symbol: "TEST".to_string(),
            id: 1,
            order_book: OrderBookSnapshot::default(),
        };

        let _ = snapshot.order_book.add_bid(order1);
        let _ = snapshot.order_book.add_ask(order2);

        let encoded = encode_snapshot(&snapshot);
        assert!(!encoded.is_empty());

        // Decode the snapshot and verify it matches the original
        let decoded = decode_snapshot(&encoded);
        assert_eq!(decoded.timestamp_ms, snapshot.timestamp_ms);
        assert_eq!(decoded.symbol, snapshot.symbol);
        assert_eq!(decoded.id, snapshot.id);
        assert_eq!(decoded.order_book.bids_len, snapshot.order_book.bids_len);
        assert_eq!(decoded.order_book.asks_len, snapshot.order_book.asks_len);
        assert_eq!(decoded.order_book.bids[0].price, snapshot.order_book.bids[0].price);
        assert_eq!(decoded.order_book.bids[0].quantity, snapshot.order_book.bids[0].quantity);
        assert_eq!(decoded.order_book.asks[0].price, snapshot.order_book.asks[0].price);
        assert_eq!(decoded.order_book.asks[0].quantity, snapshot.order_book.asks[0].quantity);
    }
}