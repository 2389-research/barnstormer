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
async fn download_html_serves_text_plain_to_neuter_xss() {
    // HTML uploads are stored as bytes but served back as text/plain to
    // neuter stored-XSS via direct navigation to /raw. Other types
    // (image/*, application/pdf, audio/*, video/*) keep their real mime.
    let ctx = common::setup_with_attachment_bytes(
        "evil.html",
        "text/html",
        b"<script>alert('xss')</script>",
    )
    .await;

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
        .expect("Content-Type header present")
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        ct.starts_with("text/plain"),
        "expected text/plain, got {ct}"
    );

    let nosniff = resp
        .headers()
        .get(http::header::X_CONTENT_TYPE_OPTIONS)
        .expect("X-Content-Type-Options header present")
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(nosniff, "nosniff");
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

#[tokio::test]
async fn download_svg_serves_rasterized_png_to_neuter_xss() {
    // SVG attachments are served back as the cached rasterized.png on the
    // /raw endpoint to neuter direct-navigation script execution. The
    // original SVG is preserved on disk; this only affects /raw output.
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\"><rect width=\"16\" height=\"16\" fill=\"red\"/></svg>";
    let ctx = common::setup_with_attachment_bytes("logo.svg", "image/svg+xml", svg).await;

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
        .expect("Content-Type header present")
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(
        ct, "image/png",
        "expected SVG to be served back as image/png from cached raster, got {ct}"
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
    // PNG magic: \x89 P N G \r \n \x1a \n
    assert!(
        body.starts_with(b"\x89PNG\r\n\x1a\n"),
        "response body should start with PNG magic; got: {:?}",
        &body[..body.len().min(16)]
    );
}

#[tokio::test]
async fn download_svg_without_raster_serves_text_plain_fallback() {
    // If the rasterized cache is missing (rasterization failed at upload),
    // the /raw endpoint falls back to text/plain so direct nav can't
    // execute the SVG's embedded scripts. We construct the attachment
    // directly via Command::AttachContext so no auto-raster fires.
    use barnstormer_core::SpecPhase;

    let ctx = common::setup_with_spec_in_brainstorming().await;
    let attachment_id = Ulid::new();

    // Write the SVG bytes to disk where download_context expects them, but
    // skip rasterized.png on purpose so we exercise the missing-cache path.
    let dir = ctx
        .state
        .barnstormer_home
        .join("specs")
        .join(ctx.spec_id.to_string())
        .join("context")
        .join(attachment_id.to_string());
    std::fs::create_dir_all(&dir).unwrap();
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\"><rect/></svg>";
    std::fs::write(dir.join("logo.svg"), svg).unwrap();

    // Send AttachContext via the actor handle directly. Matches how
    // setup_with_attachment_bytes leaves the actor state, minus the upload
    // pipeline's auto-rasterization.
    let handle = {
        let actors = ctx.state.actors.read().await;
        actors.get(&ctx.spec_id).expect("actor present").clone()
    };
    let phase = handle.read_state().await.phase.clone();
    assert_eq!(phase, SpecPhase::Brainstorming);
    handle
        .send_command(Command::AttachContext {
            attachment_id,
            filename: "logo.svg".to_string(),
            mime_type: "image/svg+xml".to_string(),
            size_bytes: svg.len() as u64,
        })
        .await
        .expect("attach via command");

    let req = Request::builder()
        .method("GET")
        .uri(format!(
            "/web/specs/{}/context/{}/raw",
            ctx.spec_id, attachment_id
        ))
        .body(Body::empty())
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let ct = resp
        .headers()
        .get(http::header::CONTENT_TYPE)
        .expect("Content-Type header present")
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        ct.starts_with("text/plain"),
        "expected text/plain fallback when rasterized.png is missing, got {ct}"
    );
}
