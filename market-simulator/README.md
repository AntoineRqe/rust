# Financial Market Simulator

This project is a financial market simulator implemented in Rust. It consists of several crates that work together to simulate a financial market, including an order book, a matching engine, and a market data feed.

For now, the simulator runs on a personal computer and is accessible from internet -> www.marketsim.site


## Project Status



- Active Rust-based market simulator for order entry, matching, execution reporting, and market data distribution.
- Currently runs two separate market processes: NASDAQ and NYSE.
- Each market currently trades one instrument: `AAPL`.
- Supports FIX connectivity, WebSocket/web access, PostgreSQL persistence, and UDP multicast distribution.
- Player Service (gRPC microservice on port 50052) handles all authentication and player state
- Cryptographic token generation using OsRng
- Trade execution with FIFO portfolio lot management
- Visitor counting across markets
- Admin commands: reset tokens, reset market state, reset FIX sequences
- Single gRPC connection pool with HTTP/2 multiplexing for all player service communication

## Features

### Connectivity

- FIX protocol support for order entry and execution reports.
- UDP multicast for market data distribution and snapshot.
- Web interface for monitoring and interaction.

### Persistence

- PostgreSQL database for order events, trades, and pending orders.


## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     CLIENT LAYER                                │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  FIX Clients                Browser (WebSocket)                 │
│       |                              |                          │
│       |                              |                          │
│       |  TCP:5000                    |  HTTP:9860               │
│       |                              |                          │
│       +-----────────────┬────────────┤                          │
│                         |            |                          │
└─────────────────────────┼────────────┼──────────────────────────┘
                          v            v
┌──────────────────────────────────────────────────────────────────┐
│              WEB SERVER (crates/web) - NASDAQ/NYSE               │
│                                                                  │
│  HTTP/WebSocket:                                                 │
│  ├─ POST /api/login (authenticate_or_register via gRPC)          │
│  └─ WebSocket /ws?token=X&username=alice&market=NASDAQ           │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │         gRPC PlayerClient (Connection Pool)                │  │
│  │  ╔════════════════════════════════════════════════════╗    │  │
│  │  ║ Single TCP/HTTP2 multiplexed connection            ║    │  │
│  │  ║ Handles: login, player state, trades, admin cmds   ║    │  │
│  │  ╚════════════════════════════════════════════════════╝    │  │
│  └────────────────────────────────────────────────────────────┘  │
│                         │                                        │
│                         │ gRPC (HTTP/2)                          │
│                         v                                        │
│                                                                  │
│  TCP Server (Port 5000):                                         │
│  ├─ Accepts FIX clients                                          │
│  └─ Routes to FIX Inbound Engine                                 │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
                          │
           gRPC           │  TCP (FIX)
    ┌──────────────┐      │
    │              │      │
    v              v      v
┌─────────────────────────────────────────────────────────────────┐
│          PLAYER SERVICE (crates/web/players)                    │
│                  (Port 50052 - gRPC)                            │
│                                                                 │
│  Core Responsibilities:                                         │
│  ├─ User authentication & token generation (cryptographic)      │
│  ├─ Player portfolio & balance management                       │
│  ├─ Trade execution & lot tracking (FIFO)                       │
│  ├─ Visitor counting & session tracking                         │
│  └─ Admin commands (reset tokens, reset market, reset seq)      │
│                                                                 │
│  Storage:                                                       │
│  └─ PostgreSQL: players, portfolio_lots, visit_counts           │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────────┐
│                   ORDER PROCESSING PIPELINE                      │
│                                                                  │
│  tcp_to_fix (tokio TCP)                                          │
│       │                                                          │
│       v                                                          │
│  ┌──────────────────────────────────────────────────────┐        │
│  │ FIX Inbound Engine (crates/protocol/FIX)             │        │
│  │ - Session handling, message parsing                  │        │
│  │ - FIX sequence tracking                              │        │
│  └──────────────────────────────────────────────────────┘        │
│       │ fix_to_ob (custom SPSC)                                  │
│       v                                                          │
│  ┌──────────────────────────────────────────────────────┐        │
│  │ Order Book Engine (crates/order-book)                │        │
│  │ - Matching & execution logic                         │        │
│  │ - Pending order tracking                             │        │
│  └──────────────────────────────────────────────────────┘        │
│       │                                                          │
│       ├─ ob_to_db ────────────────> DB Engine / PostgreSQL       │
│       ├─ ob_to_md ────────────────> Market Feed (UDP multicast)  │
│       ├─ ob_to_snapshot ─────────> Snapshot Engine (UDP)         │
│       │                                                          │
│       │ ob_to_er                                                 │
│       v                                                          │
│  ┌──────────────────────────────────────────────────────┐        │
│  │ Exec Report Engine (crates/execution-report)         │        │
│  │ - Builds execution reports                           │        │
│  │ - Routes to Player Service & FIX clients             │        │
│  └──────────────────────────────────────────────────────┘        │
│       │ er_to_fix (custom SPSC)                                  │
│       v                                                          │
│  ┌──────────────────────────────────────────────────────┐        │
│  │ FIX Outbound Engine (crates/protocol/FIX)            │        │
│  │ - FIX response formatting & sending                  │        │
│  └──────────────────────────────────────────────────────┘        │
│       │                                                          │
│       └─> TCP Server → FIX Clients                               │
│       └─> Web Server (crates/web) → WebSocket clients            │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘

