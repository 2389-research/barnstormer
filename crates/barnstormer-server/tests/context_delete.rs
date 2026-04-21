// ABOUTME: Integration tests for the DELETE context endpoint — verifies
// ABOUTME: soft-remove semantics, double-delete conflict, and 404 for unknown ids.

use axum::body::Body;
use http::{Request, StatusCode};
use tower::ServiceExt;
use ulid::Ulid;

mod common;

#[tokio::test]
async fn delete_context_soft_removes_attachment() {
    let ctx = common::setup_with_attachment().await;

    // Preflight: file must exist on disk after upload.
    let on_disk_path = {
        let actors = ctx.state.actors.read().await;
        let handle = actors.get(&ctx.spec_id).unwrap();
        let spec_state = handle.read_state().await;
        let att = &spec_state.context_attachments[0];
        barnstormer_server::context_storage::attachment_path(
            &ctx.state.barnstormer_home,
            ctx.spec_id,
            att.attachment_id,
            &att.filename,
        )
    };
    assert!(on_disk_path.exists(), "attachment file should exist before delete");

    let req = Request::builder()
        .method("DELETE")
        .uri(format!(
            "/web/specs/{}/context/{}",
            ctx.spec_id, ctx.attachment_id
        ))
        .body(Body::empty())
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Body: re-rendered context panel — attachment was the only one, so the
    // panel should now show the empty-state message.
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read body");
    let body_str = std::str::from_utf8(&body).expect("utf-8 body");
    assert!(body_str.contains("chat-panel"), "expected panel container in response body");
    assert!(
        body_str.contains("No context files yet"),
        "expected empty-state message in response body"
    );

    // State: attachment still present (soft-removed), removed=true.
    let actors = ctx.state.actors.read().await;
    let handle = actors.get(&ctx.spec_id).unwrap();
    let spec_state = handle.read_state().await;
    assert_eq!(
        spec_state.context_attachments.len(),
        1,
        "soft-delete keeps the attachment in the vec"
    );
    let att = &spec_state.context_attachments[0];
    assert_eq!(att.attachment_id, ctx.attachment_id);
    assert!(att.removed, "removed flag should be true");

    // Disk: file still present so undo can restore it.
    assert!(
        on_disk_path.exists(),
        "soft-delete must preserve the file on disk for undo"
    );
}

#[tokio::test]
async fn delete_unknown_attachment_returns_404() {
    let ctx = common::setup_with_spec_in_brainstorming().await;
    let bogus = Ulid::new();

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/web/specs/{}/context/{bogus}", ctx.spec_id))
        .body(Body::empty())
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_twice_returns_409() {
    let ctx = common::setup_with_attachment().await;

    let first = Request::builder()
        .method("DELETE")
        .uri(format!(
            "/web/specs/{}/context/{}",
            ctx.spec_id, ctx.attachment_id
        ))
        .body(Body::empty())
        .unwrap();
    let first_resp = ctx.router.clone().oneshot(first).await.unwrap();
    assert_eq!(first_resp.status(), StatusCode::OK);

    let second = Request::builder()
        .method("DELETE")
        .uri(format!(
            "/web/specs/{}/context/{}",
            ctx.spec_id, ctx.attachment_id
        ))
        .body(Body::empty())
        .unwrap();
    let second_resp = ctx.router.clone().oneshot(second).await.unwrap();
    assert_eq!(second_resp.status(), StatusCode::CONFLICT);
}
