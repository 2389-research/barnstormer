// ABOUTME: Integration tests for the GET /context-preview endpoint —
// ABOUTME: asserts the read-only preview mirrors the agent's "## Context Files" section.

use axum::body::Body;
use http::{Request, StatusCode};
use tower::ServiceExt;

mod common;

#[tokio::test]
async fn preview_contains_attachment_filename_and_summary_placeholder() {
    let ctx = common::setup_with_attachment().await;

    let req = Request::builder()
        .method("GET")
        .uri(format!("/web/specs/{}/context-preview", ctx.spec_id))
        .body(Body::empty())
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = std::str::from_utf8(&body).unwrap();

    // Filename (set by setup_with_attachment) must appear in the preview.
    assert!(
        html.contains(&ctx.filename),
        "preview should include filename; got: {html}"
    );
    // The attachment's summary is async — immediately after upload it is still
    // None, so the preview shows the "being summarized" placeholder.
    assert!(
        html.contains("being summarized"),
        "preview should show the 'being summarized' placeholder for a freshly-attached file; got: {html}"
    );
}

#[tokio::test]
async fn preview_without_attachments_shows_empty_state() {
    let ctx = common::setup_with_spec_in_brainstorming().await;

    let req = Request::builder()
        .method("GET")
        .uri(format!("/web/specs/{}/context-preview", ctx.spec_id))
        .body(Body::empty())
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = std::str::from_utf8(&body).unwrap();
    assert!(
        html.contains("No context files attached"),
        "empty preview should show explicit empty-state message; got: {html}"
    );
}
