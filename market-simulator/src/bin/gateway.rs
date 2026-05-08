use clap::Parser;
use config::GatewayConfig;

#[derive(Parser, Debug)]
#[command(name = "gateway")]
struct Cli {
    #[arg(
        short = 'c',
        long = "config",
        default_value = "crates/config/gateway/default.json"
    )]
    config_file: String,
}

fn main() {
    let cli = Cli::parse();
    let config = GatewayConfig::parse_from_file(&cli.config_file);

    tracing_subscriber::fmt()
        .with_env_filter("info,sqlx=warn,h2=warn,tokio_util=warn")
        .init();

    if config.markets.is_empty() {
        tracing::error!("No markets defined in config file '{}'", cli.config_file);
        std::process::exit(1);
    }

    let gateway_ip = config.entry_point.ip.clone();
    let gateway_port = config.entry_point.port;
    
    let gateway_markets: Vec<backend::MarketInfo> = config
        .markets
        .iter()
        .map(|m| backend::MarketInfo {
            name: m.name.clone(),
            url: m.url.clone(),
        })
        .collect();

    tracing::info!(
        "[gateway] Configured entry point → http://{}:{}",
        gateway_ip,
        gateway_port
    );

    // Start the login gateway
    {
        let login_ip = gateway_ip.clone();
        let login_port = gateway_port;
        backend::run_login_gateway(gateway_markets, &login_ip, login_port);
    }
}
