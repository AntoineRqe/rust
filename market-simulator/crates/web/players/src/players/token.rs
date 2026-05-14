use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

/// JWT claims for player session tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenClaims {
    pub username: String,
    pub is_admin: bool,
    pub issued_at: u64,
    pub exp: u64, // Expiration time (unix timestamp)
}

/// Get or create the JWT secret from environment.
/// Falls back to development default if not set.
pub fn get_jwt_secret() -> String {
    std::env::var("JWT_SECRET").unwrap_or_else(|_| {
        tracing::warn!(
            "JWT_SECRET not set, using development default. Set JWT_SECRET env var for production."
        );
        "development-default-secret-change-me-in-production".to_string()
    })
}

/// Generate a signed JWT bearer token for a player session.
///
/// Tokens are signed with HS256 algorithm and contain:
/// - username
/// - is_admin flag
/// - issued_at timestamp
/// - exp (expiration: 24 hours from now)
pub fn generate_token(username: &str, is_admin: bool) -> Result<String, String> {
    let secret = get_jwt_secret();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("time error: {}", e))?
        .as_secs();

    // Token expires in 24 hours
    let exp = now + (24 * 60 * 60);

    let claims = TokenClaims {
        username: username.to_string(),
        is_admin,
        issued_at: now,
        exp,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| format!("token generation failed: {}", e))
}

/// Validate a JWT bearer token and extract claims.
pub fn validate_token(token: &str) -> Result<TokenClaims, String> {
    let secret = get_jwt_secret();
    decode::<TokenClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map(|data| data.claims)
    .map_err(|e| format!("token validation failed: {}", e))
}

/// Extract ID suffix from password hash (last 4 characters, uppercase).
///
/// The ID suffix is used in ClOrdID generation to make order IDs unique
/// and tied to the player's password. This prevents order ID collisions
/// across different players and sessions.
///
/// If the hash is too short, use what's available.
pub fn extract_id_suffix(password_hash: &str) -> String {
    if password_hash.len() >= 4 {
        password_hash
            .chars()
            .rev()
            .take(4)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>()
            .to_uppercase()
    } else {
        password_hash.to_uppercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_generation() {
        let token1 = generate_token("alice", false).expect("token generation failed");
        let token2 = generate_token("alice", false).expect("token generation failed");

        // Tokens should be different (different issued_at timestamps)
        // (unless they happen in the exact same nanosecond, extremely unlikely)
        // But they should both be valid JWTs
        assert!(token1.contains('.'));
        assert!(token2.contains('.'));

        // Should be able to decode them
        let claims1 = validate_token(&token1).expect("validation failed");
        let claims2 = validate_token(&token2).expect("validation failed");

        assert_eq!(claims1.username, "alice");
        assert_eq!(claims2.username, "alice");
        assert!(!claims1.is_admin);
        assert!(!claims2.is_admin);
    }

    #[test]
    fn test_admin_flag() {
        let token = generate_token("admin", true).expect("token generation failed");
        let claims = validate_token(&token).expect("validation failed");

        assert_eq!(claims.username, "admin");
        assert!(claims.is_admin);
    }

    #[test]
    fn test_token_validation() {
        let token = generate_token("bob", false).expect("token generation failed");
        let claims = validate_token(&token).expect("validation failed");

        assert_eq!(claims.username, "bob");
        assert!(!claims.is_admin);
    }

    #[test]
    fn test_invalid_token() {
        let result = validate_token("invalid.token.here");
        assert!(result.is_err());
    }

    #[test]
    fn test_tampered_token() {
        let token = generate_token("alice", false).expect("token generation failed");

        // Try to tamper with the token
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() == 3 {
            let tampered = format!("{}.{}.tampered", parts[0], parts[1]);
            let result = validate_token(&tampered);
            assert!(result.is_err(), "Tampered token should be rejected");
        }
    }

    #[test]
    fn test_id_suffix_extraction() {
        // Normal case
        assert_eq!(extract_id_suffix("hash0123456789"), "6789");

        // Shorter hash
        assert_eq!(extract_id_suffix("ABC"), "ABC");

        // Single char
        assert_eq!(extract_id_suffix("X"), "X");

        // Empty
        assert_eq!(extract_id_suffix(""), "");

        // Uppercase conversion
        assert_eq!(extract_id_suffix("abcdefgh"), "EFGH");
    }
}
