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

    // Content-Type is always forced to text/plain regardless of the uploaded
    // mime_type — the stored mime is attacker-controlled and a `text/html` or
    // `image/svg+xml` value would make /raw a same-origin XSS sink.
    let ct = resp
        .headers()
        .get(http::header::CONTENT_TYPE)
        .expect("Content-Type header present")
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(ct, "text/plain; charset=utf-8");

    // And nosniff so the browser doesn't override the forced type.
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
async fn get_raw_forces_text_plain_even_when_uploaded_mime_is_html() {
    // Regression: a UTF-8 file uploaded with mime_type=text/html (or
    // image/svg+xml) used to be served back with that exact Content-Type,
    // turning /raw into a same-origin stored-XSS sink. The endpoint must
    // pin Content-Type to text/plain regardless of the stored mime.
    let ctx = common::setup_with_spec_in_brainstorming().await;
    let boundary = "----BarnstormerHtmlBoundary";
    let body = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"evil.html\"\r\n\
         Content-Type: text/html\r\n\r\n\
         <script>alert('xss')</script>\r\n\
         --{boundary}--\r\n"
    );
    let upload = Request::builder()
        .method("POST")
        .uri(format!("/web/specs/{}/context", ctx.spec_id))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let upload_resp = ctx.router.clone().oneshot(upload).await.unwrap();
    assert_eq!(
        upload_resp.status(),
        StatusCode::OK,
        "upload of UTF-8 bytes labelled as text/html must succeed (the bytes are valid UTF-8)"
    );

    // Look up the new attachment id from state.
    let attachment_id = {
        let actors = ctx.state.actors.read().await;
        let handle = actors.get(&ctx.spec_id).expect("actor present").clone();
        drop(actors);
        let s = handle.read_state().await;
        s.context_attachments
            .last()
            .expect("at least one attachment")
            .attachment_id
    };

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
    assert_eq!(
        resp.headers()
            .get(http::header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap(),
        "text/plain; charset=utf-8",
        "uploaded mime=text/html must NOT leak through as Content-Type"
    );
    assert_eq!(
        resp.headers()
            .get(http::header::X_CONTENT_TYPE_OPTIONS)
            .unwrap()
            .to_str()
            .unwrap(),
        "nosniff",
    );
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
