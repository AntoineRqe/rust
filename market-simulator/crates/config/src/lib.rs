use std::fs;
use std::env;

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
pub struct MarketConfig {
    pub name: String,
    pub database_url: Option<String>,
    pub database_url_env: Option<String>,
    pub web: Connection,
    pub tcp: Connection,
    pub grpc: Connection,
    pub market_feed_multicast: MulticastConfig,
    pub snapshot_multicast: MulticastConfig,
    pub core_mapping: EngineCoreMapping,
    pub snapshot: SnapshotConfig,
}

impl MarketConfig {
    pub fn resolve_database_url(&self) -> Result<String, String> {
        if let Some(env_key) = &self.database_url_env {
            return env::var(env_key).map_err(|_| {
                format!(
                    "missing env var '{}' for market '{}'",
                    env_key,
                    self.name
                )
            });
        }

        self.database_url
            .clone()
            .ok_or_else(|| format!("missing database_url for market '{}'", self.name))
    }
}

#[derive(Clone, Deserialize)]
pub struct MarketsConfig {
    pub ring_buffer_size: usize,
    pub entry_point: Connection,
    pub markets: Vec<MarketConfig>,
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
    pub tcp_core: usize,
    pub global_core: usize,
    pub snapshot_core: usize,
}

impl MarketsConfig {
    pub fn new() -> Self {
        MarketsConfig {
            ring_buffer_size: 0,
            entry_point: Connection {
                ip: "127.0.0.1".to_string(),
                port: 9875,
            },
            markets: vec![],
        }
    }

    pub fn parse_from_file(file_path: &str) -> Self {
        let file_content = fs::read_to_string(file_path)
            .unwrap_or_else(|err| panic!("failed to read config file '{}': {err}", file_path));

        serde_json::from_str::<MarketsConfig>(&file_content)
            .unwrap_or_else(|err| panic!("failed to parse config file '{}': {err}", file_path))
    }
}