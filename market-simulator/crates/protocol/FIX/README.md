# FIX Protocol Engine

This crate provides a FIX protocol engine implementation in Rust. It includes a parser for FIX messages and supports both scalar and SIMD parsing methods for performance benchmarking. The engine is designed to handle incoming FIX messages, extract relevant fields, and convert them into order events that can be processed by the order book and matching engine.

Check out the [FIX Protocol](../../../papers/fix-parser.md) crate for more details on the implementation and usage of the FIX protocol engine.