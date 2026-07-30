// ABOUTME: End-to-end smoke test for the full barnstormer lifecycle.
// ABOUTME: Tests spec creation, card CRUD, undo, state verification, and export generation.

use std::sync::Arc;

use axum::body::Body;
use barnstormer_server::{AppState, ProviderStatus, create_router};
use barnstormer_store::StorageManager;
use http::Request;
use tower::ServiceExt;

/// Helper to create a test AppState with a temp directory.
fn test_app_state(home: std::path::PathBuf) -> Arc<AppState> {
    Arc::new(AppState::new(home, ProviderStatus::detect()))
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
    let app = create_router(Arc::clone(&state), None);
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
    let app = create_router(Arc::clone(&state), None);
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
    let app = create_router(Arc::clone(&state), None);
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
    let app = create_router(Arc::clone(&state), None);
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
    let app = create_router(Arc::clone(&state), None);
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
    let app = create_router(Arc::clone(&state), None);
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
    assert!(
        html.contains("barnstormer"),
        "index should contain barnstormer"
    );
}

/// Helper: GET the left-rail spec list partial as a string.
async fn rail_html(state: &Arc<AppState>) -> String {
    let resp = create_router(Arc::clone(state), None)
        .oneshot(Request::get("/web/specs").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}

/// Helper: GET a path and return (status, body string).
async fn get_text(state: &Arc<AppState>, path: &str) -> (http::StatusCode, String) {
    let resp = create_router(Arc::clone(state), None)
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

/// Helper: POST a form body, return status.
async fn post_form(state: &Arc<AppState>, path: &str, body: &'static str) -> http::StatusCode {
    create_router(Arc::clone(state), None)
        .oneshot(
            Request::post(path)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// Helper: POST with an empty body (pin/unpin/archive/unarchive), return status.
async fn post_empty(state: &Arc<AppState>, path: &str) -> http::StatusCode {
    create_router(Arc::clone(state), None)
        .oneshot(Request::post(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn web_project_management_flow() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = dir.path().to_path_buf();
    let _storage = StorageManager::new(home.clone()).unwrap();
    let state = test_app_state(home.clone());

    // Create a spec via the API (gives it a real title).
    let create_body = serde_json::json!({
        "title": "Alpha Spec",
        "one_liner": "first",
        "goal": "verify project management"
    });
    let resp = create_router(Arc::clone(&state), None)
        .oneshot(
            Request::post("/api/specs")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let spec_id = json_body(resp).await["spec_id"].as_str().unwrap().to_string();

    // Rail lists the spec.
    assert!(rail_html(&state).await.contains("Alpha Spec"));

    // Rename -> rail shows the new title.
    assert_eq!(
        post_form(&state, &format!("/web/specs/{spec_id}/rename"), "title=Renamed+Spec").await,
        200
    );
    assert!(rail_html(&state).await.contains("Renamed Spec"));

    // Empty/whitespace title -> 400.
    assert_eq!(
        post_form(&state, &format!("/web/specs/{spec_id}/rename"), "title=%20%20").await,
        400
    );

    // Archive -> gone from the rail.
    assert_eq!(post_empty(&state, &format!("/web/specs/{spec_id}/archive")).await, 200);
    assert!(!rail_html(&state).await.contains("Renamed Spec"));

    // Archive view lists it.
    let (status, arch_html) = get_text(&state, "/web/settings/archive").await;
    assert_eq!(status, 200);
    assert!(arch_html.contains("Renamed Spec"));

    // Unarchive -> back in the rail.
    assert_eq!(post_empty(&state, &format!("/web/specs/{spec_id}/unarchive")).await, 200);
    assert!(rail_html(&state).await.contains("Renamed Spec"));

    // Pin/unpin succeed and keep the spec in the rail.
    assert_eq!(post_empty(&state, &format!("/web/specs/{spec_id}/pin")).await, 200);
    assert!(rail_html(&state).await.contains("Renamed Spec"));
    assert_eq!(post_empty(&state, &format!("/web/specs/{spec_id}/unpin")).await, 200);

    // Unknown spec id -> 404 on archive (valid ULID format, not present).
    assert_eq!(
        post_empty(&state, "/web/specs/01BX5ZZKBKACTAV9WEVGEMMVRZ/archive").await,
        404
    );
}

#[tokio::test]
async fn web_context_multi_file_upload() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = dir.path().to_path_buf();
    let _storage = StorageManager::new(home.clone()).unwrap();
    let state = test_app_state(home.clone());

    // Create a spec (lands in Brainstorming, which gates context upload).
    let create_body = serde_json::json!({"title":"Ctx","one_liner":"x","goal":"y"});
    let resp = create_router(Arc::clone(&state), None)
        .oneshot(
            Request::post("/api/specs")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let spec_id = json_body(resp).await["spec_id"].as_str().unwrap().to_string();

    // Two markdown files in one multipart request, both named "file".
    let boundary = "BTESTBOUNDARY";
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"one.md\"\r\n\r\n# First file\r\n--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"two.md\"\r\n\r\n# Second file\r\n--{b}--\r\n",
        b = boundary
    );
    let resp = create_router(Arc::clone(&state), None)
        .oneshot(
            Request::post(format!("/web/specs/{spec_id}/context"))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "multi-file upload should succeed");

    // The context panel should now list both files.
    let (status, panel) = get_text(&state, &format!("/web/specs/{spec_id}/context-panel")).await;
    assert_eq!(status, 200);
    assert!(panel.contains("one.md"), "panel should show first file");
    assert!(panel.contains("two.md"), "panel should show second file");
}
