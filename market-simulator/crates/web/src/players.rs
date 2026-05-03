use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::sync::OnceLock;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use argon2::password_hash::SaltString;
use chrono::{DateTime, Utc};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row, query, query_scalar};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Summary of a player's holdings in a symbol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoldingSummary {
    pub quantity: f64,
    pub avg_price: f64,
}

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

fn market_name() -> &'static str {
    static MARKET_NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    MARKET_NAME
        .get_or_init(|| std::env::var("MARKET_NAME").unwrap_or_else(|_| "unknown".to_string()))
        .as_str()
}

/// Tokens each new player receives on registration.
pub const INITIAL_TOKENS: f64 = 10_000.0;

#[derive(Debug, Clone)]
pub enum AuthError {
    UsernameRequired,
    PasswordRequired,
    UserExistsWrongPassword { username: String },
    PasswordHashFailed,
}

impl AuthError {
    pub fn code(&self) -> &'static str {
        match self {
            AuthError::UsernameRequired => "USERNAME_REQUIRED",
            AuthError::PasswordRequired => "PASSWORD_REQUIRED",
            AuthError::UserExistsWrongPassword { .. } => "USER_EXISTS_WRONG_PASSWORD",
            AuthError::PasswordHashFailed => "PASSWORD_HASH_FAILED",
        }
    }

    pub fn message(&self) -> String {
        match self {
            AuthError::UsernameRequired => "Username is required".to_string(),
            AuthError::PasswordRequired => "Password is required".to_string(),
            AuthError::UserExistsWrongPassword { username } => format!(
                "User '{username}' already exists, but the password is incorrect. If you intended to create a new account, choose a different username."
            ),
            AuthError::PasswordHashFailed => {
                "Could not create account right now (password hashing failed)".to_string()
            }
        }
    }
}

// ── Portfolio lot ────────────────────────────────────────────────────────────

/// A single purchased lot in a player's portfolio.
#[derive(Debug, Clone, Serialize)]
pub struct PortfolioLot {
    pub id: i64,
    pub username: String,
    pub symbol: String,
    /// Remaining (un-sold) quantity for this lot.
    pub quantity: f64,
    /// Average purchase price per unit for this lot.
    pub price: f64,
    pub purchased_at: DateTime<Utc>,
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
    inner: Arc<Mutex<StoreInner>>,
}

