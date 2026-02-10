// ABOUTME: Route definitions and handler functions for the specd HTTP API.
// ABOUTME: Assembles all API routes into a single Axum Router with shared state.

use axum::Router;
use axum::routing::{get, post};

use crate::api;
use crate::app_state::SharedState;

/// Build the complete Axum router with all routes and shared state.
pub fn create_router(state: SharedState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/specs", get(api::specs::list_specs).post(api::specs::create_spec))
        .route("/api/specs/{id}/state", get(api::specs::get_spec_state))
        .route("/api/specs/{id}/commands", post(api::commands::submit_command))
        .route("/api/specs/{id}/events/stream", get(api::stream::event_stream))
        .route("/api/specs/{id}/undo", post(api::commands::undo))
        .with_state(state)
}

/// Health check handler. Returns 200 OK with a simple JSON body.
async fn health() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "status": "ok" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_state::AppState;
    use axum::body::Body;
    use http::Request;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_state() -> SharedState {
        Arc::new(AppState::new(std::env::temp_dir().join("specd-test")))
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let app = create_router(test_state());
        let resp = app
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
    }
}
