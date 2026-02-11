// ABOUTME: Module for domain-specific tools implementing the mux Tool trait.
// ABOUTME: Provides a registry factory that creates and registers all 7 spec tools.

mod ask_user;
mod emit_diff_summary;
mod emit_narration;
mod read_state;
mod write_commands;

pub use ask_user::{AskUserBooleanTool, AskUserFreeformTool, AskUserMultipleChoiceTool};
pub use emit_diff_summary::EmitDiffSummaryTool;
pub use emit_narration::EmitNarrationTool;
pub use read_state::ReadStateTool;
pub use write_commands::WriteCommandsTool;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mux::tool::Registry;
use barnstormer_core::actor::SpecActorHandle;

/// Build a tool registry with all 7 domain tools registered.
///
/// The returned registry contains: read_state, write_commands, emit_narration,
/// emit_diff_summary, ask_user_boolean, ask_user_multiple_choice, ask_user_freeform.
pub async fn build_registry(
    actor: Arc<SpecActorHandle>,
    question_pending: Arc<AtomicBool>,
    agent_id: String,
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
            agent_id,
        })
        .await;

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

    #[tokio::test]
    async fn build_registry_registers_all_7_tools() {
        let (_id, handle) = make_test_actor();
        let registry = build_registry(
            Arc::new(handle),
            Arc::new(AtomicBool::new(false)),
            "test-agent".to_string(),
        )
        .await;

        assert_eq!(registry.count().await, 7);

        let names = registry.list().await;
        assert!(names.contains(&"read_state".to_string()));
        assert!(names.contains(&"write_commands".to_string()));
        assert!(names.contains(&"emit_narration".to_string()));
        assert!(names.contains(&"emit_diff_summary".to_string()));
        assert!(names.contains(&"ask_user_boolean".to_string()));
        assert!(names.contains(&"ask_user_multiple_choice".to_string()));
        assert!(names.contains(&"ask_user_freeform".to_string()));
    }

    #[tokio::test]
    async fn registry_tools_are_retrievable_by_name() {
        let (_id, handle) = make_test_actor();
        let registry = build_registry(
            Arc::new(handle),
            Arc::new(AtomicBool::new(false)),
            "test-agent".to_string(),
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
        ] {
            let tool = registry.get(name).await;
            assert!(tool.is_some(), "tool '{}' should be in registry", name);
            assert_eq!(tool.unwrap().name(), *name);
        }
    }
}
