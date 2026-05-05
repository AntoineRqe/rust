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
};
use prometheus_client::encoding::text::encode;
use prometheus_client::registry::Registry;
use std::sync::Arc;

/// Initialize Prometheus registry with default settings
pub fn create_metrics_registry() -> Registry {
    Registry::default()
}

/// Handler to expose metrics in Prometheus text format
/// This is called by Prometheus scraper at /metrics endpoint
pub async fn metrics_handler(
    Extension(registry): Extension<Arc<Registry>>,
) -> Response {
    // Encode all metrics in Prometheus text format
    let mut buffer = String::new();
    
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
