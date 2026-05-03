use axum::{
    response::{Html, IntoResponse, Redirect},
    extract::{State, Query},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use crate::server::AppState;
use crate::auth::{SessionInfo, generate_token, authenticate_token, admin_password};
use utils::market_name;

/// Serve the login page (always accessible, no auth required).
#[derive(Deserialize)]
pub struct LoginPageParams {
    token: Option<String>,
}

/// POST /api/login — body: `{ "username": "...", "password": "..." }`
/// Returns 200 `{ token, username }` on success, 401 `{ error }` on failure.
#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Serve the login page. If a valid token is provided, redirect to the app.
pub async fn login_page_handler(
    State(state): State<AppState>,
    Query(params): Query<LoginPageParams>,
) -> impl IntoResponse {
    if let Some(token) = params.token {
        if authenticate_token(&state.sessions, &token).is_some() {
            return Redirect::to(&format!("/app?token={token}")).into_response();
        }
    }

    Html(include_str!("../frontend/login.html")).into_response()
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
    let html = include_str!("../frontend/index.html")
        .replace("{{MARKET_NAME}}", market)
        .replace("{{LOGIN_GATEWAY_URL}}", &login_gateway_url)
        .replace("{{CURRENT_MARKET_NAME}}", market)
        .replace("{{MARKETS_JSON}}", &markets_json);
    Html(html)
}

/// Handle login requests. Supports both admin login (with MARKET_SIMULATOR_ADMIN_PWD env var)
/// and player login (with username/password registration or authentication).
pub async fn api_login_handler(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> impl IntoResponse {
    let admin_login = body.username.eq_ignore_ascii_case("admin")
        && admin_password().as_deref() == Some(body.password.as_str());

    if admin_login {
        let created = state.player_store.ensure_player_exists("admin");
        if created {
            tracing::info!("[{}] Admin player profile initialized", market_name());
        }

        let token = generate_token();
        state.sessions.lock().unwrap().insert(
            token.clone(),
            SessionInfo {
                username: "admin".to_string(),
                is_admin: true,
            },
        );
        tracing::info!("[{}] Admin session created", market_name());
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "token": token,
                "username": "admin",
                "is_admin": true
            })),
        )
            .into_response();
    }

    match state.player_store.authenticate_or_register(&body.username, &body.password) {
        Ok(username) => {
            let token = generate_token();
            state.sessions.lock().unwrap().insert(
                token.clone(),
                SessionInfo {
                    username: username.clone(),
                    is_admin: false,
                },
            );
            tracing::info!("[{}] Session created for '{username}'", market_name());
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "token": token,
                    "username": username,
                    "is_admin": false
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": e.message(),
                "code": e.code(),
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
