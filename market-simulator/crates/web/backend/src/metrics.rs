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
use crate::server::{AppState, Metrics};

/// Histogram buckets for latency metrics (milliseconds)
const LATENCY_BUCKETS_MS: &[u64] = &[1, 5, 10, 25, 50, 100, 250, 500, 1000];

/// Calculate histogram bucket counts and sum from samples
fn calculate_histogram_stats(samples: &[u64]) -> (Vec<(u64, u64)>, u64, u64) {
    let mut buckets = Vec::new();
    let mut sum = 0u64;
    let count = samples.len() as u64;
    
    // Calculate sum
    for &sample in samples {
        sum += sample;
    }
    
    // Calculate bucket counts (cumulative)
    for &bucket_bound in LATENCY_BUCKETS_MS {
        let bucket_count = samples.iter().filter(|&&s| s <= bucket_bound).count() as u64;
        buckets.push((bucket_bound, bucket_count));
    }
    
    (buckets, sum, count)
}

/// Initialize Prometheus registry with default settings
pub fn create_metrics_registry() -> Registry {
    Registry::default()
}

/// Handler to expose metrics in Prometheus text format
/// This is called by Prometheus scraper at /metrics endpoint
pub async fn metrics_handler(
    Extension(registry): Extension<Arc<Registry>>,
    Extension(metrics): Extension<Arc<Metrics>>,
    State(state): State<AppState>,
) -> Response {
    // Encode all metrics in Prometheus text format
    let mut buffer = String::new();
    
    // Add business metrics from AppState
    buffer.push_str("# HELP login_attempts_total Total login attempts\n");
    buffer.push_str("# TYPE login_attempts_total counter\n");
    buffer.push_str(&format!("login_attempts_total {}\n", 
        metrics.login_attempts.load(std::sync::atomic::Ordering::Relaxed)));
    
    buffer.push_str("# HELP login_success_total Successful logins\n");
    buffer.push_str("# TYPE login_success_total counter\n");
    buffer.push_str(&format!("login_success_total {}\n", 
        metrics.login_success.load(std::sync::atomic::Ordering::Relaxed)));
    
    buffer.push_str("# HELP login_failure_total Failed login attempts\n");
    buffer.push_str("# TYPE login_failure_total counter\n");
    buffer.push_str(&format!("login_failure_total {}\n", 
        metrics.login_failure.load(std::sync::atomic::Ordering::Relaxed)));
    
    buffer.push_str("# HELP order_events_total Total order events processed\n");
    buffer.push_str("# TYPE order_events_total counter\n");
    buffer.push_str(&format!("order_events_total {}\n", 
        metrics.order_events.load(std::sync::atomic::Ordering::Relaxed)));
    
    buffer.push_str("# HELP trades_total Total trades executed\n");
    buffer.push_str("# TYPE trades_total counter\n");
    buffer.push_str(&format!("trades_total {}\n", 
        metrics.trades.load(std::sync::atomic::Ordering::Relaxed)));
    
    buffer.push_str("# HELP cancel_orders_total Total cancel orders submitted\n");
    buffer.push_str("# TYPE cancel_orders_total counter\n");
    buffer.push_str(&format!("cancel_orders_total {}\n", 
        metrics.cancel_orders.load(std::sync::atomic::Ordering::Relaxed)));
    
    // Export login latency histogram
    buffer.push_str("# HELP login_latency_ms Login latency in milliseconds\n");
    buffer.push_str("# TYPE login_latency_ms histogram\n");
    
    let login_samples = metrics.login_latency_ms.lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    let (login_buckets, login_sum, login_count) = calculate_histogram_stats(&login_samples);
    
    for (bucket_bound, count) in login_buckets {
        buffer.push_str(&format!("login_latency_ms_bucket{{le=\"{}\"}} {}\n", bucket_bound, count));
    }
    buffer.push_str(&format!("login_latency_ms_bucket{{le=\"+Inf\"}} {}\n", login_count));
    buffer.push_str(&format!("login_latency_ms_sum {}\n", login_sum));
    buffer.push_str(&format!("login_latency_ms_count {}\n", login_count));
    
    // Export order latency histogram
    buffer.push_str("# HELP order_latency_ms Order submission latency in milliseconds\n");
    buffer.push_str("# TYPE order_latency_ms histogram\n");
    
    let order_samples = metrics.order_latency_ms.lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    let (order_buckets, order_sum, order_count) = calculate_histogram_stats(&order_samples);
    
    for (bucket_bound, count) in order_buckets {
        buffer.push_str(&format!("order_latency_ms_bucket{{le=\"{}\"}} {}\n", bucket_bound, count));
    }
    buffer.push_str(&format!("order_latency_ms_bucket{{le=\"+Inf\"}} {}\n", order_count));
    buffer.push_str(&format!("order_latency_ms_sum {}\n", order_sum));
    buffer.push_str(&format!("order_latency_ms_count {}\n", order_count));
    
    // Export execution latency histogram
    buffer.push_str("# HELP execution_latency_ms Execution/trade latency in milliseconds\n");
    buffer.push_str("# TYPE execution_latency_ms histogram\n");
    
    let exec_samples = metrics.execution_latency_ms.lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    let (exec_buckets, exec_sum, exec_count) = calculate_histogram_stats(&exec_samples);
    
    for (bucket_bound, count) in exec_buckets {
        buffer.push_str(&format!("execution_latency_ms_bucket{{le=\"{}\"}} {}\n", bucket_bound, count));
    }
    buffer.push_str(&format!("execution_latency_ms_bucket{{le=\"+Inf\"}} {}\n", exec_count));
    buffer.push_str(&format!("execution_latency_ms_sum {}\n", exec_sum));
    buffer.push_str(&format!("execution_latency_ms_count {}\n", exec_count));
    
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
