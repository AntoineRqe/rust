use std::sync::Arc;
use tonic::{Request, Response, Status};

use crate::pb::players::v1::*;
use crate::players::{PlayerStore, generate_token, extract_id_suffix};
use utils::market_name;

pub struct PlayerServiceImpl {
    store: Arc<PlayerStore>,
}

impl PlayerServiceImpl {
    pub fn new(store: Arc<PlayerStore>) -> Self {
        Self { store }
    }
}

#[tonic::async_trait]
impl player_service_server::PlayerService for PlayerServiceImpl {
    async fn authenticate(
        &self,
        request: Request<AuthRequest>,
    ) -> Result<Response<AuthResponse>, Status> {
        let req = request.into_inner();

        if req.username.is_empty() {
            return Ok(Response::new(AuthResponse {
                success: false,
                token: String::new(),
                id_suffix: String::new(),
                username: String::new(),
                is_admin: false,
                error_code: AuthErrorCode::UsernameRequired as i32,
                error_message: "Username is required".to_string(),
            }));
        }

        if req.password.is_empty() {
            return Ok(Response::new(AuthResponse {
                success: false,
                token: String::new(),
                id_suffix: String::new(),
                username: String::new(),
                is_admin: false,
                error_code: AuthErrorCode::PasswordRequired as i32,
                error_message: "Password is required".to_string(),
            }));
        }

        match self.store.authenticate_or_register(&req.username, &req.password) {
            Ok(username) => {
                // Get player record to extract id_suffix from password hash
                let id_suffix = if let Some(player) = self.store.get_player(&username) {
                    extract_id_suffix(&player.password)
                } else {
                    String::new()
                };
                
                // Generate a unique bearer token for this session
                let token = generate_token();
                
                Ok(Response::new(AuthResponse {
                    success: true,
                    token,
                    id_suffix,
                    username: username.clone(),
                    is_admin: false,
                    error_code: AuthErrorCode::Unspecified as i32,
                    error_message: String::new(),
                }))
            }
            Err(e) => Ok(Response::new(AuthResponse {
                success: false,
                token: String::new(),
                id_suffix: String::new(),
                username: req.username,
                is_admin: false,
                error_code: match e {
                    crate::players::AuthError::UsernameRequired => AuthErrorCode::UsernameRequired as i32,
                    crate::players::AuthError::PasswordRequired => AuthErrorCode::PasswordRequired as i32,
                    crate::players::AuthError::UserExistsWrongPassword { .. } => AuthErrorCode::UserExistsWrongPassword as i32,
                    crate::players::AuthError::PasswordHashFailed => AuthErrorCode::PasswordHashFailed as i32,
                },
                error_message: e.message(),
            })),
        }
    }

    async fn get_player_state(
        &self,
        request: Request<GetPlayerStateRequest>,
    ) -> Result<Response<PlayerState>, Status> {
        let req = request.into_inner();

        match self.store.get_player(&req.username) {
            Some(player) => {
                let holdings = self.store.get_holdings_summary(&req.username);
                let holdings_pb = holdings
                    .into_iter()
                    .map(|(symbol, holding)| {
                        (
                            symbol.clone(),
                            Holding {
                                symbol,
                                quantity: holding.quantity,
                                avg_price: holding.avg_price,
                            },
                        )
                    })
                    .collect();

                let pending_orders = player
                    .pending_orders
                    .iter()
                    .map(|order| PendingOrder {
                        cl_ord_id: order.cl_ord_id.clone(),
                        symbol: order.symbol.clone(),
                        side: order.side.clone(),
                        qty: order.qty,
                        price: order.price,
                    })
                    .collect();

                let order_owners = self.store.get_order_owners();

                Ok(Response::new(PlayerState {
                    username: player.username.clone(),
                    tokens: player.tokens,
                    pending_orders,
                    holdings: holdings_pb,
                    order_owners,
                    is_admin: false,
                    visitor_count: 0,
                    total_visitor_count: 0,
                    id_suffix: "TEST".to_string(),
                }))
            }
            None => Err(Status::not_found("Player not found")),
        }
    }

    async fn update_pending_orders(
        &self,
        request: Request<UpdatePendingOrdersRequest>,
    ) -> Result<Response<UpdateResult>, Status> {
        let req = request.into_inner();

        // Clear existing orders and add new ones
        for order in req.pending_orders {
            let pending_order = crate::players::PendingOrder {
                cl_ord_id: order.cl_ord_id,
                symbol: order.symbol,
                side: order.side,
                qty: order.qty,
                price: order.price,
            };
            self.store.add_pending_order(&req.username, pending_order);
        }

        Ok(Response::new(UpdateResult {
            success: true,
            error_message: String::new(),
        }))
    }

