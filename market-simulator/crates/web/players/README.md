# Players Crate

A dedicated microservice for managing player state, authentication, and portfolio management in the market simulator. This crate runs as a standalone gRPC server that serves as the single authority for all player data, eliminating database deadlocks in multi-market trading scenarios.

## Architecture

### Microservice Design

The players crate is dual-purpose:

1. **Library** (`players` crate): Provides core data structures and logic for use by other crates
2. **Binary** (`players-server` binary): Standalone gRPC microservice listening on `[::1]:50052`

Market instances (backend) run as stateless clients that call the player server's gRPC API instead of accessing the player database directly. This eliminates all lock contention on the `players` table.

```
┌─────────────────┐         ┌──────────────────┐         ┌─────────────────┐
│  NASDAQ Market  │         │  Players Server  │         │  NYSE Market    │
│   (Backend)     │────────▶│  (gRPC Service)  │◀────────│   (Backend)     │
└─────────────────┘         │                  │         └─────────────────┘
                            │ PostgreSQL       │
                            │ - players        │
                            │ - portfolio_lots │
                            └──────────────────┘
```

## Module Structure

```
src/
├── lib.rs              # Library exports
├── main.rs             # Server entry point
├── service.rs          # gRPC service implementation
└── players/
    ├── mod.rs          # Core PlayerStore, Player, PendingOrder types
    ├── auth.rs         # Authentication logic
    └── portfolio.rs    # Portfolio and holdings management
```

## Core Types

### PlayerStore

Thread-safe registry of all player accounts. Backed by PostgreSQL with in-memory caching.

```rust
pub struct PlayerStore {
    inner: Arc<Mutex<StoreInner>>,
}

impl PlayerStore {
    pub fn load_postgres(database_url: &str) -> Self;
    pub fn get_player(&self, username: &str) -> Option<Player>;
    pub fn get_portfolio(&self, username: &str) -> Vec<PortfolioLot>;
    pub fn get_holdings_summary(&self, username: &str) -> HashMap<String, HoldingSummary>;
    // ... more methods
}
```

**Features:**
- PostgreSQL backing with in-memory cache
- Thread-safe operations via Arc<Mutex<>>
- Deduplication of FIX execution reports (prevents double-execution)
- Visitor count tracking (all-time and current)
- Connection tracking per player (IP logging)

### Player

Represents a single user account.

```rust
pub struct Player {
    pub username: String,
    pub password: String,                  // Argon2-hashed
    pub tokens: f64,                       // Cash balance
    pub pending_orders: Vec<PendingOrder>, // Orders in the order book
    pub connection_count: u64,             // Total WebSocket connections
    pub ips: Vec<String>,                  // Unique IPs ever connected
}
```

**Database:** `players` table
- `username` (text, primary key)
- `password` (text, Argon2 hash)
- `tokens` (float8)
- `connection_count` (int8)
- `ips_json` (jsonb array)
- `created_at`, `updated_at` (timestamps)

### PendingOrder

An order resting in the order book.

```rust
pub struct PendingOrder {
    pub cl_ord_id: String,  // Unique order ID
    pub symbol: String,     // NASDAQ, NYSE, etc.
    pub side: String,       // "1" = buy, "2" = sell
    pub qty: f64,           // Remaining quantity
    pub price: f64,         // Limit price
}
```

### PortfolioLot

A single purchased lot (for FIFO accounting).

```rust
pub struct PortfolioLot {
    pub id: i64,
    pub username: String,
    pub symbol: String,
    pub quantity: f64,
    pub price: f64,
    pub purchased_at: DateTime<Utc>,
}
```

**Database:** `portfolio_lots` table
- `id` (bigserial, primary key)
- `username` (text, foreign key to players)
- `symbol` (text, e.g., "AAPL")
- `quantity` (float8)
- `price` (float8, average purchase price)
- `purchased_at` (timestamp)

Holdings are calculated via FIFO: as players sell, the oldest lots are consumed first.

### HoldingSummary

Aggregate holdings for a symbol.

