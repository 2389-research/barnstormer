// ABOUTME: HTTP endpoint for importing specs from arbitrary text via LLM extraction.
// ABOUTME: POST /api/specs/import accepts content + optional format hint and creates a full spec.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use barnstormer_agent::client::create_llm_client;
use barnstormer_agent::import::{parse_with_llm, to_commands};
use barnstormer_core::{SpecState, spawn};
use barnstormer_store::JsonlLog;
use ulid::Ulid;

use crate::app_state::SharedState;

/// Request body for importing a spec from arbitrary content.
#[derive(Debug, Deserialize)]
pub struct ImportSpecRequest {
    pub content: String,
    #[serde(default)]
    pub source_format: Option<String>,
}

/// Response body after importing a spec.
#[derive(Debug, Serialize)]
pub struct ImportSpecResponse {
    pub spec_id: String,
    pub title: String,
    pub card_count: usize,
}

/// POST /api/specs/import — parse arbitrary content via LLM, create a spec.
pub async fn import_spec(
    State(state): State<SharedState>,
    Json(req): Json<ImportSpecRequest>,
) -> impl IntoResponse {
    if req.content.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "content must not be empty" })),
        )
            .into_response();
    }

    // Create LLM client from configured provider
    let provider = &state.provider_status.default_provider;
    let (client, model) = match create_llm_client(provider, state.provider_status.default_model.as_deref()) {
        Ok(pair) => pair,
        Err(e) => {
            tracing::error!("failed to create LLM client: {}", e);
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": format!("LLM provider not available: {}", e) })),
            )
                .into_response();
        }
    };

    // Parse content via LLM
    let source_hint = req.source_format.as_deref().filter(|s| *s != "auto");
    let import_result = match parse_with_llm(&req.content, source_hint, &client, &model).await {
        Ok(result) => result,
        Err(e) => {
            tracing::error!("LLM import failed: {}", e);
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": format!("failed to parse content: {}", e) })),
            )
                .into_response();
        }
    };

    let title = import_result.spec.title.clone();
    let card_count = import_result.cards.len();
    let commands = to_commands(&import_result);

    // Create spec directory and JSONL log
    let spec_id = Ulid::new();
    let spec_dir = state.barnstormer_home.join("specs").join(spec_id.to_string());
    if let Err(e) = std::fs::create_dir_all(&spec_dir) {
        tracing::error!("failed to create spec directory: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "failed to create spec directory" })),
        )
            .into_response();
    }

    let log_path = spec_dir.join("events.jsonl");
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

    // Spawn actor and send all commands
    let handle = spawn(spec_id, SpecState::new());
    for cmd in commands {
        match handle.send_command(cmd).await {
            Ok(events) => {
                for event in &events {
                    if let Err(e) = log.append(event) {
                        tracing::error!("failed to persist event: {}", e);
                    }
                }
            }
            Err(e) => {
                tracing::error!("failed to send command: {}", e);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("failed to create spec: {}", e) })),
                )
                    .into_response();
            }
        }
    }

    // Subscribe event persister
    let persister_handle =
        crate::web::spawn_event_persister(&handle, spec_id, &state.barnstormer_home);
    state
        .event_persisters
        .write()
        .await
        .insert(spec_id, persister_handle);

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
        Json(serde_json::json!({
            "spec_id": spec_id.to_string(),
            "title": title,
            "card_count": card_count,
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use crate::app_state::AppState;
    use crate::providers::ProviderStatus;
    use crate::routes::create_router;
    use axum::body::Body;
    use http::Request;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_state() -> Arc<AppState> {
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
    async fn import_rejects_empty_content() {
        let state = test_state();
        let app = create_router(state, None);

        let body = serde_json::json!({
            "content": "",
        });

        let resp = app
            .oneshot(
                Request::post("/api/specs/import")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), http::StatusCode::BAD_REQUEST);

        let resp_body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
        assert!(json["error"].as_str().unwrap().contains("empty"));
    }

    #[tokio::test]
    async fn import_rejects_whitespace_only_content() {
        let state = test_state();
        let app = create_router(state, None);

        let body = serde_json::json!({
            "content": "   \n  \t  ",
        });

        let resp = app
            .oneshot(
                Request::post("/api/specs/import")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), http::StatusCode::BAD_REQUEST);
    }
}
