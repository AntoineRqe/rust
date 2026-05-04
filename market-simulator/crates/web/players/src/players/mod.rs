pub mod auth;
pub mod portfolio;
pub mod token;

pub use auth::{AuthError, hash_password, verify_or_upgrade_password};
pub use portfolio::{HoldingSummary, PortfolioLot, delete_all_portfolio_lots};
pub use token::{generate_token, extract_id_suffix};

use serde::{Deserialize, Serialize};
use sqlx::{query, query_scalar, PgPool, Row};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use utils::market_name;

/// Tokens each new player receives on registration.
pub const INITIAL_TOKENS: f64 = 10_000.0;

/// A single order resting in the order book on behalf of a player.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingOrder {
    pub cl_ord_id: String,
    pub symbol: String,
    /// FIX side: "1" = buy, "2" = sell.
    pub side: String,
    pub qty: f64,
    pub price: f64,
}

// ── Stored player record ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Player {
    pub username: String,
    /// Plaintext password (suitable for a local simulator; hash it for production use).
    pub password: String,
    /// Remaining token balance.
    pub tokens: f64,
    /// Orders currently resting in the order book (not yet filled or cancelled).
    #[serde(default, skip_serializing, skip_deserializing)]
    pub pending_orders: Vec<PendingOrder>,
    /// Total number of authenticated websocket connections observed for this player.
    #[serde(default)]
    pub connection_count: u64,
    /// Unique client IPs seen for this player.
    #[serde(default)]
    pub ips: Vec<String>,
}

impl Player {
    pub fn new(username: String, password_hash: String) -> Self {
        Self {
            username,
            password: password_hash,
            tokens: INITIAL_TOKENS,
            pending_orders: Vec::new(),
            connection_count: 0,
            ips: Vec::new(),
        }
    }
}

// ── Serialisation wrapper ─────────────────────────────────────────────────────

#[derive(Debug, Default, Serialize, Deserialize)]
struct StorageData {
    players: HashMap<String, Player>,
    #[serde(default)]
    order_owners: HashMap<String, String>,
    #[serde(default)]
    total_visitor_count: u64,
}

// ── PlayerStore ───────────────────────────────────────────────────────────────

/// Thread-safe player registry backed by PostgreSQL.
/// Cloning is cheap — the inner state is Arc-backed.
#[derive(Clone)]
pub struct PlayerStore {
    pub inner: Arc<Mutex<StoreInner>>,
}

pub struct StoreInner {
    pub players: HashMap<String, Player>,
    pub order_owners: HashMap<String, String>,
    pub processed_exec_ids: HashSet<String>,
    pub total_visitor_count: u64,
    pub pool: Option<Arc<PgPool>>,
}

impl PlayerStore {
    fn resolve_username_from_sender_id(inner: &StoreInner, sender_id: &str) -> Option<String> {
        let sender = sender_id.trim();
        if sender.is_empty() {
            return None;
        }

        if inner.players.contains_key(sender) {
            return Some(sender.to_string());
        }

        let normalized_sender = sender.to_ascii_uppercase();
        inner
            .players
            .keys()
            .find(|username| username.trim().to_ascii_uppercase() == normalized_sender)
            .cloned()
    }

    /// Load the store from PostgreSQL. If `should_persist` is false, this instance will not
    /// write changes back to the database (for secondary market instances).
    pub fn load_postgres(database_url: &str) -> Self {
        Self::load_postgres_with_persistence(database_url, true)
    }

    pub fn load_postgres_read_only(database_url: &str) -> Self {
        Self::load_postgres_with_persistence(database_url, false)
    }

