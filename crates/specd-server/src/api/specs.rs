// ABOUTME: Spec CRUD API handlers for listing, creating, and reading spec state.
// ABOUTME: Manages spec lifecycle through actor creation and state materialization.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use specd_core::{Command, SpecState, spawn};
use specd_store::JsonlLog;
use ulid::Ulid;

use crate::app_state::SharedState;

/// Summary of a spec for the list endpoint.
#[derive(Debug, Serialize)]
pub struct SpecSummary {
    pub spec_id: String,
    pub title: String,
    pub one_liner: String,
    pub updated_at: String,
}

/// Request body for creating a new spec.
#[derive(Debug, Deserialize)]
pub struct CreateSpecRequest {
    pub title: String,
    pub one_liner: String,
    pub goal: String,
}

/// Response body after creating a spec.
#[derive(Debug, Serialize)]
pub struct CreateSpecResponse {
    pub spec_id: String,
}

/// GET /api/specs - List all specs with summary info.
pub async fn list_specs(State(state): State<SharedState>) -> Json<Vec<SpecSummary>> {
    let actors = state.actors.read().await;
    let mut summaries = Vec::new();

    for (spec_id, handle) in actors.iter() {
        let spec_state = handle.read_state().await;
        if let Some(ref core) = spec_state.core {
            summaries.push(SpecSummary {
                spec_id: spec_id.to_string(),
                title: core.title.clone(),
                one_liner: core.one_liner.clone(),
                updated_at: core.updated_at.to_rfc3339(),
            });
        }
    }

    Json(summaries)
}

/// POST /api/specs - Create a new spec.
pub async fn create_spec(
    State(state): State<SharedState>,
    Json(req): Json<CreateSpecRequest>,
) -> impl IntoResponse {
    let spec_id = Ulid::new();

    // Create directory structure for this spec
    let spec_dir = state.specd_home.join("specs").join(spec_id.to_string());
    if let Err(e) = std::fs::create_dir_all(&spec_dir) {
        tracing::error!("failed to create spec directory: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "failed to create spec directory" })),
        )
            .into_response();
    }
    let log_path = spec_dir.join("events.jsonl");

    // Initialize JSONL log
    let mut log = match JsonlLog::open(&log_path) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("failed to create JSONL log: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "failed to create spec storage" })),
            )
                .into_response();
        }
    };

    // Spawn actor and send CreateSpec command
    let handle = spawn(spec_id, SpecState::new());
    let events = match handle
        .send_command(Command::CreateSpec {
            title: req.title,
            one_liner: req.one_liner,
            goal: req.goal,
        })
        .await
    {
        Ok(events) => events,
        Err(e) => {
            tracing::error!("failed to create spec: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("failed to create spec: {}", e) })),
            )
                .into_response();
        }
    };

    // Persist events to JSONL
    for event in &events {
        if let Err(e) = log.append(event) {
            tracing::error!("failed to persist event: {}", e);
        }
    }

    // Store actor handle
    state.actors.write().await.insert(spec_id, handle);

    // Auto-start agents if a provider is available
    {
        let actors = state.actors.read().await;
        if let Some(handle_ref) = actors.get(&spec_id) {
            crate::web::try_start_agents(&state, spec_id, handle_ref).await;
        }
    }

    (
        StatusCode::CREATED,
        Json(serde_json::json!({ "spec_id": spec_id.to_string() })),
    )
        .into_response()
}

/// GET /api/specs/{id}/state - Get the full materialized state.
pub async fn get_spec_state(
    State(state): State<SharedState>,
    Path(id): Path<String>,
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
    match actors.get(&spec_id) {
        Some(handle) => {
            let spec_state = handle.read_state().await;
            let state_clone: SpecState = spec_state.clone();
            Json(state_clone).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "spec not found" })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_state::AppState;
    use crate::providers::ProviderStatus;
    use crate::routes::create_router;
    use axum::body::Body;
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

    #[tokio::test]
    async fn create_spec_returns_201() {
        let state = test_state();
        let app = create_router(state, None);

        let body = serde_json::json!({
            "title": "Test Spec",
            "one_liner": "A test",
            "goal": "Verify creation"
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

        assert_eq!(resp.status(), StatusCode::CREATED);

        let resp_body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
        assert!(json["spec_id"].as_str().is_some());
        // Verify the spec_id is a valid ULID
        let spec_id_str = json["spec_id"].as_str().unwrap();
        assert!(spec_id_str.parse::<Ulid>().is_ok());
    }

    #[tokio::test]
    async fn list_specs_returns_created() {
        let state = test_state();

        // Create a spec first
        {
            let app = create_router(Arc::clone(&state), None);
            let body = serde_json::json!({
                "title": "Listed Spec",
                "one_liner": "Should appear in list",
                "goal": "Verify listing"
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
            assert_eq!(resp.status(), StatusCode::CREATED);
        }

        // Now list specs
        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(Request::get("/api/specs").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);

        let resp_body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Vec<serde_json::Value> = serde_json::from_slice(&resp_body).unwrap();
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["title"], "Listed Spec");
        assert_eq!(json[0]["one_liner"], "Should appear in list");
    }

    #[tokio::test]
    async fn get_state_returns_spec() {
        let state = test_state();

        // Create a spec
        let spec_id: String;
        {
            let app = create_router(Arc::clone(&state), None);
            let body = serde_json::json!({
                "title": "State Spec",
                "one_liner": "Check state",
                "goal": "Verify state retrieval"
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
            assert_eq!(resp.status(), StatusCode::CREATED);

            let resp_body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let json: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
            spec_id = json["spec_id"].as_str().unwrap().to_string();
        }

        // Get state
        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::get(&format!("/api/specs/{}/state", spec_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);

        let resp_body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
        assert_eq!(json["core"]["title"], "State Spec");
        assert_eq!(json["core"]["one_liner"], "Check state");
        assert_eq!(json["core"]["goal"], "Verify state retrieval");
    }
}
