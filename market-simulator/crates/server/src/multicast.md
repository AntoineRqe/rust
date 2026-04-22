# Multicast 

The `multicast` module implements the logic for broadcasting market data updates to clients using UDP multicast. It listens for messages from the FIX engine, processes them, and sends out market data snapshots and trade updates to subscribed clients. The module also handles client subscriptions and manages the multicast socket.

## Architecture

```
INGEST FLOW (UDP MULTICAST)
Market Feed Sources        Multicast Receiver Thread              Parser
 (ip:port per market)                |                              |
  |                                  |                              |
  | -- UDP packet -----------------> | -- parse_market_data_message -> |

EVENT FLOW (BROWSER LOG STREAM)
Parser                      EventBus (broadcast)                WebSocket clients
  |                                 |                                 |
  | -- WsEvent::FixMessage -------> | ------------------------------> |

BOOK FLOW (LIVE ORDER BOOK VIEW)
Parser + header/body decode      Shared OrderBookState (Mutex)      EventBus            WebSocket clients
  |                               |                                     |                      |
  | -- add/modify/delete/trade -->|                                     |                      |
  | -- snapshot apply ----------->|                                     |                      |
  |                               | -- WsEvent::OrderBook ------------> | -------------------> |
```

Transport / state primitives:
- Multicast ingress: `UdpSocket` per source (`SourceSocket`), joined via `join_multicast_v4`
- Polling model: single receiver thread, round-robin over subscribed sockets
- Backpressure behavior: socket read timeout (`100ms`) and best-effort processing (drops only at UDP/network layer)
- Browser fan-out: `EventBus` publish/subscribe (`WsEvent::FixMessage`, `WsEvent::OrderBook`)
- In-memory book state: shared `OrderBookState` protected by `Mutex`

# Registration and Subscription

How multicast registration actually works

When a host wants to receive a multicast stream:

1. App joins multicast group (example 239.1.1.1)
2. OS sends IGMP Membership Report (IPv4)
3. Local router learns there is a listener on that LAN
4. Router updates multicast routing tree upstream

This only ensures the host receives the multicast packets sent to that group. The app still needs to bind a socket to the appropriate port and join the group to receive the data.

# Limitations and Future Improvements

Multicast is only available on the same local network as the server, which may limit its usefulness for clients connecting from different locations. Future improvements could include implementing a more robust and scalable messaging system, such as using a message broker or a publish-subscribe pattern over TCP, to allow for wider distribution of market data updates.

However, for clients on the same local network, multicast can provide a low-latency and efficient way to receive market data updates without the overhead of establishing individual TCP connections for each client.