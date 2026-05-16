// Players library exports for use by other crates
pub mod players;
pub mod service;

pub use players::token::{TokenClaims, generate_token, get_jwt_secret, validate_token};
pub use players::{
    AuthError, HoldingSummary, INITIAL_TOKENS, PendingOrder, Player, PlayerStore, PortfolioLot,
};
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
