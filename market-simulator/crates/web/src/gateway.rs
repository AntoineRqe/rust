use serde::Deserialize;
use std::time::Duration;

use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite;
use futures_util::{SinkExt, StreamExt};

use std::net::SocketAddr;

use axum::{
    Router,
    routing::{get, post},
    extract::{Query, State},
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
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
/// The gateway serves the login page and proxies login requests to configured market servers.
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
                .route("/ws", get(gateway_ws_handler))
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
    Html(include_str!("../frontend/login.html"))
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

    let html = include_str!("../frontend/index.html")
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
struct GatewayWsQuery {
    token: String,
    market: Option<String>,
}

fn market_ws_url(base_url: &str, token: &str) -> String {
    let endpoint = format!("{}/ws?token={}", base_url.trim_end_matches('/'), token);
    if endpoint.starts_with("https://") {
        return endpoint.replacen("https://", "wss://", 1);
    }
    if endpoint.starts_with("http://") {
        return endpoint.replacen("http://", "ws://", 1);
    }
    endpoint
}

fn pick_market<'a>(markets: &'a [MarketInfo], requested: Option<&str>) -> Option<&'a MarketInfo> {
    if let Some(name) = requested {
        let requested_trimmed = name.trim();
        if !requested_trimmed.is_empty() {
            if let Some(found) = markets.iter().find(|m| m.name.eq_ignore_ascii_case(requested_trimmed)) {
                return Some(found);
            }
        }
    }
    markets.first()
}

async fn gateway_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<LoginGatewayState>,
    Query(query): Query<GatewayWsQuery>,
) -> impl IntoResponse {
    if query.token.trim().is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let markets = crate::auth::advertised_markets(&state.markets);
    let Some(target_market) = pick_market(&markets, query.market.as_deref()) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };

    let target_market_name = target_market.name.clone();
    let target_ws = market_ws_url(&target_market.url, query.token.trim());
    ws.on_upgrade(move |socket| proxy_websocket(socket, target_ws, target_market_name))
        .into_response()
}

async fn proxy_websocket(client_ws: WebSocket, target_ws_url: String, target_market_name: String) {
    let upstream = match connect_async(&target_ws_url).await {
        Ok((ws_stream, _)) => ws_stream,
        Err(e) => {
            tracing::warn!(
                "[gateway] WS proxy failed to connect to market '{}' at '{}': {}",
                target_market_name,
                target_ws_url,
                e
            );
            return;
        }
    };

    let (mut client_tx, mut client_rx) = client_ws.split();
    let (mut upstream_tx, mut upstream_rx) = upstream.split();

    let client_to_upstream = async {
        while let Some(next) = client_rx.next().await {
            let Ok(msg) = next else { break; };
            let mapped = match msg {
                Message::Text(text) => tungstenite::Message::Text(text.to_string().into()),
                Message::Binary(bin) => tungstenite::Message::Binary(bin.to_vec().into()),
                Message::Ping(data) => tungstenite::Message::Ping(data.to_vec().into()),
                Message::Pong(data) => tungstenite::Message::Pong(data.to_vec().into()),
                Message::Close(_) => {
                    let _ = upstream_tx.send(tungstenite::Message::Close(None)).await;
                    break;
                }
            };

            if upstream_tx.send(mapped).await.is_err() {
                break;
            }
        }
    };

    let upstream_to_client = async {
        while let Some(next) = upstream_rx.next().await {
            let Ok(msg) = next else { break; };
            let mapped = match msg {
                tungstenite::Message::Text(text) => Message::Text(text.to_string().into()),
                tungstenite::Message::Binary(bin) => Message::Binary(bin.into()),
                tungstenite::Message::Ping(data) => Message::Ping(data.into()),
                tungstenite::Message::Pong(data) => Message::Pong(data.into()),
                tungstenite::Message::Close(_) => {
                    let _ = client_tx.send(Message::Close(None)).await;
                    break;
                }
                tungstenite::Message::Frame(_) => continue,
            };

            if client_tx.send(mapped).await.is_err() {
                break;
            }
        }
    };

    tokio::select! {
        _ = client_to_upstream => {}
        _ = upstream_to_client => {}
    }
}

fn is_loopback_host(host: &str) -> bool {
    let value = host.trim().to_ascii_lowercase();
    matches!(
        value.as_str(),
        "localhost" | "127.0.0.1" | "::1" | "0.0.0.0" | "host.docker.internal"
    )
}

