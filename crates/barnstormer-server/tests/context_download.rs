// ABOUTME: Integration tests for the GET /raw context download endpoint —
// ABOUTME: asserts content/Content-Type round-trip and 404 for removed/unknown.

use axum::body::Body;
use http::{Request, StatusCode};
use tower::ServiceExt;
use ulid::Ulid;

use barnstormer_core::Command;

mod common;

#[tokio::test]
async fn get_raw_returns_file_content() {
    let ctx = common::setup_with_attachment().await;

    let req = Request::builder()
        .method("GET")
        .uri(format!(
            "/web/specs/{}/context/{}/raw",
            ctx.spec_id, ctx.attachment_id
        ))
        .body(Body::empty())
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Content-Type mirrors the mime_type stored on the attachment (text/markdown
    // in setup_with_attachment).
    let ct = resp
        .headers()
        .get(http::header::CONTENT_TYPE)
        .expect("Content-Type header present")
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(ct, "text/markdown");

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(std::str::from_utf8(&body).unwrap(), ctx.file_content);
}

#[tokio::test]
async fn get_raw_on_removed_attachment_returns_404() {
    let ctx = common::setup_with_attachment().await;

    // Soft-remove first.
    {
        let actors = ctx.state.actors.read().await;
        let handle = actors.get(&ctx.spec_id).unwrap();
        handle
            .send_command(Command::RemoveContext {
                attachment_id: ctx.attachment_id,
            })
            .await
            .expect("remove");
    }

    let req = Request::builder()
        .method("GET")
        .uri(format!(
            "/web/specs/{}/context/{}/raw",
            ctx.spec_id, ctx.attachment_id
        ))
        .body(Body::empty())
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_raw_unknown_attachment_returns_404() {
    let ctx = common::setup_with_spec_in_brainstorming().await;
    let bogus = Ulid::new();

    let req = Request::builder()
        .method("GET")
        .uri(format!("/web/specs/{}/context/{bogus}/raw", ctx.spec_id))
        .body(Body::empty())
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
