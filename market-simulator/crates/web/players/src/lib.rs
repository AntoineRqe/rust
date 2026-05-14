// Players library exports for use by other crates
pub mod players;
pub mod service;

pub use players::{PlayerStore, Player, PendingOrder, INITIAL_TOKENS, AuthError, HoldingSummary, PortfolioLot};
pub use players::token::{TokenClaims, generate_token, validate_token, get_jwt_secret};
pub use service::PlayerServiceImpl;

// Proto-generated code
pub mod pb {
    pub mod players {
        pub mod v1 {
            tonic::include_proto!("players.v1");
        }
    }
}

pub use pb::players::v1::player_service_server::PlayerServiceServer;
