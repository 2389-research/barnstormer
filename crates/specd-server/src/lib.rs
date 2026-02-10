// ABOUTME: HTTP server for specd, providing REST API, SSE streaming, auth, and config.
// ABOUTME: Uses Axum with shared actor state for spec management and real-time updates.

pub mod api;
pub mod app_state;
pub mod auth;
pub mod config;
pub mod providers;
pub mod routes;
pub mod web;

pub use app_state::{AppState, SharedState};
pub use auth::AuthLayer;
pub use config::{ConfigError, SpecdConfig};
pub use providers::ProviderStatus;
pub use routes::create_router;
