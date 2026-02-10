// ABOUTME: HTTP server for specd, providing REST API and SSE event streaming.
// ABOUTME: Uses Axum with shared actor state for spec management and real-time updates.

pub mod api;
pub mod app_state;
pub mod routes;

pub use app_state::{AppState, SharedState};
pub use routes::create_router;
