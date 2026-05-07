// ABOUTME: Shared integration-test helpers — sets up an in-memory router with
// ABOUTME: a temp BARNSTORMER_HOME and a created spec for context-upload tests.

#![allow(dead_code)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use http::Request;
use tempfile::TempDir;
use tower::ServiceExt;
use ulid::Ulid;

use barnstormer_core::{Command, SpecPhase};
use barnstormer_server::{AppState, ProviderStatus, SharedState, create_router};

/// Context returned from `setup_*` helpers. Holds the assembled router, the
/// shared `AppState`, the created spec's ULID, and keeps the tempdir alive
/// for the duration of the test.
pub struct TestCtx {
    pub router: Router,
    pub state: SharedState,
    pub spec_id: Ulid,
    pub _tmp: TempDir,
}

fn empty_provider_status() -> ProviderStatus {
    ProviderStatus {
        default_provider: "anthropic".to_string(),
        default_model: None,
        providers: vec![],
        any_available: false,
    }
}

/// Build a `SharedState` rooted at a fresh tempdir, plus the owning TempDir
/// handle (callers must keep it alive to prevent cleanup mid-test).
fn make_state() -> (SharedState, TempDir) {
    let tmp = TempDir::new().expect("create tempdir");
    let state = Arc::new(AppState::new(
        tmp.path().to_path_buf(),
        empty_provider_status(),
    ));
    (state, tmp)
}

/// Build a `multipart/form-data` body containing just a `description`
/// field — enough for tests that only need a spec created, without any
/// context files attached. Returns `(content_type_header, body_bytes)`.
pub fn multipart_description_body(description: &str) -> (String, Vec<u8>) {
    let boundary = "----BarnstormerCommonBoundary";
    let body = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"description\"\r\n\r\n\
         {description}\r\n\
         --{boundary}--\r\n"
    );
    (
        format!("multipart/form-data; boundary={boundary}"),
        body.into_bytes(),
    )
}

/// Create a spec via `POST /web/specs` and return its ULID. Uses the web
/// form handler so the spec is fully wired (actor registered, etc.), matching
/// how production routes create specs.
async fn create_spec_via_web(router: Router) -> () {
    let (ct, body) = multipart_description_body("Context upload test");
    let resp = router
        .oneshot(
            Request::post("/web/specs")
                .header("content-type", ct)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("create spec request");
    assert!(
        resp.status().is_success() || resp.status().is_redirection(),
        "create_spec returned {}",
        resp.status()
    );
}

/// Build a `TestCtx` with a single spec that's in `SpecPhase::Brainstorming`
/// (the default post-creation phase).
pub async fn setup_with_spec_in_brainstorming() -> TestCtx {
    let (state, tmp) = make_state();

    // Create a spec via the web form; new specs start in Brainstorming.
    create_spec_via_web(create_router(Arc::clone(&state), None)).await;

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().expect("spec should have been created")
    };

    TestCtx {
        router: create_router(Arc::clone(&state), None),
        state,
        spec_id,
        _tmp: tmp,
    }
}

/// Build a `TestCtx` with a spec transitioned into `SpecPhase::Refining`.
pub async fn setup_with_spec_in_active() -> TestCtx {
    let ctx = setup_with_spec_in_brainstorming().await;

    // Transition directly via the actor handle — avoids the web form roundtrip.
    // Clone the handle and drop the read-guard before awaiting the command so
    // we don't hold the actors lock across an await point.
    let handle = {
        let actors = ctx.state.actors.read().await;
        actors.get(&ctx.spec_id).expect("actor present").clone()
    };
    handle
        .send_command(Command::TransitionPhase {
            target: SpecPhase::Refining,
        })
        .await
        .expect("transition to refining");

    // Rebuild the router with the same state so each test gets a fresh service.
    TestCtx {
        router: create_router(Arc::clone(&ctx.state), None),
        state: ctx.state,
        spec_id: ctx.spec_id,
        _tmp: ctx._tmp,
    }
}

