# Snapshot

This crate contains the code related to taking snapshots of the market state, which can be used for debugging, testing, or replaying market scenarios. It includes functionality for serializing and deserializing market data, as well as utilities for managing snapshot files. Maximum depth is **10** levels of the order book. Snapshot preserve the best 10 bids and 10 asks for each symbol, along with their respective quantities and prices.

## Serialization

The snapshot module provides functions to serialize the current state of the market, including order books, trades, and other relevant data. This allows you to capture a moment in time of the market for later analysis or replay.

Snapshots are stored in a compact binary packet. All integer fields are **big-endian**.

### Snapshot packet layout

```text
+----------------------+-----------------------------+
| Field                | Size (bytes)                |
+----------------------+-----------------------------+
| timestamp_ms         | 8   (u64)                   |
| symbol_len           | 4   (u32)                   |
| symbol               | N   (UTF-8 bytes)           |
| snapshot_id          | 4   (u32)                   |
| bids_len             | 4   (u32)                   |
| bids[]               | bids_len * 41               |
| asks_len             | 4   (u32)                   |
| asks[]               | asks_len * 41               |
+----------------------+-----------------------------+
```

### Order level encoding (41 bytes each)

```text
+----------------------+-----------------------------+
| Field                | Size (bytes)                |
+----------------------+-----------------------------+
| price                | 20 (12 integer + 8 decimal) |
| quantity             | 20 (12 integer + 8 decimal) |
| side                 | 1  (0 = bid, 1 = ask)       |
+----------------------+-----------------------------+
```

### Packet size formula

```text
total_size = 8 + 4 + N + 4 + 4 + (bids_len * 41) + 4 + (asks_len * 41)
```

### Byte stream order (visual)

```text
[timestamp_ms]
[symbol_len][symbol]
[snapshot_id]
[bids_len][bid_0][bid_1]...[bid_n]
[asks_len][ask_0][ask_1]...[ask_n]
```