    fn load_postgres_with_persistence(database_url: &str, should_persist: bool) -> Self {
        let pool = if should_persist {
            Some(Arc::new(
                block_on_storage(db::connect(database_url))
                    .unwrap_or_else(|e| panic!("failed to connect player store to postgres: {e}")),
            ))
        } else {
            // Read-only instances don't need a pool for persistence
            None
        };

        // Still need to load initial data
        let initial_pool = Arc::new(
            block_on_storage(db::connect(database_url))
                .unwrap_or_else(|e| panic!("failed to connect player store to postgres: {e}")),
        );

        block_on_storage(create_player_tables(&initial_pool))
            .unwrap_or_else(|e| panic!("failed to create player store tables: {e}"));

        let (players, order_owners, total_visitor_count) =
            block_on_storage(load_storage_from_db(&initial_pool))
                .unwrap_or_else(|e| panic!("failed to load player store from postgres: {e}"));

        tracing::info!(
            "[{}] Player store: {} player(s) loaded from PostgreSQL",
            market_name(),
            players.len(),
        );

        PlayerStore {
            inner: Arc::new(Mutex::new(StoreInner {
                players,
                order_owners,
                processed_exec_ids: HashSet::new(),
                total_visitor_count,
                pool,
            })),
        }
    }

    #[cfg(test)]
    fn from_storage_data(storage: StorageData) -> Self {
        PlayerStore {
            inner: Arc::new(Mutex::new(StoreInner {
                players: storage.players,
                order_owners: storage.order_owners,
                processed_exec_ids: HashSet::new(),
                total_visitor_count: storage.total_visitor_count,
                pool: None,
            })),
        }
    }

    /// Authenticate an existing player **or** register a brand-new one with
    /// [`INITIAL_TOKENS`] tokens and an empty pending-order list.
    ///
    /// Returns `Ok(username)` on success, `Err(reason)` if the player exists
    /// but the password does not match.
    /// Return a snapshot of the player's current state, or `None` if unknown.
    pub fn get_player(&self, username: &str) -> Option<Player> {
        self.inner.lock().unwrap().players.get(username).cloned()
    }

    /// Record a newly placed order.
    ///
    /// Token balance does NOT change at NEW time; it changes only when
    /// transactions (fills) occur via execution reports.
    pub fn add_pending_order(&self, username: &str, order: PendingOrder) {
        let mut inner = self.inner.lock().unwrap();
        let cl_ord_id = order.cl_ord_id.clone();
        let mut inserted = false;
        if let Some(player) = inner.players.get_mut(username) {
            player.pending_orders.push(order);
            inserted = true;
        }

        if inserted {
            inner.order_owners.insert(cl_ord_id, username.to_string());
        }
        drop(inner);
        self.flush();
    }

    /// Remove a pending order by `cl_ord_id`.
    ///
    /// Token balance does NOT change on cancel/remove; only fills move tokens.
    pub fn remove_pending_order(&self, username: &str, cl_ord_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(player) = inner.players.get_mut(username) {
            if let Some(pos) = player
                .pending_orders
                .iter()
                .position(|o| o.cl_ord_id == cl_ord_id)
            {
                player.pending_orders.remove(pos);
            }
        }

        inner.order_owners.remove(cl_ord_id);
        drop(inner);
        self.flush();
    }

    /// Return a snapshot of all known cl_ord_id -> username associations.
    pub fn get_order_owners(&self) -> HashMap<String, String> {
        self.inner.lock().unwrap().order_owners.clone()
    }

    /// Hydrate cl_ord_id -> username associations during startup.
    ///
    /// Each tuple is `(cl_ord_id, sender_id)` from persisted pending orders.
    /// Only sender IDs matching known players are accepted.
    ///
    /// Returns the number of associations inserted or updated.
    pub fn hydrate_order_owners_from_sender_ids(&self, entries: &[(String, String)]) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let mut updated = 0usize;

        for (cl_ord_id, sender_id) in entries {
            let cl_ord_id = cl_ord_id.trim();
            let sender_id = sender_id.trim();
            if cl_ord_id.is_empty() || sender_id.is_empty() {
                continue;
            }

            let Some(owner_username) = Self::resolve_username_from_sender_id(&inner, sender_id)
            else {
                continue;
            };

            let replaced = inner
                .order_owners
                .insert(cl_ord_id.to_string(), owner_username.clone());
            if replaced.as_deref() != Some(owner_username.as_str()) {
                updated += 1;
            }
        }

        drop(inner);
        if updated > 0 {
            self.flush();
        }

