# Financial Market Simulator

This project is a financial market simulator implemented in Rust. It consists of several crates that work together to simulate a financial market, including an order book, a matching engine, and a market data feed.

### Architecture

```
Browser ──WebSocket──► axum web server (crates/web)
                              │
                    tokio broadcast channel
                              │
              ┌───────────────┴───────────────┐
              │                               │
    FixInboundEngine                 FixOutboundEngine
    (crates/protocol/FIX)            (crates/protocol/FIX)
              │                               │
         OrderEvent                    ExecReport
              │                               │
    OrderBookEngine ──────────────── ExecutionReportEngine
    (crates/order-book)            (crates/execution-report)
              │                               │
              └──────────────┬────────────────┘
                             │
                       DatabaseEngine
                         (crates/db)
```

Technical choices and discussion about the architecture and design of the simulator can be found here [Architecture and Design](./papers/market-simulator.md).

## Crates

- `order-book`: This crate implements the order book, which is responsible for maintaining the list of buy and sell orders in the market. It provides functionality for adding, removing, and matching orders. [Order Book](crates/order-book/README.md)
- `fix-protocol`: This crate implements the FIX protocol, which is a standard protocol for electronic trading. It provides functionality for encoding and decoding FIX messages, as well as handling FIX sessions. [FIX Protocol](crates/protocol/FIX/README.md)
- `execution-report`: This crate is responsible for generating execution reports based on the results of order processing. It takes the order events and their corresponding results to create detailed execution reports that can be sent back to clients or used for analysis. [Execution Report](crates/execution-report/README.md)
- `server`: This crate implements the server that handles incoming FIX messages, processes orders through the order book and matching engine, and sends execution reports back to clients. [Server](crates/server/README.md)
- `logging`: This crate provides logging functionality for the entire project, allowing for detailed logs of the order processing and market events. [Logging](crates/logging/README.md)
- `types`: This crate defines the common types used across the project, such as orders, trades, and market data. [Types](crates/types/README.md)
- `utils`: This crate provides utility functions and types that are used across the project, such as timestamp handling and fixed-point arithmetic. [Utils](crates/utils/README.md)
- `memory`: This crate provides an in-memory implementation of the order book and matching engine, allowing for fast processing of orders without the need for persistent storage. [Memory](crates/memory/README.md)
- `web`: This crate implements a web-based client that allows users to interact with the market simulator through a web interface. It provides functionality for sending FIX messages to the server and receiving execution reports in real-time. [Web Client](crates/web/README.md)
- `db`: This crate provides a PostgreSQL persistence layer for the market simulator, allowing for the storage and retrieval of order events, order results, trades, and pending orders. [Database](crates/db/README.md)
─
## Running the Simulator

To run the simulator, you can use the following command:

```bash
cargo run --release
```

This will start the server and allow clients to connect and interact with the simulated market. The default IP address and port for the server can be configured in the `server` crate. (eg. `1127.0.0.1:9876`)

## Running the Client

Refer to the [Tools README](./tools/README.md) for instructions on how to run the client and connect to the server.

## Future Work

- Implement the replayer based on log files to allow for backtesting and analysis of market data.
- Adding more support for logging and monitoring, including metrics collection and alerting.
- Add more command [Cancel, Replace] and order types [Stop, StopLimit] to the order book and matching engine.
- Add more instruments and support for multiple symbols in the order book and matching engine.
- Implement the Market Data Feed to simulate real-time market data and allow clients to subscribe to market data updates.
- Implement the UDP multicast for market data distribution to allow for efficient dissemination of market data to multiple clients.