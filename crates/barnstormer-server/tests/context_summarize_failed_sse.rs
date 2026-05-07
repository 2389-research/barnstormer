// ABOUTME: Verifies that Command::MarkContextSummarizeFailed produces an SSE
// ABOUTME: event with the wire name `context_summarize_failed`.

use std::time::Duration;

use axum::body::Body;
use barnstormer_core::Command;
use http::{Request, StatusCode};
use tower::ServiceExt;

mod common;

/// Read SSE response bytes until the accumulated text contains all the
/// supplied needles, or the timeout elapses. Returns the accumulated UTF-8
/// string. Used to scan for `event: <name>` plus any payload markers — the
/// upload path may auto-fire a summarize task whose failure event hits the
/// stream first, so we keep consuming until the specific event we triggered
/// lands.
async fn collect_sse_until_all(
    body: Body,
    needles: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    let mut accumulated = String::new();
    let deadline = tokio::time::Instant::now() + timeout;

    let mut stream = body.into_data_stream();
    loop {
        if needles.iter().all(|n| accumulated.contains(n)) {
            return Ok(accumulated);
        }
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .ok_or_else(|| {
                format!("timed out waiting for SSE markers {needles:?}; got: {accumulated}")
            })?;
        let next = tokio::time::timeout(remaining, futures::StreamExt::next(&mut stream)).await;
        match next {
            Ok(Some(Ok(chunk))) => {
                accumulated.push_str(&String::from_utf8_lossy(&chunk));
            }
            Ok(Some(Err(e))) => return Err(format!("stream error: {e}")),
            Ok(None) => {
                return Err(format!(
                    "stream ended before markers {needles:?} arrived; got: {accumulated}"
                ));
            }
            Err(_) => {
                return Err(format!(
                    "timed out waiting for SSE markers {needles:?}; got: {accumulated}"
                ));
            }
        }
    }
}

#[tokio::test]
async fn mark_summarize_failed_emits_named_sse_event() {
    // Build a brainstorming spec, then attach a context entry via the actor
    // handle directly — avoiding the upload HTTP path so we don't kick off the
    // auto-summarize background task whose own ContextSummarizeFailed (from
    // missing ANTHROPIC_API_KEY in test env) would otherwise crowd the stream.
    let ctx = common::setup_with_spec_in_brainstorming().await;
    let handle = {
        let actors = ctx.state.actors.read().await;
        actors.get(&ctx.spec_id).expect("actor present").clone()
    };
    let attachment_id = ulid::Ulid::new();
    handle
        .send_command(Command::AttachContext {
            attachment_id,
            filename: "img.png".into(),
            mime_type: "image/png".into(),
            size_bytes: 1,
        })
        .await
        .expect("AttachContext should succeed");

    // Open the SSE stream BEFORE firing the command so the broadcast subscriber
    // is registered in time to receive the event.
    let stream_req = Request::builder()
        .method("GET")
        .uri(format!("/api/specs/{}/events/stream", ctx.spec_id))
        .body(Body::empty())
        .unwrap();
    let stream_resp = ctx
        .router
        .clone()
        .oneshot(stream_req)
        .await
        .expect("open SSE stream");
    assert_eq!(stream_resp.status(), StatusCode::OK);

    // Give the SSE handler a moment to register its subscriber on the broadcast
    // channel before we send the command. Without this, the command's event can
    // race past the subscriber and never reach the stream.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // A reason string with a sentinel substring distinct enough that any other
    // failure event's reason won't accidentally satisfy the assertion.
    const REASON: &str = "wire-name-test-sentinel: provider rejected payload";

    // Fire MarkContextSummarizeFailed via the actor handle — same path the
    // server uses when the LLM call fails.
    handle
        .send_command(Command::MarkContextSummarizeFailed {
            attachment_id,
            reason: REASON.into(),
        })
        .await
        .expect("MarkContextSummarizeFailed should succeed");

    // Read the SSE response body until we see `event: context_summarize_failed`
    // accompanied by our sentinel reason and the attachment id.
    let collected = collect_sse_until_all(
        stream_resp.into_body(),
        &[
            "event: context_summarize_failed\n",
            &attachment_id.to_string(),
            REASON,
        ],
        Duration::from_secs(3),
    )
    .await
    .expect("expected named SSE event to arrive");

    // The collected buffer should contain a `data:` line carrying the JSON
    // payload right after the named event.
    assert!(
        collected.contains("\"type\":\"ContextSummarizeFailed\""),
        "JSON data payload must carry the ContextSummarizeFailed variant; got: {collected}"
    );

    // And the actor's state should reflect the failure so subsequent panel
    // re-renders show the card-error block.
    let spec_state = handle.read_state().await;
    let att = spec_state
        .context_attachments
        .iter()
        .find(|a| a.attachment_id == attachment_id)
        .expect("attachment present");
    assert_eq!(
        att.summary_error.as_deref(),
        Some(REASON),
        "summary_error must be persisted in state for panel re-render"
    );
}