        updated
    }

    /// Record a successful websocket connection for a player and persist
    /// associated client IP (if provided).
    /// Reset a player's token balance to the initial amount.
    ///
    /// Returns `true` if the player exists and was updated.
    pub fn reset_tokens(&self, username: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(player) = inner.players.get_mut(username) else {
            return false;
        };

        player.tokens = INITIAL_TOKENS;
        drop(inner);
        self.flush();
        true
    }

    /// Reset every player's token balance to the initial amount.
    ///
    /// Returns the number of players updated.
    pub fn reset_all_tokens(&self) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let mut updated = 0usize;

        for player in inner.players.values_mut() {
            player.tokens = INITIAL_TOKENS;
            updated += 1;
        }

        drop(inner);
        self.flush();
        updated
    }

    /// Reset player-side market state after a global market reset.
    ///
    /// Clears all pending orders and holdings for every player and drops execution-report
    /// deduplication state so future reports are processed normally.
    ///
    /// Returns `(players_touched, orders_removed)`.
    pub fn reset_market_state(&self) -> (usize, usize) {
        let mut inner = self.inner.lock().unwrap();
        let mut players_touched = 0usize;
        let mut orders_removed = 0usize;

        for player in inner.players.values_mut() {
            if !player.pending_orders.is_empty() {
                orders_removed += player.pending_orders.len();
                player.pending_orders.clear();
                players_touched += 1;
            }
        }

        inner.order_owners.clear();
        inner.processed_exec_ids.clear();
        let pool = inner.pool.clone();
        drop(inner);

        // Always persist to keep on-disk state aligned with the market reset.
        self.flush();

        // Clear all portfolio lots.
        if let Some(pool) = pool {
            if let Err(e) = block_on_storage(delete_all_portfolio_lots(&pool)) {
                tracing::error!(
                    "[{}] Failed to clear portfolio lots on market reset: {e}",
                    market_name()
                );
            }
        }

        (players_touched, orders_removed)
    }

    /// Return the all-time total visitor count persisted across restarts.
    pub fn total_visitors(&self) -> u64 {
        self.inner.lock().unwrap().total_visitor_count
    }

    /// Increment the all-time visitor counter and persist immediately.
    /// Returns the new total.
    pub fn record_visit(&self) -> u64 {
        let new_total = {
            let mut inner = self.inner.lock().unwrap();
            inner.total_visitor_count = inner.total_visitor_count.saturating_add(1);
            inner.total_visitor_count
        };
        self.flush();
        new_total
    }

    /// Sync visitor counts from the backend (update total visitor count).
    /// Called by the backend to persist its tracked visitor metrics.
    pub fn update_visitor_count(&self, total: i32) {
        let new_total = total as u64;
        let mut inner = self.inner.lock().unwrap();
        // Use max to avoid overwriting with lower value (in case of race conditions)
        inner.total_visitor_count = inner.total_visitor_count.max(new_total);
        drop(inner);
        self.flush();
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Persist the current in-memory state to PostgreSQL. This is called after every state change to ensure durability and consistency between the in-memory state and the database.
    /// Can be expensive, use batching for optimization if needed.
    pub fn flush(&self) {
        let inner = self.inner.lock().unwrap();
        let data = StorageData {
            players: inner.players.clone(),
            order_owners: inner.order_owners.clone(),
            total_visitor_count: inner.total_visitor_count,
        };
        let pool = inner.pool.clone();
        drop(inner);

        let Some(pool) = pool else {
            return;
        };

        // Retry persistence with exponential backoff to handle transient deadlocks
        const MAX_RETRIES: u32 = 3;
        for attempt in 0..MAX_RETRIES {
            match block_on_storage(persist_storage_to_db(&pool, &data)) {
                Ok(_) => return,
                Err(e) if e.to_string().contains("deadlock") && attempt < MAX_RETRIES - 1 => {
                    let backoff_ms = 100 * (2_u64.pow(attempt));
                    tracing::warn!(
                        "[{}] Deadlock persisting player data (attempt {}), retrying in {}ms: {e}",
                        market_name(),
                        attempt + 1,
                        backoff_ms
                    );
                    std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
                }
                Err(e) => {
                    tracing::error!(
                        "[{}] Failed to persist player data to PostgreSQL after {} attempts: {e}",
                        market_name(),
                        MAX_RETRIES
                    );
                    return;
                }
            }
        }
    }
}

