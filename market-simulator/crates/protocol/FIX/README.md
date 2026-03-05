# FIX Protocol Engine

This crate provides a FIX protocol engine implementation in Rust. It includes a parser for FIX messages and supports both scalar and SIMD parsing methods for performance benchmarking. The engine is designed to handle incoming FIX messages, extract relevant fields, and convert them into order events that can be processed by the order book and matching engine.

Check out the [FIX Protocol](../../../papers/fix-parser.md) crate for more details on the implementation and usage of the FIX protocol engine.

It also keep track of pending orders, as it is responsible of replying to the client with execution reports. It is connected to the order book and execution report generator through custom ring buffer channels to ensure low-latency processing of orders and generation of execution reports.

```
Network -> Fix server -> FiFO channel -> Order Book
                                              |
                                              v
                                        FIFO channel
                                              |
                                              v
             Network <- Fix server <- Execution Report
```

## TODO List

- Implement support for additional FIX message types and fields to handle a wider range of trading scenarios.
- For now, the FIX engine wait for execution reports to reply to the client, but in the future, it could be enhanced to send immediate acknowledgments for received orders and then follow up with execution reports once the orders are processed.
- Also, FIX server should have a specific thread to handle incoming messages and another thread to handle outgoing messages, to ensure that the processing of incoming messages does not block the sending of execution reports back to clients.