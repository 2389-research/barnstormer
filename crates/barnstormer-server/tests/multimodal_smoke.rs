// ABOUTME: Live-LLM smoke test for the multimodal context-files pipeline.
// ABOUTME: Gated on BARNSTORMER_LIVE_LLM=1 to keep CI off the LLM provider's tab.

mod common;

use std::time::{Duration, Instant};

/// End-to-end smoke test: upload a tiny PNG via the real HTTP route and wait
/// for the summarizer (running against whatever provider is configured) to
/// land a non-empty summary on the attachment. Skipped unless the developer
/// explicitly sets `BARNSTORMER_LIVE_LLM=1` so CI never burns LLM credits on
/// this path. No mocking is involved — this test exercises the production
/// summarizer against a real provider account.
#[tokio::test]
async fn live_llm_image_upload_summarizes_eventually() {
    if std::env::var("BARNSTORMER_LIVE_LLM").is_err() {
        eprintln!(
            "skipping live-LLM smoke test (set BARNSTORMER_LIVE_LLM=1 \
             with a real provider API key in env to run)"
        );
        return;
    }

    // Upload a tiny PNG through the real upload endpoint. This goes through
    // multipart parsing, MIME sniffing, on-disk storage, and event emission
    // — i.e. the same path a browser drag-drop would hit.
    let bytes = include_bytes!("fixtures/tiny.png");
    let ctx =
        common::setup_with_attachment_bytes("tiny.png", "application/octet-stream", bytes).await;

    // Grab a clone of the actor handle so we can poll state without holding
    // the actors lock across awaits.
    let handle = {
        let actors = ctx.state.actors.read().await;
        actors
            .get(&ctx.spec_id)
            .expect("actor present after upload")
            .clone()
    };

    // Sanity: the upload should already record the attachment with image/png mime.
    {
        let spec_state = handle.read_state().await;
        let att = spec_state
            .context_attachments
            .iter()
            .find(|a| a.attachment_id == ctx.attachment_id)
            .expect("attachment present in state");
        assert_eq!(
            att.mime_type, "image/png",
            "expected sniffed mime image/png, got {}",
            att.mime_type
        );
    }

    // Poll for up to 60s for the summary to land. The summarizer task is
    // spawned synchronously off the upload event, so the wait is purely on
    // the provider's response time.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let (summary, summary_error) = {
            let spec_state = handle.read_state().await;
            let att = spec_state
                .context_attachments
                .iter()
                .find(|a| a.attachment_id == ctx.attachment_id)
                .expect("attachment present in state");
            (att.summary.clone(), att.summary_error.clone())
        };

        if let Some(err) = summary_error {
            panic!(
                "summarize failed for attachment {}: {}",
                ctx.attachment_id, err
            );
        }
        if let Some(summary) = summary {
            assert!(
                !summary.trim().is_empty(),
                "live-LLM summary was empty (attachment {})",
                ctx.attachment_id
            );
            eprintln!("live-LLM summary for tiny.png: {summary}");
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "summary did not land within 60s for attachment {} \
                 (no summary, no summary_error — summarizer may not have spawned, \
                 or the provider is unreachable / unconfigured)",
                ctx.attachment_id
            );
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