fn block_on_storage<F>(future: F) -> F::Output
where
    F: Future,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        tokio::task::block_in_place(|| handle.block_on(future))
    } else {
        static STORAGE_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
        let runtime = STORAGE_RUNTIME.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("player-store-db")
                .build()
                .expect("failed to build shared tokio runtime for player store")
        });
        runtime.block_on(future)
    }
}

async fn create_player_tables(pool: &PgPool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    query("SELECT pg_advisory_xact_lock($1)")
        .bind(42_4243_i64)
        .execute(&mut *tx)
        .await?;

    query(
        r#"
        CREATE TABLE IF NOT EXISTS players (
            username TEXT PRIMARY KEY,
            password TEXT NOT NULL,
            tokens DOUBLE PRECISION NOT NULL,
            connection_count BIGINT NOT NULL DEFAULT 0,
            ips_json TEXT NOT NULL DEFAULT '[]',
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        "#,
    )
    .execute(&mut *tx)
    .await?;

    query(
        r#"
        CREATE TABLE IF NOT EXISTS player_order_owners (
            cl_ord_id TEXT PRIMARY KEY,
            username TEXT NOT NULL REFERENCES players(username) ON DELETE CASCADE,
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        "#,
    )
    .execute(&mut *tx)
    .await?;

    query(
        r#"
        CREATE TABLE IF NOT EXISTS portfolio_lots (
            id          BIGSERIAL PRIMARY KEY,
            username    TEXT NOT NULL REFERENCES players(username) ON DELETE CASCADE,
            symbol      TEXT NOT NULL,
            quantity    DOUBLE PRECISION NOT NULL CHECK (quantity > 0),
            price       DOUBLE PRECISION NOT NULL,
            purchased_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        "#,
    )
    .execute(&mut *tx)
    .await?;

    query("CREATE INDEX IF NOT EXISTS portfolio_lots_user_symbol ON portfolio_lots(username, symbol, purchased_at)")
        .execute(&mut *tx)
        .await?;

    query(
        r#"
        CREATE TABLE IF NOT EXISTS player_store_meta (
            meta_key TEXT PRIMARY KEY,
            meta_value_bigint BIGINT NOT NULL DEFAULT 0,
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        "#,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await
}

async fn load_storage_from_db(
    pool: &PgPool,
) -> Result<(HashMap<String, Player>, HashMap<String, String>, u64), sqlx::Error> {
    let player_rows =
        query("SELECT username, password, tokens, connection_count, ips_json FROM players")
            .fetch_all(pool)
            .await?;

    let mut players = HashMap::new();
    for row in player_rows {
        let username: String = row.try_get("username")?;
        let password: String = row.try_get("password")?;
        let tokens: f64 = row.try_get("tokens")?;
        let connection_count_raw: i64 = row.try_get("connection_count")?;
        let ips_json: String = row.try_get("ips_json")?;

        let ips = serde_json::from_str::<Vec<String>>(&ips_json).unwrap_or_default();

        players.insert(
            username.clone(),
            Player {
                username,
                password,
                tokens,
                pending_orders: Vec::new(),
                connection_count: connection_count_raw.max(0) as u64,
                ips,
            },
        );
    }

    let owner_rows = query("SELECT cl_ord_id, username FROM player_order_owners")
        .fetch_all(pool)
        .await?;
    let mut order_owners = HashMap::new();
    for row in owner_rows {
        order_owners.insert(row.try_get("cl_ord_id")?, row.try_get("username")?);
    }

    let total_visitor_count = query_scalar::<_, i64>(
        "SELECT meta_value_bigint FROM player_store_meta WHERE meta_key = 'total_visitor_count'",
    )
    .fetch_optional(pool)
    .await?
    .unwrap_or(0)
    .max(0) as u64;

    Ok((players, order_owners, total_visitor_count))
}

/// Generate a lock ID from username for advisory locking
fn username_lock_id(username: &str) -> i64 {
    let mut hasher = DefaultHasher::new();
    username.hash(&mut hasher);
    // Convert to i64, ensure positive by taking absolute value
    (hasher.finish() as i64).abs()
}

async fn persist_storage_to_db(pool: &PgPool, data: &StorageData) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    for player in data.players.values() {
        // Acquire advisory lock for this username to prevent deadlocks
        // when multiple markets try to update the same player simultaneously
        let lock_id = username_lock_id(&player.username);
        query("SELECT pg_advisory_lock($1)")
            .bind(lock_id)
            .execute(&mut *tx)
            .await?;

        let ips_json = serde_json::to_string(&player.ips).unwrap_or_else(|_| "[]".to_string());
        query(
            r#"
            INSERT INTO players (username, password, tokens, connection_count, ips_json)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (username) DO UPDATE SET
                password = EXCLUDED.password,
                tokens = EXCLUDED.tokens,
                connection_count = EXCLUDED.connection_count,
                ips_json = EXCLUDED.ips_json,
                updated_at = NOW()
            "#,
        )
        .bind(&player.username)
        .bind(&player.password)
        .bind(player.tokens)
        .bind(player.connection_count as i64)
        .bind(ips_json)
        .execute(&mut *tx)
        .await?;

        // Release advisory lock (automatically released when transaction ends)
        query("SELECT pg_advisory_unlock($1)")
            .bind(lock_id)
            .execute(&mut *tx)
            .await?;
    }

    query("DELETE FROM player_order_owners")
        .execute(&mut *tx)
        .await?;

    for (cl_ord_id, username) in &data.order_owners {
        query(
            r#"
            INSERT INTO player_order_owners (cl_ord_id, username)
            VALUES ($1, $2)
            ON CONFLICT (cl_ord_id) DO UPDATE SET
                username = EXCLUDED.username,
                updated_at = NOW()
            "#,
        )
        .bind(cl_ord_id)
        .bind(username)
        .execute(&mut *tx)
        .await?;
    }

    query(
        r#"
        INSERT INTO player_store_meta (meta_key, meta_value_bigint)
        VALUES ('total_visitor_count', $1)
        ON CONFLICT (meta_key) DO UPDATE SET
            meta_value_bigint = EXCLUDED.meta_value_bigint,
            updated_at = NOW()
        "#,
    )
    .bind(data.total_visitor_count as i64)
    .execute(&mut *tx)
    .await?;

    tx.commit().await
}

pub fn parse_fix_fields(body: &str) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    for part in body.split('│') {
        let token = part.trim();
        if token.is_empty() {
            continue;
        }
        if let Some((k, v)) = token.split_once('=') {
            fields.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    fields
}

pub fn parse_f64(v: Option<&str>) -> f64 {
    v.and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trade_updates_both_participants_without_pending_orders_loaded() {
        let mut players = HashMap::new();
        players.insert(
            "alice".to_string(),
            Player {
                username: "alice".to_string(),
                password: "pw".to_string(),
                tokens: 10000.0,
                pending_orders: Vec::new(),
                connection_count: 0,
                ips: Vec::new(),
            },
        );
        players.insert(
            "bob".to_string(),
            Player {
                username: "bob".to_string(),
                password: "pw".to_string(),
                tokens: 10000.0,
                pending_orders: Vec::new(),
                connection_count: 0,
                ips: Vec::new(),
            },
        );

        let store = PlayerStore::from_storage_data(StorageData {
            players,
            order_owners: HashMap::from([
                ("ALICESELL1".to_string(), "alice".to_string()),
                ("BOBBUY1".to_string(), "bob".to_string()),
            ]),
            total_visitor_count: 0,
        });

        let buyer_fill =
            "35=8 │ 39=2 │ 11=BOBBUY1 │ 54=1 │ 55=AAPL │ 31=100 │ 32=5 │ 151=0 │ 17=E-BUY-1";
        assert!(store.apply_fix_execution_report(buyer_fill));

        let seller_fill =
            "35=8 │ 39=2 │ 11=ALICESELL1 │ 54=2 │ 55=AAPL │ 31=100 │ 32=5 │ 151=0 │ 17=E-SELL-1";
        assert!(store.apply_fix_execution_report(seller_fill));

        let alice = store.get_player("alice").expect("alice exists");
        let bob = store.get_player("bob").expect("bob exists");

        assert!((alice.tokens - 10500.0).abs() < 1e-9);
        assert!((bob.tokens - 9500.0).abs() < 1e-9);

        let owners = store.get_order_owners();
        assert!(!owners.contains_key("ALICESELL1"));
        assert!(!owners.contains_key("BOBBUY1"));
    }
}
