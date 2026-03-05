# TCP Server

The `server` crate implements the TCP server that handles incoming FIX messages, processes orders through the order book and matching engine, and sends execution reports back to clients. It serves as the main entry point for the financial market simulator, allowing clients to connect and interact with the simulated market.

It relies on `use std::net::{TcpListener, TcpStream}` for handling TCP connections and uses custom ring buffer channels to communicate with the order book and execution report generator for low-latency processing of orders and generation of execution reports.

Each time a new client connects, the server spawns a new thread to handle the communication with that client, allowing for concurrent handling of multiple clients. The server listens for incoming FIX messages, processes them by sending orders to the order book and receiving execution reports, and then sends the appropriate responses back to the clients.

Warning:  There is not limitation on the number of threads that can be spawned for handling client connections, so in a production environment, it would be important to implement a thread pool or other concurrency management strategy to prevent resource exhaustion.

## Architecture

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

- Implement support for handling multiple client connections concurrently, potentially using a thread pool or asynchronous I/O to manage resources efficiently.
- Implement secured connections using TLS to ensure the confidentiality and integrity of the communication between clients and the server. (To be discussed as TLS potentielly add significant complexity and overhead, and may not be necessary for a simulator depending on the use case.)
- Add authentication and authorization mechanisms to control access to the server and ensure that only authorized clients can connect and interact with the market simulator. (To be discussed as it may not be necessary for a simulator depending on the use case.)