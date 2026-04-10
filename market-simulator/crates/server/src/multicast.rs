use std::net::{Ipv4Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use utils::market_name;

use market_feed::types::{
    AddOrder, DeleteOrder, MarketDataHeader, MessageType, ModifyOrder, OrderBookSnapshot,
    SNAPSHOT_BYTES, Trade,
};
use web::state::{EventBus, WsEvent};

use types::multicast::{MulticastSource, SourceSocket};


fn parse_market_data_message(packet: &[u8], market: &str) -> Option<(String, String)> {
    if packet.len() < 24 {
        return None;
    }

    let header = MarketDataHeader::from_bytes(&packet[0..24])?;
    let body = &packet[24..];

    let header_seq_num = header.seq_num;
    let header_timestamp_ns = header.timestamp_ns;
    let header_symbol_id = header.symbol.to_numeric() as u32; // Assuming symbol_id is in the high 32 bits of a u64
    let header_version = header.version;
    let header_length = header.length;
    // symbol_id is the first 4 ASCII bytes of the symbol, packed into the high 32 bits
    // of a u64 then cast to u32 — decode back to a string here.
    let symbol = {
        let bytes = header_symbol_id.to_be_bytes();
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(4);
        String::from_utf8(bytes[..end].to_vec())
            .unwrap_or_else(|_| header_symbol_id.to_string())
    };

    let label = match header.msg_type {
        x if x == MessageType::AddOrder as u8 => "◀ MD ADD",
        x if x == MessageType::ModifyOrder as u8 => "◀ MD MODIFY",
        x if x == MessageType::DeleteOrder as u8 => "◀ MD DELETE",
        x if x == MessageType::Trade as u8 => "◀ MD TRADE",
        x if x == MessageType::Snapshot as u8 => "◀ MD SNAPSHOT",
        _ => "◀ MD UNKNOWN",
    }
    .to_string();

    let details = match header.msg_type {
        x if x == MessageType::AddOrder as u8 => {
            AddOrder::from_bytes(body).map(|msg| {
                let order_id = msg.order_id;
                let side = msg.side;
                let price = msg.price;
                let quantity = msg.quantity;
                format!(
                    "35=8 │ 39=0 │ 11={} │ 54={} │ 44={} │ 38={} │ 151={} │ 55={} │ 9001={} │ 9002={} │ 9003={} │ 9004={} │ 9005={}",
                    order_id,
                    side,
                    price.to_f64(),
                    quantity.to_f64(),
                    quantity.to_f64(),
                    symbol,
                    market,
                    header_seq_num,
                    header_timestamp_ns,
                    header_version,
                    header_length,
                )
            })
        }
        x if x == MessageType::ModifyOrder as u8 => {
            ModifyOrder::from_bytes(body).map(|msg| {
                let order_id = msg.order_id;
                let new_price = msg.new_price;
                let new_quantity = msg.new_quantity;
                format!(
                    "35=8 │ 39=1 │ 11={} │ 44={} │ 151={} │ 38={} │ 55={} │ 9001={} │ 9002={} │ 9003={} │ 9004={} │ 9005={}",
                    order_id,
                    new_price.to_f64(),
                    new_quantity.to_f64(),
                    new_quantity.to_f64(),
                    symbol,
                    market,
                    header_seq_num,
                    header_timestamp_ns,
                    header_version,
                    header_length,
                )
            })
        }
        x if x == MessageType::DeleteOrder as u8 => {
            DeleteOrder::from_bytes(body).map(|msg| {
                let order_id = msg.order_id;
                format!(
                    "35=8 │ 39=4 │ 11={} │ 55={} │ 9001={} │ 9002={} │ 9003={} │ 9004={} │ 9005={}",
                    order_id,
                    symbol,
                    market,
                    header_seq_num,
                    header_timestamp_ns,
                    header_version,
                    header_length,
                )
            })
        }
        x if x == MessageType::Trade as u8 => Trade::from_bytes(body).map(|msg| {
            let trade_id = msg.trade_id;
            let side = msg.side;
            let price = msg.price;
            let quantity = msg.quantity;
            format!(
                "35=8 │ 39=2 │ 11=T{} │ 17={} │ 54={} │ 31={} │ 32={} │ 38={} │ 151=0 │ 55={} │ 9001={} │ 9002={} │ 9003={} │ 9004={} │ 9005={}",
                trade_id,
                trade_id,
                side,
                price.to_f64(),
                quantity.to_f64(),
                quantity.to_f64(),
                symbol,
                market,
                header_seq_num,
                header_timestamp_ns,
                header_version,
                header_length,
            )
        }),
        x if x == MessageType::Snapshot as u8 => {
            OrderBookSnapshot::from_bytes(&body[..body.len().min(SNAPSHOT_BYTES)]).map(|msg| {
                format!(
                    "35=8 │ 39=3 │ 9000=SNAPSHOT │ 55={} │ 268={} │ 269={} │ 9001={} │ 9002={} │ 9003={} │ 9004={} │ 9005={}",
                    symbol,
                    msg.num_bid_levels,
                    msg.num_ask_levels,
                    market,
                    header_seq_num,
                    header_timestamp_ns,
                    header_version,
                    header_length,
                )
            })
        }
        _ => Some("unsupported payload".to_string()),
    }?;

    Some((label, details))
}

pub fn spawn_market_feed_receiver(
    bus: EventBus,
    sources: Vec<MulticastSource>,
    shutdown: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut sockets = Vec::<SourceSocket>::new();

        for source in sources {
            let bind_addr = format!("0.0.0.0:{}", source.port);
            let socket = match SourceSocket::create_multicast_socket(source.port) {
                Ok(socket) => socket,
                Err(e) => {
                    tracing::warn!(
                        "[{}] Market-feed multicast bind failed for {} ({}): {}",
                        market_name(),
                        source.market,
                        bind_addr,
                        e
                    );
                    continue;
                }
            };

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
        while !shutdown.load(Ordering::Relaxed) {
            for source_socket in &sockets {
                match source_socket.socket.recv_from(&mut buf) {
                    Ok((n, _)) => {
                        if let Some((label, body)) =
                            parse_market_data_message(&buf[..n], &source_socket.source.market)
                        {
                            bus.publish(WsEvent::FixMessage {
                                label: format!("{} [{}]", label, source_socket.source.market),
                                body,
                                tag: "md".into(),
                                recipient: None,
                            });
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