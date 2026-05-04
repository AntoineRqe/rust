use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use rand_core::OsRng;
use utils::market_name;

use super::{PlayerStore, Player, INITIAL_TOKENS};

/// Authentication error types.
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

/// Internal enum for password checking results.
pub enum PasswordCheck {
    Verified,
    VerifiedAndUpgraded(String),
}

/// Hash a plaintext password using Argon2.
pub fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("hash error: {e}"))
}

/// Verify a plaintext password against a stored hash.
/// Returns upgraded hash if stored password is plaintext (for backward compatibility).
pub fn verify_or_upgrade_password(stored: &str, provided: &str) -> Result<PasswordCheck, String> {
    if stored.starts_with("$argon2") {
        let parsed_hash =
            PasswordHash::new(stored).map_err(|e| format!("invalid hash format: {e}"))?;
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

impl PlayerStore {
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
            Some(player) => match verify_or_upgrade_password(&player.password, password) {
                Ok(PasswordCheck::Verified) => {
                    tracing::debug!("[{}] Player '{username}' authenticated", market_name());
                    Ok(username.to_string())
                }
                Ok(PasswordCheck::VerifiedAndUpgraded(new_hash)) => {
                    player.password = new_hash;
                    tracing::info!(
                        "[{}] Player '{username}' password upgraded to hash",
                        market_name()
                    );
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
            },
            None => {
                tracing::info!(
                    "[{}] Registering new player '{username}' with {INITIAL_TOKENS} tokens",
                    market_name()
                );
                let password_hash =
                    hash_password(password).map_err(|_| AuthError::PasswordHashFailed)?;
                inner.players.insert(
                    username.to_string(),
                    Player::new(username.to_string(), password_hash),
                );
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

    /// Record a player's connection (increment connection count and track IP).
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
}
