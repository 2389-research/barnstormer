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
async fn upload_png_succeeds_and_records_image_mime() {
    let ctx = common::setup_with_spec_in_brainstorming().await;
    let bytes = include_bytes!("fixtures/tiny.png");
    let resp = common::upload_file(
        ctx.router.clone(),
        ctx.spec_id,
        "tiny.png",
        "application/octet-stream",
        bytes,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let actors = ctx.state.actors.read().await;
    let handle = actors.get(&ctx.spec_id).unwrap();
    let spec_state = handle.read_state().await;
    let att = spec_state
        .context_attachments
        .iter()
        .find(|a| a.filename == "tiny.png")
        .expect("png attachment present");
    assert_eq!(att.mime_type, "image/png");
}

#[tokio::test]
async fn upload_pdf_succeeds() {
    let ctx = common::setup_with_spec_in_brainstorming().await;
    let bytes = include_bytes!("fixtures/tiny.pdf");
    let resp = common::upload_file(
        ctx.router.clone(),
        ctx.spec_id,
        "tiny.pdf",
        "application/octet-stream",
        bytes,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let actors = ctx.state.actors.read().await;
    let handle = actors.get(&ctx.spec_id).unwrap();
    let spec_state = handle.read_state().await;
    assert!(
        spec_state
            .context_attachments
            .iter()
            .any(|a| a.mime_type == "application/pdf"),
        "expected an application/pdf attachment to be recorded"
    );
}

#[tokio::test]
async fn upload_audio_succeeds() {
    let ctx = common::setup_with_spec_in_brainstorming().await;
    let bytes = include_bytes!("fixtures/tiny.wav");
    let resp = common::upload_file(
        ctx.router.clone(),
        ctx.spec_id,
        "tiny.wav",
        "application/octet-stream",
        bytes,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let actors = ctx.state.actors.read().await;
    let handle = actors.get(&ctx.spec_id).unwrap();
    let spec_state = handle.read_state().await;
    let att = spec_state
        .context_attachments
        .iter()
        .find(|a| a.filename == "tiny.wav")
        .expect("wav attachment present");
    assert!(
        att.mime_type.starts_with("audio/"),
        "expected audio/* mime, got {}",
        att.mime_type
    );
}

#[tokio::test]
async fn upload_video_succeeds() {
    let ctx = common::setup_with_spec_in_brainstorming().await;
    let bytes = include_bytes!("fixtures/tiny.mp4");
    let resp = common::upload_file(
        ctx.router.clone(),
        ctx.spec_id,
        "tiny.mp4",
        "application/octet-stream",
        bytes,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let actors = ctx.state.actors.read().await;
    let handle = actors.get(&ctx.spec_id).unwrap();
    let spec_state = handle.read_state().await;
    let att = spec_state
        .context_attachments
        .iter()
        .find(|a| a.filename == "tiny.mp4")
        .expect("mp4 attachment present");
    assert!(
        att.mime_type.starts_with("video/"),
        "expected video/* mime, got {}",
        att.mime_type
    );
}

#[tokio::test]
async fn upload_svg_writes_rasterized_png() {
    let ctx = common::setup_with_spec_in_brainstorming().await;
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\"><rect width=\"16\" height=\"16\" fill=\"red\"/></svg>";
    let resp = common::upload_file(
        ctx.router.clone(),
        ctx.spec_id,
        "logo.svg",
        "image/svg+xml",
        svg,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let (attachment_id, mime) = {
        let actors = ctx.state.actors.read().await;
        let handle = actors.get(&ctx.spec_id).unwrap();
        let spec_state = handle.read_state().await;
        let att = spec_state
            .context_attachments
            .iter()
            .find(|a| a.filename == "logo.svg")
            .expect("svg attachment present");
        (att.attachment_id, att.mime_type.clone())
    };
    assert_eq!(mime, "image/svg+xml");

    let raster_path = ctx
        .state
        .barnstormer_home
        .join("specs")
        .join(ctx.spec_id.to_string())
        .join("context")
        .join(attachment_id.to_string())
        .join("rasterized.png");
    assert!(
        raster_path.exists(),
        "rasterized PNG should be cached on disk at {}",
        raster_path.display()
    );
}

#[tokio::test]
async fn upload_executable_returns_415() {
    let ctx = common::setup_with_spec_in_brainstorming().await;
    // PE/DOS magic — `infer` recognizes this as application/vnd.microsoft.portable-executable.
    let bytes: &[u8] =
        b"MZ\x90\x00\x03\x00\x00\x00\x04\x00\x00\x00\xff\xff\x00\x00\xb8\x00\x00\x00";
    let resp = common::upload_file(
        ctx.router.clone(),
        ctx.spec_id,
        "evil.exe",
        "application/octet-stream",
        bytes,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let actors = ctx.state.actors.read().await;
    let handle = actors.get(&ctx.spec_id).unwrap();
    let spec_state = handle.read_state().await;
    assert_eq!(
        spec_state.context_attachments.len(),
        0,
        "no state mutation on executable reject"
    );
}

#[tokio::test]
async fn upload_zip_returns_415() {
    let ctx = common::setup_with_spec_in_brainstorming().await;
    // ZIP magic with a minimal local file header tail.
    let bytes: &[u8] = b"PK\x03\x04\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
    let resp = common::upload_file(
        ctx.router.clone(),
        ctx.spec_id,
        "archive.zip",
        "application/zip",
        bytes,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let actors = ctx.state.actors.read().await;
    let handle = actors.get(&ctx.spec_id).unwrap();
    let spec_state = handle.read_state().await;
    assert_eq!(
        spec_state.context_attachments.len(),
        0,
        "no state mutation on zip reject"
    );
}

#[tokio::test]
async fn upload_browser_lies_about_content_type() {
    // Browser claims image/png; payload is actually a PDF. Server must sniff
    // the bytes and store application/pdf, not the claimed mime.
    let ctx = common::setup_with_spec_in_brainstorming().await;
    let bytes = include_bytes!("fixtures/tiny.pdf");
    let resp = common::upload_file(
        ctx.router.clone(),
        ctx.spec_id,
        "sneaky.png",
        "image/png",
        bytes,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let actors = ctx.state.actors.read().await;
    let handle = actors.get(&ctx.spec_id).unwrap();
    let spec_state = handle.read_state().await;
    let att = spec_state
        .context_attachments
        .iter()
        .find(|a| a.filename == "sneaky.png")
        .expect("attachment present");
    assert_eq!(
        att.mime_type, "application/pdf",
        "server should ignore browser-claimed mime and store the sniffed one"
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
