// ABOUTME: Server-side impl of barnstormer_agent::NarrationRenderer that
// ABOUTME: renders structured narration intent into prose via a Haiku-class LLM.

use async_trait::async_trait;
use barnstormer_agent::{NarrationIntent, NarrationRenderer};

/// Default Haiku model used for narration rendering. Override via
/// `BARNSTORMER_NARRATION_MODEL` env var. Haiku is intentional here — the
/// 2026-05-09 cost-optimization experiments showed it produces good prose
/// in the right voice when given a structured intent + ordered points, at
/// a fraction of Sonnet's per-token cost.
const DEFAULT_NARRATION_MODEL: &str = "claude-haiku-4-5";

/// Server-side adapter that fulfills the agent crate's `NarrationRenderer`
/// trait by spinning up a fresh LLM client per call (mirrors the summarizer's
/// approach so provider + base-url + caching all flow through env config).
///
/// Currently uses the `anthropic` provider unconditionally, since the voice
/// library and caching strategy is calibrated for Anthropic's caching API.
#[derive(Debug)]
pub struct ServerNarrationRenderer;

#[async_trait]
impl NarrationRenderer for ServerNarrationRenderer {
    async fn render(
        &self,
        intent: NarrationIntent,
        points: &[String],
        spec_state_relevant: bool,
    ) -> Result<String, String> {
        let model = std::env::var("BARNSTORMER_NARRATION_MODEL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_NARRATION_MODEL.to_string());

        let (client, resolved_model) =
            barnstormer_agent::client::create_llm_client("anthropic", Some(&model))
                .map_err(|e| format!("failed to create narration LLM client: {e}"))?;

        let req = build_narration_request(intent, points, spec_state_relevant, &resolved_model);
        let resp = client
            .create_message(&req)
            .await
            .map_err(|e| format!("narration LLM call failed: {e}"))?;
        let text = resp.text();
        if text.trim().is_empty() {
            return Err("narration LLM returned empty text".to_string());
        }
        Ok(text)
    }
}

/// System prompt for the narration renderer. Encodes the voice library from
/// the 2026-05-09 run-03-tool-dispatch experiment. Each intent gets a target
/// length range + format hint; the LLM picks the right shape from the intent.
const NARRATION_SYSTEM_PROMPT: &str = r#"You produce narrations for a spec-builder UI. The calling agent provides a structured intent + an ordered list of points. Expand them into prose in the right voice for the intent.

Voice library:

structural_analysis — 2-4 paragraphs, analytical. Reference specific cards/lanes/relationships when the points name them. Target 600-1200 chars.

gap_identification — short list under a "Critical Gaps" or "Open Questions" heading. 3-6 bulleted items, each 1-2 sentences. Target 400-900 chars.

completion_summary — 1-2 paragraphs recapping what was just accomplished. Concrete, not promotional. Target 300-700 chars.

user_acknowledgment — conversational reply to the user. 1-3 sentences. Target 150-400 chars.

step_explanation — brief paragraph explaining what you're about to do next. 1-3 sentences. Target 200-500 chars.

phase_transition_recap — 1-2 paragraphs recapping progress through the prior phase. Target 400-800 chars.

exploratory_brainstorm — list of 3-7 raw ideas with a brief lead-in. Target 400-1000 chars.

Always declarative voice. No "I" first-person framing. No preamble like "Here is the narration". No closing like "Let me know if...". Output ONLY the prose."#;

/// Build the narration LLM request. Stays small and structured — Haiku's job
/// is to expand the points list in the right voice; the agent already made
/// the architectural decisions.
fn build_narration_request(
    intent: NarrationIntent,
    points: &[String],
    spec_state_relevant: bool,
    model: &str,
) -> mux::llm::Request {
    let intent_label = match intent {
        NarrationIntent::StructuralAnalysis => "structural_analysis",
        NarrationIntent::GapIdentification => "gap_identification",
        NarrationIntent::CompletionSummary => "completion_summary",
        NarrationIntent::UserAcknowledgment => "user_acknowledgment",
        NarrationIntent::StepExplanation => "step_explanation",
        NarrationIntent::PhaseTransitionRecap => "phase_transition_recap",
        NarrationIntent::ExploratoryBrainstorm => "exploratory_brainstorm",
    };
    let mut user_msg = format!(
        "Intent: {intent_label}\nSpec-state-relevant: {spec_state_relevant}\n\nPoints (in order):\n"
    );
    for (i, p) in points.iter().enumerate() {
        user_msg.push_str(&format!("{}. {}\n", i + 1, p));
    }
    user_msg.push_str("\nProduce the narration now.");

    mux::llm::Request::new(model)
        .system(NARRATION_SYSTEM_PROMPT)
        .message(mux::llm::Message::user(user_msg))
        .max_tokens(900)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the request shape so a future refactor doesn't silently change the
    /// prompt format. Asserts: intent label routed in, points enumerated 1.._n,
    /// system prompt present, max_tokens set.
    #[test]
    fn build_request_includes_intent_and_points() {
        let req = build_narration_request(
            NarrationIntent::GapIdentification,
            &["Validate the auth flow".into(), "Audit retry policy".into()],
            true,
            "claude-haiku-4-5",
        );
        assert_eq!(req.model, "claude-haiku-4-5");
        assert_eq!(req.max_tokens, Some(900));
        assert!(req.system.as_deref().unwrap_or("").contains("Voice library"));
        assert_eq!(req.messages.len(), 1);
        let body = &req.messages[0].content[0];
        let text = match body {
            mux::llm::ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        assert!(text.contains("Intent: gap_identification"));
        assert!(text.contains("Spec-state-relevant: true"));
        assert!(text.contains("1. Validate the auth flow"));
        assert!(text.contains("2. Audit retry policy"));
    }

    #[test]
    fn build_request_handles_each_intent_label() {
        // Each variant should produce a distinct intent label in the body.
        // Catches a future refactor that drops a variant from the match.
        let intents = [
            (NarrationIntent::StructuralAnalysis, "structural_analysis"),
            (NarrationIntent::GapIdentification, "gap_identification"),
            (NarrationIntent::CompletionSummary, "completion_summary"),
            (NarrationIntent::UserAcknowledgment, "user_acknowledgment"),
            (NarrationIntent::StepExplanation, "step_explanation"),
            (NarrationIntent::PhaseTransitionRecap, "phase_transition_recap"),
            (NarrationIntent::ExploratoryBrainstorm, "exploratory_brainstorm"),
        ];
        for (intent, label) in intents {
            let req = build_narration_request(intent, &["x".into()], false, "test-model");
            let body = match &req.messages[0].content[0] {
                mux::llm::ContentBlock::Text { text } => text.clone(),
                _ => panic!("expected text"),
            };
            assert!(
                body.contains(&format!("Intent: {label}")),
                "intent {label} should appear in the body; got: {body}"
            );
        }
    }
}
