//! PlayerClient with gRPC connection pooling and optimization.
//!
//! # Connection Pooling Strategy
//!
//! The PlayerClient uses tonic's built-in connection pooling mechanisms:
//!
//! ## HTTP/2 Multiplexing
//! - A single underlying HTTP/2 connection can carry multiple concurrent streams
//! - Each gRPC call uses a separate stream on the same connection
//! - No per-request TCP connection overhead
//!
//! ## Connection Reuse
//! - The `Channel` is shared across all PlayerClient instances
//! - Multiple clones of PlayerClient reference the same underlying connection
//! - Wrapped in `Arc<tokio::sync::Mutex<>>` for thread-safe access
//!
//! ## Keepalive Configuration
//! - Interval: 15 seconds (prevents idle connection drops)
//! - Timeout: 5 seconds (allows detection of dead connections)
//! - Idle keepalive enabled (maintains connection even when no requests)
//!
//! ## Timeout Strategy
//! - Connection timeout: 5 seconds (fail fast if server is unreachable)
//! - Per-request timeout: 30 seconds (sufficient for most operations)
//!
//! # Usage
//!
//! ```ignore
//! // Single client connects once, then is cloned for use across the app
//! let client = Arc::new(tokio::sync::Mutex::new(
//!     PlayerClient::connect("http://[::1]:50052").await?
//! ));
//!
//! // Each component that needs the client gets an Arc clone (cheap clone)
//! // No additional connections are created
//! state.player_client = client;
//! ```

use players::pb::players::v1::{
    player_service_client::PlayerServiceClient, AuthRequest, GetPlayerStateRequest,
    UpdatePendingOrdersRequest, ResetTokensRequest, ApplyFixExecutionReportRequest,
    RecordVisitRequest, UpdateVisitorsRequest, ResetMarketStateRequest, ResetAllTokensRequest,
};
use players::players::{PendingOrder, HoldingSummary};
use std::collections::HashMap;
use std::time::Duration;
use tonic::transport::{Channel, Endpoint};
use tracing::error;

/// Result of player authentication, including the issued token.
#[derive(Clone, Debug)]
pub struct AuthenticationResult {
    pub token: String,
    pub username: String,
    pub is_admin: bool,
    pub id_suffix: String,
}

/// Wrapper around the gRPC PlayerServiceClient with connection pooling.
/// This allows the backend to communicate with the standalone player-server microservice.
/// 
/// Features:
/// - Connection reuse via shared Channel
/// - Configurable timeouts and keepalive
/// - Error handling with graceful degradation
#[derive(Clone)]
pub struct PlayerClient {
    client: PlayerServiceClient<Channel>,
}

impl PlayerClient {
    /// Create a new PlayerClient with optimized connection settings.
    /// 
    /// Configures:
    /// - Connection timeout: 5 seconds
    /// - Request timeout: 30 seconds
    /// - Keepalive interval: 15 seconds
    /// - Keepalive timeout: 5 seconds
    /// - Connection pooling via shared Channel
    ///
    /// Example: "http://[::1]:50052"
    pub async fn connect(addr: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let endpoint = Endpoint::try_from(addr.to_string())?
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .keep_alive_while_idle(true)
            .http2_keep_alive_interval(Duration::from_secs(15))
            .keep_alive_timeout(Duration::from_secs(5));

        let channel = endpoint.connect().await?;
        let client = PlayerServiceClient::new(channel);
        
        tracing::info!("PlayerClient connected to {}", addr);
        Ok(Self { client })
    }

    /// Create a new PlayerClient from an existing Channel.
    /// Useful for sharing a channel across multiple client instances (connection pooling).
    pub fn from_channel(channel: Channel) -> Self {
        Self {
            client: PlayerServiceClient::new(channel),
        }
    }

