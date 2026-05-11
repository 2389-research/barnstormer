// ABOUTME: delegate_card_decomposition mux tool — runs an architect+executor
// ABOUTME: pipeline under a Haiku-class model to produce many cards from a
// ABOUTME: source brief, instead of bundling them into write_commands output tokens.

use std::sync::Arc;

use async_trait::async_trait;
use mux::tool::{Tool, ToolResult};
use serde_json::json;
use ulid::Ulid;

use barnstormer_core::actor::SpecActorHandle;
use barnstormer_core::command::Command;

use crate::card_decomposer::CardDecomposer;

/// Tool that decomposes an attached source brief into many cards via a
/// Haiku-class architect+executor pipeline, applying the resulting
/// `CreateCard` commands to the actor.
///
/// Sonnet emits structured intent — `{brief_attachment_id, target_card_count,
/// decomposition_hints}` — and the tool handler runs the prose-generation
/// work internally. This pulls bulk card prose out of Sonnet's expensive
/// output tokens. The decomposition pattern was validated in 2026-05-09
/// run-04 experiments (Haiku-arch + Haiku-exec produced ~$0.0045/card vs
/// Sonnet's $0.0086/card baseline at the operation level).
#[derive(Clone)]
pub struct DelegateCardDecompositionTool {
    pub(crate) actor: Arc<SpecActorHandle>,
    pub(crate) agent_id: String,
    pub(crate) decomposer: Arc<dyn CardDecomposer>,
}

#[async_trait]
impl Tool for DelegateCardDecompositionTool {
    fn name(&self) -> &str {
        "delegate_card_decomposition"
    }

    fn description(&self) -> &str {
        "Decompose an attached source brief into many cards with bodies, using a faster model. \
         Use this when the user attached a brief/RFP/design doc and the board needs the initial \
         set of cards populated. Cheaper than writing many CreateCard commands with full bodies \
         through write_commands. The tool runs an architect+executor split internally; you just \
         provide the attachment, target count, and any focus hints. Cards are applied to the \
         board automatically — you do NOT need a follow-up write_commands call."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "brief_attachment_id": {
                    "type": "string",
                    "description": "ULID of the attached source brief (from read_state.context_attachments[].attachment_id)."
                },
                "target_card_count": {
                    "type": "integer",
                    "description": "Approximate number of cards. 18-25 typical for a normal-sized brief; up to 30 for a large RFP.",
                    "minimum": 5,
                    "maximum": 50
                },
                "decomposition_hints": {
                    "type": "string",
                    "description": "Optional free-text guidance on what to emphasize (e.g. 'focus on validation engine architecture' or 'include risk cards for each external dependency')."
                }
            },
            "required": ["brief_attachment_id", "target_card_count"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult, anyhow::Error> {
        let brief_id_str = params
            .get("brief_attachment_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'brief_attachment_id' parameter"))?;
        let brief_attachment_id: Ulid = brief_id_str
            .parse()
            .map_err(|e| anyhow::anyhow!("bad brief_attachment_id: {e}"))?;

        let target_count_i = params
            .get("target_card_count")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| anyhow::anyhow!("missing 'target_card_count' parameter"))?;
        if !(5..=50).contains(&target_count_i) {
            return Err(anyhow::anyhow!(
                "'target_card_count' must be between 5 and 50, got {target_count_i}"
            ));
        }
        let target_count = target_count_i as u32;

