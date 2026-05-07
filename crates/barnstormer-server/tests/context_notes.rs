// ABOUTME: Integration tests for the PATCH notes endpoint — exercises happy
// ABOUTME: path, removed-attachment conflict, and unknown-attachment 404.

use axum::body::Body;
use http::{Request, StatusCode};
use tower::ServiceExt;
use ulid::Ulid;

use barnstormer_core::Command;

mod common;

#[tokio::test]
async fn patch_notes_updates_attachment() {
    let ctx = common::setup_with_attachment().await;

    let req = Request::builder()
        .method("PATCH")
        .uri(format!(
            "/web/specs/{}/context/{}/notes",
            ctx.spec_id, ctx.attachment_id
        ))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("notes=hello"))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify state-side: user_notes was set.
    let actors = ctx.state.actors.read().await;
    let handle = actors.get(&ctx.spec_id).unwrap();
    let spec_state = handle.read_state().await;
    let att = spec_state
        .context_attachments
        .iter()
        .find(|a| a.attachment_id == ctx.attachment_id)
        .expect("attachment present");
    assert_eq!(att.user_notes.as_deref(), Some("hello"));
}

#[tokio::test]
async fn patch_notes_on_removed_attachment_returns_409() {
    let ctx = common::setup_with_attachment().await;

    // Soft-remove via the actor handle directly (endpoint for this is T10).
    {
        let actors = ctx.state.actors.read().await;
        let handle = actors.get(&ctx.spec_id).unwrap();
        handle
            .send_command(Command::RemoveContext {
                attachment_id: ctx.attachment_id,
            })
            .await
            .expect("remove via command");
    }

    let req = Request::builder()
        .method("PATCH")
        .uri(format!(
            "/web/specs/{}/context/{}/notes",
            ctx.spec_id, ctx.attachment_id
        ))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("notes=after-remove"))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn patch_notes_unknown_attachment_returns_404() {
    let ctx = common::setup_with_spec_in_brainstorming().await;
    let bogus = Ulid::new();

    let req = Request::builder()
        .method("PATCH")
        .uri(format!("/web/specs/{}/context/{bogus}/notes", ctx.spec_id))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("notes=whatever"))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn updating_notes_triggers_resummarize() {
    // Upload happens inside `setup_with_attachment`, which itself fires one
    // `spawn_summarize`. The PATCH afterward should fire another, so we expect
    // the spawn counter to have advanced.
    let ctx = common::setup_with_attachment().await;

    // Settle the upload's summarize spawn (synchronous increment, but the
    // spawned task is async — sleep just lets any racing spawn flush).
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let count_before = common::summarize_spawn_count();

    let resp = common::patch_notes(
        ctx.router.clone(),
        ctx.spec_id,
        ctx.attachment_id,
        "the vibes we want",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let count_after = common::summarize_spawn_count();
    assert!(
        count_after > count_before,
        "PATCH notes should fire a fresh summarize; count was {count_before} before, {count_after} after",
    );
}
