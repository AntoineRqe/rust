use crossbeam_channel::Sender;
use order_book::OrderBookControl;
use sqlx::PgPool;
use std::sync::Arc;
use tonic::{transport::Server, Request, Response, Status};

// Include the generated protobuf/gRPC bindings.
pub mod proto {
    tonic::include_proto!("market_control");
}

use proto::{
    market_control_server::{MarketControl, MarketControlServer},
    ResetRequest, ResetResponse,
};

/// gRPC service that exposes market-control operations.
#[derive(Clone)]
pub struct MarketControlService {
    /// Sends control messages to the order-book engine.
    ob_control_tx: Sender<OrderBookControl>,
    /// Database pool used to reset the persisted state.
    db_pool: Arc<PgPool>,
}

impl MarketControlService {
    pub fn new(ob_control_tx: Sender<OrderBookControl>, db_pool: Arc<PgPool>) -> Self {
        Self {
            ob_control_tx,
            db_pool,
        }
    }
}

#[tonic::async_trait]
impl MarketControl for MarketControlService {
    /// Resets both the in-memory order book and the database tables.
    async fn reset_market(
        &self,
        _request: Request<ResetRequest>,
    ) -> Result<Response<ResetResponse>, Status> {
        tracing::info!("gRPC ResetMarket called");

        // 1. Signal the order-book engine to reset.
        if let Err(e) = self.ob_control_tx.send(OrderBookControl::Reset) {
            let msg = format!("Failed to send reset to order book: {e}");
            tracing::error!("{msg}");
            return Ok(Response::new(ResetResponse {
                success: false,
                message: msg,
            }));
        }

        // 2. Reset the database.
        if let Err(e) = db::reset_database(&self.db_pool).await {
            let msg = format!("Failed to reset database: {e}");
            tracing::error!("{msg}");
            return Ok(Response::new(ResetResponse {
                success: false,
                message: msg,
            }));
        }

        tracing::info!("Market reset completed successfully");
        Ok(Response::new(ResetResponse {
            success: true,
            message: "Market reset completed successfully".to_string(),
        }))
    }
}

/// Start the gRPC server on the given address (e.g. `"[::1]:50051"`).
///
/// This is an async function intended to be spawned inside a Tokio runtime:
/// ```rust,ignore
/// tokio::spawn(grpc::serve(addr, service));
/// ```
pub async fn serve(
    addr: std::net::SocketAddr,
    service: MarketControlService,
) -> Result<(), tonic::transport::Error> {
    tracing::info!("gRPC MarketControl server listening on {addr}");
    Server::builder()
        .add_service(MarketControlServer::new(service))
        .serve(addr)
        .await
}
