# TCP Server

The `server` crate implements the TCP server that handles incoming FIX messages, processes orders through the order book and matching engine, and sends execution reports back to clients. It serves as the main entry point for the financial market simulator, allowing clients to connect and interact with the simulated market.

It relies on `Tokio` for asynchronous I/O and `socket2` for low-level socket management, enabling efficient handling of multiple client connections. The server listens for incoming TCP connections, create Tokio tasks to handle each client connection concurrently, and uses custom SPSC channels to communicate with the order book and matching engine.

**Warning**: It can handle at maximum 2048 concurrent client connections, as the server uses a semaphore to limit the number of active connections. This is a design choice to prevent resource exhaustion and ensure stable performance, but it may need to be adjusted based on the expected load and available system resources.

## Architecture

```
REQUEST FLOW
Client            TCP Server             FIX Inbound             Order Book/Match
  |                  |                       |                         |
  | -- FIX req ----> |                       |                         |
  |                  | -- tcp_to_fix ------> |                         |
  |                  |    (crossbeam bounded)| -- fix_to_ob ---------> |
  |                  |                       |    (custom SPSC)        |

RESPONSE FLOW
Client            TCP Server            FIX Outbound        Execution Report      Order Book/Match
  |                  |                       |                     |                     |
  | <--- FIX resp -- |                       |                     |                     |
  |                  | <--- resp_queue ----- |                     |                     |
  |                  |   (Tokio mpsc/client) | <--- er_to_fix ---- |                     |
  |                  |                       |    (custom SPSC)    | <--- ob_to_er ----- |
  |                  |                       |                     |    (custom SPSC)    |
```

Queue types:
- `tcp_to_fix`: bounded crossbeam MPMC queue (`crossbeam::channel::bounded`)
- `resp_queue`: `tokio::sync::mpsc` bounded per-client response queue
- `fix_to_ob`, `ob_to_er`, `er_to_fix`: custom SPSC lock-free ring buffers

Check the documentation for multicast for the market data feed architecture and flow : [Multicast](./src/multicast.md)

## TODO List

- Implement secured connections using TLS to ensure the confidentiality and integrity of the communication between clients and the server. (To be discussed as TLS potentielly add significant complexity and overhead, and may not be necessary for a simulator depending on the use case.)
- Search for potential optimizations in the server's handling of client connections and message processing to further reduce latency and improve throughput.
- Thinking about scalability, consider implementing load balancing strategies to distribute incoming client connections across multiple server instances, allowing the system to handle a larger number of concurrent connections and improve overall performance.