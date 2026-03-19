pub mod state;
pub mod server;
pub mod ws;

// Re-export the main entry point for convenience
pub use server::run_web_server;
pub use state::EventBus;