    /// Authenticate a player or register them if they don't exist.
    /// Returns the username on success.
    pub async fn authenticate_or_register(
        &mut self,
        username: &str,
        password: &str,
    ) -> Result<AuthenticationResult, String> {
        let request = AuthRequest {
            username: username.to_string(),
            password: password.to_string(),
        };

        match self.client.authenticate(request).await {
            Ok(response) => {
                let auth_resp = response.into_inner();
                if auth_resp.success {
                    Ok(AuthenticationResult {
                        token: auth_resp.token,
                        username: auth_resp.username,
                        is_admin: auth_resp.is_admin,
                        id_suffix: auth_resp.id_suffix,
                    })
                } else {
                    Err(auth_resp.error_message)
                }
            }
            Err(e) => {
                error!("gRPC authenticate error: {}", e);
                Err(format!("Authentication service unavailable: {}", e))
            }
        }
    }

    /// Get a player's full state (tokens, pending orders, holdings).
    pub async fn get_player_state(&mut self, username: &str) -> Option<PlayerStateView> {
        let request = GetPlayerStateRequest {
            username: username.to_string(),
            token: String::new(), // TODO: pass actual token if needed
        };

        match self.client.get_player_state(request).await {
            Ok(response) => {
                let state = response.into_inner();
                Some(PlayerStateView {
                    username: state.username,
                    tokens: state.tokens,
                    pending_orders: state
                        .pending_orders
                        .into_iter()
                        .map(|o| PendingOrder {
                            cl_ord_id: o.cl_ord_id,
                            symbol: o.symbol,
                            side: o.side,
                            qty: o.qty,
                            price: o.price,
                        })
                        .collect(),
                    holdings: state
                        .holdings
                        .into_iter()
                        .map(|(symbol, h)| {
                            (
                                symbol,
                                HoldingSummary {
                                    quantity: h.quantity,
                                    avg_price: h.avg_price,
                                },
                            )
                        })
                        .collect(),
                    order_owners: state.order_owners,
                    id_suffix: state.id_suffix,
                    visitor_count: state.visitor_count as usize,
                    total_visitor_count: state.total_visitor_count as usize,
                })
            }
            Err(e) => {
                error!("gRPC get_player_state error: {}", e);
                None
            }
        }
    }

    /// Get holdings for a player.
    pub async fn get_holdings_summary(&mut self, username: &str) -> HashMap<String, HoldingSummary> {
        match self.get_player_state(username).await {
            Some(state) => state.holdings,
            None => HashMap::new(),
        }
    }

    /// Get order owners mapping (clord_id -> username).
    pub async fn get_order_owners(&mut self) -> HashMap<String, String> {
        // This requires a new RPC method in players service to return all order owners
        // For now, return empty map - TODO: implement in service
        HashMap::new()
    }

    /// Add a pending order for a player.
    pub async fn add_pending_order(&mut self, username: &str, order: PendingOrder) -> bool {
        let request = UpdatePendingOrdersRequest {
            username: username.to_string(),
            pending_orders: vec![players::pb::players::v1::PendingOrder {
                cl_ord_id: order.cl_ord_id,
                symbol: order.symbol,
                side: order.side,
                qty: order.qty,
                price: order.price,
            }],
        };

        match self.client.update_pending_orders(request).await {
            Ok(response) => response.into_inner().success,
            Err(e) => {
                error!("gRPC add_pending_order error: {}", e);
                false
            }
        }
    }

    /// Remove a pending order for a player.
    pub async fn remove_pending_order(&mut self, username: &str, clord_id: &str) -> bool {
        // This requires filtering the pending orders
        // Get current state, remove the order, and update
        match self.get_player_state(username).await {
            Some(mut state) => {
                state.pending_orders.retain(|o| o.cl_ord_id != clord_id);
                let request = UpdatePendingOrdersRequest {
                    username: username.to_string(),
                    pending_orders: state
                        .pending_orders
                        .into_iter()
                        .map(|o| players::pb::players::v1::PendingOrder {
                            cl_ord_id: o.cl_ord_id,
                            symbol: o.symbol,
                            side: o.side,
                            qty: o.qty,
                            price: o.price,
                        })
                        .collect(),
                };

                match self.client.update_pending_orders(request).await {
                    Ok(response) => response.into_inner().success,
                    Err(e) => {
                        error!("gRPC remove_pending_order error: {}", e);
                        false
                    }
                }
            }
            None => false,
        }
    }

