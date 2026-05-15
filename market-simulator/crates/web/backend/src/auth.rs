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

/// Information about a single market, served to the login page.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarketInfo {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub public_url: Option<String>,
}

/// Markets advertised to the login page and gateway.
pub fn advertised_markets(markets: &[MarketInfo]) -> Vec<MarketInfo> {
    markets.to_vec()
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
