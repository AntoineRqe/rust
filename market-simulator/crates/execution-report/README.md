# Execution Report

This crate is responsible for generating execution reports based on the results of order processing. It takes the order events and their corresponding results to create detailed FIX execution reports that can be sent back to clients or used for analysis.

The execution report generator is connected to the order book and the FIX server through custom ring buffer channels to ensure low-latency processing of orders and generation of execution reports.

```
Order Book -> FIFO channel -> Execution Report Generator -> FIFO channel -> FIX server
```

## TODO List

- Implement support for additional FIX fields in the execution reports to provide more detailed information about the order processing results.
- Implement support for generating execution reports for different order types and scenarios, such as partial fills, cancellations, and rejections.