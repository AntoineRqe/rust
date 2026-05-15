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
/// Histogram buckets for latency metrics (microseconds)
const LATENCY_BUCKETS_US: &[u64] = &[
    10, 25, 50, 100, 250, 500,
    1_000, 2_500, 5_000, 10_000,
    25_000, 50_000, 100_000, 250_000,
    500_000, 1_000_000, 2_000_000,
    5_000_000, 10_000_000,
];

/// Calculate histogram bucket counts and sum from samples
fn calculate_histogram_stats(samples: &[u64], bucket_bounds: &[u64]) -> (Vec<(u64, u64)>, u64, u64) {
    let mut buckets = Vec::new();
    let mut sum = 0u64;
    let count = samples.len() as u64;
    
    // Calculate sum
    for &sample in samples {
        sum += sample;
    }
    
    // Calculate bucket counts (cumulative)
    for &bucket_bound in bucket_bounds {
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
    let (login_buckets, login_sum, login_count) = calculate_histogram_stats(&login_samples, LATENCY_BUCKETS_MS);
    
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
    let (order_buckets, order_sum, order_count) = calculate_histogram_stats(&order_samples, LATENCY_BUCKETS_MS);
    
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
    let (exec_buckets, exec_sum, exec_count) = calculate_histogram_stats(&exec_samples, LATENCY_BUCKETS_MS);
    
    for (bucket_bound, count) in exec_buckets {
        buffer.push_str(&format!("execution_latency_ms_bucket{{le=\"{}\"}} {}\n", bucket_bound, count));
    }
    buffer.push_str(&format!("execution_latency_ms_bucket{{le=\"+Inf\"}} {}\n", exec_count));
    buffer.push_str(&format!("execution_latency_ms_sum {}\n", exec_sum));
    buffer.push_str(&format!("execution_latency_ms_count {}\n", exec_count));

    buffer.push_str("# HELP order_book_events_total Total order book events processed\n");
    buffer.push_str("# TYPE order_book_events_total counter\n");
    buffer.push_str(&format!("order_book_events_total {}\n",
        metrics.order_book_events.load(std::sync::atomic::Ordering::Relaxed)));

    buffer.push_str("# HELP order_book_event_to_fanout_latency_ms Order book latency from event dequeue to fan-out completion in milliseconds\n");
    buffer.push_str("# TYPE order_book_event_to_fanout_latency_ms histogram\n");

    let ob_samples = metrics.order_book_event_to_fanout_latency_ms.lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    let (ob_buckets, ob_sum, ob_count) = calculate_histogram_stats(&ob_samples, LATENCY_BUCKETS_MS);

    for (bucket_bound, count) in ob_buckets {
        buffer.push_str(&format!("order_book_event_to_fanout_latency_ms_bucket{{le=\"{}\"}} {}\n", bucket_bound, count));
    }
    buffer.push_str(&format!("order_book_event_to_fanout_latency_ms_bucket{{le=\"+Inf\"}} {}\n", ob_count));
    buffer.push_str(&format!("order_book_event_to_fanout_latency_ms_sum {}\n", ob_sum));
    buffer.push_str(&format!("order_book_event_to_fanout_latency_ms_count {}\n", ob_count));

    buffer.push_str("# HELP execution_report_events_total Total execution report events processed\n");
    buffer.push_str("# TYPE execution_report_events_total counter\n");
    buffer.push_str(&format!("execution_report_events_total {}\n",
        metrics.execution_report_events.load(std::sync::atomic::Ordering::Relaxed)));

    buffer.push_str("# HELP execution_report_event_to_fanout_latency_ms Execution report latency from event dequeue to fan-out completion in milliseconds\n");
    buffer.push_str("# TYPE execution_report_event_to_fanout_latency_ms histogram\n");

    let er_samples = metrics.execution_report_event_to_fanout_latency_ms.lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    let (er_buckets, er_sum, er_count) = calculate_histogram_stats(&er_samples, LATENCY_BUCKETS_MS);

    for (bucket_bound, count) in er_buckets {
        buffer.push_str(&format!("execution_report_event_to_fanout_latency_ms_bucket{{le=\"{}\"}} {}\n", bucket_bound, count));
    }
    buffer.push_str(&format!("execution_report_event_to_fanout_latency_ms_bucket{{le=\"+Inf\"}} {}\n", er_count));
    buffer.push_str(&format!("execution_report_event_to_fanout_latency_ms_sum {}\n", er_sum));
    buffer.push_str(&format!("execution_report_event_to_fanout_latency_ms_count {}\n", er_count));

    buffer.push_str("# HELP order_db_writes_total Total order events written to the database\n");
    buffer.push_str("# TYPE order_db_writes_total counter\n");
    buffer.push_str(&format!("order_db_writes_total {}\n",
        metrics.order_db_writes.load(std::sync::atomic::Ordering::Relaxed)));

    buffer.push_str("# HELP order_db_write_latency_ms Database write latency for order events in milliseconds\n");
    buffer.push_str("# TYPE order_db_write_latency_ms histogram\n");

    let db_samples = metrics.order_db_write_latency_ms.lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    let (db_buckets, db_sum, db_count) = calculate_histogram_stats(&db_samples, LATENCY_BUCKETS_MS);

    for (bucket_bound, count) in db_buckets {
        buffer.push_str(&format!("order_db_write_latency_ms_bucket{{le=\"{}\"}} {}\n", bucket_bound, count));
    }
    buffer.push_str(&format!("order_db_write_latency_ms_bucket{{le=\"+Inf\"}} {}\n", db_count));
    buffer.push_str(&format!("order_db_write_latency_ms_sum {}\n", db_sum));
    buffer.push_str(&format!("order_db_write_latency_ms_count {}\n", db_count));

    buffer.push_str("# HELP fix_requests_total Total FIX requests processed\n");
    buffer.push_str("# TYPE fix_requests_total counter\n");
    buffer.push_str(&format!("fix_requests_total {}\n",
        metrics.fix_requests.load(std::sync::atomic::Ordering::Relaxed)));

    buffer.push_str("# HELP fix_responses_total Total FIX responses delivered\n");
    buffer.push_str("# TYPE fix_responses_total counter\n");
    buffer.push_str(&format!("fix_responses_total {}\n",
        metrics.fix_responses.load(std::sync::atomic::Ordering::Relaxed)));

    buffer.push_str("# HELP fix_response_dropped_total Total FIX responses dropped because client response channel was full\n");
    buffer.push_str("# TYPE fix_response_dropped_total counter\n");
    buffer.push_str(&format!("fix_response_dropped_total {}\n",
        metrics.fix_response_dropped.load(std::sync::atomic::Ordering::Relaxed)));

    buffer.push_str("# HELP fix_request_to_response_latency_ms FIX request to response delivery latency in milliseconds\n");
    buffer.push_str("# TYPE fix_request_to_response_latency_ms histogram\n");

    let fix_samples = metrics.fix_request_to_response_latency_ms.lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    let (fix_buckets, fix_sum, fix_count) = calculate_histogram_stats(&fix_samples, LATENCY_BUCKETS_MS);

    for (bucket_bound, count) in fix_buckets {
        buffer.push_str(&format!("fix_request_to_response_latency_ms_bucket{{le=\"{}\"}} {}\n", bucket_bound, count));
    }
    buffer.push_str(&format!("fix_request_to_response_latency_ms_bucket{{le=\"+Inf\"}} {}\n", fix_count));
    buffer.push_str(&format!("fix_request_to_response_latency_ms_sum {}\n", fix_sum));
    buffer.push_str(&format!("fix_request_to_response_latency_ms_count {}\n", fix_count));

    buffer.push_str("# HELP player_api_calls_total Total backend to player service API calls\n");
    buffer.push_str("# TYPE player_api_calls_total counter\n");
    buffer.push_str(&format!("player_api_calls_total {}\n",
        metrics.player_api_calls.load(std::sync::atomic::Ordering::Relaxed)));

    buffer.push_str("# HELP player_api_errors_total Total failed backend to player service API calls\n");
    buffer.push_str("# TYPE player_api_errors_total counter\n");
    buffer.push_str(&format!("player_api_errors_total {}\n",
        metrics.player_api_errors.load(std::sync::atomic::Ordering::Relaxed)));

    buffer.push_str("# HELP player_api_latency_ms Backend to player service API call latency in milliseconds\n");
    buffer.push_str("# TYPE player_api_latency_ms histogram\n");

    let player_samples = metrics.player_api_latency_ms.lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    let (player_buckets, player_sum, player_count) = calculate_histogram_stats(&player_samples, LATENCY_BUCKETS_MS);

    for (bucket_bound, count) in player_buckets {
        buffer.push_str(&format!("player_api_latency_ms_bucket{{le=\"{}\"}} {}\n", bucket_bound, count));
    }
    buffer.push_str(&format!("player_api_latency_ms_bucket{{le=\"+Inf\"}} {}\n", player_count));
    buffer.push_str(&format!("player_api_latency_ms_sum {}\n", player_sum));
    buffer.push_str(&format!("player_api_latency_ms_count {}\n", player_count));

    buffer.push_str("# HELP ui_order_round_trip_latency_ms UI order round-trip latency from click to response in milliseconds\n");
    buffer.push_str("# TYPE ui_order_round_trip_latency_ms histogram\n");

    let ui_samples = metrics.ui_order_round_trip_latency_ms.lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    let (ui_buckets, ui_sum, ui_count) = calculate_histogram_stats(&ui_samples, LATENCY_BUCKETS_MS);

    for (bucket_bound, count) in ui_buckets {
        buffer.push_str(&format!("ui_order_round_trip_latency_ms_bucket{{le=\"{}\"}} {}\n", bucket_bound, count));
    }
    buffer.push_str(&format!("ui_order_round_trip_latency_ms_bucket{{le=\"+Inf\"}} {}\n", ui_count));
    buffer.push_str(&format!("ui_order_round_trip_latency_ms_sum {}\n", ui_sum));
    buffer.push_str(&format!("ui_order_round_trip_latency_ms_count {}\n", ui_count));

    buffer.push_str("# HELP websocket_fanout_to_browser_latency_us WebSocket fanout to browser latency in microseconds\n");
    buffer.push_str("# TYPE websocket_fanout_to_browser_latency_us histogram\n");

    let ws_samples = metrics.websocket_fanout_to_browser_latency_us.lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    let (ws_buckets, ws_sum, ws_count) = calculate_histogram_stats(&ws_samples, LATENCY_BUCKETS_US);

    for (bucket_bound, count) in ws_buckets {
        buffer.push_str(&format!("websocket_fanout_to_browser_latency_us_bucket{{le=\"{}\"}} {}\n", bucket_bound, count));
    }
    buffer.push_str(&format!("websocket_fanout_to_browser_latency_us_bucket{{le=\"+Inf\"}} {}\n", ws_count));
    buffer.push_str(&format!("websocket_fanout_to_browser_latency_us_sum {}\n", ws_sum));
    buffer.push_str(&format!("websocket_fanout_to_browser_latency_us_count {}\n", ws_count));

    buffer.push_str("# HELP websocket_fanout_order_book_levels Last observed order book size when broadcasting an order book event\n");
    buffer.push_str("# TYPE websocket_fanout_order_book_levels gauge\n");
    buffer.push_str(&format!(
        "websocket_fanout_order_book_levels {}\n",
        metrics
            .websocket_fanout_order_book_levels
            .load(std::sync::atomic::Ordering::Relaxed)
    ));

    buffer.push_str("# HELP websocket_lagged_events_total Total websocket events dropped because a browser lagged behind\n");
    buffer.push_str("# TYPE websocket_lagged_events_total counter\n");
    buffer.push_str(&format!(
        "websocket_lagged_events_total {}\n",
        metrics.websocket_lagged_events.load(std::sync::atomic::Ordering::Relaxed)
    ));

    buffer.push_str("# HELP websocket_send_latency_us WebSocket send latency in microseconds\n");
    buffer.push_str("# TYPE websocket_send_latency_us histogram\n");

    let ws_send_samples = metrics.websocket_send_latency_us.lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    let (ws_send_buckets, ws_send_sum, ws_send_count) =
        calculate_histogram_stats(&ws_send_samples, LATENCY_BUCKETS_US);

    for (bucket_bound, count) in ws_send_buckets {
        buffer.push_str(&format!("websocket_send_latency_us_bucket{{le=\"{}\"}} {}\n", bucket_bound, count));
    }
    buffer.push_str(&format!("websocket_send_latency_us_bucket{{le=\"+Inf\"}} {}\n", ws_send_count));
    buffer.push_str(&format!("websocket_send_latency_us_sum {}\n", ws_send_sum));
    buffer.push_str(&format!("websocket_send_latency_us_count {}\n", ws_send_count));

    buffer.push_str("# HELP websocket_player_state_send_latency_us Player state send latency in microseconds\n");
    buffer.push_str("# TYPE websocket_player_state_send_latency_us histogram\n");

    let ps_samples = metrics.websocket_player_state_send_latency_us.lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    let (ps_buckets, ps_sum, ps_count) = calculate_histogram_stats(&ps_samples, LATENCY_BUCKETS_US);

    for (bucket_bound, count) in ps_buckets {
        buffer.push_str(&format!("websocket_player_state_send_latency_us_bucket{{le=\"{}\"}} {}\n", bucket_bound, count));
    }
    buffer.push_str(&format!("websocket_player_state_send_latency_us_bucket{{le=\"+Inf\"}} {}\n", ps_count));
    buffer.push_str(&format!("websocket_player_state_send_latency_us_sum {}\n", ps_sum));
    buffer.push_str(&format!("websocket_player_state_send_latency_us_count {}\n", ps_count));
    
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
