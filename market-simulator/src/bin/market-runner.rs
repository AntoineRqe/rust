use clap::Parser;
use std::fs;
use std::os::unix::process::CommandExt;

#[derive(Parser, Debug)]
#[command(name = "market-runner")]
struct Cli {
    #[arg(short = 'c', long = "config")]
    config_file: String,
}

fn main() {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter("debug,sqlx=warn,h2=warn,tokio_util=warn")
        .init();

    // Read the market-specific config
    let config_content = fs::read_to_string(&cli.config_file).unwrap_or_else(|e| {
        tracing::error!("Failed to read config file '{}': {e}", cli.config_file);
        std::process::exit(1);
    });

    let market_config: serde_json::Value =
        serde_json::from_str(&config_content).unwrap_or_else(|e| {
            tracing::error!("Failed to parse config file '{}': {e}", cli.config_file);
            std::process::exit(1);
        });

    let market_name = market_config
        .get("market")
        .and_then(|m| m.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("UNKNOWN");

    tracing::info!(
        "[{}] Starting market from config: {}",
        market_name,
        cli.config_file
    );

    // Build a full MarketsConfig from the per-market config
    // This allows market-simulator to parse it normally
    let full_config = serde_json::json!({
        "ring_buffer_size": market_config.get("ring_buffer_size").unwrap_or(&serde_json::json!(1024)),
        "entry_point": {
            "ip": "0.0.0.0",
            "port": 9999  // Not used in single-market mode, but required by the config parser
        },
        "players_service": market_config.get("players_service").cloned().unwrap_or_default(),
        "markets": [market_config.get("market").cloned().unwrap_or_default()]
    });

    // Write the constructed config to a temp file for market-simulator
    let temp_config = format!("/tmp/market-runner-{}.json", std::process::id());
    fs::write(&temp_config, full_config.to_string()).unwrap_or_else(|e| {
        tracing::error!("Failed to write temp config: {e}");
        std::process::exit(1);
    });

    tracing::info!("[{}] Executing market-simulator for market", market_name);

    // Execute market-simulator with the constructed full config
    // and --market-index 0 since we only have one market in the array
    // Set MARKET_NAME in the current process environment so it is inherited by exec.
    // Safety: single-threaded at this point, no other threads reading env vars
    unsafe { std::env::set_var("MARKET_NAME", market_name) };

    let output = std::process::Command::new("market-simulator")
        .arg("--config")
        .arg(&temp_config)
        .arg("--market-index")
        .arg("0")
        .exec();

    // If exec returns, it means it failed to start the process
    tracing::error!("Failed to execute market-simulator: {output:?}");
}
