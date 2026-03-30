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
pub const INITIAL_TOKENS: f64 = 100_000.0;

// ── Stored player record ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Player {
    pub username: String,
    /// Plaintext password (suitable for a local simulator; hash it for production use).
    pub password: String,
    /// Remaining token balance.
    pub tokens: f64,
    /// Orders currently resting in the order book (not yet filled or cancelled).
    pub pending_orders: Vec<PendingOrder>,
}

impl Player {
    pub fn new(username: String, password_hash: String) -> Self {
        Self {
            username,
            password: password_hash,
            tokens: INITIAL_TOKENS,
            pending_orders: Vec::new(),
        }
    }
}

// ── Serialisation wrapper ─────────────────────────────────────────────────────

#[derive(Debug, Default, Serialize, Deserialize)]
struct StorageData {
    players: HashMap<String, Player>,
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
    processed_exec_ids: HashSet<String>,
    path: PathBuf,
}

impl PlayerStore {
    /// Load the store from `path`, creating an empty one if the file does not exist yet.
    pub fn load(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let players = if path.exists() {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            serde_json::from_str::<StorageData>(&text)
                .unwrap_or_default()
                .players
        } else {
            HashMap::new()
        };
        tracing::info!(
            "[{}] Player store: {} player(s) loaded from {:?}",
            market_name(),
            players.len(),
            path
        );
        PlayerStore {
            inner: Arc::new(Mutex::new(StoreInner {
                players,
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
    ) -> Result<String, String> {
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
                        Err("Wrong password".into())
                    }
                }
            }
            None => {
                tracing::info!(
                    "[{}] Registering new player '{username}' with {INITIAL_TOKENS} tokens",
                    market_name()
                );
                let password_hash = hash_password(password)
                    .map_err(|_| "Password hashing failed")?;
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
        if let Some(player) = inner.players.get_mut(username) {
            player.pending_orders.push(order);
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

        let mut changed = false;
        let mut inner = self.inner.lock().unwrap();

        if !inner.processed_exec_ids.insert(exec_id) {
            return false;
        }

        for player in inner.players.values_mut() {
            if let Some(pos) = player
                .pending_orders
                .iter()
                .position(|o| o.cl_ord_id == cl_ord_id)
            {
                let side = player.pending_orders[pos].side.clone();

                if (ord_status == "1" || ord_status == "2") && last_qty > 0.0 && last_px > 0.0 {
                    let traded_notional = last_qty * last_px;
                    if side == "1" {
                        player.tokens -= traded_notional;
                    } else if side == "2" {
                        player.tokens += traded_notional;
                    }
                    changed = true;
                }

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

                break;
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
