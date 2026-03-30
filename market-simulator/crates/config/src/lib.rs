use std::fs;

use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub struct Connection {
    pub ip: String,
    pub port: u16,
}

#[derive(Clone, Deserialize)]
pub struct MarketConfig {
    pub name: String,
    pub web: Connection,
    pub tcp: Connection,
    pub grpc: Connection,
    pub core_mapping: EngineCoreMapping,
}

#[derive(Clone, Deserialize)]
pub struct MarketsConfig {
    pub ring_buffer_size: usize,
    pub markets: Vec<MarketConfig>,
}

#[derive(Clone, Deserialize)]
pub struct EngineCoreMapping {
    pub fix_inbound_core: usize,
    pub fix_outbound_core: usize,
    pub order_book_core: usize,
    pub execution_report_core: usize,
    pub db_core: usize,
    pub web_core: usize,
    pub tcp_core: usize,
    pub global_core: usize,
}

impl MarketsConfig {
    pub fn new() -> Self {
        MarketsConfig {
            ring_buffer_size: 0,
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