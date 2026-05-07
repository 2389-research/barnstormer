// ABOUTME: Integration tests for POST /web/specs/{id}/context/{att_id}/resummarize.
// ABOUTME: Covers happy path (spawn fires) and the 404/410 error cases.

use http::StatusCode;
use ulid::Ulid;

mod common;

#[tokio::test]
async fn resummarize_unknown_attachment_returns_404() {
    // Spec exists, attachment id is fabricated — handler should 404.
    let ctx = common::setup_with_spec_in_brainstorming().await;
    let bogus = Ulid::new();

    let resp = common::post(
        ctx.router.clone(),
        &format!("/web/specs/{}/context/{bogus}/resummarize", ctx.spec_id),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn resummarize_removed_attachment_returns_410() {
    // Soft-remove the attachment first, then hit resummarize. Since the
    // attachment exists in state but is tombstoned, the handler should
    // return 410 Gone (not 404 — we know it existed).
    let ctx = common::setup_with_attachment().await;

    let del_resp =
        common::delete_attachment(ctx.router.clone(), ctx.spec_id, ctx.attachment_id).await;
    assert_eq!(del_resp.status(), StatusCode::OK);

    let resp = common::post(
        ctx.router.clone(),
        &format!(
            "/web/specs/{}/context/{}/resummarize",
            ctx.spec_id, ctx.attachment_id
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::GONE);
}

#[tokio::test]
async fn resummarize_live_attachment_spawns_summarizer() {
    // The upload itself fires one `spawn_summarize`. Resummarize should fire
    // another, so the global spawn counter must advance.
    let ctx = common::setup_with_attachment().await;

    // Settle the upload's summarize spawn (the spawn counter is incremented
    // synchronously, but sleeping briefly makes the test less racy when the
    // spawned task is also running concurrent work).
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let count_before = common::summarize_spawn_count();

    let resp = common::post(
        ctx.router.clone(),
        &format!(
            "/web/specs/{}/context/{}/resummarize",
            ctx.spec_id, ctx.attachment_id
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let count_after = common::summarize_spawn_count();
    assert!(
        count_after > count_before,
        "POST .../resummarize should fire a fresh summarize; count was {count_before} before, {count_after} after",
    );
}
