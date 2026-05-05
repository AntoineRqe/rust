/// Prometheus metrics collection and export for the web server
/// 
/// Provides:
/// - Automatic HTTP request metrics (count, latency, status codes)
/// - Custom business metrics (orders, executions, etc.)
/// - /metrics endpoint for Prometheus scraping

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension, 
    extract::State,
};
use prometheus_client::encoding::text::encode;
use prometheus_client::registry::Registry;
use std::sync::Arc;
use crate::server::AppState;

/// Initialize Prometheus registry with default settings
pub fn create_metrics_registry() -> Registry {
    Registry::default()
}

/// Handler to expose metrics in Prometheus text format
/// This is called by Prometheus scraper at /metrics endpoint
pub async fn metrics_handler(
    Extension(registry): Extension<Arc<Registry>>,
    State(state): State<AppState>,
) -> Response {
    // Encode all metrics in Prometheus text format
    let mut buffer = String::new();
    
    // Add business metrics from AppState
    buffer.push_str("# HELP login_attempts_total Total login attempts\n");
    buffer.push_str("# TYPE login_attempts_total counter\n");
    buffer.push_str(&format!("login_attempts_total {}\n", 
        state.metrics.login_attempts.load(std::sync::atomic::Ordering::Relaxed)));
    
    buffer.push_str("# HELP login_success_total Successful logins\n");
    buffer.push_str("# TYPE login_success_total counter\n");
    buffer.push_str(&format!("login_success_total {}\n", 
        state.metrics.login_success.load(std::sync::atomic::Ordering::Relaxed)));
    
    buffer.push_str("# HELP login_failure_total Failed login attempts\n");
    buffer.push_str("# TYPE login_failure_total counter\n");
    buffer.push_str(&format!("login_failure_total {}\n", 
        state.metrics.login_failure.load(std::sync::atomic::Ordering::Relaxed)));
    
    buffer.push_str("# HELP order_events_total Total order events processed\n");
    buffer.push_str("# TYPE order_events_total counter\n");
    buffer.push_str(&format!("order_events_total {}\n", 
        state.metrics.order_events.load(std::sync::atomic::Ordering::Relaxed)));
    
    buffer.push_str("# HELP trades_total Total trades executed\n");
    buffer.push_str("# TYPE trades_total counter\n");
    buffer.push_str(&format!("trades_total {}\n", 
        state.metrics.trades.load(std::sync::atomic::Ordering::Relaxed)));
    
    buffer.push_str("# HELP cancel_orders_total Total cancel orders submitted\n");
    buffer.push_str("# TYPE cancel_orders_total counter\n");
    buffer.push_str(&format!("cancel_orders_total {}\n", 
        state.metrics.cancel_orders.load(std::sync::atomic::Ordering::Relaxed)));
    
    buffer.push_str("# HELP websocket_connections Active WebSocket connections\n");
    buffer.push_str("# TYPE websocket_connections gauge\n");
    buffer.push_str(&format!("websocket_connections {}\n", 
        state.active_visitors.load(std::sync::atomic::Ordering::Relaxed)));
    
    buffer.push_str("# HELP total_visitors_ever Total visitors (all-time)\n");
    buffer.push_str("# TYPE total_visitors_ever counter\n");
    buffer.push_str(&format!("total_visitors_ever {}\n", 
        state.total_visitors.load(std::sync::atomic::Ordering::Relaxed)));
    
    match encode(&mut buffer, &registry) {
        Ok(()) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            buffer,
        )
            .into_response(),
        Err(e) => {
            tracing::error!("Failed to encode metrics: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to encode metrics").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_metrics_registry() {
        let registry = create_metrics_registry();
        // Just verify it can be created without panic
        assert!(!std::mem::size_of_val(&registry) == 0);
    }
}
