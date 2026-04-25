use std::net::{Ipv4Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use utils::market_name;

use market_feed::types::{
    AddOrder, DeleteOrder, MarketDataHeader, MessageType, ModifyOrder, OrderBookSnapshot,
    SNAPSHOT_BYTES, Trade, MARKET_DATA_HEADER_SIZE,
};
use web::state::{EventBus, WsEvent, PriceLevel, OrderBookState};
use web::players::PlayerStore;

use types::multicast::{MulticastSource, SourceSocket};

enum ParsedMarketDataKind {
    Add {
        order_id: u64,
        side: u8,
        price: f64,
        quantity: f64,
    },
    Modify {
        order_id: u64,
        new_price: f64,
        new_quantity: f64,
    },
    Delete {
        order_id: u64,
    },
    Trade {
        trade_id: u64,
        side: u8,
        price: f64,
        quantity: f64,
        passive_cl_ord_id: String,
    },
    Snapshot {
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
    },
    Unknown,
}

struct ParsedMarketDataPacket {
    label: String,
    details: String,
    symbol: String,
    timestamp_ms: u64,
    kind: ParsedMarketDataKind,
}

fn symbol_for_book(header: &MarketDataHeader) -> String {
    header.symbol.to_string()
}

/// Parses and decodes a raw market-data packet once, returning data reused by
/// both FIX-message logging and order-book updates.
fn parse_market_data_message(packet: &[u8], market: &str) -> Option<ParsedMarketDataPacket> {
    if packet.len() < MARKET_DATA_HEADER_SIZE {
        return None;
    }

    let header = MarketDataHeader::from_bytes(&packet[0..MARKET_DATA_HEADER_SIZE])?;
    let body = &packet[MARKET_DATA_HEADER_SIZE..];

    let header_seq_num = header.seq_num;
    let header_timestamp_ns = header.timestamp_ns;
    let header_version = header.version;
    let header_length = header.length;

    let symbol_for_log = header.symbol.to_string();
    let symbol = symbol_for_book(&header);
    let timestamp_ms = (header_timestamp_ns / 1_000_000) as u64;

    // Determine the message type and parse the body accordingly, while also constructing a human-readable label and details string for logging and client updates.
    let label = match header.msg_type {
        x if x == MessageType::AddOrder as u8 => "◀ MD ADD",
        x if x == MessageType::ModifyOrder as u8 => "◀ MD MODIFY",
        x if x == MessageType::DeleteOrder as u8 => "◀ MD DELETE",
        x if x == MessageType::Trade as u8 => "◀ MD TRADE",
        x if x == MessageType::Snapshot as u8 => "◀ MD SNAPSHOT",
        _ => "◀ MD UNKNOWN",
    }
    .to_string();

    let (details, kind) = match header.msg_type {
        x if x == MessageType::AddOrder as u8 => {
            let msg = AddOrder::from_bytes(body)?;
            let order_id = msg.order_id;
            let side = msg.side;
            let price = msg.price.to_f64();
            let quantity = msg.quantity.to_f64();
            (
                format!(
                    "35=8 │ 39=0 │ 11={} │ 54={} │ 44={} │ 38={} │ 151={} │ 55={} │ 9001={} │ 9002={} │ 9003={} │ 9004={} │ 9005={}",
                    order_id,
                    side,
                    price,
                    quantity,
                    quantity,
                    symbol_for_log,
                    market,
                    header_seq_num,
                    header_timestamp_ns,
                    header_version,
                    header_length,
                ),
                ParsedMarketDataKind::Add {
                    order_id,
                    side,
                    price,
                    quantity,
                },
            )
        }
        x if x == MessageType::ModifyOrder as u8 => {
            let msg = ModifyOrder::from_bytes(body)?;
            let order_id = msg.order_id;
            let new_price = msg.new_price.to_f64();
            let new_quantity = msg.new_quantity.to_f64();
            (
                format!(
                    "35=8 │ 39=1 │ 11={} │ 44={} │ 151={} │ 38={} │ 55={} │ 9001={} │ 9002={} │ 9003={} │ 9004={} │ 9005={}",
                    order_id,
                    new_price,
                    new_quantity,
                    new_quantity,
                    symbol_for_log,
                    market,
                    header_seq_num,
                    header_timestamp_ns,
                    header_version,
                    header_length,
                ),
                ParsedMarketDataKind::Modify {
                    order_id,
                    new_price,
                    new_quantity,
                },
            )
        }
        x if x == MessageType::DeleteOrder as u8 => {
            let msg = DeleteOrder::from_bytes(body)?;
            let order_id = msg.order_id;
            (
                format!(
                    "35=8 │ 39=4 │ 11={} │ 55={} │ 9001={} │ 9002={} │ 9003={} │ 9004={} │ 9005={}",
                    order_id,
                    symbol_for_log,
                    market,
                    header_seq_num,
                    header_timestamp_ns,
                    header_version,
                    header_length,
                ),
                ParsedMarketDataKind::Delete { order_id },
            )
        }
        x if x == MessageType::Trade as u8 => {
            let msg = Trade::from_bytes(body)?;
            let trade_id = msg.trade_id;
            let side = msg.side;
            let price = msg.price.to_f64();
            let quantity = msg.quantity.to_f64();
            let passive_cl_ord_id = msg.passive_cl_ord_id.to_string();
            (
                format!(
                    "35=8 │ 39=2 │ 11=T{} │ 17={} │ 41={} │ 54={} │ 31={} │ 32={} │ 38={} │ 151=0 │ 55={} │ 9001={} │ 9002={} │ 9003={} │ 9004={} │ 9005={}",
                    trade_id,
                    trade_id,
                    passive_cl_ord_id,
                    side,
                    price,
                    quantity,
                    quantity,
                    symbol_for_log,
                    market,
                    header_seq_num,
                    header_timestamp_ns,
                    header_version,
                    header_length,
                ),
                ParsedMarketDataKind::Trade {
                    trade_id,
                    side,
                    price,
                    quantity,
                    passive_cl_ord_id,
                },
            )
        }
        x if x == MessageType::Snapshot as u8 => {
            let msg = OrderBookSnapshot::from_bytes(&body[..body.len().min(SNAPSHOT_BYTES)])?;
            let bids: Vec<PriceLevel> = msg.bids[..msg.num_bid_levels as usize]
                .iter()
                .map(|l| PriceLevel {
                    price: l.price.to_f64(),
                    quantity: l.quantity.to_f64(),
                })
                .collect();
            let asks: Vec<PriceLevel> = msg.asks[..msg.num_ask_levels as usize]
                .iter()
                .map(|l| PriceLevel {
                    price: l.price.to_f64(),
                    quantity: l.quantity.to_f64(),
                })
                .collect();
            (
                format!(
                    "35=8 │ 39=3 │ 9000=SNAPSHOT │ 55={} │ 268={} │ 269={} │ 9001={} │ 9002={} │ 9003={} │ 9004={} │ 9005={}",
                    symbol_for_log,
                    msg.num_bid_levels,
                    msg.num_ask_levels,
                    market,
                    header_seq_num,
                    header_timestamp_ns,
                    header_version,
                    header_length,
                ),
                ParsedMarketDataKind::Snapshot { bids, asks },
            )
        }
        _ => (
            "unsupported payload".to_string(),
            ParsedMarketDataKind::Unknown,
        ),
    };

    Some(ParsedMarketDataPacket {
        label,
        details,
        symbol,
        timestamp_ms,
        kind,
    })
}

/// Spawns a thread to receive market data from multicast sources, parse the messages, update the order book state, and publish events to the WebSocket clients.
/// Arguments:
/// - `bus`: The event bus used to publish events to WebSocket clients.
/// - `sources`: A list of multicast sources to subscribe to for receiving market data.
/// - `shutdown`: An atomic boolean used to signal the thread to shut down gracefully.
/// - `order_book`: A shared, thread-safe reference to the order book state that will be updated with incoming market data messages.
/// Returns: A `JoinHandle` for the spawned thread, which can be used to wait for the thread to finish when shutting down the server.
pub fn spawn_market_feed_receiver(
    bus: EventBus,
    sources: Vec<MulticastSource>,
    shutdown: Arc<AtomicBool>,
    order_book: Arc<std::sync::Mutex<OrderBookState>>,
    player_store: PlayerStore,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut sockets = Vec::<SourceSocket>::new();

        for source in sources {
            // Attempt to bind to the port to check if it's available
            let socket = match SourceSocket::create_multicast_receiver_socket(source.port) {
                Ok(socket) => socket,
                Err(e) => {
                    tracing::warn!(
                        "[{}] Market-feed multicast bind failed for {} ({}): {}",
                        market_name(),
                        source.market,
                        format!("0.0.0.0:{}", source.port),
                        e
                    );
                    continue;
                }
            };

            // Attempt to join the multicast group
            let group = match source.ip.parse::<Ipv4Addr>() {
                Ok(group) => group,
                Err(e) => {
                    tracing::warn!(
                        "[{}] Invalid multicast address for {} ({}): {}",
                        market_name(),
                        source.market,
                        source.ip,
                        e
                    );
                    continue;
                }
            };

            if let Err(e) = socket.join_multicast_v4(&group, &Ipv4Addr::UNSPECIFIED) {
                tracing::warn!(
                    "[{}] Failed to join multicast group {}:{} for {}: {}",
                    market_name(),
                    source.ip,
                    source.port,
                    source.market,
                    e
                );
                continue;
            }

            // Set a read timeout to allow the thread to check for shutdown signals periodically
            let _ = socket.set_read_timeout(Some(Duration::from_millis(100)));

            tracing::info!(
                "[{}] Subscribed to market-feed multicast {}:{} ({})",
                market_name(),
                source.ip,
                source.port,
                source.market
            );

            sockets.push(SourceSocket { source, socket });
        }

        let mut buf = [0u8; 2048];

        // Main loop to receive and process market data messages until a shutdown signal is received.
        // Robin round between multiple multicast sources to ensure we process messages from all subscribed markets in a timely manner.
        while !shutdown.load(Ordering::Relaxed) {
            for source_socket in &sockets {
                match source_socket.socket.recv_from(&mut buf) {
                    Ok((n, _)) => {
                        if let Some(parsed) =
                            parse_market_data_message(&buf[..n], &source_socket.source.market)
                        {
                            // Publish the parsed FIX message to WebSocket clients for logging and updates.
                            bus.publish(WsEvent::FixMessage {
                                label: format!("{} [{}]", parsed.label, source_socket.source.market),
                                body: parsed.details,
                                tag: "md".into(),
                                recipient: None,
                            });

                            // TODO  : Handle snapshots separately to avoid blocking the order book updates with potentially large snapshot messages, and to ensure we apply snapshots atomically to avoid inconsistent state during snapshot application.
                            // In HFT scenarios, snapshots can be large and frequent, so it's important to handle them efficiently.
                            let mut passive_trade_update: Option<(u64, String, String, f64, f64)> = None;
                            if let Ok(mut ob) = order_book.lock() {
                                let book = ob.get_or_create(&parsed.symbol);
                                match parsed.kind {
                                    ParsedMarketDataKind::Add {
                                        order_id,
                                        side,
                                        price,
                                        quantity,
                                    } => {
                                        book.add_or_update_order(
                                            order_id,
                                            side,
                                            price,
                                            quantity,
                                            None,
                                            parsed.timestamp_ms,
                                        );
                                    }
                                    ParsedMarketDataKind::Modify {
                                        order_id,
                                        new_price,
                                        new_quantity,
                                    } => {
                                        book.modify_order(
                                            order_id,
                                            new_price,
                                            new_quantity,
                                            parsed.timestamp_ms,
                                        );
                                    }
                                    ParsedMarketDataKind::Delete { order_id } => {
                                        book.delete_order(order_id, parsed.timestamp_ms);
                                    }
                                    ParsedMarketDataKind::Trade {
                                        trade_id,
                                        side,
                                        price,
                                        quantity,
                                        passive_cl_ord_id,
                                    } => {
                                        book.apply_trade(
                                            side,
                                            price,
                                            quantity,
                                            parsed.timestamp_ms,
                                        );
                                        book.record_trade(
                                            side,
                                            price,
                                            quantity,
                                            parsed.timestamp_ms,
                                        );
                                        if !passive_cl_ord_id.is_empty() {
                                            passive_trade_update = Some((
                                                trade_id,
                                                passive_cl_ord_id,
                                                parsed.symbol.clone(),
                                                quantity,
                                                price,
                                            ));
                                        }
                                    }
                                    ParsedMarketDataKind::Snapshot { bids, asks } => {
                                        book.apply_snapshot(bids, asks, parsed.timestamp_ms);
                                    }
                                    ParsedMarketDataKind::Unknown => {}
                                }

                                bus.publish(WsEvent::OrderBook {
                                    symbol: parsed.symbol.clone(),
                                    bids: book.l3_bids_sorted(),
                                    asks: book.l3_asks_sorted(),
                                    timestamp_ms: parsed.timestamp_ms,
                                });
                            }

                            if let Some((trade_id, passive_cl_ord_id, symbol, quantity, price)) = passive_trade_update {
                                player_store.apply_trade_from_feed(
                                    trade_id,
                                    &passive_cl_ord_id,
                                    &symbol,
                                    quantity,
                                    price,
                                );
                            }
                        }
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) => {}
                    Err(e) => {
                        tracing::debug!(
                            "[{}] Multicast recv error for {}: {}",
                            market_name(),
                            source_socket.source.market,
                            e
                        );
                    }
                }
            }
        }

        tracing::info!("[{}] Market-feed multicast receiver stopped", market_name());
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use types::FixedPointArithmetic;
    use types::macros::{SymbolId, OrderId};

    use market_feed::types::{
        MAX_LEVELS,
        PriceLevel as MdPriceLevel,
    };

    fn build_packet(msg_type: MessageType, symbol: SymbolId, seq_num: u64, timestamp_ns: u64, body: &[u8]) -> Vec<u8> {
        let header = MarketDataHeader {
            msg_type: msg_type as u8,
            version: 1,
            seq_num,
            timestamp_ns,
            symbol,
            length: (MARKET_DATA_HEADER_SIZE + body.len()) as u16,
        };

        let mut packet = Vec::with_capacity(MARKET_DATA_HEADER_SIZE + body.len());
        packet.extend_from_slice(&header.to_bytes());
        packet.extend_from_slice(body);
        packet
    }

    #[test]
    fn test_parse_market_data_message() {
        let market = "XPAR";
        let symbol = SymbolId::from_ascii("ABCD");

        // --- AddOrder ---
        let add = AddOrder {
            order_id: 42,
            side: 1,
            price: FixedPointArithmetic::from_f64(101.25),
            quantity: FixedPointArithmetic::from_f64(7.5),
        };
        let add_packet = build_packet(
            MessageType::AddOrder,
            symbol,
            1001,
            2_000_000,
            &add.to_bytes(),
        );
        let add_parsed = parse_market_data_message(&add_packet, market).expect("add packet should parse");
        assert_eq!(add_parsed.label, "◀ MD ADD");
        assert_eq!(add_parsed.symbol, "ABCD");
        assert_eq!(add_parsed.timestamp_ms, 2);
        assert!(add_parsed.details.contains("35=8"));
        assert!(add_parsed.details.contains("39=0"));
        assert!(add_parsed.details.contains("11=42"));
        assert!(add_parsed.details.contains("55=ABCD"));
        assert!(add_parsed.details.contains("9001=XPAR"));
        match add_parsed.kind {
            ParsedMarketDataKind::Add {
                order_id,
                side,
                price,
                quantity,
            } => {
                assert_eq!(order_id, 42);
                assert_eq!(side, 1);
                assert_eq!(price, 101.25);
                assert_eq!(quantity, 7.5);
            }
            _ => panic!("expected ParsedMarketDataKind::Add"),
        }

        // --- Trade ---
        let trade = Trade {
            trade_id: 777,
            side: 2,
            price: FixedPointArithmetic::from_f64(100.5),
            quantity: FixedPointArithmetic::from_f64(3.0),
            aggressive_cl_ord_id: OrderId::from_ascii("ORD-A"),
            passive_cl_ord_id: OrderId::from_ascii("ORD-B"),
        };
        let trade_packet = build_packet(
            MessageType::Trade,
            symbol,
            1002,
            5_500_000,
            &trade.to_bytes(),
        );
        let trade_parsed = parse_market_data_message(&trade_packet, market).expect("trade packet should parse");
        assert_eq!(trade_parsed.label, "◀ MD TRADE");
        assert_eq!(trade_parsed.symbol, "ABCD");
        assert_eq!(trade_parsed.timestamp_ms, 5);
        assert!(trade_parsed.details.contains("39=2"));
        assert!(trade_parsed.details.contains("17=777"));
        assert!(trade_parsed.details.contains("41=ORD-B"));
        match trade_parsed.kind {
            ParsedMarketDataKind::Trade {
                trade_id,
                side,
                price,
                quantity,
                passive_cl_ord_id,
            } => {
                assert_eq!(trade_id, 777);
                assert_eq!(side, 2);
                assert_eq!(price, 100.5);
                assert_eq!(quantity, 3.0);
                assert_eq!(passive_cl_ord_id, "ORD-B");
            }
            _ => panic!("expected ParsedMarketDataKind::Trade"),
        }

        // --- Snapshot ---
        let level = MdPriceLevel {
            price: FixedPointArithmetic::from_f64(99.0),
            quantity: FixedPointArithmetic::from_f64(12.0),
        };
        let snapshot = OrderBookSnapshot {
            num_bid_levels: 1,
            num_ask_levels: 1,
            bids: [level; MAX_LEVELS],
            asks: [level; MAX_LEVELS],
        };
        let snapshot_packet = build_packet(
            MessageType::Snapshot,
            symbol,
            1003,
            7_000_000,
            &snapshot.to_bytes(),
        );
        let snapshot_parsed = parse_market_data_message(&snapshot_packet, market)
            .expect("snapshot packet should parse");
        assert_eq!(snapshot_parsed.label, "◀ MD SNAPSHOT");
        assert_eq!(snapshot_parsed.symbol, "ABCD");
        assert_eq!(snapshot_parsed.timestamp_ms, 7);
        assert!(snapshot_parsed.details.contains("9000=SNAPSHOT"));
        assert!(snapshot_parsed.details.contains("268=1"));
        assert!(snapshot_parsed.details.contains("269=1"));
        match snapshot_parsed.kind {
            ParsedMarketDataKind::Snapshot { bids, asks } => {
                assert_eq!(bids.len(), 1);
                assert_eq!(asks.len(), 1);
                assert_eq!(bids[0].price, 99.0);
                assert_eq!(bids[0].quantity, 12.0);
                assert_eq!(asks[0].price, 99.0);
                assert_eq!(asks[0].quantity, 12.0);
            }
            _ => panic!("expected ParsedMarketDataKind::Snapshot"),
        }
    }

    #[test]
    fn test_parse_market_data_message_modify_and_delete() {
        let market = "XPAR";
        let symbol = SymbolId::from_ascii("ABCD");

        // --- ModifyOrder ---
        let modify = ModifyOrder {
            order_id: 99,
            new_price: FixedPointArithmetic::from_f64(102.75),
            new_quantity: FixedPointArithmetic::from_f64(4.25),
        };
        let modify_packet = build_packet(
            MessageType::ModifyOrder,
            symbol,
            2001,
            10_000_000,
            &modify.to_bytes(),
        );
        let modify_parsed = parse_market_data_message(&modify_packet, market)
            .expect("modify packet should parse");
        assert_eq!(modify_parsed.label, "◀ MD MODIFY");
        assert_eq!(modify_parsed.symbol, "ABCD");
        assert_eq!(modify_parsed.timestamp_ms, 10);
        assert!(modify_parsed.details.contains("39=1"));
        assert!(modify_parsed.details.contains("11=99"));
        match modify_parsed.kind {
            ParsedMarketDataKind::Modify {
                order_id,
                new_price,
                new_quantity,
            } => {
                assert_eq!(order_id, 99);
                assert_eq!(new_price, 102.75);
                assert_eq!(new_quantity, 4.25);
            }
            _ => panic!("expected ParsedMarketDataKind::Modify"),
        }

        // --- DeleteOrder ---
        let delete = DeleteOrder { order_id: 1234 };
        let delete_packet = build_packet(
            MessageType::DeleteOrder,
            symbol,
            2002,
            11_000_000,
            &delete.to_bytes(),
        );
        let delete_parsed = parse_market_data_message(&delete_packet, market)
            .expect("delete packet should parse");
        assert_eq!(delete_parsed.label, "◀ MD DELETE");
        assert_eq!(delete_parsed.symbol, "ABCD");
        assert_eq!(delete_parsed.timestamp_ms, 11);
        assert!(delete_parsed.details.contains("39=4"));
        assert!(delete_parsed.details.contains("11=1234"));
        match delete_parsed.kind {
            ParsedMarketDataKind::Delete { order_id } => {
                assert_eq!(order_id, 1234);
            }
            _ => panic!("expected ParsedMarketDataKind::Delete"),
        }
    }

    #[test]
    fn test_parse_market_data_message_invalid_packets() {
        let market = "XPAR";
        let symbol = SymbolId::from_ascii("ABCD");

        // Too short to contain a full header.
        assert!(parse_market_data_message(&[0u8; MARKET_DATA_HEADER_SIZE - 1], market).is_none());

        // Header is present but body is too short for AddOrder payload.
        let short_body_packet = build_packet(
            MessageType::AddOrder,
            symbol,
            3001,
            12_000_000,
            &[0u8; 8],
        );
        assert!(parse_market_data_message(&short_body_packet, market).is_none());
    }
}