/// Upload a file via the real `POST /web/specs/{id}/context` endpoint using
/// a synthesized multipart body. The browser-claimed `content_type` is what
/// the multipart `Content-Type` header for the `file` part says — the server
/// is expected to sniff and ignore it. Returns the raw `Response` so callers
/// can inspect status and body.
pub async fn upload_file(
    router: Router,
    spec_id: Ulid,
    filename: &str,
    claimed_mime: &str,
    bytes: &[u8],
) -> http::Response<Body> {
    let boundary = "----BarnstormerUploadHelper";
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\n\
             Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n\
             Content-Type: {claimed_mime}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let req = Request::builder()
        .method("POST")
        .uri(format!("/web/specs/{spec_id}/context"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();

    router.oneshot(req).await.expect("upload request")
}

/// Context returned from `setup_with_attachment`. Includes everything from
/// `TestCtx` plus the attachment id, the expected filename on disk, and the
/// exact bytes the upload endpoint received.
pub struct AttachmentCtx {
    pub router: Router,
    pub state: SharedState,
    pub spec_id: Ulid,
    pub attachment_id: Ulid,
    pub filename: String,
    pub file_content: &'static str,
    pub _tmp: TempDir,
}

/// Build an `AttachmentCtx`: a brainstorming spec with exactly one text
/// attachment, uploaded through the real HTTP endpoint so the disk layout and
/// state events match production. The attachment id is discovered by reading
/// the actor's state after the upload.
pub async fn setup_with_attachment() -> AttachmentCtx {
    const FILE_CONTENT: &str = "# Context notes\nhello world\n";
    const FILENAME: &str = "notes.md";

    let ctx = setup_with_spec_in_brainstorming().await;

    // Synthesize a minimal multipart/form-data body matching the upload handler.
    let boundary = "----BarnstormerCommonBoundary";
    let body = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"{FILENAME}\"\r\n\
         Content-Type: text/markdown\r\n\r\n\
         {FILE_CONTENT}\r\n\
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

    let resp = ctx
        .router
        .clone()
        .oneshot(req)
        .await
        .expect("upload request");
    assert_eq!(
        resp.status(),
        http::StatusCode::OK,
        "upload in setup_with_attachment must succeed"
    );

    // Pull the attachment_id out of state now that the event has landed.
    // Clone the handle and drop the actors guard before awaiting `read_state`
    // so we never hold the lock across an await point.
    let handle = {
        let actors = ctx.state.actors.read().await;
        actors.get(&ctx.spec_id).expect("actor present").clone()
    };
    let attachment_id = {
        let spec_state = handle.read_state().await;
        assert_eq!(
            spec_state.context_attachments.len(),
            1,
            "setup_with_attachment expected exactly one attachment"
        );
        spec_state.context_attachments[0].attachment_id
    };

    AttachmentCtx {
        // Fresh router so each test gets its own service instance.
        router: create_router(Arc::clone(&ctx.state), None),
        state: ctx.state,
        spec_id: ctx.spec_id,
        attachment_id,
        filename: FILENAME.to_string(),
        file_content: FILE_CONTENT,
        _tmp: ctx._tmp,
    }
}

/// Snapshot the test-only `SUMMARIZE_SPAWN_COUNT` atomic. Lets event-driven
/// tests (notes-update fan-out, manual Resummarize) assert that a summarize
/// task was actually spawned without standing up a real LLM client.
pub fn summarize_spawn_count() -> usize {
    barnstormer_server::summarizer::SUMMARIZE_SPAWN_COUNT.load(std::sync::atomic::Ordering::Relaxed)
}

/// PATCH the notes endpoint with a url-encoded form body. Mirrors how the
/// browser submits the auto-save blur from the context panel — the handler
/// expects `Form<NotesForm>` (i.e. `application/x-www-form-urlencoded`).
///
/// The `notes` value is percent-encoded for the body so spaces, `&`, and `=`
/// round-trip through the form decoder unchanged.
pub async fn patch_notes(
    router: Router,
    spec_id: Ulid,
    attachment_id: Ulid,
    notes: &str,
) -> http::Response<Body> {
    let body = format!("notes={}", percent_encode_form_value(notes));
    let req = Request::builder()
        .method("PATCH")
        .uri(format!(
            "/web/specs/{spec_id}/context/{attachment_id}/notes"
        ))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    router.oneshot(req).await.expect("patch notes request")
}

/// POST a path with an empty body. Used by tests that need to hit a fire-and-
/// forget endpoint (like `.../resummarize`) without crafting a multipart or
/// form body. Returns the raw response for status / body inspection.
pub async fn post(router: Router, path: &str) -> http::Response<Body> {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .body(Body::empty())
        .unwrap();
    router.oneshot(req).await.expect("post request")
}

/// DELETE the context attachment via the real HTTP endpoint. Mirrors the
/// browser's "Remove" affordance — soft-removes the attachment and returns
/// the re-rendered panel.
pub async fn delete_attachment(
    router: Router,
    spec_id: Ulid,
    attachment_id: Ulid,
) -> http::Response<Body> {
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/web/specs/{spec_id}/context/{attachment_id}"))
        .body(Body::empty())
        .unwrap();
    router
        .oneshot(req)
        .await
        .expect("delete attachment request")
}

/// Minimal `application/x-www-form-urlencoded` value encoder — enough for the
/// integration test surface. Spaces become `+`; everything outside the
/// unreserved set becomes a `%HH` triple. Reaching for the `url` crate just
/// for this would inflate dev-dependencies.
fn percent_encode_form_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{:02X}", other)),
        }
    }
    out
}
