// ABOUTME: Integration test for context file upload endpoint — verifies
// ABOUTME: multipart parsing, disk write, and ContextAttached event emission.

use axum::body::Body;
use http::{Request, StatusCode};
use tower::ServiceExt;

mod common;

#[tokio::test]
async fn upload_text_file_emits_attached_event() {
    let ctx = common::setup_with_spec_in_brainstorming().await;

    let boundary = "----BarnstormerTest";
    let body = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"notes.md\"\r\n\
         Content-Type: text/markdown\r\n\r\n\
         # Hello\n\r\n\
         --{boundary}--\r\n"
    );

    let req = Request::builder()
        .method("POST")
        .uri(format!("/web/specs/{}/context", ctx.spec_id))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Response body is now the re-rendered context panel partial — it should
    // contain the uploaded filename and the panel's container class.
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read body");
    let body_str = std::str::from_utf8(&body).expect("utf-8 body");
    assert!(
        body_str.contains("chat-panel"),
        "expected panel container in response body"
    );
    assert!(
        body_str.contains("notes.md"),
        "expected filename in response body"
    );

    // Verify the event landed in state.
    let actors = ctx.state.actors.read().await;
    let handle = actors.get(&ctx.spec_id).unwrap();
    let spec_state = handle.read_state().await;
    assert_eq!(spec_state.context_attachments.len(), 1);
    assert_eq!(spec_state.context_attachments[0].filename, "notes.md");
}

#[tokio::test]
async fn upload_binary_file_returns_415() {
    let ctx = common::setup_with_spec_in_brainstorming().await;
    let boundary = "----BarnstormerTest";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\n\
             Content-Disposition: form-data; name=\"file\"; filename=\"a.bin\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&[0xff, 0xfe, 0x00, 0x01]);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let req = Request::builder()
        .method("POST")
        .uri(format!("/web/specs/{}/context", ctx.spec_id))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let actors = ctx.state.actors.read().await;
    let handle = actors.get(&ctx.spec_id).unwrap();
    let spec_state = handle.read_state().await;
    assert_eq!(
        spec_state.context_attachments.len(),
        0,
        "no state mutation on binary reject"
    );
}

#[tokio::test]
async fn upload_outside_brainstorming_returns_409() {
    let ctx = common::setup_with_spec_in_active().await;
    let boundary = "----BarnstormerTest";
    let body = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"a.md\"\r\n\
         Content-Type: text/markdown\r\n\r\n\
         data\r\n\
         --{boundary}--\r\n"
    );
    let req = Request::builder()
        .method("POST")
        .uri(format!("/web/specs/{}/context", ctx.spec_id))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}