ADMIN / CONTROL FLOW (gRPC from Web Server)
┌─────────────────────────────────────────────────────────────────┐
│ Player Service (gRPC)                                           │
│ ├─ reset_all_tokens() ─────────→ Reset all player balances      │
│ ├─ reset_market_state() ──────→ Clear pending orders & state    │
│ ├─ reset_seq() ────────────────→ Reset FIX sequence per player  │
│ └─ get_player_state() ────────→ Query player state & portfolio  │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

Technical choices and discussion about the architecture and design of the simulator can be found here [Architecture and Design](./papers/market-simulator.md).

## Workflow: Login → WebSocket Connection

The following diagram shows how a user logs in and establishes a WebSocket connection to a market:

```
BROWSER                          WEB SERVER                      PLAYER SERVICE
   │                                  │                                 │
   │  1. POST /api/login              │                                 │
   │     {username, password}         │                                 │
   ├─────────────────────────────────>│                                 │
   │                                  │  2. gRPC: authenticate_or_register
   │                                  ├────────────────────────────────>│
   │                                  │  3. Validate credentials        │
   │                                  │     Generate cryptographic token│
   │                                  │  4. gRPC Response               │
   │                                  │     {token, username, is_admin} │
   │                                  │<────────────────────────────────┤
   │  5. HTTP Response                │                                 │
   │     {token, username, is_admin}  │                                 │
   │<─────────────────────────────────┤                                 │
   │                                  │                                 │
   │  6. Store in sessionStorage:     │                                 │
   │     - auth_token                 │                                 │
   │     - username                   │                                 │
   │     - is_admin                   │                                 │
   │                                  │                                 │
   │  7. WebSocket: ws://market:9860/ │                                 │
   │     ws?token=X&username=alice    │                                 │
   │     &market=NASDAQ               │                                 │
   ├─────────────────────────────────>│                                 │
   │                                  │  8. gRPC: Implicit token        │
   │                                  │     validation on player methods│
   │                                  ├────────────────────────────────>│
   │                                  │  9. Player state loaded         │
   │                                  │<────────────────────────────────┤
   │  10. WebSocket Connected         │                                 │
   │      Ready for orders & updates  │                                 │
   │<─────────────────────────────────┤                                 │
   │                                  │                                 │
   │  11. Place Order                 │                                 │
   │      {symbol, qty, price, side}  │                                 │
   ├─────────────────────────────────>│                                 │
   │                                  │  12. gRPC: add_trade()          │
   │                                  ├────────────────────────────────>│
   │                                  │  13. Update portfolio & balance │
   │                                  │<────────────────────────────────┤
   │  14. Order Confirmation          │  15. Order → Order Book         │
   │      with token, lots, balance   │      → Exec Report → FIX Output │
   │<─────────────────────────────────┤                                 │
   │                                  │                                 │
```

**Key Points**:
- **Login (Steps 1-5)**: Browser sends credentials → Web Server calls Player Service via gRPC → Token returned to browser
- **Token Storage (Step 6)**: Token stored in sessionStorage (browser-only, not URL, prevents leaks)
- **WebSocket Connection (Steps 7-10)**: Browser opens WebSocket with token & username as query params → Web Server validates token with Player Service implicitly → Connection established
- **Trade Execution (Steps 11-15)**: WebSocket order → Web Server calls Player Service gRPC `add_trade()` → Portfolio updated → Order routed to Order Book → Execution report sent back
- **No Session Registry**: Player Service is stateless; each request validates the token implicitly
- **HTTP/2 Multiplexing**: All gRPC calls (login, add_trade, etc.) use single TCP connection with concurrent streams

## Crates

| Crate | What it does | Docs |
|---|---|---|
| `order-book` | Maintains the order book and matching logic (add/remove/match orders). | [Order Book](crates/order-book/README.md) |
| `fix-protocol` | Implements FIX parsing/session handling for inbound and outbound trading messages. | [FIX Protocol](crates/protocol/FIX/README.md) |
| `execution-report` | Builds execution reports from order events/results for client responses and analysis. | [Execution Report](crates/execution-report/README.md) |
| `server` | TCP entrypoint that receives FIX traffic and routes requests/responses through the engines. | [Server](crates/server/README.md) |
| `proxy` | Bridges market/snapshot UDP multicast feeds to WebSocket clients via Axum. | [Proxy](crates/proxy/README.md) |
| `logging` | Project-wide logging/observability for order processing and market events. | [Logging](crates/logging/README.md) |
| `types` | Shared domain types (orders, trades, market data, etc.) used across crates. | [Types](crates/types/README.md) |
| `utils` | Shared utility helpers (timestamps, fixed-point arithmetic, traits/functions). | [Utils](crates/utils/README.md) |
| `memory` | In-memory components for low-latency order book/matching workflows. | [Memory](crates/memory/README.md) |
| `web` | Web interface layer (WebSocket/API) for interacting with the simulator. | [Web Client](crates/web/README.md) |
| `db` | PostgreSQL persistence for order events/results, trades, and pending orders. | [Database](crates/db/README.md) |

