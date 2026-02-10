// ABOUTME: Route definitions and handler functions for the specd HTTP API.
// ABOUTME: Assembles all API routes, web UI routes, and static file serving into a single Axum Router.

use axum::Router;
use axum::routing::{get, post, put};
use tower_http::services::ServeDir;

use crate::api;
use crate::app_state::SharedState;
use crate::auth::AuthLayer;
use crate::web;

/// Build the complete Axum router with all routes and shared state.
/// If `auth_token` is provided, the `AuthLayer` middleware is applied
/// to protect /api/* routes with bearer token authentication.
/// If `None`, no authentication is applied (local-only mode).
pub fn create_router(state: SharedState, auth_token: Option<String>) -> Router {
    let router = Router::new()
        // Health check
        .route("/health", get(health))
        // API routes (JSON)
        .route(
            "/api/specs",
            get(api::specs::list_specs).post(api::specs::create_spec),
        )
        .route("/api/specs/{id}/state", get(api::specs::get_spec_state))
        .route(
            "/api/specs/{id}/commands",
            post(api::commands::submit_command),
        )
        .route(
            "/api/specs/{id}/events/stream",
            get(api::stream::event_stream),
        )
        .route("/api/specs/{id}/undo", post(api::commands::undo))
        // Web UI routes (HTML)
        .route("/", get(web::index))
        .route("/web/specs", get(web::spec_list).post(web::create_spec))
        .route("/web/specs/new", get(web::create_spec_form))
        .route("/web/specs/{id}", get(web::spec_view))
        .route("/web/specs/{id}/board", get(web::board))
        .route("/web/specs/{id}/document", get(web::document))
        .route("/web/specs/{id}/activity", get(web::activity))
        .route("/web/specs/{id}/answer", post(web::answer_question))
        .route("/web/specs/{id}/undo", post(web::undo))
        .route("/web/specs/{id}/cards/new", get(web::create_card_form))
        .route("/web/specs/{id}/cards", post(web::create_card))
        .route(
            "/web/specs/{id}/cards/{card_id}/edit",
            get(web::edit_card_form),
        )
        .route(
            "/web/specs/{id}/cards/{card_id}",
            put(web::update_card).delete(web::delete_card),
        )
        // Static file serving
        .nest_service("/static", ServeDir::new("static"))
        .with_state(state);

    if let Some(token) = auth_token {
        router.layer(AuthLayer::new(token))
    } else {
        router
    }
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
        let app = create_router(test_state(), None);
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

    #[tokio::test]
    async fn auth_middleware_wired_when_token_provided() {
        let app = create_router(test_state(), Some("secret-token".to_string()));

        // API route without token should be rejected
        let resp = app
            .oneshot(
                Request::get("/api/specs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            http::StatusCode::UNAUTHORIZED,
            "API route should require auth when token is configured"
        );
    }

    #[tokio::test]
    async fn auth_middleware_allows_with_valid_token() {
        let app = create_router(test_state(), Some("secret-token".to_string()));

        // API route with correct token should succeed
        let resp = app
            .oneshot(
                Request::get("/api/specs")
                    .header("authorization", "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            http::StatusCode::OK,
            "API route should allow access with valid token"
        );
    }

    #[tokio::test]
    async fn no_auth_when_no_token_provided() {
        let app = create_router(test_state(), None);

        // API route without token should work when no auth configured
        let resp = app
            .oneshot(
                Request::get("/api/specs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            http::StatusCode::OK,
            "API route should be open when no auth token configured"
        );
    }
}
