use serde::Deserialize;
use std::time::Duration;
use std::net::SocketAddr;

use axum::{
    Router,
    routing::{get, post},
    extract::State,
    response::{Html, IntoResponse},
    http::{HeaderMap, StatusCode},
    Json,
};
use crate::auth::MarketInfo;

#[derive(Clone)]
struct LoginGatewayState {
    markets: Vec<MarketInfo>,
}

/// Run the login gateway server on the specified IP and port.
/// The gateway serves the login page and proxies login requests to configured markets.
/// Browsers connect directly to market servers (no WebSocket proxy).
pub fn run_login_gateway(markets: Vec<MarketInfo>, ip: &str, port: u16) {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("login-gateway")
        .build()
        .expect("Failed to build tokio runtime for login gateway")
        .block_on(async move {
            let advertised = crate::auth::advertised_markets(&markets);
            if !markets.is_empty() {
                if advertised.is_empty() {
                    tracing::warn!(
                        "[gateway] MARKET_SIM_PUBLIC_MARKETS_ONLY=1 filtered out all configured markets; /api/markets will be empty"
                    );
                } else if advertised.len() != markets.len() {
                    tracing::info!(
                        "[gateway] MARKET_SIM_PUBLIC_MARKETS_ONLY=1 filtered {} non-public market URL(s)",
                        markets.len().saturating_sub(advertised.len())
                    );
                }
            }

            let state = LoginGatewayState { markets };
            let app = Router::new()
                .route("/", get(gateway_login_page_handler))
                .route("/app", get(gateway_app_handler))
                .route("/api/markets", get(gateway_markets_handler))
                .route("/api/login", post(gateway_login_handler))
                .with_state(state);

            let addr: SocketAddr = format!("0.0.0.0:{port}")
                .parse()
                .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], port)));

            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .unwrap_or_else(|e| panic!("Cannot bind login gateway to port {port}: {e}"));

            tracing::info!("[gateway] Login page → http://{}:{}", ip, port);

            axum::serve(listener, app)
                .await
                .expect("login gateway server error");
        });
}

async fn gateway_login_page_handler() -> Html<&'static str> {
    Html(frontend::LOGIN_HTML)
}

async fn gateway_app_handler(
    State(state): State<LoginGatewayState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let advertised = crate::auth::advertised_markets(&state.markets);
    if advertised.is_empty() {
        return Html("No markets available.").into_response();
    }

    let markets_for_client: Vec<MarketInfo> = advertised
        .iter()
        .map(|market| MarketInfo {
            name: market.name.clone(),
            url: adapt_market_url_for_client(&market.url, &headers),
        })
        .collect();

    let current_market_name = markets_for_client
        .first()
        .map(|market| market.name.as_str())
        .unwrap_or("unknown");
    let login_gateway_url = gateway_public_base_url(&headers);
    let markets_json = serde_json::to_string(&markets_for_client)
        .unwrap_or_else(|_| "[]".to_string());

    let html = frontend::APP_HTML
        .replace("{{MARKET_NAME}}", "gateway")
        .replace("{{LOGIN_GATEWAY_URL}}", &login_gateway_url)
        .replace("{{CURRENT_MARKET_NAME}}", current_market_name)
        .replace("{{MARKETS_JSON}}", &markets_json);

    Html(html).into_response()
}

async fn gateway_markets_handler(State(state): State<LoginGatewayState>) -> Json<Vec<MarketInfo>> {
    Json(crate::auth::advertised_markets(&state.markets))
}

#[derive(Deserialize)]
struct GatewayLoginRequest {
    username: String,
    password: String,
}

#[derive(serde::Serialize)]
struct MarketCredentials {
    name: String,
    token: String,
    url: String,
    is_admin: bool,
}

#[derive(serde::Serialize)]
struct GatewayLoginResponse {
    username: String,
    is_admin: bool,
    markets: Vec<MarketCredentials>,
}

