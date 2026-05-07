// ABOUTME: Server-side impl of barnstormer_agent::AttachmentSummarizer that
// ABOUTME: dispatches a fresh summarize_now call against an attachment with a
// ABOUTME: targeted question.

use async_trait::async_trait;
use barnstormer_agent::AttachmentSummarizer;
use barnstormer_core::state::ContextAttachment;
use std::path::PathBuf;
use ulid::Ulid;

/// Adapter that wires the agent crate's `retrieve_context` question-mode
/// dispatch back into the server's summarizer module. Holds only the
/// barnstormer home directory; everything else (provider config, LLM client
/// construction) is pulled from env at call time inside `summarize_now`,
/// matching the upload/notes/resummarize paths.
#[derive(Debug)]
pub struct ServerSummarizer {
    pub home: PathBuf,
}

#[async_trait]
impl AttachmentSummarizer for ServerSummarizer {
    async fn answer_question(
        &self,
        spec_id: Ulid,
        attachment: &ContextAttachment,
        question: &str,
    ) -> Result<String, String> {
        let input = crate::context_storage::build_summarizer_input(&self.home, spec_id, attachment)
            .await
            .map_err(|e| format!("could not build summarizer input: {e}"))?;
        crate::summarizer::summarize_now(
            &attachment.filename,
            attachment.user_notes.as_deref(),
            &input,
            Some(question),
        )
        .await
        // anyhow `{:#}` walks the cause chain so the agent sees the LLM error,
        // not just the outer "summarize_now failed" wrapper.
        .map_err(|e| format!("{e:#}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use barnstormer_core::state::ContextAttachment;
    use chrono::Utc;

    /// `ServerSummarizer::answer_question` relies on `format!("{e:#}")` to walk
    /// an `anyhow::Error` chain so the agent sees the inner LLM/IO error rather
    /// than just the outer wrapper. This pins the alt-display formatting
    /// contract: a future anyhow upgrade that changes that semantics will
    /// surface here before users see degraded error messages.
    #[test]
    fn anyhow_error_chain_formats_with_alt_display() {
        let err = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let outer: anyhow::Error = anyhow::Error::from(err).context("could not read attachment");
        let formatted = format!("{outer:#}");
        assert!(
            formatted.contains("could not read attachment"),
            "outer context must appear in alt-display; got: {formatted}"
        );
        assert!(
            formatted.contains("no such file"),
            "inner cause must appear in alt-display; got: {formatted}"
        );
    }

    /// Direct end-to-end check of the `answer_question` error path: build a
    /// `ServerSummarizer` rooted at a tempdir, hand it an attachment whose
    /// file isn't on disk, and assert the returned `Err(String)` carries the
    /// outer "could not build summarizer input" wrapper plus the inner OS
    /// error from the failed `read_to_string`. Together these prove the
    /// cause-chain formatting reaches the caller.
    #[tokio::test]
    async fn answer_question_returns_chained_error_when_file_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let summarizer = ServerSummarizer {
            home: tmp.path().to_path_buf(),
        };
        let spec_id = Ulid::new();
        let attachment = ContextAttachment {
            attachment_id: Ulid::new(),
            filename: "missing.md".into(),
            mime_type: "text/markdown".into(),
            size_bytes: 0,
            summary: None,
            user_notes: None,
            added_at: Utc::now(),
            removed: false,
            summary_error: None,
        };

        let result = summarizer
            .answer_question(spec_id, &attachment, "what does this say?")
            .await;
        let err = result.expect_err("missing on-disk file should produce Err");
        assert!(
            err.contains("could not build summarizer input"),
            "outer wrapper must appear in error string; got: {err}"
        );
        // The inner cause is the OS read error — the exact wording varies by
        // platform, so just check it carries some signal of a missing file.
        assert!(
            err.contains("No such file") || err.contains("cannot find"),
            "inner cause from failed read should appear via alt-display; got: {err}"
        );
    }
}
