// ABOUTME: Trait for the question-mode of the retrieve_context tool — answers
// ABOUTME: a targeted question about an attachment. Implemented by the server
// ABOUTME: crate to keep the agent crate decoupled from summarizer internals.

use async_trait::async_trait;
use barnstormer_core::state::ContextAttachment;
use ulid::Ulid;

/// Implemented by whichever component owns access to the multimodal LLM
/// client. Used by `retrieve_context(id, question)` to dispatch a fresh
/// summarizer call against the attachment with a targeted question.
#[async_trait]
pub trait AttachmentSummarizer: Send + Sync + std::fmt::Debug {
    /// Answer a targeted question about the attachment using the multimodal
    /// summarizer. Returns the answer text on success, or a string error
    /// message on failure (capability gating, LLM error, file read failure).
    async fn answer_question(
        &self,
        spec_id: Ulid,
        attachment: &ContextAttachment,
        question: &str,
    ) -> Result<String, String>;
}
