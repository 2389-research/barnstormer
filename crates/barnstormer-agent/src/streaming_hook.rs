// ABOUTME: Mux Hook implementation that forwards LLM streaming events to the spec actor.
// ABOUTME: Bridges StreamDelta and tool-use callbacks into ephemeral broadcast events.

use std::sync::Arc;

use async_trait::async_trait;
use barnstormer_core::{Command, SpecActorHandle};
use mux::hook::{Hook, HookAction, HookEvent};

/// A mux Hook that forwards streaming events from the LLM agent loop into
/// the barnstormer event system via the SpecActorHandle.
///
/// Manager agents stream text deltas to the UI. All agents (manager and worker)
/// stream tool activity notifications so users can see what the agent is doing.
pub struct StreamingHook {
    actor: Arc<SpecActorHandle>,
    agent_id: String,
    is_manager: bool,
}

impl StreamingHook {
    /// Create a new StreamingHook.
    ///
    /// - `actor`: handle to the spec actor for sending commands
    /// - `agent_id`: identifier for the agent producing events
    /// - `is_manager`: if true, text deltas are forwarded; workers skip text streaming
    pub fn new(actor: Arc<SpecActorHandle>, agent_id: String, is_manager: bool) -> Self {
        Self {
            actor,
            agent_id,
            is_manager,
        }
    }
}

#[async_trait]
impl Hook for StreamingHook {
    fn accepts(&self, event: &HookEvent) -> bool {
        matches!(
            event,
            HookEvent::StreamDelta { .. }
                | HookEvent::PostToolUse { .. }
                | HookEvent::Iteration { .. }
        )
    }

    async fn on_event(&self, event: &HookEvent) -> Result<HookAction, anyhow::Error> {
        match event {
            HookEvent::StreamDelta { text, .. } if self.is_manager => {
                let _ = self
                    .actor
                    .send_command(Command::StreamDelta {
                        agent_id: self.agent_id.clone(),
                        text: text.clone(),
                    })
                    .await;
            }

            HookEvent::StreamDelta { .. } => {
                // Workers don't stream text deltas
            }

            HookEvent::PostToolUse {
                tool_name, input, ..
            } => {
                let title = input.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let activity = if title.is_empty() {
                    tool_name.clone()
                } else {
                    format!("{tool_name}: {title}")
                };
                let _ = self
                    .actor
                    .send_command(Command::StreamToolActivity {
                        agent_id: self.agent_id.clone(),
                        activity,
                    })
                    .await;
            }

            HookEvent::Iteration { .. } => {
                let _ = self
                    .actor
                    .send_command(Command::StreamToolActivity {
                        agent_id: self.agent_id.clone(),
                        activity: "thinking...".to_string(),
                    })
                    .await;
            }

            _ => {}
        }

        Ok(HookAction::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use barnstormer_core::{SpecState, spawn};
    use mux::tool::ToolResult;
    use ulid::Ulid;

    /// Helper: create a SpecActorHandle wrapped in Arc plus a broadcast receiver.
    fn setup_actor() -> (
        Arc<SpecActorHandle>,
        tokio::sync::broadcast::Receiver<barnstormer_core::Event>,
    ) {
        let handle = spawn(Ulid::new(), SpecState::new());
        let rx = handle.subscribe();
        (Arc::new(handle), rx)
    }

    #[tokio::test]
    async fn hook_sends_streaming_delta_for_manager() {
        let (actor, mut rx) = setup_actor();
        let hook = StreamingHook::new(actor, "manager-1".to_string(), true);

        let event = HookEvent::StreamDelta {
            agent_id: "manager-1".to_string(),
            text: "Hello".to_string(),
        };

        let action = hook.on_event(&event).await.unwrap();
        assert!(matches!(action, HookAction::Continue));

        let broadcast = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("should receive broadcast within timeout")
            .expect("broadcast recv should succeed");

        match &broadcast.payload {
            barnstormer_core::EventPayload::StreamingDelta { agent_id, text } => {
                assert_eq!(agent_id, "manager-1");
                assert_eq!(text, "Hello");
            }
            other => panic!("expected StreamingDelta, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn hook_ignores_streaming_delta_for_non_manager() {
        let (actor, mut rx) = setup_actor();
        let hook = StreamingHook::new(actor, "worker-1".to_string(), false);

        let event = HookEvent::StreamDelta {
            agent_id: "worker-1".to_string(),
            text: "should be ignored".to_string(),
        };

        let action = hook.on_event(&event).await.unwrap();
        assert!(matches!(action, HookAction::Continue));

        // No broadcast should have been sent
        let result = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await;
        assert!(
            result.is_err(),
            "expected timeout (no broadcast), but got a message"
        );
    }

    #[tokio::test]
    async fn hook_sends_tool_activity_for_any_agent() {
        let (actor, mut rx) = setup_actor();
        // Use is_manager=false to show tool activity works for workers too
        let hook = StreamingHook::new(actor, "worker-1".to_string(), false);

        let event = HookEvent::PostToolUse {
            tool_name: "create_card".to_string(),
            tool_use_id: "toolu_123".to_string(),
            input: serde_json::json!({ "title": "Auth Flow" }),
            result: ToolResult::text("ok"),
        };

        let action = hook.on_event(&event).await.unwrap();
        assert!(matches!(action, HookAction::Continue));

        let broadcast = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("should receive broadcast within timeout")
            .expect("broadcast recv should succeed");

        match &broadcast.payload {
            barnstormer_core::EventPayload::StreamingToolActivity { agent_id, activity } => {
                assert_eq!(agent_id, "worker-1");
                assert!(
                    activity.contains("create_card"),
                    "activity should contain tool name"
                );
                assert!(
                    activity.contains("Auth Flow"),
                    "activity should contain title"
                );
            }
            other => panic!("expected StreamingToolActivity, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn hook_sends_thinking_on_iteration() {
        let (actor, mut rx) = setup_actor();
        let hook = StreamingHook::new(actor, "manager-1".to_string(), true);

        let event = HookEvent::Iteration {
            agent_id: "manager-1".to_string(),
            iteration: 1,
        };

        let action = hook.on_event(&event).await.unwrap();
        assert!(matches!(action, HookAction::Continue));

        let broadcast = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("should receive broadcast within timeout")
            .expect("broadcast recv should succeed");

        match &broadcast.payload {
            barnstormer_core::EventPayload::StreamingToolActivity { agent_id, activity } => {
                assert_eq!(agent_id, "manager-1");
                assert!(
                    activity.contains("thinking"),
                    "activity should contain 'thinking'"
                );
            }
            other => panic!("expected StreamingToolActivity, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn hook_rejects_irrelevant_events() {
        let (actor, _rx) = setup_actor();
        let hook = StreamingHook::new(actor, "manager-1".to_string(), true);

        let event = HookEvent::AgentStart {
            agent_id: "manager-1".to_string(),
            task: "brainstorm".to_string(),
        };

        assert!(
            !hook.accepts(&event),
            "accepts() should return false for AgentStart"
        );
    }
}
