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
