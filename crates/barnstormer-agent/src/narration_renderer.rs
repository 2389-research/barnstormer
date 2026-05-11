// ABOUTME: Trait for rendering structured narration intent into prose. Allows the
// ABOUTME: emit_narration tool to delegate prose generation to a faster/cheaper model.

use async_trait::async_trait;

/// Structured narration intent. Mirrors the seven voice categories validated
/// in the 2026-05-09 cost-optimization experiments (run-03-tool-dispatch).
/// Each variant carries different rendering conventions on the renderer side
/// (length, voice, structure) — the agent's job is to pick the right intent
/// and supply ordered points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NarrationIntent {
    /// Analytical paragraphs referring to specific cards/lanes/relationships.
    StructuralAnalysis,
    /// Short bulleted list of "Critical Gaps" / open questions.
    GapIdentification,
    /// 1-2 paragraph recap of what was just accomplished.
    CompletionSummary,
    /// Conversational reply to the user (1-3 sentences).
    UserAcknowledgment,
    /// Brief paragraph explaining the next planned step.
    StepExplanation,
    /// 1-2 paragraph recap of progress through the prior phase.
    PhaseTransitionRecap,
    /// Bulleted raw ideas with brief framing.
    ExploratoryBrainstorm,
}

impl NarrationIntent {
    /// Parse from the wire-format string the agent emits in tool args.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "structural_analysis" => Some(Self::StructuralAnalysis),
            "gap_identification" => Some(Self::GapIdentification),
            "completion_summary" => Some(Self::CompletionSummary),
            "user_acknowledgment" => Some(Self::UserAcknowledgment),
            "step_explanation" => Some(Self::StepExplanation),
            "phase_transition_recap" => Some(Self::PhaseTransitionRecap),
            "exploratory_brainstorm" => Some(Self::ExploratoryBrainstorm),
            _ => None,
        }
    }

    /// All wire-format strings, in stable order. Used to populate the tool's
    /// JSON schema `enum` field so the agent knows what's valid.
    pub fn all_wire_values() -> &'static [&'static str] {
        &[
            "structural_analysis",
            "gap_identification",
            "completion_summary",
            "user_acknowledgment",
            "step_explanation",
            "phase_transition_recap",
            "exploratory_brainstorm",
        ]
    }
}

/// Implemented by whichever component owns access to a Haiku-class LLM client.
/// Used by `emit_narration(intent, points)` to render structured intent into
/// prose with the right voice for the intent.
#[async_trait]
pub trait NarrationRenderer: Send + Sync + std::fmt::Debug {
    /// Render the given intent + ordered points into prose. Returns the prose
    /// on success or a string error on failure (LLM error, parse failure).
    /// The renderer chooses model, system prompt, and caching strategy.
    async fn render(
        &self,
        intent: NarrationIntent,
        points: &[String],
        spec_state_relevant: bool,
    ) -> Result<String, String>;
}
