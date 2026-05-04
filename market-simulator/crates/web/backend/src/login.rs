use axum::{
    response::{Html, IntoResponse, Redirect},
    extract::{State, Query},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use crate::server::AppState;
use crate::auth::admin_password;
use utils::market_name;

/// Serve the login page (always accessible, no auth required).
#[derive(Deserialize)]
pub struct LoginPageParams {
    #[allow(dead_code)]
    token: Option<String>,
}

/// POST /api/login — body: `{ "username": "...", "password": "..." }`
/// Returns 200 `{ token, username }` on success, 401 `{ error }` on failure.
#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Serve the login page. Always show the login page - no local token validation.
pub async fn login_page_handler(
    _state: State<AppState>,
    _params: Query<LoginPageParams>,
) -> impl IntoResponse {
    Html(frontend::LOGIN_HTML).into_response()
}

/// Redirect the root URL to the canonical app route.
pub async fn root_handler() -> impl IntoResponse {
    Redirect::to("/app")
}

/// Serve the trading terminal. Auth is enforced client-side via sessionStorage
/// token; the WebSocket upgrade enforces it server-side.
pub async fn app_handler(State(state): State<AppState>) -> impl IntoResponse {
    use crate::auth::advertised_markets;
    
    let market = market_name();
    let login_gateway_url = std::env::var("LOGIN_GATEWAY_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:9875".to_string());
    let markets_json = serde_json::to_string(&advertised_markets(&state.known_markets))
        .unwrap_or_else(|_| "[]".to_string());
    let html = frontend::APP_HTML
        .replace("{{MARKET_NAME}}", market)
        .replace("{{LOGIN_GATEWAY_URL}}", &login_gateway_url)
        .replace("{{CURRENT_MARKET_NAME}}", market)
        .replace("{{MARKETS_JSON}}", &markets_json);
    Html(html)
}

/// Handle login requests. Supports both admin login (with MARKET_SIMULATOR_ADMIN_PWD env var)
/// and player login (with username/password registration or authentication).
/// Returns a token issued by the Player Service.
pub async fn api_login_handler(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> impl IntoResponse {
    let admin_login = body.username.eq_ignore_ascii_case("admin")
        && admin_password().as_deref() == Some(body.password.as_str());

    if admin_login {
        match state.player_client.lock().await.authenticate_or_register("admin", &body.password).await {
            Ok(auth_result) => {
                tracing::info!("[{}] Admin authenticated", market_name());
                return (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "token": auth_result.token,
                        "username": auth_result.username,
                        "is_admin": auth_result.is_admin
                    })),
                )
                    .into_response();
            }
            Err(e) => {
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

    match state.player_client.lock().await.authenticate_or_register(&body.username, &body.password).await {
        Ok(auth_result) => {
            tracing::info!("[{}] Player '{}' authenticated", market_name(), auth_result.username);
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
        Err(e) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": e,
            })),
        )
            .into_response(),
    }
}

/// GET /api/markets — returns the list of all configured markets.
pub async fn api_markets_handler(State(state): State<AppState>) -> impl IntoResponse {
    use crate::auth::advertised_markets;
    Json(advertised_markets(&state.known_markets))
}

pub async fn api_trades_handler(State(state): State<AppState>) -> impl IntoResponse {
    let trades_queue = state.trades_queue.lock().unwrap();
    let trades: Vec<crate::state::TradeView> = trades_queue
        .iter()
        .cloned()
        .collect();
    
    Json(serde_json::json!({
        "success": true,
        "trades": trades,
    }))
}
