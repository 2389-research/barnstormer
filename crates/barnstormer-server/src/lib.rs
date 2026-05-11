// ABOUTME: HTTP server for barnstormer, providing REST API, SSE streaming, auth, and config.
// ABOUTME: Uses Axum with shared actor state for spec management and real-time updates.

pub mod api;
pub mod app_state;
pub mod attachment_summarizer;
pub mod auth;
pub mod config;
pub mod context_storage;
pub mod narration_renderer;
pub mod providers;
pub mod routes;
pub mod summarizer;
pub mod svg_raster;
pub mod web;

pub use app_state::{AppState, SharedState};
pub use auth::AuthLayer;
pub use config::{BarnstormerConfig, ConfigError};
pub use providers::ProviderStatus;
pub use routes::create_router;
