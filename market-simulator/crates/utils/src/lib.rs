pub mod functions;
pub mod timestamp;
pub mod traits;

pub use functions::*;
pub use timestamp::UtcTimestamp;
pub use traits::*;

static MARKET_NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();

// Utility functions and traits shared across multiple crates.
// Initialize the market name from runtime configuration.
pub fn set_market_name(name: &str) {
    if MARKET_NAME.get().is_none() {
        let _ = MARKET_NAME.set(name.to_string());
    }
}

// Return the configured market name. Falls back to `MARKET_NAME` env var or "unknown".
pub fn market_name() -> &'static str {
    MARKET_NAME
        .get_or_init(|| std::env::var("MARKET_NAME").unwrap_or_else(|_| "unknown".to_string()))
        .as_str()
}

/// Combine two 32-bit integers into a single 64-bit key, with `client_id` in the high 32 bits and `order_id` in the low 32 bits.
#[inline(always)]
pub fn make_key(client_id: u32, order_id: u32) -> u64 {
    ((client_id as u64) << 32) | (order_id as u64)
}
