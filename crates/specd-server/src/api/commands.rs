// ABOUTME: Command submission and undo API handlers for spec mutation.
// ABOUTME: Routes commands to spec actors and returns results. Persistence is handled by background broadcast subscribers.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use specd_core::Command;
use ulid::Ulid;

use crate::app_state::SharedState;

/// POST /api/specs/{id}/commands - Submit a command to a spec actor.
pub async fn submit_command(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(cmd): Json<Command>,
) -> impl IntoResponse {
    let spec_id = match id.parse::<Ulid>() {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "invalid spec id" })),
            )
                .into_response();
        }
    };

    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "spec not found" })),
            )
                .into_response();
        }
    };

    let events = match handle.send_command(cmd).await {
        Ok(events) => events,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("{}", e) })),
            )
                .into_response();
        }
    };

    // Events are persisted by the background broadcast subscriber
    // (spawned via spawn_event_persister when the actor was created).

    (
        StatusCode::OK,
        Json(serde_json::json!({ "events": events })),
    )
        .into_response()
}

/// POST /api/specs/{id}/undo - Undo the last undoable operation on a spec.
pub async fn undo(State(state): State<SharedState>, Path(id): Path<String>) -> impl IntoResponse {
    let spec_id = match id.parse::<Ulid>() {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "invalid spec id" })),
            )
                .into_response();
        }
    };

    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "spec not found" })),
            )
                .into_response();
        }
    };

    let events = match handle.send_command(Command::Undo).await {
        Ok(events) => events,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("{}", e) })),
            )
                .into_response();
        }
    };

    // Events are persisted by the background broadcast subscriber.

    (
        StatusCode::OK,
        Json(serde_json::json!({ "events": events })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use crate::app_state::AppState;
    use crate::app_state::SharedState;
    use crate::providers::ProviderStatus;
    use crate::routes::create_router;
    use axum::body::Body;
    use axum::http::StatusCode;
    use http::Request;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_state() -> SharedState {
        let dir = tempfile::TempDir::new().unwrap();
        let provider_status = ProviderStatus {
            default_provider: "anthropic".to_string(),
            default_model: None,
            providers: vec![],
            any_available: false,
        };
        Arc::new(AppState::new(dir.keep(), provider_status))
    }

    /// Helper: create a spec and return (state, spec_id).
    async fn create_test_spec(state: &SharedState) -> String {
        let app = create_router(Arc::clone(state), None);
        let body = serde_json::json!({
            "title": "Command Spec",
            "one_liner": "For commands",
            "goal": "Test commands"
        });

        let resp = app
            .oneshot(
                Request::post("/api/specs")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        let resp_body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
        json["spec_id"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn submit_create_card_command() {
        let state = test_state();
        let spec_id = create_test_spec(&state).await;

        // Submit a CreateCard command
        let app = create_router(Arc::clone(&state), None);
        let cmd = serde_json::json!({
            "type": "CreateCard",
            "card_type": "idea",
            "title": "My Idea",
            "body": null,
            "lane": null,
            "created_by": "human"
        });

        let resp = app
            .oneshot(
                Request::post(&format!("/api/specs/{}/commands", spec_id))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&cmd).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let resp_body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
        assert!(json["events"].as_array().is_some());
        assert_eq!(json["events"].as_array().unwrap().len(), 1);

        // Verify card appears in state
        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::get(&format!("/api/specs/{}/state", spec_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let resp_body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let state_json: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
        let cards = state_json["cards"].as_object().unwrap();
        assert_eq!(cards.len(), 1);

        // Find the card and verify its title
        let card = cards.values().next().unwrap();
        assert_eq!(card["title"], "My Idea");
    }

    #[tokio::test]
    async fn submit_undo_reverses_card() {
        let state = test_state();
        let spec_id = create_test_spec(&state).await;

        // Create a card
        {
            let app = create_router(Arc::clone(&state), None);
            let cmd = serde_json::json!({
                "type": "CreateCard",
                "card_type": "idea",
                "title": "Undo Me",
                "body": null,
                "lane": null,
                "created_by": "human"
            });

            let resp = app
                .oneshot(
                    Request::post(&format!("/api/specs/{}/commands", spec_id))
                        .header("content-type", "application/json")
                        .body(Body::from(serde_json::to_vec(&cmd).unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }

        // Verify card exists
        {
            let app = create_router(Arc::clone(&state), None);
            let resp = app
                .oneshot(
                    Request::get(&format!("/api/specs/{}/state", spec_id))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            let resp_body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let state_json: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
            let cards = state_json["cards"].as_object().unwrap();
            assert_eq!(cards.len(), 1);
        }

        // Undo
        {
            let app = create_router(Arc::clone(&state), None);
            let resp = app
                .oneshot(
                    Request::post(&format!("/api/specs/{}/undo", spec_id))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }

        // Verify card is gone
        {
            let app = create_router(Arc::clone(&state), None);
            let resp = app
                .oneshot(
                    Request::get(&format!("/api/specs/{}/state", spec_id))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            let resp_body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let state_json: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
            let cards = state_json["cards"].as_object().unwrap();
            assert_eq!(cards.len(), 0, "card should be removed after undo");
        }
    }
}
