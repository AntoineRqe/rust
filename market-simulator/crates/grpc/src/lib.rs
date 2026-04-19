use crossbeam_channel::Sender;
use order_book::OrderBookControl;
use sqlx::PgPool;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tonic::{transport::Server, Request, Response, Status};

// Include the generated protobuf/gRPC bindings.
pub mod proto {
    tonic::include_proto!("market_control");
}

use proto::{
    market_control_server::{MarketControl, MarketControlServer},
    DumpOrderBookRequest, DumpOrderBookResponse, PendingOrder, ResetRequest, ResetResponse,
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

    /// Returns all currently pending orders persisted in the database.
    async fn dump_order_book(
        &self,
        _request: Request<DumpOrderBookRequest>,
    ) -> Result<Response<DumpOrderBookResponse>, Status> {
        tracing::info!("gRPC DumpOrderBook called");

        match db::collect_all_pending_orders(&self.db_pool).await {
            Ok(pending_orders) => {
                let orders = pending_orders
                    .into_iter()
                    .map(|order| PendingOrder {
                        price: order.price.to_f64(),
                        quantity: order.quantity.to_f64(),
                        side: order.side.to_string(),
                        symbol: order.symbol.to_string(),
                        order_type: order.order_type.to_string(),
                        cl_ord_id: order.cl_ord_id.to_string(),
                        orig_cl_ord_id: order
                            .orig_cl_ord_id
                            .map(|id| id.to_string())
                            .unwrap_or_default(),
                        sender_id: order.sender_id.to_string(),
                        target_id: order.target_id.to_string(),
                        timestamp: order.timestamp,
                    })
                    .collect();

                Ok(Response::new(DumpOrderBookResponse {
                    success: true,
                    message: "Pending orders retrieved successfully".to_string(),
                    orders,
                }))
            }
            Err(e) => {
                let msg = format!("Failed to fetch pending orders: {e}");
                tracing::error!("{msg}");
                Ok(Response::new(DumpOrderBookResponse {
                    success: false,
                    message: msg,
                    orders: Vec::new(),
                }))
            }
        }
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
    shutdown: Arc<AtomicBool>,
) -> Result<(), tonic::transport::Error> {
    tracing::info!("gRPC MarketControl server listening on {addr}");
    Server::builder()
        .add_service(MarketControlServer::new(service))
        .serve_with_shutdown(addr, async move {
            while !shutdown.load(Ordering::Relaxed) {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            tracing::info!("gRPC shutdown signal received");
        })
        .await
}
