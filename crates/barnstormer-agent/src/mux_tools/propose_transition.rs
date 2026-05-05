// ABOUTME: Tool that lets the Manager propose transitioning to the next spec phase.
// ABOUTME: Reuses existing AskQuestion infrastructure with swarm-level answer-watching.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mux::tool::{Tool, ToolResult};
use serde_json::json;
use ulid::Ulid;

use barnstormer_core::actor::SpecActorHandle;
use barnstormer_core::command::Command;
use barnstormer_core::state::SpecPhase;
use barnstormer_core::transcript::UserQuestion;

#[derive(Clone)]
pub struct ProposeTransitionTool {
    pub(crate) actor: Arc<SpecActorHandle>,
    pub(crate) question_pending: Arc<AtomicBool>,
    pub(crate) pending_transition_question: Arc<Mutex<Option<Ulid>>>,
}

#[async_trait]
impl Tool for ProposeTransitionTool {
    fn name(&self) -> &str {
        "propose_transition"
    }

    fn description(&self) -> &str {
        "Propose transitioning to the next phase of the spec. Phase progression: Brainstorming → Refining → Complete. \
         From Brainstorming this proposes Refining; from Refining this proposes Complete. \
         The `summary` argument is a brief RECAP of what has been accomplished in the current phase — \
         do NOT include any question, call to action, or 'ready to move on?' phrasing; the runtime \
         appends the appropriate confirmation prompt automatically. \
         After the user answers Yes, the runtime fires the phase transition — wait for the \
         resulting `PhaseTransitioned` event before continuing, do NOT re-propose. \
         If the user answers No, address their feedback before proposing again."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Brief recap of what you've learned from brainstorming."
                }
            },
            "required": ["summary"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult, anyhow::Error> {
        // Check current phase to determine appropriate question text
        let current_phase = self.actor.read_state().await.phase.clone();
        if current_phase == SpecPhase::Complete {
            return Ok(ToolResult::text(
                "The spec is already in the Complete phase. No further transitions are available.",
            ));
        }

        if self
            .question_pending
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(ToolResult::text(
                "A question is already pending. Wait for the user to answer before proposing a transition.",
            ));
        }

        let summary = match params.get("summary").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => {
                self.question_pending.store(false, Ordering::SeqCst);
                return Err(anyhow::anyhow!("missing 'summary' parameter"));
            }
        };

        let question_text = match current_phase {
            SpecPhase::Brainstorming => {
                format!("{}\n\nReady to start organizing the spec?", summary)
            }
            SpecPhase::Refining => {
                format!("{}\n\nThe spec looks complete. Ready to finalize?", summary)
            }
            SpecPhase::Complete => unreachable!(),
        };

        let question_id = Ulid::new();
        let question = UserQuestion::Boolean {
            question_id,
            question: question_text,
            default: Some(true),
        };

        if let Err(e) = self
            .actor
            .send_command(Command::AskQuestion { question })
            .await
        {
            self.question_pending.store(false, Ordering::SeqCst);
            return Err(anyhow::anyhow!("failed to ask transition question: {}", e));
        }

        {
            let mut guard = self.pending_transition_question.lock().unwrap();
            *guard = Some(question_id);
        }

        Ok(ToolResult::text(
            "Transition proposal sent to the user. They will see a confirmation prompt. Wait for their response before continuing.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use barnstormer_core::actor;
    use barnstormer_core::state::SpecState;

    fn make_test_actor() -> (Ulid, SpecActorHandle) {
        let spec_id = Ulid::new();
        let handle = actor::spawn(spec_id, SpecState::new());
        (spec_id, handle)
    }

    #[tokio::test]
    async fn tool_name_is_propose_transition() {
        let (_id, handle) = make_test_actor();
        let tool = ProposeTransitionTool {
            actor: Arc::new(handle),
            question_pending: Arc::new(AtomicBool::new(false)),
            pending_transition_question: Arc::new(Mutex::new(None)),
        };
        assert_eq!(tool.name(), "propose_transition");
    }

    #[tokio::test]
    async fn description_tells_summary_not_to_contain_questions() {
        // Regression: an earlier description said "Summarize progress so far
        // and ASK the user if they're ready to move on." The model took that
        // literally and packed transition questions into the `summary`
        // argument — so the user saw a doubled prompt
        // ("...move to Refining?... Ready to finalize?") even after the phase
        // had already advanced to Refining. The description must steer the
        // summary toward a pure recap, not a question.
        let (_id, handle) = make_test_actor();
        let tool = ProposeTransitionTool {
            actor: Arc::new(handle),
            question_pending: Arc::new(AtomicBool::new(false)),
            pending_transition_question: Arc::new(Mutex::new(None)),
        };
        let desc = tool.description();
        assert!(
            desc.contains("do NOT include any question"),
            "description must explicitly forbid questions in the summary: {desc}"
        );
        assert!(
            desc.contains("Brainstorming → Refining → Complete")
                || desc.contains("Brainstorming -> Refining -> Complete"),
            "description must spell out the phase progression: {desc}"
        );
    }

    #[tokio::test]
    async fn propose_transition_sends_boolean_question() {
        let (_id, handle) = make_test_actor();
        let handle = Arc::new(handle);
        handle
            .send_command(Command::CreateSpec {
                title: "Test".to_string(),
                one_liner: "t".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();

        let question_pending = Arc::new(AtomicBool::new(false));
        let pending_transition = Arc::new(Mutex::new(None));

        let tool = ProposeTransitionTool {
            actor: handle.clone(),
            question_pending: question_pending.clone(),
            pending_transition_question: pending_transition.clone(),
        };

        let result = tool
            .execute(json!({"summary": "We decided on WebSocket architecture."}))
            .await
            .unwrap();
        assert!(result.content.contains("Transition proposal sent"));

        assert!(question_pending.load(Ordering::SeqCst));
        let stored = pending_transition.lock().unwrap();
        assert!(stored.is_some());

        let state = handle.read_state().await;
        assert!(state.pending_question.is_some());
    }

    #[tokio::test]
    async fn propose_transition_rejects_when_question_pending() {
        let (_id, handle) = make_test_actor();
        let handle = Arc::new(handle);
        let question_pending = Arc::new(AtomicBool::new(true));

        let tool = ProposeTransitionTool {
            actor: handle,
            question_pending,
            pending_transition_question: Arc::new(Mutex::new(None)),
        };

        let result = tool.execute(json!({"summary": "test"})).await.unwrap();
        assert!(result.content.contains("already pending"));
    }

    #[tokio::test]
    async fn propose_transition_stores_question_id() {
        let (_id, handle) = make_test_actor();
        let handle = Arc::new(handle);
        handle
            .send_command(Command::CreateSpec {
                title: "Test".to_string(),
                one_liner: "t".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();

        let pending_transition = Arc::new(Mutex::new(None));
        let tool = ProposeTransitionTool {
            actor: handle,
            question_pending: Arc::new(AtomicBool::new(false)),
            pending_transition_question: pending_transition.clone(),
        };

        tool.execute(json!({"summary": "test"})).await.unwrap();
        let stored = pending_transition.lock().unwrap();
        assert!(stored.is_some(), "should store question ID");
    }

    #[tokio::test]
    async fn propose_transition_allows_reproposal_after_clear() {
        let (_id, handle) = make_test_actor();
        let handle = Arc::new(handle);
        handle
            .send_command(Command::CreateSpec {
                title: "Test".to_string(),
                one_liner: "t".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();

        let question_pending = Arc::new(AtomicBool::new(false));
        let pending_transition = Arc::new(Mutex::new(None));

        let tool = ProposeTransitionTool {
            actor: handle.clone(),
            question_pending: question_pending.clone(),
            pending_transition_question: pending_transition.clone(),
        };

        // First proposal
        tool.execute(json!({"summary": "first"})).await.unwrap();
        let q1 = *pending_transition.lock().unwrap();
        assert!(q1.is_some());

        // Simulate "no" answer clearing the state
        *pending_transition.lock().unwrap() = None;
        question_pending.store(false, Ordering::SeqCst);
        handle
            .send_command(Command::AnswerQuestion {
                question_id: q1.unwrap(),
                answer: "no".to_string(),
            })
            .await
            .unwrap();

        // Second proposal should work
        let result = tool
            .execute(json!({"summary": "second"}))
            .await
            .unwrap();
        assert!(result.content.contains("Transition proposal sent"));
        let q2 = *pending_transition.lock().unwrap();
        assert!(q2.is_some());
        assert_ne!(q1, q2);
    }

    #[tokio::test]
    async fn propose_transition_resets_pending_on_missing_summary() {
        let (_id, handle) = make_test_actor();
        let handle = Arc::new(handle);
        handle
            .send_command(Command::CreateSpec {
                title: "Test".to_string(),
                one_liner: "t".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();

        let question_pending = Arc::new(AtomicBool::new(false));
        let tool = ProposeTransitionTool {
            actor: handle,
            question_pending: question_pending.clone(),
            pending_transition_question: Arc::new(Mutex::new(None)),
        };

        // Call without summary parameter — should error
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());

        // question_pending must be reset so future questions still work
        assert!(
            !question_pending.load(Ordering::SeqCst),
            "question_pending should be reset after parameter validation failure"
        );
    }

    #[tokio::test]
    async fn propose_transition_brainstorming_asks_about_organizing() {
        let (_id, handle) = make_test_actor();
        let handle = Arc::new(handle);
        handle
            .send_command(Command::CreateSpec {
                title: "Test".to_string(),
                one_liner: "t".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();

        let tool = ProposeTransitionTool {
            actor: handle.clone(),
            question_pending: Arc::new(AtomicBool::new(false)),
            pending_transition_question: Arc::new(Mutex::new(None)),
        };

        tool.execute(json!({"summary": "Brainstorming complete."}))
            .await
            .unwrap();

        let state = handle.read_state().await;
        if let Some(UserQuestion::Boolean { question, .. }) = &state.pending_question {
            assert!(
                question.contains("organizing the spec"),
                "brainstorming question should mention organizing: {}",
                question
            );
        } else {
            panic!("expected Boolean question");
        }
    }

    #[tokio::test]
    async fn propose_transition_refining_asks_about_finalizing() {
        let (_id, handle) = make_test_actor();
        let handle = Arc::new(handle);
        handle
            .send_command(Command::CreateSpec {
                title: "Test".to_string(),
                one_liner: "t".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();
        // Move to Refining phase
        handle
            .send_command(Command::TransitionPhase {
                target: SpecPhase::Refining,
            })
            .await
            .unwrap();

        let tool = ProposeTransitionTool {
            actor: handle.clone(),
            question_pending: Arc::new(AtomicBool::new(false)),
            pending_transition_question: Arc::new(Mutex::new(None)),
        };

        tool.execute(json!({"summary": "Spec is refined."}))
            .await
            .unwrap();

        let state = handle.read_state().await;
        if let Some(UserQuestion::Boolean { question, .. }) = &state.pending_question {
            assert!(
                question.contains("Ready to finalize"),
                "refining question should mention finalizing: {}",
                question
            );
        } else {
            panic!("expected Boolean question");
        }
    }

    #[tokio::test]
    async fn propose_transition_complete_phase_returns_noop() {
        let (_id, handle) = make_test_actor();
        let handle = Arc::new(handle);
        handle
            .send_command(Command::CreateSpec {
                title: "Test".to_string(),
                one_liner: "t".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();
        // Move to Refining, then Complete
        handle
            .send_command(Command::TransitionPhase {
                target: SpecPhase::Refining,
            })
            .await
            .unwrap();
        handle
            .send_command(Command::TransitionPhase {
                target: SpecPhase::Complete,
            })
            .await
            .unwrap();

        let tool = ProposeTransitionTool {
            actor: handle.clone(),
            question_pending: Arc::new(AtomicBool::new(false)),
            pending_transition_question: Arc::new(Mutex::new(None)),
        };

        let result = tool
            .execute(json!({"summary": "Already done."}))
            .await
            .unwrap();
        assert!(
            result.content.contains("already in the Complete phase"),
            "should indicate no transitions available: {}",
            result.content
        );
    }
}