```rust
pub struct HoldingSummary {
    pub quantity: f64,  // Total remaining quantity
    pub avg_price: f64, // Weighted average price per unit
}
```

Derived from open portfolio lots (no database row).

## Module: `auth.rs`

Authentication and password management.

### Public API

```rust
pub fn authenticate_or_register(
    &self,
    username: &str,
    password: &str,
) -> Result<String, AuthError>;

pub fn ensure_player_exists(&self, username: &str) -> bool;

pub fn record_connection(&self, username: &str, ip: Option<&str>);

pub fn hash_password(password: &str) -> Result<String, String>;
pub fn verify_or_upgrade_password(stored: &str, provided: &str) -> Result<PasswordCheck, String>;
```

### Features

- **Argon2 password hashing** – uses `argon2` crate with cryptographically-secure salt generation
- **Password upgrade** – seamlessly upgrades plaintext passwords to hashes on first login (backward compatible)
- **User registration** – auto-creates new players with `INITIAL_TOKENS` (10,000 cash) on first login
- **Connection tracking** – records IP addresses and connection count per player
- **Error handling** – detailed error types (`AuthError`) with user-friendly messages

### Error Types

```rust
pub enum AuthError {
    UsernameRequired,
    PasswordRequired,
    UserExistsWrongPassword { username: String },
    PasswordHashFailed,
}
```

## Module: `portfolio.rs`

Portfolio and holdings management, trade execution.

### Public API

```rust
pub fn get_portfolio(&self, username: &str) -> Vec<PortfolioLot>;

pub fn get_holdings_summary(&self, username: &str) -> HashMap<String, HoldingSummary>;

pub fn apply_fix_execution_report(&self, fix_body: &str) -> bool;

pub fn apply_trade_from_feed(
    &self,
    trade_id: u64,
    passive_cl_ord_id: &str,
    symbol: &str,
    quantity: f64,
    price: f64,
) -> bool;

pub async fn load_portfolio_lots_for_user(
    pool: &PgPool,
    username: &str,
) -> Result<Vec<PortfolioLot>, sqlx::Error>;

pub async fn insert_portfolio_lot(
    pool: &PgPool,
    username: &str,
    symbol: &str,
    quantity: f64,
    price: f64,
) -> Result<(), sqlx::Error>;

pub async fn consume_portfolio_lots_fifo(
    pool: &PgPool,
    username: &str,
    symbol: &str,
    sell_qty: f64,
) -> Result<(), sqlx::Error>;

pub async fn delete_all_portfolio_lots(pool: &PgPool) -> Result<(), sqlx::Error>;
```

### Trade Execution Flow

#### FIX Execution Report Processing

When a buyer or seller's FIX TCP connection receives an execution report:

1. **Buy (side = "1")**: Insert a new portfolio lot with the executed price and quantity
2. **Sell (side = "2")**: Consume matching lots using FIFO (oldest first)
3. **Token adjustment**: Deduct cash for buys, add cash for sells
4. **Order update**: Mark order as filled or reduce remaining quantity
5. **Deduplication**: Track executed order IDs to prevent double-processing

#### Passive Trade Processing

When a trade is reconstructed from the multicast market data feed (for cases where the passive participant lost their connection):

1. Locate the passive (resting) order via client order ID
2. Consume portfolio lots from the passive side
3. Add cash proceeds to the passive player
4. Remove order from their pending orders if fully filled

### Portfolio Accounting

- **FIFO (First-In-First-Out)**: Oldest purchase lots are consumed first when selling
- **Atomic transactions**: All portfolio operations use PostgreSQL transactions (`FOR UPDATE` locks)
- **Holdings calculation**: Aggregate quantity and average price across open lots for a symbol

## gRPC Service

The `PlayerService` provides remote RPC endpoints for market instances.

### Proto Definition

See `proto/players.proto` for full service definition.

### Service Methods

1. **Authenticate** – Register or log in a player
   - Input: username, password
   - Output: Bearer token, ID suffix for ClOrdID generation, admin status
   - Errors: Username/password validation

2. **GetPlayerState** – Query player balance, holdings, orders, admin status
   - Input: username, bearer token
   - Output: Full PlayerState with tokens, pending orders, holdings map, visitor counts