    async fn add_trade(
        &self,
        request: Request<AddTradeRequest>,
    ) -> Result<Response<UpdateResult>, Status> {
        let req = request.into_inner();
        
        // Side "1" = buy, "2" = sell
        let is_buy = req.side == "1";
        let is_sell = req.side == "2";
        
        if !is_buy && !is_sell {
            return Ok(Response::new(UpdateResult {
                success: false,
                error_message: "Invalid side: must be '1' (buy) or '2' (sell)".to_string(),
            }));
        }
        
        // Get the pool for database operations
        let pool = {
            let inner = self.store.inner.lock().unwrap();
            inner.pool.clone()
        };
        
        if let Some(pool) = pool {
            if is_buy {
                // For buy trades, insert a new portfolio lot
                if let Err(e) = crate::players::portfolio::insert_portfolio_lot(
                    &pool,
                    &req.username,
                    &req.symbol.to_uppercase(),
                    req.quantity,
                    req.price,
                ).await {
                    return Ok(Response::new(UpdateResult {
                        success: false,
                        error_message: format!("Failed to add buy trade: {}", e),
                    }));
                }
            } else {
                // For sell trades, consume portfolio lots (FIFO)
                if let Err(e) = crate::players::portfolio::consume_portfolio_lots_fifo(
                    &pool,
                    &req.username,
                    &req.symbol.to_uppercase(),
                    req.quantity,
                ).await {
                    return Ok(Response::new(UpdateResult {
                        success: false,
                        error_message: format!("Failed to consume portfolio for sell: {}", e),
                    }));
                }
            }
        }
        
        // Update player's token balance: increase for sell, decrease for buy
        let notional = req.quantity * req.price;
        {
            let mut inner = self.store.inner.lock().unwrap();
            if let Some(player) = inner.players.get_mut(&req.username) {
                if is_buy {
                    player.tokens -= notional;
                    if player.tokens < 0.0 {
                        // Token balance went negative, log a warning
                        tracing::warn!(
                            "[{}] Player {} token balance negative after buy trade: {}",
                            market_name(),
                            req.username,
                            player.tokens
                        );
                    }
                } else {
                    player.tokens += notional;
                }
                
                // Remove the order from pending_orders if it exists
                if let Some(pos) = player.pending_orders.iter().position(|o| o.cl_ord_id == req.cl_ord_id) {
                    player.pending_orders.remove(pos);
                }
            }
            drop(inner);
        }
        
        self.store.flush();
        
        Ok(Response::new(UpdateResult {
            success: true,
            error_message: String::new(),
        }))
    }

    async fn apply_fix_execution_report(
        &self,
        request: Request<ApplyFixExecutionReportRequest>,
    ) -> Result<Response<UpdateResult>, Status> {
        let req = request.into_inner();
        let success = self.store.apply_fix_execution_report(&req.fix_body);
        
        Ok(Response::new(UpdateResult {
            success,
            error_message: if success {
                String::new()
            } else {
                "Failed to apply FIX execution report".to_string()
            },
        }))
    }

    async fn record_visit(
        &self,
        _request: Request<RecordVisitRequest>,
    ) -> Result<Response<RecordVisitResponse>, Status> {
        // Increment the all-time visitor count and return the new total
        let total = self.store.record_visit();
        
        Ok(Response::new(RecordVisitResponse {
            total_visitor_count: total as i64,
        }))
    }

    async fn reset_tokens(
        &self,
        request: Request<ResetTokensRequest>,
    ) -> Result<Response<UpdateResult>, Status> {
        let req = request.into_inner();
        self.store.reset_tokens(&req.username);

        Ok(Response::new(UpdateResult {
            success: true,
            error_message: String::new(),
        }))
    }

    async fn reset_seq(
        &self,
        request: Request<ResetSeqRequest>,
    ) -> Result<Response<UpdateResult>, Status> {
        let req = request.into_inner();
        
        // Reset FIX message sequence number for the player
        // Currently, sequence tracking is per-player in the backend's FIX session reader
        // This is a no-op in the player service, but we return success
        // The backend will handle actual sequence reset in its FIX session
        
        tracing::debug!(
            "[Players] ResetSeq requested for user: {}",
            req.username
        );
        
        Ok(Response::new(UpdateResult {
            success: true,
            error_message: String::new(),
        }))
    }

    async fn reset_market_state(
        &self,
        _request: Request<ResetMarketStateRequest>,
    ) -> Result<Response<ResetMarketStateResponse>, Status> {
        let (players_touched, orders_removed) = self.store.reset_market_state();
        
        tracing::info!(
            "[Players] Market reset: {} players touched, {} orders removed",
            players_touched,
            orders_removed
        );
        
        Ok(Response::new(ResetMarketStateResponse {
            players_touched: players_touched as i32,
            orders_removed: orders_removed as i32,
        }))
    }

    async fn reset_all_tokens(
        &self,
        _request: Request<ResetAllTokensRequest>,
    ) -> Result<Response<ResetAllTokensResponse>, Status> {
        let players_reset = self.store.reset_all_tokens();
        
        tracing::info!(
            "[Players] Reset all tokens: {} players updated",
            players_reset
        );
        
        Ok(Response::new(ResetAllTokensResponse {
            players_reset: players_reset as i32,
        }))
    }

    async fn update_visitors(
        &self,
        request: Request<UpdateVisitorsRequest>,
    ) -> Result<Response<UpdateResult>, Status> {
        let req = request.into_inner();
        
        // Update the total visitor count (active_count is managed by the backend)
        self.store.update_visitor_count(req.total_count);
        
        Ok(Response::new(UpdateResult {
            success: true,
            error_message: String::new(),
        }))
    }
}
