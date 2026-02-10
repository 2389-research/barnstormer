// ABOUTME: End-to-end smoke test for the full specd lifecycle.
// ABOUTME: Tests spec creation, card CRUD, undo, state verification, and export generation.

use std::sync::Arc;

use axum::body::Body;
use http::Request;
use specd_server::{AppState, create_router};
use specd_store::StorageManager;
use tower::ServiceExt;

/// Helper to create a test AppState with a temp directory.
fn test_app_state(home: std::path::PathBuf) -> Arc<AppState> {
    Arc::new(AppState::new(home))
}

/// Helper to extract JSON body from a response.
async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn smoke_test_full_lifecycle() {
    // 1. Create StorageManager with temp dir
    let dir = tempfile::TempDir::new().unwrap();
    let home = dir.path().to_path_buf();
    let _storage = StorageManager::new(home.clone()).unwrap();

    // 2. Create AppState
    let state = test_app_state(home.clone());

    // 3. Create router (don't bind to port -- use oneshot)

    // 4. POST /api/specs -> create spec
    let app = create_router(Arc::clone(&state));
    let create_body = serde_json::json!({
        "title": "Smoke Test Spec",
        "one_liner": "Full lifecycle test",
        "goal": "Verify end-to-end flow"
    });

    let resp = app
        .oneshot(
            Request::post("/api/specs")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), 201, "create spec should return 201");
    let json = json_body(resp).await;
    let spec_id = json["spec_id"].as_str().unwrap().to_string();
    assert!(!spec_id.is_empty(), "spec_id should be present");

    // 5. POST /api/specs/{id}/commands -> CreateCard
    let app = create_router(Arc::clone(&state));
    let card_cmd = serde_json::json!({
        "type": "CreateCard",
        "card_type": "idea",
        "title": "Smoke Card",
        "body": "This card tests the full flow",
        "lane": null,
        "created_by": "smoke-test"
    });

    let resp = app
        .oneshot(
            Request::post(format!("/api/specs/{}/commands", spec_id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&card_cmd).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "create card should return 200");
    let json = json_body(resp).await;
    let events = json["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "should produce one event");

    // 6. GET /api/specs/{id}/state -> verify card exists
    let app = create_router(Arc::clone(&state));
    let resp = app
        .oneshot(
            Request::get(format!("/api/specs/{}/state", spec_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "get state should return 200");
    let state_json = json_body(resp).await;
    let cards = state_json["cards"].as_object().unwrap();
    assert_eq!(cards.len(), 1, "should have one card");
    let card = cards.values().next().unwrap();
    assert_eq!(card["title"], "Smoke Card");
    assert_eq!(card["created_by"], "smoke-test");

    // 7. POST /api/specs/{id}/undo -> undo card
    let app = create_router(Arc::clone(&state));
    let resp = app
        .oneshot(
            Request::post(format!("/api/specs/{}/undo", spec_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "undo should return 200");

    // 8. GET /api/specs/{id}/state -> verify card gone
    let app = create_router(Arc::clone(&state));
    let resp = app
        .oneshot(
            Request::get(format!("/api/specs/{}/state", spec_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "get state after undo should return 200");
    let state_json = json_body(resp).await;
    let cards = state_json["cards"].as_object().unwrap();
    assert_eq!(cards.len(), 0, "card should be gone after undo");

    // 9. Verify exports exist on disk (write them manually since handlers don't auto-export)
    let spec_dir = home.join("specs").join(&spec_id);
    std::fs::create_dir_all(spec_dir.join("exports")).unwrap();

    // Get the state to write exports
    let actors = state.actors.read().await;
    let handle = actors.get(&spec_id.parse::<ulid::Ulid>().unwrap()).unwrap();
    let spec_state = handle.read_state().await;
    StorageManager::write_exports(&spec_dir, &spec_state).unwrap();
    drop(spec_state);
    drop(actors);

    assert!(
        spec_dir.join("exports").join("spec.md").exists(),
        "spec.md should exist"
    );
    assert!(
        spec_dir.join("exports").join("spec.yaml").exists(),
        "spec.yaml should exist"
    );
    assert!(
        spec_dir.join("exports").join("pipeline.dot").exists(),
        "pipeline.dot should exist"
    );

    // Verify spec.md content
    let md = std::fs::read_to_string(spec_dir.join("exports").join("spec.md")).unwrap();
    assert!(
        md.contains("Smoke Test Spec"),
        "spec.md should contain spec title"
    );

    // 10. GET / -> verify HTML renders
    let app = create_router(Arc::clone(&state));
    let resp = app
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "index should return 200");
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        html.contains("<!DOCTYPE html>"),
        "index should return valid HTML"
    );
    assert!(html.contains("specd"), "index should contain specd");
}
