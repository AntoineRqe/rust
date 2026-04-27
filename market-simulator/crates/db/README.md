# db

PostgreSQL persistence layer for the market simulator.

This crate persists order lifecycle data from the matching pipeline and provides a blocking `DatabaseEngine` API for thread-based engine integration.

## Environment

- Uses a market-specific PostgreSQL URL passed from app config
- Preferred config path per market: `database_url_env` (e.g. `DATABASE_URL_NASDAQ`)
- Optional fallback per market: `database_url`
- Loads `.env` automatically via `dotenvy`

## What it stores

- `OrderEvent` snapshots (`order_event`)
- `OrderResult` metadata (`order_result`)
- executions/trades (`trades`)
- resting orders across market restarts (`pending_orders`)

This crate does **not** create or manage player/account tables.
Player state (`players`, portfolio lots, player metadata) is owned by the web player-store layer and should live in the shared `market_simulator` database, not in per-market databases.

## DatabaseEngine (blocking API)

`DatabaseEngine<'a, const N: usize>` owns:

- inbound `Consumer<'a, (OrderEvent, OrderResult), N>`
- `shutdown` flag
- shared `PgPool`

Exposed methods:

- `new(fifo_in, database_url) -> Result<Self, sqlx::Error>`
- `init() -> Result<(), sqlx::Error>`
- `run()`
- `persist_order_update(&self, &OrderEvent, &OrderResult) -> Result<(), sqlx::Error>`
- `get_all_order_events(&self) -> Result<Vec<OrderEvent>, sqlx::Error>`
- `get_all_order_results(&self) -> Result<Vec<OrderResult>, sqlx::Error>`
- `get_all_pending_orders(&self) -> Result<Vec<OrderEvent>, sqlx::Error>`
- `reset_database(&self) -> Result<(), sqlx::Error>`
- `pool(&self) -> Arc<PgPool>` — exposes the shared pool for the gRPC control service

Notes:

- The engine API is blocking; async DB helpers are bridged internally with a Tokio runtime helper.
- `run()` drains the FIFO and persists records synchronously, then closes the pool when shutdown completes.
- Helper `connect_from_env()` still exists, but simulator startup now resolves per-market URLs via config and passes them into `DatabaseEngine::new`.

## Schema

### `order_event`

| Column | Type | Notes |
|---|---|---|
| `id` | `BIGSERIAL PRIMARY KEY` | |
| `price` | `DOUBLE PRECISION` | |
| `quantity` | `DOUBLE PRECISION` | |
| `side` | `TEXT` | `"Buy"` / `"Sell"` |
| `symbol` | `TEXT` | |
| `order_type` | `TEXT` | `"LimitOrder"` / `"MarketOrder"` / `"CancelOrder"` |
| `cl_ord_id` | `TEXT` | |
| `orig_cl_ord_id` | `TEXT` | |
| `order_id` | `TEXT` | internal order ID assigned by the matching engine (NULL for CancelRejected) |
| `sender_id` | `TEXT` | |
| `target_id` | `TEXT` | |
| `event_timestamp` | `BIGINT` | milliseconds since Unix epoch |
| `payload` | `JSONB` | structured JSON copy of all fields |
| `created_at` | `TIMESTAMPTZ NOT NULL DEFAULT NOW()` | |

### `order_result`

| Column | Type | Notes |
|---|---|---|
| `id` | `BIGSERIAL PRIMARY KEY` | |
| `cl_ord_id` | `TEXT` | |
| `order_id` | `BIGINT` | internal order ID (matches `order_event.order_id`) |
| `result_timestamp` | `BIGINT` | milliseconds since Unix epoch |
| `result_type` | `TEXT NOT NULL` | `NEW`, `PARTIAL_FILL`, `FILL`, `CANCELLED`, `CANCEL_REJECTED`, `UNMATCHED` |
| `status` | `TEXT` | debug-format `OrderStatus` |
| `reason` | `TEXT` | `"cancelled"` / `"cancel_rejected"` or NULL |
| `payload` | `JSONB` | structured JSON copy of all fields |
| `created_at` | `TIMESTAMPTZ NOT NULL DEFAULT NOW()` | |

### `trades`

| Column | Type | Notes |
|---|---|---|
| `id` | `BIGSERIAL PRIMARY KEY` | |
| `trade_id` | `BIGINT` | numeric trade ID from the matching engine |
| `exec_id` | `TEXT UNIQUE` | string form of `trade_id`; used for deduplication |
| `symbol` | `TEXT` | |
| `buy_cl_ord_id` | `TEXT` | |
| `sell_cl_ord_id` | `TEXT` | |
| `qty` | `DOUBLE PRECISION NOT NULL` | filled quantity |
| `price` | `DOUBLE PRECISION NOT NULL` | execution price |
| `order_qty` | `DOUBLE PRECISION` | original quantity of the resting (maker) order |
| `leaves_qty` | `DOUBLE PRECISION` | remaining quantity of the resting order after this trade |
| `payload` | `JSONB` | includes `order_qty` and `leaves_qty` for query convenience |
| `traded_at` | `TIMESTAMPTZ NOT NULL DEFAULT NOW()` | |