3. **UpdatePendingOrders** – Bulk update a player's pending orders
   - Input: username, new pending orders list
   - Output: Success flag

4. **AddTrade** – Execute a buy or sell trade (legacy, not used by new flow)
   - Input: username, symbol, quantity, price, side
   - Output: Success flag

5. **ResetTokens** – Reset a single player's tokens to initial amount
   - Input: username, admin token
   - Output: Success flag

6. **ResetSeq** – Clear execution deduplication state (for market resets)
   - Input: username, admin token
   - Output: Success flag

7. **UpdateVisitors** – Update visitor counters (admin-only)
   - Input: active count, total count, admin token
   - Output: Success flag

### Server Configuration

```rust
// crates/web/players/src/main.rs
#[tokio::main]
async fn main() {
    let player_store = PlayerStore::load_postgres(&database_url);
    let service = PlayerServiceImpl::new(player_store);
    
    let addr = "[::1]:50052".parse()?;
    Server::builder()
        .add_service(PlayerServiceServer::new(service))
        .serve(addr)
        .await?;
}
```

**Listen address:** `[::1]:50052` (localhost, IPv6)

## Database Schema

### `players` Table

```sql
CREATE TABLE IF NOT EXISTS players (
    username TEXT PRIMARY KEY,
    password TEXT NOT NULL,
    tokens FLOAT8 DEFAULT 10000.0,
    connection_count INT8 DEFAULT 0,
    ips_json TEXT DEFAULT '[]',
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
);
```

### `portfolio_lots` Table

```sql
CREATE TABLE IF NOT EXISTS portfolio_lots (
    id BIGSERIAL PRIMARY KEY,
    username TEXT NOT NULL REFERENCES players(username),
    symbol TEXT NOT NULL,
    quantity FLOAT8 NOT NULL CHECK (quantity > 0),
    price FLOAT8 NOT NULL CHECK (price > 0),
    purchased_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
);

CREATE INDEX idx_portfolio_lots_username_symbol ON portfolio_lots(username, symbol);
CREATE INDEX idx_portfolio_lots_username_purchased_at ON portfolio_lots(username, purchased_at);
```

## Constants

```rust
pub const INITIAL_TOKENS: f64 = 10_000.0;  // Cash given to new players
```

## Key Implementation Details

### Thread-Safe Design

- All shared state is protected by `Arc<Mutex<>>` allowing cheap cloning across threads
- Database operations are async and don't block the in-memory cache
- FIX execution reports are processed sequentially per player

### Execution Report Deduplication

Prevents the same trade from being processed twice:
- Track `exec_id` (extracted from FIX message or constructed)
- Use a `HashSet<String>` to reject duplicate `exec_id` values
- Reset deduplication state via `ResetSeq` RPC on market resets

### Visitor Tracking

Two metrics:
1. **Active visitors** – currently connected WebSocket clients
2. **Total visitors** – all-time count persisted to database (survives restarts)

### Token (Cash) Flow

- **Initial**: 10,000 per new player
- **On buy**: tokens -= (quantity × price)
- **On sell**: tokens += (quantity × price)
- **Reset**: `reset_tokens()` or `reset_all_tokens()` RPC

## Building

### Library

```bash
cargo build -p players --lib
```

### Standalone Server

```bash
cargo build -p players --bin players-server --release
DATABASE_URL="postgres://user:pass@localhost:5432/db" ./target/release/players-server
```

### With gRPC client

Market instances link the `players` crate as a dependency and call its types, then communicate with the running server via gRPC.

## Testing

Unit tests included for core functionality:

```bash
cargo test -p players
```

Example test: Verify that executing trades on both buyer and seller sides updates both tokens and portfolio lots correctly.

## Future Enhancements

- [ ] Leader election for high availability (multiple player-server instances)
- [ ] Read replicas for player queries (eventually consistent)
- [ ] Token settlement layer (cash transfers between players)
- [ ] Leverage locks to ensure execution atomicity across multiple markets
- [ ] Performance optimizations (batching, connection pooling)
