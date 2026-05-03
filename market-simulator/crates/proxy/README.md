# Proxy Crate

The `proxy` crate acts as a bridge between the market data multicast feeds and WebSocket clients. It receives market and snapshot feeds via UDP multicast, then forwards them to connected clients over WebSocket endpoints using an Axum/Tokio server.

**Key Features:**

- Listens to market data and snapshot multicast feeds (UDP).
- Forwards received data to all connected WebSocket clients in real time.
- Provides two WebSocket endpoints:
    - `/ws/market` for market data feed
    - `/ws/snapshot` for snapshot feed
- Graceful shutdown and CPU pinning support for low-latency operation.

**Architecture Workflow:**

```
    +-------------------+         +-------------------+         +-------------------+
    |                   |         |                   |         |                   |
    |  Market Data Feed |         |   Proxy (Axum)    |         |   WebSocket       |
    |  Multicast Source | ======> |  Multicast Socket | ======> |   Clients         |
    |                   |         |   Listener(s)     |         |                   |
    +-------------------+         |                   |         +-------------------+
                                                                |   Snapshot Socket |
    +-------------------+         |   Listener(s)     |
    |                   |         |                   |
    | Snapshot Feed     | ======> |                   |
    | Multicast Source  |         |                   |
    +-------------------+         +-------------------+

Legend:
======>  UDP Multicast
----->  Internal async channel (broadcast)
=====>  WebSocket (Axum endpoint)
```

**Endpoints:**

- `ws://<proxy_ip>:<proxy_port>/ws/market` — Market data stream
- `ws://<proxy_ip>:<proxy_port>/ws/snapshot` — Snapshot stream

**How it works:**

1. The proxy joins the configured multicast groups for market and snapshot feeds.
2. Each feed is received on a UDP socket and forwarded to a Tokio broadcast channel.
3. Axum WebSocket handlers subscribe to these channels and push data to all connected clients.
4. The proxy can be pinned to a specific CPU core for low-latency, high-performance operation.

**Configuration:**

- The proxy’s IP, port, and multicast group addresses are set in the main config file (`crates/config/default.json`).
- The proxy can be started as part of the simulator or standalone for market data distribution.

**Example Python WebSocket client:**

```python
import asyncio
import websockets

async def main():
        uri = "ws://127.0.0.1:9889/ws/snapshot"
        async with websockets.connect(uri) as websocket:
                async for message in websocket:
                        print(f"Received: {len(message)} bytes")

asyncio.run(main())
```

**See also:**
- [crates/proxy/README.md](crates/proxy/README.md) for implementation details.
