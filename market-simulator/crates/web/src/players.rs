use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use argon2::password_hash::SaltString;
use rand_core::OsRng;
use serde::{Deserialize, Serialize};

use crate::state::PendingOrder;

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
    /// Equity inventory held by the player (symbol -> quantity).
    #[serde(default)]
    pub holdings: HashMap<String, f64>,
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
            holdings: HashMap::new(),
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
}

// ── PlayerStore ───────────────────────────────────────────────────────────────

/// Thread-safe player registry backed by a JSON file.
/// Cloning is cheap — the inner state is Arc-backed.
#[derive(Clone)]
pub struct PlayerStore {
    inner: Arc<Mutex<StoreInner>>,
}

struct StoreInner {
    players: HashMap<String, Player>,
    order_owners: HashMap<String, String>,
    processed_exec_ids: HashSet<String>,
    path: PathBuf,
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

    /// Load the store from `path`, creating an empty one if the file does not exist yet.
    pub fn load(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let storage = if path.exists() {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            serde_json::from_str::<StorageData>(&text)
                .unwrap_or_default()
        } else {
            StorageData::default()
        };
        let players = storage.players;
        let order_owners = storage.order_owners;
        tracing::info!(
            "[{}] Player store: {} player(s) loaded from {:?}",
            market_name(),
            players.len(),
            path
        );
        PlayerStore {
            inner: Arc::new(Mutex::new(StoreInner {
                players,
                order_owners,
                processed_exec_ids: HashSet::new(),
                path,
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
    /// Returns `(players_touched, orders_removed, holdings_cleared)`.
    pub fn reset_market_state(&self) -> (usize, usize, usize) {
        let mut inner = self.inner.lock().unwrap();
        let mut players_touched = 0usize;
        let mut orders_removed = 0usize;
        let mut holdings_cleared = 0usize;

        for player in inner.players.values_mut() {
            let had_pending = !player.pending_orders.is_empty();
            let had_holdings = !player.holdings.is_empty();

            if !player.pending_orders.is_empty() {
                orders_removed += player.pending_orders.len();
                player.pending_orders.clear();
            }

            if !player.holdings.is_empty() {
                holdings_cleared += player.holdings.len();
                player.holdings.clear();
            }

            if had_pending || had_holdings {
                players_touched += 1;
            }
        }

        inner.order_owners.clear();

        inner.processed_exec_ids.clear();
        drop(inner);

        // Always persist to keep on-disk state aligned with the market reset.
        self.flush();
        (players_touched, orders_removed, holdings_cleared)
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
        let candidate_sender = fields
            .get("56")
            .or_else(|| fields.get("49"))
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

        if !cl_ord_id.is_empty() {
            if let Some(owner_username) = Self::resolve_username_from_sender_id(&inner, candidate_sender) {
                let replaced = inner
                    .order_owners
                    .insert(cl_ord_id.to_string(), owner_username.clone());
                if replaced.as_deref() != Some(owner_username.as_str()) {
                    changed = true;
                }
            }
        }

        let owner = inner.order_owners.get(cl_ord_id).cloned();
        if let Some(owner_username) = owner {
            if let Some(player) = inner.players.get_mut(&owner_username) {
                if (ord_status == "1" || ord_status == "2") && last_qty > 0.0 && last_px > 0.0 {
                    let traded_notional = last_qty * last_px;
                    if side == "1" {
                        player.tokens -= traded_notional;
                        if !symbol.is_empty() {
                            let entry = player.holdings.entry(symbol.clone()).or_insert(0.0);
                            *entry += last_qty;
                        }
                    } else if side == "2" {
                        player.tokens += traded_notional;
                        if !symbol.is_empty() {
                            if let Some(owned) = player.holdings.get_mut(&symbol) {
                                *owned = (*owned - last_qty).max(0.0);
                                if *owned <= 1e-9 {
                                    player.holdings.remove(&symbol);
                                }
                            }
                        }
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
        changed
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn flush(&self) {
        let inner = self.inner.lock().unwrap();
        let data = StorageData {
            players: inner.players.clone(),
            order_owners: inner.order_owners.clone(),
        };
        match serde_json::to_string_pretty(&data) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&inner.path, json) {
                    tracing::error!(
                        "[{}] Failed to persist player data to {:?}: {e}",
                        market_name(),
                        inner.path
                    );
                }
            }
            Err(e) => tracing::error!("[{}] Failed to serialise player data: {e}", market_name()),
        }
    }
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
        use std::fs;
        use std::time::{SystemTime, UNIX_EPOCH};

        fn unique_temp_path(prefix: &str) -> PathBuf {
                let nanos = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_nanos())
                        .unwrap_or(0);
                std::env::temp_dir().join(format!("{prefix}-{}-{nanos}.json", std::process::id()))
        }

        #[test]
        fn trade_updates_both_participants_without_pending_orders_loaded() {
                let path = unique_temp_path("market-sim-players");

                let seed = r#"{
    "players": {
        "alice": {
            "username": "alice",
            "password": "pw",
            "tokens": 10000.0,
            "holdings": { "AAPL": 10.0 },
            "connection_count": 0,
            "ips": []
        },
        "bob": {
            "username": "bob",
            "password": "pw",
            "tokens": 10000.0,
            "holdings": {},
            "connection_count": 0,
            "ips": []
        }
    },
    "order_owners": {
        "ALICESELL1": "alice",
        "BOBBUY1": "bob"
    }
}"#;
                fs::write(&path, seed).expect("seed players.json");

                let store = PlayerStore::load(&path);

                // Buyer fill (bob buys 5 @ 100): tokens down, holdings up.
                let buyer_fill = "35=8 │ 39=2 │ 11=BOBBUY1 │ 54=1 │ 55=AAPL │ 31=100 │ 32=5 │ 151=0 │ 17=E-BUY-1";
                assert!(store.apply_fix_execution_report(buyer_fill));

                // Seller fill (alice sells 5 @ 100): tokens up, holdings down.
                let seller_fill = "35=8 │ 39=2 │ 11=ALICESELL1 │ 54=2 │ 55=AAPL │ 31=100 │ 32=5 │ 151=0 │ 17=E-SELL-1";
                assert!(store.apply_fix_execution_report(seller_fill));

                let alice = store.get_player("alice").expect("alice exists");
                let bob = store.get_player("bob").expect("bob exists");

                assert!((alice.tokens - 10500.0).abs() < 1e-9);
                assert!((bob.tokens - 9500.0).abs() < 1e-9);
                assert!((alice.holdings.get("AAPL").copied().unwrap_or(0.0) - 5.0).abs() < 1e-9);
                assert!((bob.holdings.get("AAPL").copied().unwrap_or(0.0) - 5.0).abs() < 1e-9);

                // Terminal fills should clear owner mappings.
                let owners = store.get_order_owners();
                assert!(!owners.contains_key("ALICESELL1"));
                assert!(!owners.contains_key("BOBBUY1"));

                let _ = fs::remove_file(path);
        }
}