        let hints = params
            .get("decomposition_hints")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string());

        // Sanity-check that the attachment exists and isn't removed before
        // we pay for any LLM calls. Also extract its stored summary so the
        // decomposer can fall back to that when the raw bytes aren't UTF-8
        // text (PDFs, images, etc.).
        let state = self.actor.read_state().await;
        let attachment = state
            .context_attachments
            .iter()
            .find(|a| a.attachment_id == brief_attachment_id && !a.removed)
            .cloned();
        drop(state);
        let attachment = match attachment {
            Some(a) => a,
            None => {
                return Ok(ToolResult::error(format!(
                    "attachment {brief_attachment_id} not found"
                )));
            }
        };
        let attachment_summary = attachment.summary.clone();

        let spec_id = self.actor.spec_id;
        let output = match self
            .decomposer
            .decompose(
                spec_id,
                brief_attachment_id,
                target_count,
                hints.as_deref(),
                attachment_summary.as_deref(),
            )
            .await
        {
            Ok(o) => o,
            Err(e) => return Ok(ToolResult::error(format!("decomposition failed: {e}"))),
        };

        // Apply each decomposed card as a CreateCard command. The actor's
        // reducer turns it into a CardCreated event; the SSE compositor
        // re-renders the board.
        let mut created = 0u32;
        let mut create_errors: Vec<String> = Vec::new();
        for c in &output.cards {
            let cmd = Command::CreateCard {
                card_type: c.card_type.clone(),
                title: c.title.clone(),
                body: Some(c.body.clone()),
                lane: c.lane.clone(),
                created_by: self.agent_id.clone(),
                source_attachment_id: Some(brief_attachment_id),
            };
            match self.actor.send_command(cmd).await {
                Ok(_) => created += 1,
                Err(e) => create_errors.push(format!("{}: {}", c.title, e)),
            }
        }

        // Record each Haiku call's usage as its own AgentStepUsage event so
        // post-run cost analysis can attribute spend to the decomposer's
        // sub-agents without scraping API logs. Failures here are logged but
        // not surfaced — telemetry shouldn't block the tool's success path.
        for u in &output.usage {
            let cmd = Command::RecordAgentUsage {
                agent_id: u.agent_id.clone(),
                model: u.model.clone(),
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
                cache_read_tokens: u.cache_read_tokens,
                cache_write_tokens: u.cache_write_tokens,
            };
            if let Err(e) = self.actor.send_command(cmd).await {
                tracing::warn!(
                    agent_id = %u.agent_id,
                    error = %e,
                    "failed to record decomposer usage event"
                );
            }
        }

        let summary = if create_errors.is_empty() {
            format!(
                "Decomposed brief into {} cards ({}/{} applied successfully).",
                output.cards.len(),
                created,
                output.cards.len()
            )
        } else {
            format!(
                "Decomposed brief into {} cards; {} applied, {} failed: {}",
                output.cards.len(),
                created,
                create_errors.len(),
                create_errors.join("; ")
            )
        };
        Ok(ToolResult::text(summary))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_decomposer::{DecomposedCard, DecomposerOutput, DecomposerUsage};
    use barnstormer_core::actor;
    use barnstormer_core::state::SpecState;

    #[derive(Debug)]
    struct StubDecomposer {
        cards: Vec<DecomposedCard>,
        usage: Vec<DecomposerUsage>,
    }

    #[async_trait::async_trait]
    impl CardDecomposer for StubDecomposer {
        async fn decompose(
            &self,
            _spec_id: Ulid,
            _brief_attachment_id: Ulid,
            _target_card_count: u32,
            _decomposition_hints: Option<&str>,
            _attachment_summary: Option<&str>,
        ) -> Result<DecomposerOutput, String> {
            Ok(DecomposerOutput {
                cards: self.cards.clone(),
                usage: self.usage.clone(),
            })
        }
    }

    #[derive(Debug)]
    struct FailingDecomposer(&'static str);

    #[async_trait::async_trait]
    impl CardDecomposer for FailingDecomposer {
        async fn decompose(
            &self,
            _spec_id: Ulid,
            _brief_attachment_id: Ulid,
            _target_card_count: u32,
            _decomposition_hints: Option<&str>,
            _attachment_summary: Option<&str>,
        ) -> Result<DecomposerOutput, String> {
            Err(self.0.to_string())
        }
    }

    async fn make_test_actor() -> (Ulid, SpecActorHandle) {
        let spec_id = Ulid::new();
        let handle = actor::spawn(spec_id, SpecState::new());
        // AttachContext requires a spec to exist (state.cards expects a spec_core);
        // create one up front so attach_brief works in tests.
        handle
            .send_command(Command::CreateSpec {
                title: "test".to_string(),
                one_liner: "test spec".to_string(),
                goal: "exercise the tool".to_string(),
            })
            .await
            .unwrap();
        (spec_id, handle)
    }

    async fn attach_brief(handle: &SpecActorHandle) -> Ulid {
        let attachment_id = Ulid::new();
        handle
            .send_command(Command::AttachContext {
                attachment_id,
                filename: "brief.md".to_string(),
                mime_type: "text/markdown".to_string(),
                size_bytes: 100,
            })
            .await
            .unwrap();
        attachment_id
    }

    fn sample_cards() -> Vec<DecomposedCard> {
        vec![
            DecomposedCard {
                title: "First Card".into(),
                card_type: "idea".into(),
                body: "Body 1".into(),
                lane: Some("Ideas".into()),
            },
            DecomposedCard {
                title: "Second Card".into(),
                card_type: "task".into(),
                body: "Body 2".into(),
                lane: Some("Plan".into()),
            },
        ]
    }

    fn sample_usage() -> Vec<DecomposerUsage> {
        vec![
            DecomposerUsage {
                agent_id: "card-decomposer-architect".into(),
                model: "claude-haiku-4-5".into(),
                input_tokens: 5000,
                output_tokens: 800,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            },
            DecomposerUsage {
                agent_id: "card-decomposer-executor".into(),
                model: "claude-haiku-4-5".into(),
                input_tokens: 600,
                output_tokens: 400,
                cache_read_tokens: 5000,
                cache_write_tokens: 0,
            },
        ]
    }

    #[tokio::test]
    async fn tool_name_and_schema() {
        let (_id, handle) = make_test_actor().await;
        let tool = DelegateCardDecompositionTool {
            actor: Arc::new(handle),
            agent_id: "manager-test".into(),
            decomposer: Arc::new(StubDecomposer {
                cards: vec![],
                usage: vec![],
            }),
        };
        assert_eq!(tool.name(), "delegate_card_decomposition");
        let schema = tool.schema();
        assert!(schema.is_object());
        let props = schema.get("properties").unwrap().as_object().unwrap();
        assert!(props.contains_key("brief_attachment_id"));
        assert!(props.contains_key("target_card_count"));
        assert!(props.contains_key("decomposition_hints"));
    }

    #[tokio::test]
    async fn rejects_missing_brief_attachment_id() {
        let (_id, handle) = make_test_actor().await;
        let tool = DelegateCardDecompositionTool {
            actor: Arc::new(handle),
            agent_id: "manager-test".into(),
            decomposer: Arc::new(StubDecomposer {
                cards: vec![],
                usage: vec![],
            }),
        };
        let err = tool
            .execute(json!({ "target_card_count": 20 }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("brief_attachment_id"));
    }

    #[tokio::test]
    async fn rejects_bad_attachment_id() {
        let (_id, handle) = make_test_actor().await;
        let tool = DelegateCardDecompositionTool {
            actor: Arc::new(handle),
            agent_id: "manager-test".into(),
            decomposer: Arc::new(StubDecomposer {
                cards: vec![],
                usage: vec![],
            }),
        };
        let err = tool
            .execute(json!({
                "brief_attachment_id": "not-a-ulid",
                "target_card_count": 20
            }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("bad brief_attachment_id"));
    }

    #[tokio::test]
    async fn rejects_out_of_range_target_count() {
        let (_id, handle) = make_test_actor().await;
        let attachment_id = attach_brief(&handle).await;
        let tool = DelegateCardDecompositionTool {
            actor: Arc::new(handle),
            agent_id: "manager-test".into(),
            decomposer: Arc::new(StubDecomposer {
                cards: vec![],
                usage: vec![],
            }),
        };
        let err = tool
            .execute(json!({
                "brief_attachment_id": attachment_id.to_string(),
                "target_card_count": 100
            }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("between 5 and 50"));
    }

    #[tokio::test]
    async fn returns_error_when_attachment_missing() {
        let (_id, handle) = make_test_actor().await;
        let tool = DelegateCardDecompositionTool {
            actor: Arc::new(handle),
            agent_id: "manager-test".into(),
            decomposer: Arc::new(StubDecomposer {
                cards: vec![],
                usage: vec![],
            }),
        };
        let unknown = Ulid::new();
        let result = tool
            .execute(json!({
                "brief_attachment_id": unknown.to_string(),
                "target_card_count": 20
            }))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn returns_tool_error_when_decomposer_fails() {
        let (_id, handle) = make_test_actor().await;
        let attachment_id = attach_brief(&handle).await;
        let tool = DelegateCardDecompositionTool {
            actor: Arc::new(handle),
            agent_id: "manager-test".into(),
            decomposer: Arc::new(FailingDecomposer("upstream broke")),
        };
        let result = tool
            .execute(json!({
                "brief_attachment_id": attachment_id.to_string(),
                "target_card_count": 20
            }))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("upstream broke"));
    }

    #[tokio::test]
    async fn applies_each_card_to_actor_with_source_attachment_id() {
        let (_id, handle) = make_test_actor().await;
        let attachment_id = attach_brief(&handle).await;
        let tool = DelegateCardDecompositionTool {
            actor: Arc::new(handle.clone()),
            agent_id: "manager-test".into(),
            decomposer: Arc::new(StubDecomposer {
                cards: sample_cards(),
                usage: sample_usage(),
            }),
        };
        let result = tool
            .execute(json!({
                "brief_attachment_id": attachment_id.to_string(),
                "target_card_count": 20
            }))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("2 cards"));

        let state = handle.read_state().await;
        assert_eq!(state.cards.len(), 2);
        for card in state.cards.values() {
            assert_eq!(card.source_attachment_id, Some(attachment_id));
            assert_eq!(card.created_by, "manager-test");
        }
    }

    #[tokio::test]
    async fn decomposition_hints_propagate_through_to_decomposer() {
        // Stub captures the hint via a shared Mutex so we can assert it was
        // forwarded. Confirms the tool doesn't silently drop the field.
        use std::sync::Mutex;

        #[derive(Debug)]
        struct CapturingDecomposer {
            captured: Arc<Mutex<Option<String>>>,
        }

        #[async_trait::async_trait]
        impl CardDecomposer for CapturingDecomposer {
            async fn decompose(
                &self,
                _spec_id: Ulid,
                _brief_attachment_id: Ulid,
                _target_card_count: u32,
                decomposition_hints: Option<&str>,
                _attachment_summary: Option<&str>,
            ) -> Result<DecomposerOutput, String> {
                *self.captured.lock().unwrap() = decomposition_hints.map(|s| s.to_string());
                Ok(DecomposerOutput {
                    cards: vec![],
                    usage: vec![],
                })
            }
        }

        let captured = Arc::new(Mutex::new(None));
        let (_id, handle) = make_test_actor().await;
        let attachment_id = attach_brief(&handle).await;
        let tool = DelegateCardDecompositionTool {
            actor: Arc::new(handle),
            agent_id: "manager-test".into(),
            decomposer: Arc::new(CapturingDecomposer {
                captured: Arc::clone(&captured),
            }),
        };
        tool.execute(json!({
            "brief_attachment_id": attachment_id.to_string(),
            "target_card_count": 20,
            "decomposition_hints": "focus on validation engine"
        }))
        .await
        .unwrap();

        let got = captured.lock().unwrap().clone();
        assert_eq!(got.as_deref(), Some("focus on validation engine"));
    }
}
