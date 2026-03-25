# db

PostgreSQL persistence layer for the market simulator.

This crate persists order lifecycle data from the matching pipeline and provides a blocking `DatabaseEngine` API for thread-based engine integration.

## Environment

- Requires `DATABASE_URL` (PostgreSQL connection string)
- Loads `.env` automatically via `dotenvy`

## What it stores

- `OrderEvent` snapshots (`order_event`)
- `OrderResult` metadata (`order_result`)
- executions/trades (`trades`)
- resting orders across market restarts (`pending_orders`)

## DatabaseEngine (blocking API)

`DatabaseEngine<'a, const N: usize>` owns:

- inbound `Consumer<'a, (OrderEvent, OrderResult), N>`
- `shutdown` flag
- shared `PgPool`

Exposed methods:

- `new(fifo_in) -> Result<Self, sqlx::Error>`
- `init() -> Result<(), sqlx::Error>`
- `run()`
- `persist_order_update(&self, &OrderEvent, &OrderResult) -> Result<(), sqlx::Error>`
- `get_all_order_events(&self) -> Result<Vec<OrderEvent>, sqlx::Error>`
- `get_all_order_results(&self) -> Result<Vec<OrderResult>, sqlx::Error>`
- `get_all_pending_orders(&self) -> Result<Vec<OrderEvent>, sqlx::Error>`
- `reset_database(&self) -> Result<(), sqlx::Error>`

Notes:

- The engine API is blocking; async DB helpers are bridged internally with a Tokio runtime helper.
- `run()` drains the FIFO and persists records synchronously, then closes the pool when shutdown completes.

## Schema

### `order_event`

- `id BIGSERIAL PRIMARY KEY`
- `price DOUBLE PRECISION`
- `quantity DOUBLE PRECISION`
- `side TEXT`
- `symbol TEXT`
- `order_type TEXT`
- `cl_ord_id TEXT`
- `orig_cl_ord_id TEXT`
- `order_id TEXT`
- `sender_id TEXT`
- `target_id TEXT`
- `event_timestamp TEXT`
- `payload JSONB`
- `created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()`

### `order_result`

- `id BIGSERIAL PRIMARY KEY`
- `cl_ord_id TEXT`
- `result_type TEXT NOT NULL`
- `status TEXT`
- `reason TEXT`
- `payload JSONB`
- `created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()`

### `trades`

- `id BIGSERIAL PRIMARY KEY`
- `exec_id TEXT UNIQUE`
- `symbol TEXT`
- `buy_cl_ord_id TEXT`
- `sell_cl_ord_id TEXT`
- `qty DOUBLE PRECISION NOT NULL`
- `price DOUBLE PRECISION NOT NULL`
- `payload JSONB` (`order_qty`, `leaves_qty`)
- `traded_at TIMESTAMPTZ NOT NULL DEFAULT NOW()`

### `pending_orders`

Purpose: keep order book resting orders when market closes/restarts.

- `cl_ord_id TEXT PRIMARY KEY`
- `price DOUBLE PRECISION`
- `quantity DOUBLE PRECISION`
- `side TEXT`
- `symbol TEXT`
- `order_type TEXT`
- `orig_cl_ord_id TEXT`
- `order_id TEXT`
- `sender_id TEXT`
- `target_id TEXT`
- `event_timestamp TEXT`
- `payload JSONB`
- `updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()`

## Schema management

`create_tables(pool)`:

- takes a transaction-scoped advisory lock
- checks table existence via `to_regclass`
- creates missing tables
- runs `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` to keep schema forward-compatible

## Persistence rules

`persist_order_update(pool, order_event, order_result)` runs in one transaction:

1. insert into `order_event`
2. insert into `order_result`
3. insert into `trades` only for fill-like results (`FILL`, `PARTIAL_FILL`)
4. synchronize `pending_orders`

Trade deduplication uses `ON CONFLICT (exec_id) DO NOTHING`.

### `pending_orders` synchronization

- `NEW` with remaining qty > 0: upsert pending order
- `PARTIAL_FILL` with remaining qty > 0: update pending order quantity
- `FILL`: remove pending order
- `CANCELLED`: remove by `orig_cl_ord_id` (fallback to `cl_ord_id`)
- `CANCEL_REJECTED`: leave pending orders unchanged
- `CancelOrder` requests are not stored as pending orders

## Reset behavior

`reset_database(pool)` clears all managed data in a transaction:

- truncates `trades`, `order_result`, `order_event` with `RESTART IDENTITY`
- truncates `pending_orders`

Uses advisory lock for safety under concurrent operations.

## Retrieval helpers

- `collect_all_orders(pool) -> Vec<OrderEvent>`
- `collect_all_order_results(pool) -> Vec<OrderResult>` (status-centric reconstruction)
- `collect_all_trades(pool) -> Vec<Trade>`
- `collect_all_pending_orders(pool) -> Vec<OrderEvent>`

## Known limitations

- `Instant` cannot be faithfully restored from SQL; reconstructed values use `Instant::now()`.
- `collect_all_order_results` currently reconstructs `status` + empty trades; it does not reconstruct original full `OrderResult` trade arrays.

## Tests currently cover

- DB connectivity and schema setup
- filled-trade persistence correctness
- partial-fill trade persistence correctness
- pending order lifecycle (`NEW` -> `PARTIAL_FILL` -> `FILL` removal)
- pending order removal on cancel
