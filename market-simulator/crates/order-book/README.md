# Order Book

This crate implements core features for the market simulator, including:
- Order book management: maintaining the state of the order book, including buy and sell orders, and matching incoming orders against existing orders in the book -> `book.rs`
- Order processing: handling incoming orders, updating the order book state -> `engine.rs`
- Fan-out of execution reports: sending execution reports to be processed by:
  - the snapshot engine to update the order book snapshot after each order is processed -> `snapshot.rs`
  - the database engine to persist executed trades and order book updates for historical analysis and backtesting -> [database](../db/README.md)
  - the market data engine to generate market data updates (e.g., best bid/ask, trade prints) based on the latest state of the order book and executed trades -> [market_data](../market-feed/README.md)

## Architecture

```
ORDER PROCESSING FLOW
FIX Inbound        Order Book Engine         Order Book (book.rs)
     |                    |                         |
     | -- fix_to_ob ----> |                         |
     |   (custom SPSC)    | -- process_order() ---> |
     |                    |                         | -- match/update -->
     |                    | <-- (OrderEvent,        |
     |                    |      OrderResult) ----- |

FAN-OUT FLOW
Order Book Engine     Exec Report Engine     Snapshot Engine       DB Engine        Market Feed Engine
       |                       |                     |                  |                  |
       | -- ob_to_er --------> |                     |                  |                  |
       |   (OrderEvent,        |                     |                  |                  |
       |    OrderResult)       |                     |                  |                  |
       |                       |                     |                  |                  |
       | -- ob_to_snapshot ------------------------> |                  |                  |
       |   (OrderEvent,        |                     |                  |                  |
       |    OrderResult)       |                     |                  |                  |
       |                       |                     |                  |                  |
       | -- ob_to_db -------------------------------------------------> |                  |
       |   (OrderEvent,        |                     |                  |                  |
       |    OrderResult)       |                     |                  |                  |
       |                       |                     |                  |                  |
       | -- ob_to_md --------------------------------------------------------------------> |
       |   (OrderEvent,        |                     |                  |                  |
       |    OrderResult)       |                     |                  |                  |

CONTROL FLOW
gRPC / External        Order Book Engine
       |                      |
       | -- control_rx -----> |  (crossbeam channel: Reset, ...)
       |                      | -- import_order_book() --> (restore state from DB)
```

Queue / shared-state types:
- `fix_to_ob`: custom SPSC lock-free ring buffer (`spsc::spsc_lock_free`)
- `ob_to_er`, `ob_to_snapshot`, `ob_to_db`, `ob_to_md`: custom SPSC lock-free ring buffers, each carrying `(OrderEvent, OrderResult)`
- `control_rx`: `crossbeam_channel::Receiver<OrderBookControl>` (MPSC)

## Features

### Orders

- Support for limit orders, market orders, and cancel orders with price-time priority matching.

### Snapshot

- Incremental snapshot updates: After processing each order, the order book engine sends incremental updates to the snapshot engine to update the order book snapshot with the latest state of the order book. This allows the snapshot engine to maintain an up-to-date snapshot of the order book without having to generate a full snapshot after each order is processed, reducing overhead and improving performance.

Snapshot is updated in a periodic manner (e.g., every second) by the snapshot engine, which generates a full snapshot of the order book based on the latest state of the order book and the incremental updates received from the order book engine. It then stores the snapshot in ArcSwap by RCU, allowing other components (e.g., market feed engine) to access the latest snapshot of the order book with minimal latency.


## Performance Optimization

- Use of custom SPSC lock-free ring buffers for communication between the order book engine and other components (snapshot engine, database engine, market feed engine) to minimize latency and maximize throughput.
- Incremental snapshot updates to reduce the overhead of generating snapshots in the order book engine and allow the snapshot engine to maintain an up-to-date snapshot of the order book with minimal overhead.

At 23/06/2026:

Name                 │   p50 (ns) │   p99 (ns) │  p999 (ns) │   vs base
────────────────────────────────────────────────────────────────────────
Order Book           │       1052 │       1994 │       4319 │      1.00x


## TODO List

- Implement support for more order types (e.g., stop orders, stop-limit orders) and additional order attributes (e.g., time-in-force).
- Implement multiple instruments and support for multiple symbols in the order book.
- Implement batch processing of orders to improve performance and reduce latency.