    /// Reset all tokens for a player.
    pub async fn reset_tokens(&mut self, username: &str) {
        let request = ResetTokensRequest {
            username: username.to_string(),
            token: String::new(),
        };

        if let Err(e) = self.client.reset_tokens(request).await {
            error!("gRPC reset_tokens error: {}", e);
        }
    }

    /// Record a visitor connection and return all-time visitor count.
    pub async fn record_visit(&mut self) -> usize {
        match self.client.record_visit(RecordVisitRequest {}).await {
            Ok(response) => response.into_inner().total_visitor_count as usize,
            Err(e) => {
                error!("record_visit RPC failed: {:?}", e);
                0
            }
        }
    }

    /// Get current active visitor count.
    pub async fn active_visitors(&mut self) -> usize {
        // Active visitor count is maintained by the backend, not persisted in player service
        // This is a placeholder - the actual count is in the market's atomic counter
        0
    }

    /// Get total all-time visitor count.
    pub async fn total_visitors(&mut self) -> usize {
        // Would need a GetVisitors RPC to implement - for now, just maintain locally
        0
    }

    /// Update visitor counts in the player service (for persistence across restarts).
    pub async fn update_visitors(&mut self, active: i32, total: i32) -> Result<(), String> {
        match self.client.update_visitors(UpdateVisitorsRequest {
            active_count: active,
            total_count: total,
            token: String::new(), // No token required for internal backend calls
        }).await {
            Ok(_) => Ok(()),
            Err(e) => {
                error!("update_visitors RPC failed: {:?}", e);
                Err(format!("update_visitors failed: {}", e))
            }
        }
    }

    /// Reset market state (admin command).
    pub async fn reset_market_state(&mut self) -> (usize, usize) {
        match self.client.reset_market_state(ResetMarketStateRequest {
            token: String::new(), // Token validation happens in the backend
        }).await {
            Ok(response) => {
                let resp = response.into_inner();
                (resp.players_touched as usize, resp.orders_removed as usize)
            }
            Err(e) => {
                tracing::error!("[{}] reset_market_state RPC failed: {}", utils::market_name(), e);
                (0, 0)
            }
        }
    }

    /// Reset all tokens (admin command).
    pub async fn reset_all_tokens(&mut self) -> usize {
        match self.client.reset_all_tokens(ResetAllTokensRequest {
            token: String::new(), // Token validation happens in the backend
        }).await {
            Ok(response) => {
                let resp = response.into_inner();
                resp.players_reset as usize
            }
            Err(e) => {
                tracing::error!("[{}] reset_all_tokens RPC failed: {}", utils::market_name(), e);
                0
            }
        }
    }

    /// Apply a FIX execution report to update player portfolio.
    /// This is called from the FIX session reader when an execution report is received.
    pub async fn apply_fix_execution_report(&mut self, username: &str, fix_body: &str) -> Result<(), String> {
        tracing::debug!(
            "[{}] Applying FIX execution report for '{}': {}",
            utils::market_name(),
            username,
            fix_body
        );
        
        let request = ApplyFixExecutionReportRequest {
            username: username.to_string(),
            fix_body: fix_body.to_string(),
        };
        
        match self.client.apply_fix_execution_report(request).await {
            Ok(response) => {
                let result = response.into_inner();
                if result.success {
                    tracing::info!("[{}] FIX execution report applied for '{}'", utils::market_name(), username);
                    Ok(())
                } else {
                    let msg = format!("FIX execution report failed: {}", result.error_message);
                    tracing::error!("[{}] {}", utils::market_name(), msg);
                    Err(msg)
                }
            }
            Err(e) => {
                let msg = format!("gRPC call failed: {}", e);
                tracing::error!("[{}] apply_fix_execution_report error: {}", utils::market_name(), msg);
                Err(msg)
            }
        }
    }
}

/// A view of player state returned by the gRPC service.
#[derive(Debug, Clone)]
pub struct PlayerStateView {
    pub username: String,
    pub tokens: f64,
    pub pending_orders: Vec<PendingOrder>,
    pub holdings: HashMap<String, HoldingSummary>,
    pub order_owners: HashMap<String, String>,
    pub id_suffix: String,
    pub visitor_count: usize,
    pub total_visitor_count: usize,
}
