use crate::types::Snapshot;

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
    bytes.extend_from_slice(&snapshot.timestamp.to_be_bytes());
    bytes.extend_from_slice(&(snapshot.symbol.len() as u32).to_be_bytes());
    bytes.extend_from_slice(snapshot.symbol.as_bytes());
    bytes.extend_from_slice(&(snapshot.id as u32).to_be_bytes());
    bytes.extend_from_slice(&(snapshot.order_book.bids.len() as u32).to_be_bytes());

    // Encode bids
    for bid in &snapshot.order_book.bids {
        bytes.extend_from_slice(&bid.price.to_fix_bytes());
        bytes.extend_from_slice(&bid.quantity.to_fix_bytes());
        bytes.push(0); // Side: 0 for bid
    }

    bytes.extend_from_slice(&(snapshot.order_book.asks.len() as u32).to_be_bytes());

    // Encode asks
    for ask in &snapshot.order_book.asks {
        bytes.extend_from_slice(&ask.price.to_fix_bytes());
        bytes.extend_from_slice(&ask.quantity.to_fix_bytes());
        bytes.push(1); // Side: 1 for ask
    }

    bytes
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::types::OrderBookSnapshot;
    use types::{OrderEvent, FixedPointArithmetic, Side, OrderType};
    use types::macros::{EntityId, OrderId, SymbolId};

    #[test]
    fn test_encode_snapshot() {
        let snapshot = Snapshot {
            timestamp: 1627846267000,
            symbol: "TEST".to_string(),
            id: 1,
            order_book: OrderBookSnapshot {
                bids: vec![
                    OrderEvent {
                        price: FixedPointArithmetic(12345678900),
                        quantity: FixedPointArithmetic(100000000),
                        side: Side::Buy,
                        order_type: OrderType::LimitOrder,
                        cl_ord_id: OrderId::from_ascii("bid1"),
                        orig_cl_ord_id: None,
                        sender_id: EntityId::from_ascii("trader1"),
                        target_id: EntityId::from_ascii("exchange"),
                        symbol: SymbolId::from_ascii("TEST"),
                        timestamp: 1627846267000,
                    },
                ],
                asks: vec![
                    OrderEvent {
                        price: FixedPointArithmetic(12345679000),
                        quantity: FixedPointArithmetic(200000000),
                        side: Side::Sell,
                        order_type: OrderType::LimitOrder,
                        cl_ord_id: OrderId::from_ascii("ask1"),
                        orig_cl_ord_id: None,
                        sender_id: EntityId::from_ascii("trader2"),
                        target_id: EntityId::from_ascii("exchange"),
                        symbol: SymbolId::from_ascii("TEST"),
                        timestamp: 1627846267000,
                    },
                ],
            },
        };

        let encoded = encode_snapshot(&snapshot);
        assert!(!encoded.is_empty());
        // Additional assertions can be added here to verify the correctness of the encoding
    }
}