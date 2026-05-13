// ABOUTME: Trait for writing one prose field on the spec_core via a Haiku-class
// ABOUTME: LLM. Used by `delegate_spec_core_field` to keep constraint/success/
// ABOUTME: risk/note/description prose out of Sonnet output tokens.

use async_trait::async_trait;
use ulid::Ulid;

use crate::card_decomposer::DecomposerUsage;

/// Which prose field on the spec_core to author. The 3 short fields
/// (`title`, `one_liner`, `goal`) are intentionally NOT delegate-able —
/// they're short enough that Sonnet writes them inline at negligible
/// cost, and the per-field voice library doesn't really vary at that
/// length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecCoreField {
    Description,
    Constraints,
    SuccessCriteria,
    Risks,
    Notes,
}

impl SpecCoreField {
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "description" => Some(Self::Description),
            "constraints" => Some(Self::Constraints),
            "success_criteria" => Some(Self::SuccessCriteria),
            "risks" => Some(Self::Risks),
            "notes" => Some(Self::Notes),
            _ => None,
        }
    }

    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Description => "description",
            Self::Constraints => "constraints",
            Self::SuccessCriteria => "success_criteria",
            Self::Risks => "risks",
            Self::Notes => "notes",
        }
    }

    pub fn all_wire_values() -> &'static [&'static str] {
        &[
            "description",
            "constraints",
            "success_criteria",
            "risks",
            "notes",
        ]
    }
}

#[derive(Debug, Clone)]
pub struct SpecCoreFieldRequest {
    pub field: SpecCoreField,
    /// Ordered bullets/claims the field should include. The voice library
    /// dictates how each is rendered (a bullet, a paragraph, a sub-sectioned
    /// risk entry, etc.).
    pub key_points: Vec<String>,
    /// Optional ULIDs of cards on the board that should ground the prose
    /// (the tool extracts title + short excerpt for each).
    pub related_card_ids: Vec<Ulid>,
    /// Optional free-form context Sonnet can pass to nudge the writer.
    pub free_text_context: Option<String>,
    /// Optional target length range in chars (min, max). When None, the
    /// writer uses a default per field.
    pub target_length_range: Option<(usize, usize)>,
}

#[derive(Debug, Clone)]
pub struct SpecCoreFieldOutput {
    /// The rendered markdown content for the field.
    pub markdown: String,
    pub usage: Vec<DecomposerUsage>,
}

/// Per-field grounding context the dispatching tool resolved from spec
/// state. One entry per request — index-aligned with the batch of
/// requests. Mirrors `CardBodyContext` for the same reason: keeps the
/// writer stateless.
#[derive(Debug, Clone, Default)]
pub struct SpecCoreFieldContext {
    pub related_card_summaries: Vec<(String, String)>,
}

/// Implemented by whichever component owns the LLM client. The tool
/// resolves related-card context from spec state and passes it through
/// (title + excerpt tuples) so the writer stays stateless.
///
/// `write_fields` is the batch entry point. The dispatching tool always
/// calls this; for a single-field tool invocation it just passes a Vec
/// with one element. Implementations SHOULD run the LLM calls in
/// parallel — agents usually write the spec_core fields together
/// (constraints + success_criteria + risks + notes at once) so the
/// parallelism win is large. Output Vec is index-aligned with input.
#[async_trait]
pub trait SpecCoreFieldWriter: Send + Sync + std::fmt::Debug {
    /// Render markdown for each request in `requests`. `contexts` is
    /// index-aligned with `requests` and carries the per-field grounding
    /// (related-card titles + excerpts).
    ///
    /// Per-request errors don't fail the whole batch; the tool decides
    /// how to surface partial failures. Returns a top-level `Err` only
    /// for batch-wide failures (config / contract violations).
    async fn write_fields(
        &self,
        spec_id: Ulid,
        requests: &[SpecCoreFieldRequest],
        contexts: &[SpecCoreFieldContext],
    ) -> Result<Vec<Result<SpecCoreFieldOutput, String>>, String>;
}