async fn gateway_login_handler(
    State(state): State<LoginGatewayState>,
    headers: HeaderMap,
    Json(body): Json<GatewayLoginRequest>,
) -> impl axum::response::IntoResponse {
    let markets = crate::auth::advertised_markets(&state.markets);
    if markets.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "No market login endpoint is configured.",
                "code": "NO_MARKET_LOGIN_ENDPOINT"
            })),
        )
            .into_response();
    }

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            tracing::error!("[gateway] Failed to build HTTP client for login proxy: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Gateway login service initialization failed.",
                    "code": "GATEWAY_LOGIN_INIT_FAILED"
                })),
            )
                .into_response();
        }
    };

    let mut successful_markets: Vec<MarketCredentials> = Vec::new();
    let mut last_unauthorized: Option<serde_json::Value> = None;
    let mut user_is_admin = false;

    for market in markets {
        let endpoint = format!("{}/api/login", market.url.trim_end_matches('/'));
        let response = match client
            .post(&endpoint)
            .json(&serde_json::json!({
                "username": body.username,
                "password": body.password
            }))
            .send()
            .await
        {
            Ok(response) => response,
            Err(e) => {
                tracing::warn!(
                    "[gateway] Login proxy could not reach market '{}' at '{}': {}",
                    market.name,
                    endpoint,
                    e
                );
                continue;
            }
        };

        let status = response.status();
        let payload = response
            .json::<serde_json::Value>()
            .await
            .unwrap_or_else(|_| serde_json::json!({}));

        if status.is_success() {
            // Extract token from market response
            let token = payload.get("token")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            
            let is_admin = payload.get("is_admin")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            user_is_admin = user_is_admin || is_admin;
            let market_url = adapt_market_url_for_client(&market.url, &headers);

            successful_markets.push(MarketCredentials {
                name: market.name.clone(),
                token,
                url: market_url,
                is_admin,
            });
            
            tracing::info!(
                "[gateway] User '{}' successfully logged into market '{}'",
                body.username,
                market.name
            );
        } else if status.as_u16() == StatusCode::UNAUTHORIZED.as_u16() {
            last_unauthorized = Some(payload);
            tracing::debug!(
                "[gateway] User '{}' unauthorized on market '{}'",
                body.username,
                market.name
            );
        } else {
            tracing::warn!(
                "[gateway] Login proxy market '{}' returned status {}",
                market.name,
                status
            );
        }
    }

    // If user successfully logged into at least one market, return success
    if !successful_markets.is_empty() {
        let response = GatewayLoginResponse {
            username: body.username.clone(),
            is_admin: user_is_admin,
            markets: successful_markets,
        };
        return (StatusCode::OK, Json(response)).into_response();
    }

    // If all markets returned 401, return that
    if let Some(payload) = last_unauthorized {
        return (StatusCode::UNAUTHORIZED, Json(payload)).into_response();
    }

    // Otherwise, all markets were unreachable
    (
        StatusCode::BAD_GATEWAY,
        Json(serde_json::json!({
            "error": "Could not reach any market login endpoint.",
            "code": "MARKET_LOGIN_UNREACHABLE"
        })),
    )
        .into_response()
}

/// Adapt market URL for client based on request headers.
/// 
/// If the request came through a public domain, use public URLs.
/// Otherwise, use internal URLs.
fn adapt_market_url_for_client(internal_url: &str, headers: &HeaderMap) -> String {
    // Parse internal URL to extract port
    let internal_uri = match internal_url.parse::<axum::http::Uri>() {
        Ok(uri) => uri,
        Err(_) => {
            return internal_url.to_string();
        }
    };

    // Get port from internal URL (e.g., 8081 from http://localhost:8081)
    let internal_port = internal_uri.port_u16().unwrap_or_else(|| {
        match internal_uri.scheme() {
            Some(scheme) if scheme.as_str() == "https" => 443,
            _ => 80,
        }
    });

    // Get request headers for public host/scheme
    let req_host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");

    let req_scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");

    // Use the same host+scheme but with the market port
    format!("{}://{}:{}", req_scheme, req_host.split(':').next().unwrap_or(req_host), internal_port)
}

fn forwarded_header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
}

fn gateway_public_base_url(headers: &HeaderMap) -> String {
    let host = forwarded_header(headers, "x-forwarded-host")
        .or_else(|| forwarded_header(headers, "host"))
        .unwrap_or_else(|| "127.0.0.1:9875".to_string());

    let proto = forwarded_header(headers, "x-forwarded-proto")
        .map(|p| p.to_ascii_lowercase())
        .filter(|p| p == "http" || p == "https")
        .unwrap_or_else(|| "http".to_string());

    format!("{}://{}", proto, host)
}
