use socket2::{Socket, Domain, Type, Protocol};
use std::net::{UdpSocket, Ipv4Addr};

/// Multicast endpoint description used by the market-feed and snapshot engines.
#[derive(Clone)]
pub struct MulticastSource {
    pub market: String,
    /// The IP address of the multicast group to join for receiving market data
    pub ip: String,
    /// The port number to bind to for receiving market data
    pub port: u16,
    /// The multicast group address to join for receiving market data
    pub address: String,
}

impl MulticastSource {
    pub fn new(ip: String, port: u16, market: &str) -> Self {
        let addr = format!("{}:{}", ip, port);
        Self {
            market: market.to_string(),
            ip,
            port,
            address: addr,
        }
    }
}

pub struct SourceSocket {
    pub source: MulticastSource,
    pub socket: std::net::UdpSocket,
}

impl SourceSocket {

    pub fn new(ip: String, port: u16, market: &str) -> std::io::Result<Self> {
        let source = MulticastSource::new(ip, port, market);
        let socket = Self::create_multicast_sender_socket()?;
        Ok(Self { source, socket })
    }

    pub fn create_multicast_sender_socket() -> std::io::Result<UdpSocket> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        let bind_addr = std::net::SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
        socket.bind(&bind_addr.into())?;
        let socket: UdpSocket = socket.into();
        socket.set_multicast_loop_v4(true)?;
        socket.set_multicast_ttl_v4(1)?;
        Ok(socket)
    }

    pub fn create_multicast_receiver_socket(port: u16) -> std::io::Result<UdpSocket> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        socket.set_reuse_address(true)?;

        let bind_addr = std::net::SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port);
        socket.bind(&bind_addr.into())?;
        Ok(socket.into())
    }
}