struct StoreInner {
    players: HashMap<String, Player>,
    order_owners: HashMap<String, String>,
    processed_exec_ids: HashSet<String>,
    total_visitor_count: u64,
    pool: Option<Arc<PgPool>>,
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
            Some(Arc::new(block_on_storage(db::connect(database_url))
                .unwrap_or_else(|e| panic!("failed to connect player store to postgres: {e}"))))
        } else {
            // Read-only instances don't need a pool for persistence
            None
        };

        // Still need to load initial data
        let initial_pool = Arc::new(block_on_storage(db::connect(database_url))
            .unwrap_or_else(|e| panic!("failed to connect player store to postgres: {e}")));

        block_on_storage(create_player_tables(&initial_pool))
            .unwrap_or_else(|e| panic!("failed to create player store tables: {e}"));

        let (players, order_owners, total_visitor_count) = block_on_storage(load_storage_from_db(&initial_pool))
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
    pub fn authenticate_or_register(
        &self,
        username: &str,
        password: &str,
    ) -> Result<String, AuthError> {
        let username = username.trim();
        if username.is_empty() {
            return Err(AuthError::UsernameRequired);
        }
        if password.is_empty() {
            return Err(AuthError::PasswordRequired);
        }

        let mut inner = self.inner.lock().unwrap();
        match inner.players.get_mut(username) {
            Some(player) => {
                match verify_or_upgrade_password(&player.password, password) {
                    Ok(PasswordCheck::Verified) => {
                        tracing::debug!("[{}] Player '{username}' authenticated", market_name());
                        Ok(username.to_string())
                    }
                    Ok(PasswordCheck::VerifiedAndUpgraded(new_hash)) => {
                        player.password = new_hash;
                        tracing::info!("[{}] Player '{username}' password upgraded to hash", market_name());
                        drop(inner);
                        self.flush();
                        Ok(username.to_string())
                    }
                    Err(_) => {
                        tracing::warn!("[{}] Player '{username}': wrong password", market_name());
                        Err(AuthError::UserExistsWrongPassword {
                            username: username.to_string(),
                        })
                    }
                }
            }
            None => {
                tracing::info!(
                    "[{}] Registering new player '{username}' with {INITIAL_TOKENS} tokens",
                    market_name()
                );
                let password_hash = hash_password(password)
                    .map_err(|_| AuthError::PasswordHashFailed)?;
                inner
                    .players
                    .insert(username.to_string(), Player::new(username.to_string(), password_hash));
                drop(inner);
                self.flush();
                Ok(username.to_string())
            }
        }
    }

    /// Ensure a player record exists for the given username.
    ///
    /// Returns `true` if a new record was created.
    pub fn ensure_player_exists(&self, username: &str) -> bool {
        let username = username.trim();
        if username.is_empty() {
            return false;
        }

        let mut inner = self.inner.lock().unwrap();
        if inner.players.contains_key(username) {
            return false;
        }

        let password_hash = hash_password("admin-session-placeholder")
            .unwrap_or_else(|_| "admin-session-placeholder".to_string());
        inner.players.insert(
            username.to_string(),
            Player::new(username.to_string(), password_hash),
        );

        drop(inner);
        self.flush();
        true
    }

    /// Return all open portfolio lots for a player, oldest first.
    ///
    /// Returns an empty vec if the player has no lots or the pool is unavailable.
    pub fn get_portfolio(&self, username: &str) -> Vec<PortfolioLot> {
        let pool = {
            let inner = self.inner.lock().unwrap();
            inner.pool.clone()
        };
        let Some(pool) = pool else {
            return Vec::new();
        };
        block_on_storage(load_portfolio_lots_for_user(&pool, username))
            .unwrap_or_default()
    }

    /// Return a holdings summary (symbol → total remaining quantity) derived
    /// from open portfolio lots for a player.
    pub fn get_holdings_summary(&self, username: &str) -> HashMap<String, HoldingSummary> {
        let mut total_qty: HashMap<String, f64> = HashMap::new();
        let mut total_cost: HashMap<String, f64> = HashMap::new();

        for lot in self.get_portfolio(username) {
            *total_qty.entry(lot.symbol.clone()).or_insert(0.0) += lot.quantity;
            *total_cost.entry(lot.symbol).or_insert(0.0) += lot.quantity * lot.price;
        }

        total_qty
            .into_iter()
            .filter_map(|(symbol, quantity)| {
                if quantity <= 0.0 {
                    return None;
                }

                let avg_price = total_cost.get(&symbol).copied().unwrap_or(0.0) / quantity;
                Some((
                    symbol,
                    HoldingSummary {
                        quantity,
                        avg_price,
                    },
                ))
            })
            .collect()
    }

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

            let Some(owner_username) = Self::resolve_username_from_sender_id(&inner, sender_id) else {
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
    pub fn record_connection(&self, username: &str, ip: Option<&str>) {
        let mut inner = self.inner.lock().unwrap();
        let Some(player) = inner.players.get_mut(username) else {
            return;
        };

        player.connection_count = player.connection_count.saturating_add(1);

        if let Some(raw_ip) = ip {
            let value = raw_ip.trim();
            if !value.is_empty() && !player.ips.iter().any(|existing| existing == value) {
                player.ips.push(value.to_string());
            }
        }

        drop(inner);
        self.flush();
    }

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
                tracing::error!("[{}] Failed to clear portfolio lots on market reset: {e}", market_name());
            }
        }

        (players_touched, orders_removed)
    }

    /// Apply a FIX execution report to player balances and pending orders.
    ///
    /// Returns `true` if any player state was updated.
    pub fn apply_fix_execution_report(&self, fix_body: &str) -> bool {
        let fields = parse_fix_fields(fix_body);

        if fields.get("35").map(String::as_str) != Some("8") {
            return false;
        }

        let ord_status = fields.get("39").map(String::as_str).unwrap_or("");
        let cl_ord_id = fields
            .get("41")
            .or_else(|| fields.get("11"))
            .map(String::as_str)
            .unwrap_or("");

        if cl_ord_id.is_empty() {
            return false;
        }

        let exec_id = fields.get("17").cloned().unwrap_or_else(|| {
            format!(
                "{}:{}:{}:{}:{}",
                cl_ord_id,
                ord_status,
                fields.get("32").map(String::as_str).unwrap_or("0"),
                fields.get("31").map(String::as_str).unwrap_or("0"),
                fields.get("151").map(String::as_str).unwrap_or("0")
            )
        });

        let last_qty = parse_f64(fields.get("32").map(String::as_str));
        let last_px = parse_f64(fields.get("31").map(String::as_str));
        let leaves_qty = parse_f64(fields.get("151").map(String::as_str));
        let side = fields.get("54").map(String::as_str).unwrap_or("");
        let symbol = fields
            .get("55")
            .map(|s| s.trim().to_uppercase())
            .unwrap_or_default();

        let mut changed = false;
        let mut inner = self.inner.lock().unwrap();

        if !inner.processed_exec_ids.insert(exec_id) {
            return false;
        }

        // Capture owner now — before it may be removed from order_owners below.
        let owner = inner.order_owners.get(cl_ord_id).cloned();

        // Portfolio lot operation to perform after releasing the lock.
        // We determine it here while `owner` is still available.
        let lot_op: Option<(String, String, String, f64, f64)> =
            if let Some(ref uname) = owner {
                if (ord_status == "1" || ord_status == "2")
                    && last_qty > 0.0
                    && last_px > 0.0
                    && !symbol.is_empty()
                    && (side == "1" || side == "2")
                {
                    Some((uname.clone(), side.to_string(), symbol.clone(), last_qty, last_px))
                } else {
                    None
                }
            } else {
                None
            };

        if let Some(owner_username) = owner {
            if let Some(player) = inner.players.get_mut(&owner_username) {
                if (ord_status == "1" || ord_status == "2") && last_qty > 0.0 && last_px > 0.0 {
                    let traded_notional = last_qty * last_px;
                    if side == "1" {
                        player.tokens -= traded_notional;
                    } else if side == "2" {
                        player.tokens += traded_notional;
                    }
                    changed = true;
                }

                if let Some(pos) = player
                    .pending_orders
                    .iter()
                    .position(|o| o.cl_ord_id == cl_ord_id)
                {
                    match ord_status {
                        "2" | "3" | "4" | "8" | "C" => {
                            player.pending_orders.remove(pos);
                            changed = true;
                        }
                        "1" => {
                            if leaves_qty > 0.0 {
                                player.pending_orders[pos].qty = leaves_qty;
                                changed = true;
                            } else if last_qty > 0.0 {
                                let current = player.pending_orders[pos].qty;
                                player.pending_orders[pos].qty = (current - last_qty).max(0.0);
                                changed = true;
                            }
                        }
                        _ => {}
                    }
                }
            }

            if matches!(ord_status, "2" | "3" | "4" | "8" | "C") {
                inner.order_owners.remove(cl_ord_id);
                changed = true;
            }
        }

        drop(inner);
        if changed {
            self.flush();
        }

        // Perform portfolio lot insert (buy) or FIFO consume (sell) after flush.
        if let Some((uname, lot_side, lot_symbol, lot_qty, lot_px)) = lot_op {
            let pool = {
                let inner = self.inner.lock().unwrap();
                inner.pool.clone()
            };
            if let Some(pool) = pool {
                if lot_side == "1" {
                    // Buy: insert a new lot.
                    if let Err(e) = block_on_storage(insert_portfolio_lot(&pool, &uname, &lot_symbol, lot_qty, lot_px)) {
                        tracing::error!("[{}] Failed to insert portfolio lot for {uname}: {e}", market_name());
                    }
                } else if lot_side == "2" {
                    // Sell: FIFO consume from existing lots.
                    if let Err(e) = block_on_storage(consume_portfolio_lots_fifo(&pool, &uname, &lot_symbol, lot_qty)) {
                        tracing::error!("[{}] Failed to consume portfolio lots for {uname}: {e}", market_name());
                    }
                }
            }
        }

        changed
    }

    /// Apply the passive side of a trade reconstructed from the multicast
    /// market-data feed. This closes the gap where a resting owner's FIX TCP
    /// connection no longer exists, so their execution report is never
    /// delivered back over the original response queue.
    pub fn apply_trade_from_feed(
        &self,
        trade_id: u64,
        passive_cl_ord_id: &str,
        symbol: &str,
        quantity: f64,
        price: f64,
    ) -> bool {
        let passive_cl_ord_id = passive_cl_ord_id.trim();
        let symbol = symbol.trim().to_uppercase();

        if passive_cl_ord_id.is_empty()
            || symbol.is_empty()
            || !quantity.is_finite()
            || quantity <= 0.0
            || !price.is_finite()
            || price <= 0.0
        {
            return false;
        }

        let exec_id = format!("feed:{trade_id}:{passive_cl_ord_id}");
        let notional = quantity * price;

        let mut changed = false;
        let mut inner = self.inner.lock().unwrap();

        if !inner.processed_exec_ids.insert(exec_id) {
            return false;
        }

        let owner = inner.order_owners.get(passive_cl_ord_id).cloned();

        let lot_op = if let Some(ref owner_username) = owner {
            Some((owner_username.clone(), symbol.clone(), quantity))
        } else {
            None
        };

        if let Some(owner_username) = owner {
            if let Some(player) = inner.players.get_mut(&owner_username) {
                player.tokens += notional;

                if let Some(pos) = player
                    .pending_orders
                    .iter()
                    .position(|o| o.cl_ord_id == passive_cl_ord_id)
                {
                    let current = player.pending_orders[pos].qty;
                    let remaining = (current - quantity).max(0.0);
                    if remaining <= 1e-9 {
                        player.pending_orders.remove(pos);
                        inner.order_owners.remove(passive_cl_ord_id);
                    } else {
                        player.pending_orders[pos].qty = remaining;
                    }
                }

                changed = true;
            }
        }

        drop(inner);
        if changed {
            self.flush();
        }

        if let Some((owner_username, lot_symbol, lot_qty)) = lot_op {
            let pool = {
                let inner = self.inner.lock().unwrap();
                inner.pool.clone()
            };
            if let Some(pool) = pool {
                if let Err(e) = block_on_storage(consume_portfolio_lots_fifo(&pool, &owner_username, &lot_symbol, lot_qty)) {
                    tracing::error!("[{}] Failed to consume passive-side portfolio lots for {owner_username}: {e}", market_name());
                }
            }
        }

        changed
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

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Persist the current in-memory state to PostgreSQL. This is called after every state change to ensure durability and consistency between the in-memory state and the database.
    /// Can be expensive, use batching for optimization if needed.
    fn flush(&self) {
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
                    tracing::error!("[{}] Failed to persist player data to PostgreSQL after {} attempts: {e}", market_name(), MAX_RETRIES);
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
    let player_rows = query(
        "SELECT username, password, tokens, connection_count, ips_json FROM players"
    )
    .fetch_all(pool)
    .await?;

    let mut players = HashMap::new();
    for row in player_rows {
        let username: String = row.try_get("username")?;
        let password: String = row.try_get("password")?;
        let tokens: f64 = row.try_get("tokens")?;
        let connection_count_raw: i64 = row.try_get("connection_count")?;
        let ips_json: String = row.try_get("ips_json")?;

        let ips = serde_json::from_str::<Vec<String>>(&ips_json)
            .unwrap_or_default();

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
        "SELECT meta_value_bigint FROM player_store_meta WHERE meta_key = 'total_visitor_count'"
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

async fn load_portfolio_lots_for_user(pool: &PgPool, username: &str) -> Result<Vec<PortfolioLot>, sqlx::Error> {
    let rows = query(
        "SELECT id, username, symbol, quantity, price, purchased_at \
         FROM portfolio_lots WHERE username = $1 ORDER BY purchased_at ASC"
    )
    .bind(username)
    .fetch_all(pool)
    .await?;

    let mut lots = Vec::with_capacity(rows.len());
    for row in rows {
        lots.push(PortfolioLot {
            id: row.try_get("id")?,
            username: row.try_get("username")?,
            symbol: row.try_get("symbol")?,
            quantity: row.try_get("quantity")?,
            price: row.try_get("price")?,
            purchased_at: row.try_get("purchased_at")?,
        });
    }
    Ok(lots)
}

async fn insert_portfolio_lot(
    pool: &PgPool,
    username: &str,
    symbol: &str,
    quantity: f64,
    price: f64,
) -> Result<(), sqlx::Error> {
    query(
        "INSERT INTO portfolio_lots (username, symbol, quantity, price) VALUES ($1, $2, $3, $4)"
    )
    .bind(username)
    .bind(symbol)
    .bind(quantity)
    .bind(price)
    .execute(pool)
    .await?;
    Ok(())
}

/// FIFO consumption: reduce the oldest lots first for the given symbol.
async fn consume_portfolio_lots_fifo(
    pool: &PgPool,
    username: &str,
    symbol: &str,
    mut sell_qty: f64,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    let rows = query(
        "SELECT id, quantity FROM portfolio_lots \
         WHERE username = $1 AND symbol = $2 \
         ORDER BY purchased_at ASC \
         FOR UPDATE"
    )
    .bind(username)
    .bind(symbol)
    .fetch_all(&mut *tx)
    .await?;

    for row in rows {
        if sell_qty <= 1e-9 {
            break;
        }
        let lot_id: i64 = row.try_get("id")?;
        let lot_qty: f64 = row.try_get("quantity")?;

        if sell_qty >= lot_qty - 1e-9 {
            // Consume entire lot.
            query("DELETE FROM portfolio_lots WHERE id = $1")
                .bind(lot_id)
                .execute(&mut *tx)
                .await?;
            sell_qty -= lot_qty;
        } else {
            // Partially consume lot.
            query("UPDATE portfolio_lots SET quantity = quantity - $1 WHERE id = $2")
                .bind(sell_qty)
                .bind(lot_id)
                .execute(&mut *tx)
                .await?;
            sell_qty = 0.0;
        }
    }

    tx.commit().await
}

async fn delete_all_portfolio_lots(pool: &PgPool) -> Result<(), sqlx::Error> {
    query("DELETE FROM portfolio_lots").execute(pool).await?;
    Ok(())
}

fn parse_fix_fields(body: &str) -> HashMap<String, String> {
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

fn parse_f64(v: Option<&str>) -> f64 {
    v.and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0)
}

enum PasswordCheck {
    Verified,
    VerifiedAndUpgraded(String),
}

fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("hash error: {e}"))
}

fn verify_or_upgrade_password(stored: &str, provided: &str) -> Result<PasswordCheck, String> {
    if stored.starts_with("$argon2") {
        let parsed_hash = PasswordHash::new(stored)
            .map_err(|e| format!("invalid hash format: {e}"))?;
        Argon2::default()
            .verify_password(provided.as_bytes(), &parsed_hash)
            .map(|_| PasswordCheck::Verified)
            .map_err(|_| "password mismatch".to_string())
    } else if stored == provided {
        let new_hash = hash_password(provided)?;
        Ok(PasswordCheck::VerifiedAndUpgraded(new_hash))
    } else {
        Err("password mismatch".to_string())
    }
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

                let buyer_fill = "35=8 │ 39=2 │ 11=BOBBUY1 │ 54=1 │ 55=AAPL │ 31=100 │ 32=5 │ 151=0 │ 17=E-BUY-1";
                assert!(store.apply_fix_execution_report(buyer_fill));

                let seller_fill = "35=8 │ 39=2 │ 11=ALICESELL1 │ 54=2 │ 55=AAPL │ 31=100 │ 32=5 │ 151=0 │ 17=E-SELL-1";
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
