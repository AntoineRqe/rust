pub mod timestamp;
pub mod functions;
pub mod traits;

pub use timestamp::UtcTimestamp;
pub use functions::*;
pub use traits::*;

// Utility functions and traits shared across multiple crates.
// Return the market name from the environment variable `MARKET_NAME`, or "unknown" if not set. The result is cached in a `OnceLock` for efficient repeated access.
pub fn market_name() -> &'static str {
    static MARKET_NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    MARKET_NAME
        .get_or_init(|| std::env::var("MARKET_NAME").unwrap_or_else(|_| "unknown".to_string()))
        .as_str()
}


/// Combine two 32-bit integers into a single 64-bit key, with `client_id` in the high 32 bits and `order_id` in the low 32 bits.
#[inline(always)]
pub fn make_key(client_id: u32, order_id: u32) -> u64 {
    ((client_id as u64) << 32) | (order_id as u64)
}