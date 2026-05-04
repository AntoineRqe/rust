use serde::{Deserialize, Serialize};
use crate::state::EventBus;

/// Information for an authenticated player.
/// Token validation is now delegated to the Player Service (gRPC).
#[derive(Clone, Debug)]
pub struct SessionInfo {
    /// The username associated with this session (e.g. "alice").
    pub username: String,
    /// Whether this session belongs to the admin user (which has special privileges).
    pub is_admin: bool,
}

/// Retrieve the admin password from the environment variable.
pub fn admin_password() -> Option<String> {
    std::env::var("MARKET_SIMULATOR_ADMIN_PWD")
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// Check if public markets only mode is enabled via environment variable.
fn public_markets_only_enabled() -> bool {
    std::env::var("MARKET_SIM_PUBLIC_MARKETS_ONLY")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Check if a host is a private IPv4 address.
fn is_private_ipv4(host: &str) -> bool {
    if host.starts_with("10.") || host.starts_with("192.168.") || host.starts_with("169.254.") {
        return true;
    }

    if let Some(rest) = host.strip_prefix("172.") {
        if let Some(octet_str) = rest.split('.').next() {
            if let Ok(octet) = octet_str.parse::<u8>() {
                return (16..=31).contains(&octet);
            }
        }
    }

    false
}

/// Check if a market URL is publicly accessible (not localhost or private).
fn is_public_market_url(raw_url: &str) -> bool {
    let trimmed = raw_url.trim();
    if trimmed.is_empty() {
        return false;
    }

    let without_scheme = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .unwrap_or(trimmed);

    let host_port = without_scheme.split('/').next().unwrap_or_default().trim();
    if host_port.is_empty() {
        return false;
    }

    let host = if let Some(rest) = host_port.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default()
    } else {
        host_port.split(':').next().unwrap_or_default()
    }
    .trim()
    .to_ascii_lowercase();

    if host.is_empty() {
        return false;
    }

    if matches!(
        host.as_str(),
        "localhost" | "127.0.0.1" | "::1" | "0.0.0.0" | "host.docker.internal"
    ) {
        return false;
    }

    if is_private_ipv4(&host) {
        return false;
    }

    true
}

/// Information about a single market, served to the login page.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarketInfo {
    pub name: String,
    pub url: String,
}

/// Filter markets to only include publicly accessible URLs if public markets only mode is enabled.
pub fn advertised_markets(markets: &[MarketInfo]) -> Vec<MarketInfo> {
    if !public_markets_only_enabled() {
        return markets.to_vec();
    }

    markets
        .iter()
        .filter(|market| is_public_market_url(&market.url))
        .cloned()
        .collect()
}

/// Check if a user is an admin and publish an error message if they are not.
pub fn require_admin(
    bus: &EventBus,
    username: &str,
    is_admin: bool,
) -> bool {
    if is_admin {
        return true;
    }

    bus.publish(crate::state::WsEvent::FixMessage {
        label: "ADMIN ONLY".into(),
        body: "This action is reserved to the admin user.".into(),
        tag: "err".into(),
        recipient: Some(username.to_string()),
    });
    false
}
