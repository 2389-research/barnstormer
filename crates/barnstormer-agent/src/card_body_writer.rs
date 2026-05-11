// ABOUTME: Trait for authoring a single card body via a Haiku-class LLM.
// ABOUTME: Used by `delegate_card_body` when a Sonnet SubAgent has already
// ABOUTME: decided what card to create and just needs the prose written.

use async_trait::async_trait;
use ulid::Ulid;

use crate::card_decomposer::DecomposerUsage;

/// Per-card-type voice. Mirrors the `EventPayload::CardCreated.card_type`
/// values that barnstormer-core actually accepts. The implementor's system
/// prompt parameterizes its output style by this — exploratory voice for
/// `idea`, concrete-actionable for `task`, normative for `constraint`,
/// likelihood/impact/mitigation for `risk`, question-shaped for `note`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    Idea,
    Task,
    Constraint,
    Risk,
    Note,
}

impl CardKind {
    /// Parse the wire-format string the agent emits in tool args.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "idea" => Some(Self::Idea),
            "task" => Some(Self::Task),
            "constraint" => Some(Self::Constraint),
            "risk" => Some(Self::Risk),
            "note" => Some(Self::Note),
            _ => None,
        }
    }

    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Idea => "idea",
            Self::Task => "task",
            Self::Constraint => "constraint",
            Self::Risk => "risk",
            Self::Note => "note",
        }
    }

    pub fn all_wire_values() -> &'static [&'static str] {
        &["idea", "task", "constraint", "risk", "note"]
    }
}

/// Structured intent the SubAgent passes to the writer. Title and scope
/// are required so Haiku has something to anchor on. Everything else is
/// optional context the writer may use (or ignore) to ground the body.
#[derive(Debug, Clone)]
pub struct CardBodyRequest {
    pub kind: CardKind,
    pub lane: Option<String>,
    pub title: String,
    pub scope: String,
    /// Ordered bullets/claims the body should include. May be empty —
    /// writer expands from scope alone in that case.
    pub key_points: Vec<String>,
    /// Optional ULID of an attached source brief; writer may pull supporting
    /// content from it (text or stored summary, same path as the bulk
    /// decomposer uses).
    pub source_attachment_id: Option<Ulid>,
    /// Optional ULIDs of existing cards the writer should NOT duplicate —
    /// titles + scopes get fed to the writer for context, bodies do not.
    pub related_card_ids: Vec<Ulid>,
    /// Optional free-text context Sonnet can pass to nudge the writer.
    pub free_text_context: Option<String>,
    /// Optional target length range in chars. When None, the writer's
    /// default-per-card_type applies (e.g. idea ~200-600, task ~600-1200).
    pub target_length_range: Option<(usize, usize)>,
}

/// Output: the rendered body markdown plus per-call usage telemetry so
/// the dispatching tool can record AgentStepUsage events for cost
/// attribution (same pattern as `DecomposerOutput`).
#[derive(Debug, Clone)]
pub struct CardBodyOutput {
    pub body: String,
    pub usage: Vec<DecomposerUsage>,
}

/// Implemented by whichever component owns access to a Haiku-class LLM
/// client and the on-disk attachment store. Used by `delegate_card_body`
/// when the SubAgent already knows what card it wants to author and just
/// needs the prose expansion.
///
/// Differs from `CardDecomposer` in that the agent (Sonnet) IS the
/// architect — there's no Haiku planning step. The trait implementation
/// is just an executor that writes one body in the right voice for the
/// supplied `kind`. Spec for the prose conventions and per-card_type
/// voice library lives in the implementor's system prompt, not in the
/// trait.
#[async_trait]
pub trait CardBodyWriter: Send + Sync + std::fmt::Debug {
    /// Render the body. `spec_id` is supplied so implementations that
    /// look up `source_attachment_id` on disk can resolve the file path.
    /// `attachment_summary` is the LLM-generated summary stored at upload
    /// time — used as fallback when the attachment's bytes aren't UTF-8
    /// text (PDFs, images, etc.), same pattern as `CardDecomposer`.
    /// `related_card_summaries` is `(title, scope_or_excerpt)` pairs for
    /// each related card; the dispatching tool reads them from state so
    /// the writer doesn't need actor access.
    async fn write_body(
        &self,
        spec_id: Ulid,
        request: &CardBodyRequest,
        attachment_summary: Option<&str>,
        related_card_summaries: &[(String, String)],
    ) -> Result<CardBodyOutput, String>;
}
