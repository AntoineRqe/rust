# Financial Market Simulator

This project is a financial market simulator implemented in Rust. It consists of several crates that work together to simulate a financial market, including an order book, a matching engine, and a market data feed.

## Crates

- `order-book`: This crate implements the order book, which is responsible for maintaining the list of buy and sell orders in the market. It provides functionality for adding, removing, and matching orders. [Order Book](crates/order-book/README.md)
- `fix-protocol`: This crate implements the FIX protocol, which is a standard protocol for electronic trading. It provides functionality for encoding and decoding FIX messages, as well as handling FIX sessions. [FIX Protocol](crates/fix-protocol/README.md)
- `logging`: This crate provides logging functionality for the entire project, allowing for detailed logs of the order processing and market events. [Logging](crates/logging/README.md)