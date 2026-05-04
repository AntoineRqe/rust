use std::sync::Arc;
use tonic::transport::Server;
use tracing::info;

use players::PlayerServiceImpl;
use players::PlayerServiceServer;
use players::players::PlayerStore;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let db_url = std::env::var("DATABASE_URL_MARKET_SIMULATOR")
        .expect("DATABASE_URL_MARKET_SIMULATOR environment variable must be set");

    let port = std::env::var("PLAYER_SERVICE_PORT")
        .unwrap_or_else(|_| "50052".to_string())
        .parse::<u16>()?;

    let addr = format!("[::1]:{}", port).parse()?;

    info!("Starting Player Server on {}", addr);

    let player_store = Arc::new(PlayerStore::load_postgres(&db_url));
    let service = PlayerServiceImpl::new(player_store);

    Server::builder()
        .add_service(PlayerServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
