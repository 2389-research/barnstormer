// ABOUTME: Module for domain-specific tools implementing the mux Tool trait.
// ABOUTME: Provides a registry factory that creates and registers all spec tools.

mod ask_user;
mod delegate_card_body;
mod delegate_card_decomposition;
mod emit_diff_summary;
mod emit_narration;
mod propose_transition;
mod read_state;
mod retrieve_context;
mod write_commands;

pub use ask_user::{AskUserBooleanTool, AskUserFreeformTool, AskUserMultipleChoiceTool};
pub use delegate_card_body::DelegateCardBodyTool;
pub use delegate_card_decomposition::DelegateCardDecompositionTool;
pub use emit_diff_summary::EmitDiffSummaryTool;
pub use emit_narration::EmitNarrationTool;
pub use propose_transition::ProposeTransitionTool;
pub use read_state::ReadStateTool;
pub use retrieve_context::RetrieveContextTool;
pub use write_commands::WriteCommandsTool;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use barnstormer_core::actor::SpecActorHandle;
use mux::tool::Registry;
use ulid::Ulid;

use crate::{AttachmentSummarizer, CardBodyWriter, CardDecomposer, NarrationRenderer};

/// Build a tool registry with all domain tools registered.
///
/// The returned registry contains: read_state, write_commands, emit_narration,
/// emit_diff_summary, ask_user_boolean, ask_user_multiple_choice, ask_user_freeform,
/// propose_transition, retrieve_context, and optionally delegate_card_decomposition
/// and delegate_card_body.
///
/// `narration_renderer` is optional — when present, emit_narration accepts the
/// structured `intent`+`points` schema and renders prose via the renderer.
/// When None, only the legacy `message` field is usable.
///
/// `card_decomposer` is optional — when present, delegate_card_decomposition
/// is registered and Sonnet can route bulk card-body generation through the
/// architect+executor Haiku pipeline. When None, the tool is not registered
/// and Sonnet must use write_commands.CreateCard.
///
/// `card_body_writer` is optional — when present, delegate_card_body is
/// registered and Sonnet can author single cards by supplying type+title+
/// scope+key_points; the writer expands the body in the right voice for
/// the card_type. When None, the tool is not registered and Sonnet must
/// use write_commands.CreateCard for one-off cards.
#[allow(clippy::too_many_arguments)]
pub async fn build_registry(
    actor: Arc<SpecActorHandle>,
    question_pending: Arc<AtomicBool>,
    pending_transition_question: Arc<Mutex<Option<Ulid>>>,
    agent_id: String,
    home: PathBuf,
    summarizer: Arc<dyn AttachmentSummarizer>,
    narration_renderer: Option<Arc<dyn NarrationRenderer>>,
    card_decomposer: Option<Arc<dyn CardDecomposer>>,
    card_body_writer: Option<Arc<dyn CardBodyWriter>>,
) -> Registry {
    let registry = Registry::new();

    registry
        .register(ReadStateTool {
            actor: Arc::clone(&actor),
        })
        .await;

    registry
        .register(WriteCommandsTool {
            actor: Arc::clone(&actor),
            agent_id: agent_id.clone(),
        })
        .await;

    registry
        .register(EmitNarrationTool {
            actor: Arc::clone(&actor),
            agent_id: agent_id.clone(),
            renderer: narration_renderer,
        })
        .await;

    registry
        .register(EmitDiffSummaryTool {
            actor: Arc::clone(&actor),
            agent_id: agent_id.clone(),
        })
        .await;

    registry
        .register(AskUserBooleanTool {
            actor: Arc::clone(&actor),
            question_pending: Arc::clone(&question_pending),
            agent_id: agent_id.clone(),
        })
        .await;

    registry
        .register(AskUserMultipleChoiceTool {
            actor: Arc::clone(&actor),
            question_pending: Arc::clone(&question_pending),
            agent_id: agent_id.clone(),
        })
        .await;

    registry
        .register(AskUserFreeformTool {
            actor: Arc::clone(&actor),
            question_pending: Arc::clone(&question_pending),
            agent_id: agent_id.clone(),
        })
        .await;

    registry
        .register(propose_transition::ProposeTransitionTool {
            actor: Arc::clone(&actor),
            question_pending: Arc::clone(&question_pending),
            pending_transition_question: pending_transition_question.clone(),
        })
        .await;

    registry
        .register(retrieve_context::RetrieveContextTool {
            actor: Arc::clone(&actor),
            home,
            summarizer,
        })
        .await;

    if let Some(decomposer) = card_decomposer {
        registry
            .register(DelegateCardDecompositionTool {
                actor: Arc::clone(&actor),
                agent_id: agent_id.clone(),
                decomposer,
            })
            .await;
    }

    if let Some(writer) = card_body_writer {
        registry
            .register(DelegateCardBodyTool {
                actor: Arc::clone(&actor),
                agent_id,
                writer,
            })
            .await;
    }

    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use barnstormer_core::actor;
    use barnstormer_core::state::SpecState;
    use ulid::Ulid;

    fn make_test_actor() -> (Ulid, SpecActorHandle) {
        let spec_id = Ulid::new();
        let handle = actor::spawn(spec_id, SpecState::new());
        (spec_id, handle)
    }

    #[derive(Debug)]
    struct StubSummarizer;

    #[async_trait::async_trait]
    impl crate::AttachmentSummarizer for StubSummarizer {
        async fn answer_question(
            &self,
            _spec_id: Ulid,
            _attachment: &barnstormer_core::state::ContextAttachment,
            _question: &str,
        ) -> Result<String, String> {
            Ok("stub".into())
        }
    }

    fn stub_summarizer() -> Arc<dyn crate::AttachmentSummarizer> {
        Arc::new(StubSummarizer)
    }

    #[tokio::test]
    async fn build_registry_registers_all_9_tools() {
        let (_id, handle) = make_test_actor();
        let registry = build_registry(
            Arc::new(handle),
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(None)),
            "test-agent".to_string(),
            PathBuf::from("/tmp/barnstormer-test"),
            stub_summarizer(),
            None,
            None,
            None,
        )
        .await;

        assert_eq!(registry.count().await, 9);

        let names = registry.list().await;
        assert!(names.contains(&"read_state".to_string()));
        assert!(names.contains(&"write_commands".to_string()));
        assert!(names.contains(&"emit_narration".to_string()));
        assert!(names.contains(&"emit_diff_summary".to_string()));
        assert!(names.contains(&"ask_user_boolean".to_string()));
        assert!(names.contains(&"ask_user_multiple_choice".to_string()));
        assert!(names.contains(&"ask_user_freeform".to_string()));
        assert!(names.contains(&"propose_transition".to_string()));
        assert!(names.contains(&"retrieve_context".to_string()));
    }

    #[tokio::test]
    async fn registry_tools_are_retrievable_by_name() {
        let (_id, handle) = make_test_actor();
        let registry = build_registry(
            Arc::new(handle),
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(None)),
            "test-agent".to_string(),
            PathBuf::from("/tmp/barnstormer-test"),
            stub_summarizer(),
            None,
            None,
            None,
        )
        .await;

        for name in &[
            "read_state",
            "write_commands",
            "emit_narration",
            "emit_diff_summary",
            "ask_user_boolean",
            "ask_user_multiple_choice",
            "ask_user_freeform",
            "propose_transition",
            "retrieve_context",
        ] {
            let tool = registry.get(name).await;
            assert!(tool.is_some(), "tool '{}' should be in registry", name);
            assert_eq!(tool.unwrap().name(), *name);
        }
    }
}
