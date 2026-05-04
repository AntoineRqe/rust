use rand_core::OsRng;

/// Generate a unique bearer token for a player session.
/// 
/// Tokens are 32-character hex strings generated from cryptographic randomness.
/// Format: 32 random hex characters (128 bits)
pub fn generate_token() -> String {
    let mut bytes = [0u8; 16];
    use rand_core::RngCore;
    OsRng.fill_bytes(&mut bytes);
    format!("{:032x}", u128::from_le_bytes(bytes))
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
        let token1 = generate_token();
        let token2 = generate_token();
        
        assert_eq!(token1.len(), 32);
        assert_eq!(token2.len(), 32);
        assert_ne!(token1, token2); // Should be different each time
        
        // Should be valid hex
        assert!(token1.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(token2.chars().all(|c| c.is_ascii_hexdigit()));
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