### `pending_orders`

Purpose: keep order book resting orders when the market closes/restarts.

| Column | Type | Notes |
|---|---|---|
| `cl_ord_id` | `TEXT PRIMARY KEY` | |
| `price` | `DOUBLE PRECISION` | |
| `quantity` | `DOUBLE PRECISION` | remaining (leaves) quantity |
| `side` | `TEXT` | |
| `symbol` | `TEXT` | |
| `order_type` | `TEXT` | |
| `orig_cl_ord_id` | `TEXT` | |
| `order_id` | `TEXT` | internal order ID assigned by the matching engine |
| `sender_id` | `TEXT` | |
| `target_id` | `TEXT` | |
| `event_timestamp` | `BIGINT` | milliseconds since Unix epoch |
| `payload` | `JSONB` | structured JSON copy of all fields |
| `updated_at` | `TIMESTAMPTZ NOT NULL DEFAULT NOW()` | |

## Schema management

`create_tables(pool)`:

- Takes a transaction-scoped advisory lock (`pg_advisory_xact_lock`)
- Issues four `CREATE TABLE IF NOT EXISTS` statements — one per table
- **No migration logic** — schema is defined once at table creation; a database drop and recreate is required when the schema changes

## Sentinel event filtering

`persist_order_update` silently drops the default shutdown flush:

- If both `order_event` and `order_result` are fully zeroed/defaulted (zero price, zero quantity, empty IDs, `Unmatched` status), the record is **not** written to the database.

This prevents a spurious row at the end of every market session.

## Persistence rules

`persist_order_update(pool, order_event, order_result)` runs in one transaction:

1. Insert into `order_event` — all fields including `order_id` and `event_timestamp` as `BIGINT`
2. Insert into `order_result` — all fields including `order_id` and `result_timestamp` as `BIGINT`
3. Insert into `trades` for `FILL` and `PARTIAL_FILL` results — includes `order_qty` and `leaves_qty` as explicit columns
4. Synchronize `pending_orders`

All `payload` columns store a **structured `JSONB` object** (not a text debug dump).

Trade deduplication uses `ON CONFLICT (exec_id) DO NOTHING`.

### `pending_orders` synchronization

| Result | Action |
|---|---|
| `NEW` with remaining qty > 0 | upsert row (insert or update all fields) |
| `PARTIAL_FILL` with remaining qty > 0 | upsert row; `quantity` reflects leaves qty |
| `FILL` | delete row |
| `CANCELLED` | delete by `orig_cl_ord_id` (fallback: `cl_ord_id`) |
| `CANCEL_REJECTED` | no change |
| `CancelOrder` request type | no change |

## Reset behavior

`reset_database(pool)` clears all managed data in a transaction:

- Truncates `trades`, `order_result`, `order_event` with `RESTART IDENTITY`
- Truncates `pending_orders`

Uses advisory lock for safety under concurrent operations.

## Retrieval helpers

- `collect_all_orders(pool) -> Vec<OrderEvent>` — restores `timestamp_ms` from `event_timestamp BIGINT`
- `collect_all_order_results(pool) -> Vec<OrderResult>` — restores `internal_order_id` from `order_id`, `timestamp_ms` from `result_timestamp`
- `collect_all_trades(pool) -> Vec<Trade>` — reads `order_qty` and `leaves_qty` from dedicated columns
- `collect_all_pending_orders(pool) -> Vec<OrderEvent>` — restores `timestamp_ms` from `event_timestamp BIGINT`

## Internal ID counters

The `OrderBook` internal ID counters (`internal_id_counter`, `trade_id_counter`) start at **1**. `0` is reserved as the sentinel "no ID" value. `order_id_text_from_internal(0)` returns `NULL` in the database; any value ≥ 1 is stored as its string representation.

## Known limitations

- `Instant` cannot be faithfully restored from SQL; reconstructed values use `Instant::now()`.
- `collect_all_order_results` reconstructs `status` + empty trades; it does not restore the original `OrderResult` trade arrays.

## Tests

| Test | What it covers |
|---|---|
| `test_db_connection` | schema creation and connectivity |
| `test_persist_order_update` | full order-event/result/trade round-trip |
| `test_persist_partial_fill_updates_trades_in_db` | partial fill trade persistence and `order_qty`/`leaves_qty` columns |
| `test_pending_orders_track_new_partial_and_filled_orders` | pending order lifecycle: `NEW` → `PARTIAL_FILL` → `FILL` removal |
| `test_pending_orders_removed_on_cancel` | pending order removal on cancel and cancel-rejected no-op |
| `test_sentinel_default_event_is_not_persisted` | shutdown sentinel event is silently dropped |
