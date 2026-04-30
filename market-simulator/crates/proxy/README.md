# Proxy crate

This crate implements a proxy server that listens to market data and snapshot feeds from multicast sources, and forwards them to connected clients over TCP using an Axum-based HTTP server. The proxy is designed to run concurrently with the multicast listeners, allowing it to serve client requests while continuously receiving and forwarding market data.

## Key Features

- Listens to market data and snapshot feeds from specified multicast sources.
- Forwards received data to connected clients over TCP.
- Uses Tokio for asynchronous runtime and task management.
- Implements graceful shutdown of the server and listeners.
- Core affinity for multicast listener tasks to optimize performance.
- Error handling and logging for better observability.

## Architecture

The proxy server consists of the following main components:

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
```

## Limitations and Future Improvements

For now, Tokio proxy runtime only has 1 worker thread, which is pinned to a specific core using core affinity. This is sufficient for our current use case, but in the future we may want to consider increasing the worker thread count and implementing a more sophisticated task scheduling strategy to further optimize performance and reduce latency.