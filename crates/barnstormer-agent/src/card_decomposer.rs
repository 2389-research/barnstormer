// ABOUTME: Trait for decomposing a source brief into N spec cards. Allows the
// ABOUTME: delegate_card_decomposition tool to run an architect+executor pipeline
// ABOUTME: under a Haiku-class model with prompt caching, instead of generating
// ABOUTME: card bodies in Sonnet output tokens via write_commands.CreateCard.

use async_trait::async_trait;
use ulid::Ulid;

/// A single card produced by the decomposition pipeline. The tool turns each
/// of these into a `Command::CreateCard` that gets applied to the actor.
#[derive(Debug, Clone)]
pub struct DecomposedCard {
    /// Concise card title (3-8 words typical).
    pub title: String,
    /// "idea" | "task" | "constraint" | "risk" | "note"
    pub card_type: String,
    /// Multi-paragraph card body in markdown. Format conventions (declarative
    /// opener, bold-asterisk section headers, source-grounded bullets) live in
    /// the implementor's system prompt.
    pub body: String,
    /// "Ideas" | "Plan" | "Spec" | None for unlaned.
    pub lane: Option<String>,
}

/// Cost breakdown returned alongside the cards so the swarm can record per-
/// step telemetry. One usage entry per underlying LLM call (architect plus
/// each executor). Agent IDs are synthesized by the implementor — typically
/// something like `"card-decomposer-architect"` and
/// `"card-decomposer-executor"` — so the event log preserves attribution.
#[derive(Debug, Clone)]
pub struct DecomposerUsage {
    pub agent_id: String,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_write_tokens: u32,
}

/// Result of a decomposition pass.
#[derive(Debug, Clone)]
pub struct DecomposerOutput {
    pub cards: Vec<DecomposedCard>,
    /// Per-LLM-call usage so the swarm can record telemetry events. Ordered
    /// chronologically (architect first, then each executor).
    pub usage: Vec<DecomposerUsage>,
}

/// Implemented by whichever component owns access to a Haiku-class LLM client
/// and the on-disk attachment store. Used by `delegate_card_decomposition` to
/// run the architect+executor pipeline validated in run-04 experiments.
#[async_trait]
pub trait CardDecomposer: Send + Sync + std::fmt::Debug {
    /// Decompose the brief identified by `brief_attachment_id` into roughly
    /// `target_card_count` cards. `decomposition_hints` is free-text guidance
    /// (e.g. "focus on validation engine and OSS/SaaS split") that the
    /// architect may use to bias which topics get cards.
    ///
    /// Returns the cards plus per-call usage telemetry. Errors are stringified
    /// so the tool can return them as `ToolResult::error`.
    async fn decompose(
        &self,
        spec_id: Ulid,
        brief_attachment_id: Ulid,
        target_card_count: u32,
        decomposition_hints: Option<&str>,
    ) -> Result<DecomposerOutput, String>;
}
