use crate::auth::admin_password;
use crate::server::{AppState, Metrics};
use axum::{Extension, Json, extract::State, http::StatusCode, response::IntoResponse};
use players::generate_token;
use serde::Deserialize;
use std::sync::Arc;
use utils::market_name;

/// POST /api/login — body: `{ "username": "...", "password": "..." }`
/// Returns 200 `{ token, username }` on success, 401 `{ error }` on failure.
#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Handle login requests. Supports both admin login (with MARKET_SIMULATOR_ADMIN_PWD env var)
/// and player login (with username/password registration or authentication).
/// Returns a token issued by the Player Service.
pub async fn api_login_handler(
    State(state): State<AppState>,
    Extension(metrics): Extension<Arc<Metrics>>,
    Json(body): Json<LoginRequest>,
) -> impl IntoResponse {
    let start_time = std::time::Instant::now();

    // Track login attempt
    metrics
        .login_attempts
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let admin_login = body.username.eq_ignore_ascii_case("admin")
        && admin_password().as_deref() == Some(body.password.as_str());

    if admin_login {
        match state
            .player_client
            .lock()
            .await
            .authenticate_or_register("admin", &body.password)
            .await
        {
            Ok(_) => {
                // Track successful login
                metrics
                    .login_success
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                // Generate admin JWT token (with is_admin=true)
                let admin_token = match generate_token("admin", true) {
                    Ok(token) => token,
                    Err(e) => {
                        tracing::error!(
                            "[{}] Failed to generate admin JWT token: {}",
                            market_name(),
                            e
                        );
                        // Record failed login
                        metrics
                            .login_failure
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(serde_json::json!({
                                "error": "Failed to generate token",
                            })),
                        )
                            .into_response();
                    }
                };

                // Record latency sample (milliseconds)
                let elapsed_ms = start_time.elapsed().as_millis() as u64;
                if let Ok(mut samples) = metrics.login_latency_ms.lock() {
                    samples.push(elapsed_ms);
                }

                tracing::info!("[{}] Admin authenticated", market_name());
                return (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "token": admin_token,
                        "username": "admin",
                        "is_admin": true
                    })),
                )
                    .into_response();
            }
            Err(e) => {
                // Track failed login
                metrics
                    .login_failure
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                // Record latency sample even for failures (milliseconds)
                let elapsed_ms = start_time.elapsed().as_millis() as u64;
                if let Ok(mut samples) = metrics.login_latency_ms.lock() {
                    samples.push(elapsed_ms);
                }

                return (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({
                        "error": format!("Admin authentication failed: {}", e),
                    })),
                )
                    .into_response();
            }
        }
    }

    match state
        .player_client
        .lock()
        .await
        .authenticate_or_register(&body.username, &body.password)
        .await
    {
        Ok(auth_result) => {
            // Track successful login
            metrics
                .login_success
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            // Record latency sample (milliseconds)
            let elapsed_ms = start_time.elapsed().as_millis() as u64;
            if let Ok(mut samples) = metrics.login_latency_ms.lock() {
                samples.push(elapsed_ms);
            }

            tracing::info!(
                "[{}] Player '{}' authenticated",
                market_name(),
                auth_result.username
            );
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "token": auth_result.token,
                    "username": auth_result.username,
                    "is_admin": auth_result.is_admin
                })),
            )
                .into_response()
        }
        Err(e) => {
            // Track failed login
            metrics
                .login_failure
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            // Record latency sample even for failures (milliseconds)
            let elapsed_ms = start_time.elapsed().as_millis() as u64;
            if let Ok(mut samples) = metrics.login_latency_ms.lock() {
                samples.push(elapsed_ms);
            }

            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": e,
                })),
            )
                .into_response()
        }
    }
}

pub async fn api_trades_handler(State(state): State<AppState>) -> impl IntoResponse {
    let trades_queue = state.trades_queue.lock().unwrap();
    let trades: Vec<crate::state::TradeView> = trades_queue.iter().cloned().collect();

    Json(serde_json::json!({
        "success": true,
        "trades": trades,
    }))
}