fn extract_market_url_parts(raw_url: &str) -> Option<(&str, &str, &str)> {
    let trimmed = raw_url.trim();
    if let Some(rest) = trimmed.strip_prefix("http://") {
        let slash = rest.find('/').unwrap_or(rest.len());
        return Some(("http", &rest[..slash], &rest[slash..]));
    }
    if let Some(rest) = trimmed.strip_prefix("https://") {
        let slash = rest.find('/').unwrap_or(rest.len());
        return Some(("https", &rest[..slash], &rest[slash..]));
    }
    None
}

fn split_host_port(host_port: &str) -> (String, Option<String>) {
    let trimmed = host_port.trim();
    if trimmed.is_empty() {
        return (String::new(), None);
    }

    if let Some(rest) = trimmed.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            let host = rest[..end].to_string();
            let remainder = &rest[end + 1..];
            let port = remainder.strip_prefix(':').map(|p| p.to_string());
            return (host, port);
        }
    }

    if let Some((host, port)) = trimmed.rsplit_once(':') {
        if host.contains(':') {
            return (trimmed.to_string(), None);
        }
        return (host.to_string(), Some(port.to_string()));
    }

    (trimmed.to_string(), None)
}

fn forwarded_header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn adapt_market_url_for_client(market_url: &str, headers: &HeaderMap) -> String {
    let Some((scheme, host_port, path)) = extract_market_url_parts(market_url) else {
        return market_url.to_string();
    };

    let (market_host, _market_port) = split_host_port(host_port);
    if !is_loopback_host(&market_host) {
        return market_url.to_string();
    }

    let request_host = forwarded_header(headers, "x-forwarded-host")
        .or_else(|| forwarded_header(headers, "host"));
    let Some(request_host_value) = request_host else {
        return market_url.to_string();
    };

    let (req_host, req_port) = split_host_port(&request_host_value);
    if req_host.is_empty() || is_loopback_host(&req_host) {
        return market_url.to_string();
    }

    let proto = forwarded_header(headers, "x-forwarded-proto")
        .map(|p| p.to_ascii_lowercase())
        .filter(|p| p == "http" || p == "https")
        .unwrap_or_else(|| scheme.to_string());

    // If the public host does not provide a port, keep it implicit (80/443)
    // instead of leaking the internal market port.
    let final_port = req_port;
    let final_host_port = if let Some(port) = final_port {
        format!("{}:{}", req_host, port)
    } else {
        req_host
    };

    format!("{}://{}{}", proto, final_host_port, path)
}

#[derive(Deserialize, serde::Serialize)]
struct GatewayLoginRequest {
    username: String,
    password: String,
    market: Option<String>,
}

async fn gateway_login_handler(
    State(state): State<LoginGatewayState>,
    headers: HeaderMap,
    Json(body): Json<GatewayLoginRequest>,
) -> impl IntoResponse {
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

    let requested_market = body
        .market
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());

    let markets_to_try: Vec<MarketInfo> = if let Some(name) = requested_market {
        match markets.iter().find(|market| market.name.eq_ignore_ascii_case(name)) {
            Some(found) => vec![found.clone()],
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": "Requested market is not available.",
                        "code": "MARKET_NOT_FOUND"
                    })),
                )
                    .into_response();
            }
        }
    } else {
        markets
    };

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

    let mut last_unauthorized: Option<serde_json::Value> = None;

    for market in markets_to_try {
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
            let mut out = match payload {
                serde_json::Value::Object(map) => map,
                _ => serde_json::Map::new(),
            };
            let market_url = adapt_market_url_for_client(&market.url, &headers);
            out.insert(
                "market_url".to_string(),
                serde_json::Value::String(market_url),
            );
            out.insert(
                "market_name".to_string(),
                serde_json::Value::String(market.name.clone()),
            );
            return (StatusCode::OK, Json(serde_json::Value::Object(out))).into_response();
        }

        if status.as_u16() == StatusCode::UNAUTHORIZED.as_u16() {
            last_unauthorized = Some(payload);
            continue;
        }

        tracing::warn!(
            "[gateway] Login proxy market '{}' returned status {}",
            market.name,
            status
        );
    }

    if let Some(payload) = last_unauthorized {
        return (StatusCode::UNAUTHORIZED, Json(payload)).into_response();
    }

    (
        StatusCode::BAD_GATEWAY,
        Json(serde_json::json!({
            "error": "Could not reach any market login endpoint.",
            "code": "MARKET_LOGIN_UNREACHABLE"
        })),
    )
        .into_response()
}
