use std::env;
use std::fs;

use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub struct Connection {
    pub ip: String,
    pub port: u16,
}

#[derive(Clone, Deserialize)]
pub struct MulticastConfig {
    pub ip: String,
    pub port: u16,
}

#[derive(Clone, Deserialize)]
pub struct SnapshotConfig {
    pub update_interval_ms: u64,
    pub max_depth: usize,
}

#[derive(Clone, Deserialize)]
pub struct PlayerServiceConfig {
    pub database_url_env: String,
    pub grpc: Connection,
    pub core: usize,
}

#[derive(Clone, Deserialize)]
pub struct MarketConfig {
    pub name: String,
    pub database_url_env: String,
    #[serde(default)]
    pub stocks: Vec<String>,
    pub web: Connection,
    pub grpc: Connection,
    pub proxy: Connection,
    pub market_feed_multicast: MulticastConfig,
    pub snapshot_multicast: MulticastConfig,
    pub core_mapping: EngineCoreMapping,
    pub snapshot: SnapshotConfig,
}

impl MarketConfig {
    pub fn resolve_database_url(&self) -> Result<String, String> {
        env::var(&self.database_url_env).map_err(|_| {
            format!(
                "missing env var '{}' for market '{}'",
                self.database_url_env, self.name
            )
        })
    }

    pub fn normalized_stocks(&self) -> Vec<String> {
        let mut stocks = Vec::new();
        for symbol in &self.stocks {
            let normalized = symbol.trim().to_uppercase();
            if normalized.is_empty() || stocks.contains(&normalized) {
                continue;
            }
            stocks.push(normalized);
        }
        stocks
    }
}

#[derive(Clone, Deserialize)]
pub struct MarketsConfig {
    pub ring_buffer_size: usize,
    pub entry_point: Connection,
    pub players_service: PlayerServiceConfig,
    pub markets: Vec<MarketConfig>,
}

#[derive(Clone, Deserialize)]
pub struct SingleMarketConfig {
    pub ring_buffer_size: usize,
    pub players_service: PlayerServiceConfig,
    pub market: MarketConfig,
}

#[derive(Clone, Deserialize)]
pub struct GatewayMarketConfig {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub stocks: Vec<String>,
    #[serde(default)]
    pub public_url: Option<String>,
}

#[derive(Clone, Deserialize)]
pub struct GatewayConfig {
    pub entry_point: Connection,
    pub markets: Vec<GatewayMarketConfig>,
}

#[derive(Clone, Deserialize)]
pub struct EngineCoreMapping {
    pub fix_inbound_core: usize,
    pub fix_outbound_core: usize,
    pub order_book_core: usize,
    pub execution_report_core: usize,
    pub market_feed_core: usize,
    pub db_core: usize,
    pub web_core: usize,
    pub global_core: usize,
    pub snapshot_core: usize,
    pub market_feed_multicast_core: usize,
    pub snapshot_multicast_core: usize,
    pub market_data_proxy_core: usize,
}

impl MarketsConfig {
    pub fn new() -> Self {
        MarketsConfig {
            ring_buffer_size: 0,
            entry_point: Connection {
                ip: "127.0.0.1".to_string(),
                port: 9875,
            },
            players_service: PlayerServiceConfig {
                database_url_env: "DATABASE_URL_MARKET_SIMULATOR".to_string(),
                grpc: Connection {
                    ip: "127.0.0.1".to_string(),
                    port: 50053,
                },
                core: 13,
            },
            markets: vec![],
        }
    }

    pub fn resolve_player_database_url(&self) -> Result<String, String> {
        env::var(&self.players_service.database_url_env).map_err(|_| {
            format!(
                "missing env var '{}' for global player database",
                self.players_service.database_url_env
            )
        })
    }

    pub fn parse_from_file(file_path: &str) -> Self {
        let file_content = fs::read_to_string(file_path)
            .unwrap_or_else(|err| panic!("failed to read config file '{}': {err}", file_path));

        serde_json::from_str::<MarketsConfig>(&file_content)
            .unwrap_or_else(|err| panic!("failed to parse config file '{}': {err}", file_path))
    }
}

impl SingleMarketConfig {
    pub fn resolve_player_database_url(&self) -> Result<String, String> {
        env::var(&self.players_service.database_url_env).map_err(|_| {
            format!(
                "missing env var '{}' for global player database",
                self.players_service.database_url_env
            )
        })
    }

    pub fn parse_from_file(file_path: &str) -> Self {
        let file_content = fs::read_to_string(file_path)
            .unwrap_or_else(|err| panic!("failed to read config file '{}': {err}", file_path));

        serde_json::from_str::<SingleMarketConfig>(&file_content)
            .unwrap_or_else(|err| panic!("failed to parse config file '{}': {err}", file_path))
    }
}

impl GatewayConfig {
    pub fn parse_from_file(file_path: &str) -> Self {
        let file_content = fs::read_to_string(file_path)
            .unwrap_or_else(|err| panic!("failed to read config file '{}': {err}", file_path));

        serde_json::from_str::<GatewayConfig>(&file_content)
            .unwrap_or_else(|err| panic!("failed to parse config file '{}': {err}", file_path))
    }
}
