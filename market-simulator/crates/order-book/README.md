# Order Book

This crate implements the order book, which is responsible for maintaining the list of buy and sell orders in the market. It provides functionality for adding, removing, and matching orders.

## Architecture

The order book is implemented using to `BinaryHeap` (one for buy orders and one for sell orders) to maintain the price-time priority of orders. The order book supports adding new orders and matching incoming orders against the existing orders in the book.

The order book is connected received order from the FIX server, processes them, and sends the results to the execution report generator. It uses custom ring buffer channels for communication with the FIX server and the execution report generator to ensure low-latency processing of orders.

```
Fix server -> FiFO channel -> Order Book -> FIFO channel -> Execution Report Generator
```

Order book is pinned to a specific CPU. 

## TODO List

- Implement support for more order types (e.g., stop orders, stop-limit orders) and additional order attributes (e.g., time-in-force).
- Implement multiple instruments and support for multiple symbols in the order book.
- Implement batch processing of orders to improve performance and reduce latency.