### Proxy Crate

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

─
## Quick Start

1. Set the market-specific PostgreSQL environment variables.
2. Start the simulator with `cargo run --release`.
3. Connect through a FIX client or the web interface.

The detailed setup is documented below in the simulator runtime section.

## Running the Simulator

### Database configuration

The simulator uses two persistence scopes:

- per-market databases for order/trade/pending-order persistence
- one global database for player accounts/portfolio/tokens

In `crates/config/default.json`, each market declares:

- `database_url_env` (preferred): name of the environment variable to read
- `database_url` (optional fallback): direct connection string

And globally (top-level in the same config file):

- `player_database_url_env` (preferred): env var for the shared player database
- `player_database_url` (optional fallback): direct connection string

Default config uses:

- `DATABASE_URL_MARKET_SIMULATOR`
- `DATABASE_URL_NASDAQ`
- `DATABASE_URL_NYSE`

Set all three before starting:

```bash
export DATABASE_URL_MARKET_SIMULATOR=postgres://<user>:<password>@localhost:5432/market_simulator
export DATABASE_URL_NASDAQ=postgres://<user>:<password>@localhost:5432/market_nasdaq
export DATABASE_URL_NYSE=postgres://<user>:<password>@localhost:5432/market_nyse
export GRPC_PLAYER_SERVICE_PORT=50053
```

The simulator creates its tables on startup. The PostgreSQL user in each URL must therefore be able to create and alter tables in the active schema.

If your role cannot use `public`, either grant it access or point the connection at a schema you own via `search_path`, for example:

```bash
export DATABASE_URL_NASDAQ='postgres://<user>:<password>@localhost:5432/market_nasdaq?options=-csearch_path%3Dmarket_nasdaq'
export DATABASE_URL_NYSE='postgres://<user>:<password>@localhost:5432/market_nyse?options=-csearch_path%3Dmarket_nyse'
```

Note: the schema named in `search_path` must already exist. PostgreSQL does not create a schema just because the database has the same name.

Example setup:

```sql
CREATE DATABASE market_nasdaq;
CREATE DATABASE market_nyse;

-- The next commands must be run inside each target database, not in `postgres`.
\c market_nasdaq
CREATE SCHEMA market_nasdaq AUTHORIZATION <user>;

\c market_nyse
CREATE SCHEMA market_nyse AUTHORIZATION <user>;
```

If the schema already exists but belongs to another role, grant at least:

```sql
GRANT USAGE, CREATE ON SCHEMA market_nasdaq TO <user>;
GRANT USAGE, CREATE ON SCHEMA market_nyse TO <user>;
```

Then run:

To run the simulator, you can use the following command:

```bash
cargo run --release
```

This will start the server and allow clients to connect and interact with the simulated market. The default IP address and port for the server can be configured in the `server` crate. (eg. `1127.0.0.1:9876`)

### Production login URL filtering

When exposing the login page publicly, configure market `web.ip` / `web.port` entries with reachable public hostnames/IPs.

To prevent accidental advertisement of local/private URLs (for example `127.0.0.1`, `localhost`, `10.x.x.x`, `192.168.x.x`), enable:

```bash
export MARKET_SIM_PUBLIC_MARKETS_ONLY=1
```

With this enabled, both market and gateway `/api/markets` endpoints return only public market URLs.

## Contributing

Contributions are welcome.

Suggested workflow:

1. Open an issue to discuss a bug, improvement, or feature.
2. Keep changes focused and scoped to a single concern.
3. Update documentation when behavior, configuration, or architecture changes.
4. Run the relevant tests and benchmarks before submitting a pull request.

Areas that are especially useful for contributions:

- additional order types and FIX coverage
- recovery and replay support
- monitoring, metrics, and observability
- performance optimization and benchmark coverage
- market data distribution and multicast tooling

## License

This project is licensed under the MIT License.

See [LICENSE](LICENSE) for the full text.

## Support / Contact

For bug reports, feature requests, and operational questions, please use the repository issue tracker.

When reporting a problem, include:

- the market you are running (`NASDAQ` or `NYSE`)
- the client type (`FIX`, web, or gRPC)
- relevant logs or error messages
- configuration details that may affect networking, multicast, or database setup

## Roadmap

- Create a private network so anyone can receive multicast market data updates and connect to the FIX port without exposing the server to the internet.
- Implement the replayer based on log files to allow for backtesting and analysis of market data.
- Adding more support for logging and monitoring, including metrics collection and alerting.
- Add more command [Cancel, Replace] and order types [Stop, StopLimit] to the order book and matching engine.
- Add more instruments and support for multiple symbols in the order book and matching engine.
- Improve recovery workflows with snapshots + incremental logs.
- Expand market data tooling and multicast consumers.
