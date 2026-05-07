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

    // Content-Type now reflects the server-sniffed mime (Phase 2). The
    // `notes.md` fixture sniffs to `text/markdown` via the extension fallback
    // in `sniff_mime`. We still send `nosniff` as defense-in-depth.
    let ct = resp
        .headers()
        .get(http::header::CONTENT_TYPE)
        .expect("Content-Type header present")
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        ct.starts_with("text/"),
        "expected a text/* mime for notes.md, got {ct}"
    );

    let nosniff = resp
        .headers()
        .get(http::header::X_CONTENT_TYPE_OPTIONS)
        .expect("X-Content-Type-Options header present")
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(nosniff, "nosniff");

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(std::str::from_utf8(&body).unwrap(), ctx.file_content);
}

#[tokio::test]
async fn download_image_serves_image_mime() {
    // Phase 2: image uploads must come back with their real Content-Type so
    // <img> tags render. `nosniff` still applies as defense-in-depth.
    let bytes = include_bytes!("fixtures/tiny.png");
    let ctx =
        common::setup_with_attachment_bytes("tiny.png", "application/octet-stream", bytes).await;

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
    assert_eq!(
        resp.headers()
            .get(http::header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap(),
        "image/png"
    );
    assert_eq!(
        resp.headers()
            .get(http::header::X_CONTENT_TYPE_OPTIONS)
            .unwrap()
            .to_str()
            .unwrap(),
        "nosniff"
    );

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), bytes.as_slice(), "PNG bytes round-trip");
}

#[tokio::test]
async fn download_pdf_serves_pdf_mime() {
    // PDFs render in <embed>/<iframe> only when served as application/pdf.
    let bytes = include_bytes!("fixtures/tiny.pdf");
    let ctx =
        common::setup_with_attachment_bytes("tiny.pdf", "application/octet-stream", bytes).await;

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
    assert_eq!(
        resp.headers()
            .get(http::header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap(),
        "application/pdf"
    );
}

#[tokio::test]
async fn download_text_still_serves_text_plain() {
    // Pre-existing text-attachment behavior under Phase 2: mime should match
    // what `sniff_mime` returns — `text/markdown` for `.md`, `text/plain` for
    // unknown text extensions, etc. Either way it stays in the `text/*` family.
    let ctx = common::setup_with_attachment_bytes("notes.md", "text/markdown", b"hi").await;

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
    let ct = resp
        .headers()
        .get(http::header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ct.starts_with("text/"), "expected a text/* mime, got {ct}");
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
