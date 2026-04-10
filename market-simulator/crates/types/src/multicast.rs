/// Multicast endpoint description used by the market-feed and snapshot engines.
#[derive(Clone)]
pub struct MulticastSource {
    pub market: String,
    pub address: String,
    pub port: u16,
}

// Multicast configuration and socket management for market data feeds and snapshots.
pub struct MultiCastInfo {
    pub ip: String,
    pub port: u16,
    pub socket: std::net::UdpSocket,
    pub addr: String,
}

pub struct SourceSocket {
    pub source: MulticastSource,
    pub socket: std::net::UdpSocket,
}

impl MultiCastInfo {
    pub fn new(ip: &str, port: u16) -> Self {
        let socket = std::net::UdpSocket::bind("0.0.0.0:0").expect("Failed to bind UDP socket");
        let addr = format!("{}:{}", ip, port);
        Self {
            ip: ip.to_string(),
            port,
            socket,
            addr,
        }
    }
}