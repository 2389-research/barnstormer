// ABOUTME: Integration tests for POST /web/specs when the form includes
// ABOUTME: optional file uploads — checks attach, binary reject, and the no-files path.

use std::sync::Arc;

use axum::body::Body;
use http::{Request, StatusCode};
use tempfile::TempDir;
use tower::ServiceExt;

use barnstormer_server::{AppState, ProviderStatus, SharedState, create_router};

mod common;

fn empty_provider_status() -> ProviderStatus {
    ProviderStatus {
        default_provider: "anthropic".to_string(),
        default_model: None,
        providers: vec![],
        any_available: false,
    }
}

fn fresh_state() -> (SharedState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let state = Arc::new(AppState::new(
        tmp.path().to_path_buf(),
        empty_provider_status(),
    ));
    (state, tmp)
}

/// Build a `multipart/form-data` body carrying a `description` field plus
/// any number of `files` parts. Returns `(content_type_header, body_bytes)`.
fn multipart_body_with_files(
    description: &str,
    files: &[(&str, &str, &[u8])],
) -> (String, Vec<u8>) {
    let boundary = "----BarnstormerCreateTest";
    let mut body: Vec<u8> = Vec::new();

    body.extend_from_slice(
        format!(
            "--{boundary}\r\n\
             Content-Disposition: form-data; name=\"description\"\r\n\r\n\
             {description}\r\n"
        )
        .as_bytes(),
    );

    for (filename, mime, bytes) in files {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\n\
                 Content-Disposition: form-data; name=\"files\"; filename=\"{filename}\"\r\n\
                 Content-Type: {mime}\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }

    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    (format!("multipart/form-data; boundary={boundary}"), body)
}

#[tokio::test]
async fn create_spec_with_one_file_attaches_it() {
    let (state, _tmp) = fresh_state();
    let app = create_router(Arc::clone(&state), None);

    let (ct, body) = multipart_body_with_files(
        "Build a new thing with helpful context",
        &[("notes.md", "text/markdown", b"# Notes\nhello\n")],
    );

    let resp = app
        .oneshot(
            Request::post("/web/specs")
                .header("content-type", ct)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("create spec request");

    assert_eq!(resp.status(), StatusCode::OK);

    // Find the freshly-created spec via state and confirm the attachment landed.
    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().expect("one spec should exist")
    };
    let actors = state.actors.read().await;
    let handle = actors.get(&spec_id).expect("actor present");
    let spec_state = handle.read_state().await;

    assert_eq!(
        spec_state.context_attachments.len(),
        1,
        "expected one context attachment"
    );
    assert_eq!(spec_state.context_attachments[0].filename, "notes.md");
}

#[tokio::test]
async fn create_spec_with_binary_file_returns_415_and_creates_no_spec() {
    let (state, _tmp) = fresh_state();
    let app = create_router(Arc::clone(&state), None);

    let (ct, body) = multipart_body_with_files(
        "Build a thing",
        &[(
            "payload.bin",
            "application/octet-stream",
            &[0xff, 0xfe, 0x00, 0x01],
        )],
    );

    let resp = app
        .oneshot(
            Request::post("/web/specs")
                .header("content-type", ct)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("request");

    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    // Validation runs before spec creation, so no actor should exist.
    let actors = state.actors.read().await;
    assert!(
        actors.is_empty(),
        "no spec should be created when a file fails validation"
    );
}

#[tokio::test]
async fn create_spec_rejects_oversize_file_with_413_before_buffering_full_part() {
    // Regression: the upload path used to call `field.bytes().await`, which
    // buffers the entire multipart field before any size check runs — meaning
    // a single request could allocate up to the configured global body cap
    // before being rejected. With streaming + per-file cap enforcement, a
    // 20MB+1 payload must short-circuit to 413 and leave no spec behind.
    let (state, _tmp) = fresh_state();
    let app = create_router(Arc::clone(&state), None);

    // 20MB cap + 1 byte. The bytes are valid UTF-8 ('a'), so the only
    // possible reason for rejection is the size cap.
    const MAX_BYTES: usize = 20 * 1024 * 1024;
    let oversize: Vec<u8> = vec![b'a'; MAX_BYTES + 1];
    let (ct, body) = multipart_body_with_files(
        "Build a thing with a too-big file",
        &[("big.txt", "text/plain", &oversize)],
    );

    let resp = app
        .oneshot(
            Request::post("/web/specs")
                .header("content-type", ct)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("request");

    assert_eq!(
        resp.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "oversize file must be rejected with 413"
    );

    // And no spec should have been created — multipart parsing failed before
    // any actor was spawned.
    let actors = state.actors.read().await;
    assert!(
        actors.is_empty(),
        "no spec should be created when a file exceeds the per-file cap"
    );
}

#[tokio::test]
async fn create_spec_with_png_attaches_with_image_mime() {
    let (state, _tmp) = fresh_state();
    let app = create_router(Arc::clone(&state), None);

    let bytes = include_bytes!("fixtures/tiny.png");
    let (ct, body) = multipart_body_with_files(
        "Build a thing with a PNG",
        &[("tiny.png", "application/octet-stream", bytes)],
    );

    let resp = app
        .oneshot(
            Request::post("/web/specs")
                .header("content-type", ct)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("request");

    assert_eq!(resp.status(), StatusCode::OK);

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().expect("one spec should exist")
    };
    let actors = state.actors.read().await;
    let handle = actors.get(&spec_id).expect("actor present");
    let spec_state = handle.read_state().await;

    assert_eq!(spec_state.context_attachments.len(), 1);
    assert_eq!(spec_state.context_attachments[0].mime_type, "image/png");
}

#[tokio::test]
async fn create_spec_with_pdf_attaches_with_pdf_mime() {
    let (state, _tmp) = fresh_state();
    let app = create_router(Arc::clone(&state), None);

    let bytes = include_bytes!("fixtures/tiny.pdf");
    let (ct, body) = multipart_body_with_files(
        "Build a thing with a PDF",
        &[("tiny.pdf", "application/octet-stream", bytes)],
    );

    let resp = app
        .oneshot(
            Request::post("/web/specs")
                .header("content-type", ct)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("request");

    assert_eq!(resp.status(), StatusCode::OK);

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().expect("one spec should exist")
    };
    let actors = state.actors.read().await;
    let handle = actors.get(&spec_id).expect("actor present");
    let spec_state = handle.read_state().await;

    assert_eq!(spec_state.context_attachments.len(), 1);
    assert_eq!(
        spec_state.context_attachments[0].mime_type,
        "application/pdf"
    );
}

#[tokio::test]
async fn create_spec_with_audio_attaches_with_audio_mime() {
    let (state, _tmp) = fresh_state();
    let app = create_router(Arc::clone(&state), None);

    let bytes = include_bytes!("fixtures/tiny.wav");
    let (ct, body) = multipart_body_with_files(
        "Build a thing with audio",
        &[("tiny.wav", "application/octet-stream", bytes)],
    );

    let resp = app
        .oneshot(
            Request::post("/web/specs")
                .header("content-type", ct)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("request");

    assert_eq!(resp.status(), StatusCode::OK);

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().expect("one spec should exist")
    };
    let actors = state.actors.read().await;
    let handle = actors.get(&spec_id).expect("actor present");
    let spec_state = handle.read_state().await;

    assert_eq!(spec_state.context_attachments.len(), 1);
    let mime = &spec_state.context_attachments[0].mime_type;
    assert!(
        mime.starts_with("audio/"),
        "expected audio/* mime, got {mime}"
    );
}

#[tokio::test]
async fn create_spec_with_video_attaches_with_video_mime() {
    let (state, _tmp) = fresh_state();
    let app = create_router(Arc::clone(&state), None);

    let bytes = include_bytes!("fixtures/tiny.mp4");
    let (ct, body) = multipart_body_with_files(
        "Build a thing with video",
        &[("tiny.mp4", "application/octet-stream", bytes)],
    );

    let resp = app
        .oneshot(
            Request::post("/web/specs")
                .header("content-type", ct)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("request");

    assert_eq!(resp.status(), StatusCode::OK);

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().expect("one spec should exist")
    };
    let actors = state.actors.read().await;
    let handle = actors.get(&spec_id).expect("actor present");
    let spec_state = handle.read_state().await;

    assert_eq!(spec_state.context_attachments.len(), 1);
    let mime = &spec_state.context_attachments[0].mime_type;
    assert!(
        mime.starts_with("video/"),
        "expected video/* mime, got {mime}"
    );
}

#[tokio::test]
async fn create_spec_with_svg_writes_rasterized_png() {
    let (state, tmp) = fresh_state();
    let app = create_router(Arc::clone(&state), None);

    let svg: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\"><rect width=\"16\" height=\"16\" fill=\"red\"/></svg>";
    let (ct, body) = multipart_body_with_files(
        "Build a thing with an SVG",
        &[("logo.svg", "image/svg+xml", svg)],
    );

    let resp = app
        .oneshot(
            Request::post("/web/specs")
                .header("content-type", ct)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("request");

    assert_eq!(resp.status(), StatusCode::OK);

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().expect("one spec should exist")
    };
    let (attachment_id, mime) = {
        let actors = state.actors.read().await;
        let handle = actors.get(&spec_id).expect("actor present");
        let spec_state = handle.read_state().await;
        let att = spec_state
            .context_attachments
            .iter()
            .find(|a| a.filename == "logo.svg")
            .expect("svg attachment present");
        (att.attachment_id, att.mime_type.clone())
    };
    assert_eq!(mime, "image/svg+xml");

    let raster_path = tmp
        .path()
        .join("specs")
        .join(spec_id.to_string())
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
async fn create_spec_browser_lies_about_content_type_server_sniffs_wins() {
    // Browser claims image/png but sends PDF bytes. Server must sniff the
    // bytes and store application/pdf, not the claimed mime.
    let (state, _tmp) = fresh_state();
    let app = create_router(Arc::clone(&state), None);

    let bytes = include_bytes!("fixtures/tiny.pdf");
    let (ct, body) = multipart_body_with_files(
        "Build a thing with a sneaky upload",
        &[("sneaky.png", "image/png", bytes)],
    );

    let resp = app
        .oneshot(
            Request::post("/web/specs")
                .header("content-type", ct)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("request");

    assert_eq!(resp.status(), StatusCode::OK);

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().expect("one spec should exist")
    };
    let actors = state.actors.read().await;
    let handle = actors.get(&spec_id).expect("actor present");
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
async fn create_spec_with_no_files_works_as_before() {
    let (state, _tmp) = fresh_state();
    let app = create_router(Arc::clone(&state), None);

    let (ct, body) = common::multipart_description_body("A plain spec, no files attached");

    let resp = app
        .oneshot(
            Request::post("/web/specs")
                .header("content-type", ct)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("request");

    assert_eq!(resp.status(), StatusCode::OK);

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().expect("spec should exist")
    };
    let actors = state.actors.read().await;
    let handle = actors.get(&spec_id).expect("actor present");
    let spec_state = handle.read_state().await;

    assert!(spec_state.core.is_some(), "spec core should be created");
    assert_eq!(
        spec_state.context_attachments.len(),
        0,
        "no attachments when no files were sent"
    );
}
