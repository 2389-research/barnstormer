// ABOUTME: End-to-end smoke test for the full context-attachment lifecycle:
// ABOUTME: upload -> patch notes -> read panel -> delete -> undo -> re-download.

use axum::body::Body;
use axum::body::to_bytes;
use http::{Request, StatusCode};
use tower::ServiceExt;

mod common;

#[tokio::test]
async fn smoke_full_context_lifecycle() {
    let ctx = common::setup_with_spec_in_brainstorming().await;

    // 1. Upload a file via multipart POST.
    let boundary = "----BarnstormerTest";
    let upload_body = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"lifecycle.md\"\r\n\
         Content-Type: text/markdown\r\n\r\n\
         # Lifecycle test content\r\n\
         --{boundary}--\r\n"
    );
    let req = Request::builder()
        .method("POST")
        .uri(format!("/web/specs/{}/context", ctx.spec_id))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(upload_body))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "upload should succeed");

    // Pull the attachment_id out of state (single entry, push order).
    let attachment_id = {
        let actors = ctx.state.actors.read().await;
        let handle = actors.get(&ctx.spec_id).unwrap();
        let state = handle.read_state().await;
        assert_eq!(state.context_attachments.len(), 1);
        assert_eq!(state.context_attachments[0].filename, "lifecycle.md");
        state.context_attachments[0].attachment_id
    };

    // 2. Update notes via PATCH. Form body uses `+` which decodes to space.
    let req = Request::builder()
        .method("PATCH")
        .uri(format!(
            "/web/specs/{}/context/{}/notes",
            ctx.spec_id, attachment_id
        ))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("notes=kickoff+notes"))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "notes patch should succeed");

    {
        let actors = ctx.state.actors.read().await;
        let handle = actors.get(&ctx.spec_id).unwrap();
        let state = handle.read_state().await;
        assert_eq!(
            state.context_attachments[0].user_notes.as_deref(),
            Some("kickoff notes"),
            "form decoding should turn + into space"
        );
    }

    // 3. GET the context panel — confirms filename + notes render.
    let req = Request::builder()
        .method("GET")
        .uri(format!("/web/specs/{}/context-panel", ctx.spec_id))
        .body(Body::empty())
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "panel GET should succeed");
    let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let html = std::str::from_utf8(&body).unwrap();
    assert!(html.contains("lifecycle.md"), "panel should show filename");
    assert!(
        html.contains("kickoff notes"),
        "panel should render the saved notes"
    );

    // 4. DELETE — soft-removes the attachment.
    let req = Request::builder()
        .method("DELETE")
        .uri(format!(
            "/web/specs/{}/context/{}",
            ctx.spec_id, attachment_id
        ))
        .body(Body::empty())
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "delete should succeed");

    {
        let actors = ctx.state.actors.read().await;
        let handle = actors.get(&ctx.spec_id).unwrap();
        let state = handle.read_state().await;
        assert!(
            state.context_attachments[0].removed,
            "attachment should be tombstoned after delete"
        );
    }

    // 5. UNDO — should un-tombstone without duplicating the entry
    // (this is the T4 regression fix).
    let req = Request::builder()
        .method("POST")
        .uri(format!("/web/specs/{}/undo", ctx.spec_id))
        .body(Body::empty())
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "undo should succeed");

    {
        let actors = ctx.state.actors.read().await;
        let handle = actors.get(&ctx.spec_id).unwrap();
        let state = handle.read_state().await;
        assert_eq!(
            state.context_attachments.len(),
            1,
            "undo must not duplicate the attachment entry"
        );
        assert!(
            !state.context_attachments[0].removed,
            "undo should restore removed=false"
        );
    }

    // 6. GET raw content after undo — confirms file survives the delete/undo cycle.
    let req = Request::builder()
        .method("GET")
        .uri(format!(
            "/web/specs/{}/context/{}/raw",
            ctx.spec_id, attachment_id
        ))
        .body(Body::empty())
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "raw download should succeed after undo"
    );
    let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let text = std::str::from_utf8(&body).unwrap();
    assert!(
        text.contains("Lifecycle test content"),
        "downloaded content should match original upload"
    );
}
