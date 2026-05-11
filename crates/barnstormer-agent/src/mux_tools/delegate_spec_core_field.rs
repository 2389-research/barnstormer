// ABOUTME: delegate_spec_core_field mux tool — writes ONE prose markdown field
// ABOUTME: on the spec_core via a Haiku writer when the SubAgent has decided
// ABOUTME: what bullets/themes the field should contain.

use std::sync::Arc;

use async_trait::async_trait;
use mux::tool::{Tool, ToolResult};
use serde_json::json;
use ulid::Ulid;

use barnstormer_core::actor::SpecActorHandle;
use barnstormer_core::command::Command;

use crate::spec_core_field_writer::{SpecCoreField, SpecCoreFieldRequest, SpecCoreFieldWriter};

/// Tool that authors one prose field on the spec_core via a Haiku writer
/// and applies the resulting `UpdateSpecCore` command to the actor.
///
/// Covers the 5 prose-bearing fields: `description`, `constraints`,
/// `success_criteria`, `risks`, `notes`. The 3 short fields (`title`,
/// `one_liner`, `goal`) intentionally aren't delegate-able — they're
/// short enough Sonnet writes them inline at negligible cost.
///
/// Per-field voice library lives in the writer's system prompt. Sonnet
/// just supplies field_name + key_points (+ optional related card IDs
/// for grounding) and the writer expands them into the right shape:
/// description as 1-3 paragraphs, constraints as a bullet list, risks
/// as named subsections with Likelihood/Impact/Mitigation, etc.
#[derive(Clone)]
pub struct DelegateSpecCoreFieldTool {
    pub(crate) actor: Arc<SpecActorHandle>,
    pub(crate) writer: Arc<dyn SpecCoreFieldWriter>,
}

#[async_trait]
impl Tool for DelegateSpecCoreFieldTool {
    fn name(&self) -> &str {
        "delegate_spec_core_field"
    }

    fn description(&self) -> &str {
        "Author ONE prose field on the spec_core via a faster writer model. \
         Covers description / constraints / success_criteria / risks / notes \
         (the 5 markdown-prose fields). \
         \
         Prefer this over write_commands.UpdateSpecCore when you'd otherwise be \
         writing multi-paragraph or multi-bullet markdown into one of those fields. \
         You supply field_name + key_points; the writer expands them in the right \
         voice for the field (constraints become a bullet list with rationale; \
         risks become Likelihood/Impact/Mitigation sub-sections; etc.). \
         \
         The tool applies the UpdateSpecCore command itself — you do NOT need a \
         follow-up write_commands call. Other spec_core fields are left untouched. \
         \
         For the short fields (title, one_liner, goal), just use write_commands."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "field_name": {
                    "type": "string",
                    "enum": SpecCoreField::all_wire_values(),
                    "description": "Which prose field to author. Drives voice on the writer side:\n\
                                    - description: 1-3 paragraph product summary\n\
                                    - constraints: bullet markdown, each constraint with optional rationale\n\
                                    - success_criteria: bullet markdown, each a measurable outcome\n\
                                    - risks: named subsections (or bullets), each with Likelihood/Impact/Mitigation\n\
                                    - notes: bullet markdown, open questions and deferred decisions"
                },
                "key_points": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Ordered bullets/claims the field should include. The writer expands each into the right shape for the field."
                },
                "related_card_ids": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional ULIDs of cards on the board that should ground this field — the tool reads their titles + short excerpts as context."
                },
                "free_text_context": {
                    "type": "string",
                    "description": "Optional free-form context (e.g. 'this is the post-refining pass; tighten everything')."
                },
                "target_length_range": {
                    "type": "array",
                    "items": {"type": "integer"},
                    "minItems": 2,
                    "maxItems": 2,
                    "description": "Optional [min_chars, max_chars]. When omitted, uses default per field."
                }
            },
            "required": ["field_name", "key_points"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult, anyhow::Error> {
        let field_str = params
            .get("field_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'field_name' parameter"))?;
        let field = SpecCoreField::from_wire(field_str).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown field_name '{}'; valid: {:?}",
                field_str,
                SpecCoreField::all_wire_values()
            )
        })?;

        let key_points: Vec<String> = match params.get("key_points") {
            None | Some(serde_json::Value::Null) => {
                return Err(anyhow::anyhow!("'key_points' is required and must be a non-empty array"));
            }
            Some(serde_json::Value::Array(arr)) => arr
                .iter()
                .map(|v| match v.as_str() {
                    Some(s) if !s.trim().is_empty() => Ok(s.to_string()),
                    Some(_) => Err(anyhow::anyhow!("'key_points' entries must not be empty strings")),
                    None => Err(anyhow::anyhow!("each element of 'key_points' must be a string")),
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err(anyhow::anyhow!("'key_points' must be an array of strings")),
        };
        if key_points.is_empty() {
            return Err(anyhow::anyhow!(
                "'key_points' must contain at least one entry — writer needs signal"
            ));
        }

        let related_card_ids: Vec<Ulid> = match params.get("related_card_ids") {
            None | Some(serde_json::Value::Null) => Vec::new(),
            Some(serde_json::Value::Array(arr)) => arr
                .iter()
                .map(|v| match v.as_str() {
                    Some(s) => s
                        .parse::<Ulid>()
                        .map_err(|e| anyhow::anyhow!("bad related_card_id '{s}': {e}")),
                    None => Err(anyhow::anyhow!(
                        "each element of 'related_card_ids' must be a string ULID"
                    )),
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => {
                return Err(anyhow::anyhow!(
                    "'related_card_ids' must be an array of ULID strings"
                ));
            }
        };

        let free_text_context = params
            .get("free_text_context")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string());

        let target_length_range: Option<(usize, usize)> = match params.get("target_length_range") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::Array(arr)) if arr.len() == 2 => {
                let lo = arr[0].as_i64().filter(|n| *n >= 0).map(|n| n as usize);
                let hi = arr[1].as_i64().filter(|n| *n >= 0).map(|n| n as usize);
                match (lo, hi) {
                    (Some(l), Some(h)) if l <= h => Some((l, h)),
                    _ => return Err(anyhow::anyhow!(
                        "'target_length_range' must be [min, max] non-negative integers with min <= max"
                    )),
                }
            }
            Some(_) => return Err(anyhow::anyhow!(
                "'target_length_range' must be a two-element [min, max] integer array"
            )),
        };

        // Resolve related-card context from state once.
        let state = self.actor.read_state().await;
        let mut related_summaries: Vec<(String, String)> = Vec::new();
        for cid in &related_card_ids {
            if let Some(c) = state.cards.get(cid) {
                let excerpt: String = c
                    .body
                    .as_deref()
                    .map(|b| b.lines().next().unwrap_or("").chars().take(160).collect())
                    .unwrap_or_default();
                related_summaries.push((c.title.clone(), excerpt));
            }
        }
        drop(state);

        let request = SpecCoreFieldRequest {
            field,
            key_points,
            related_card_ids,
            free_text_context,
            target_length_range,
        };

        let spec_id = self.actor.spec_id;
        let output = match self.writer.write_field(spec_id, &request, &related_summaries).await {
            Ok(o) => o,
            Err(e) => return Ok(ToolResult::error(format!("spec_core write failed: {e}"))),
        };

        // Apply the UpdateSpecCore command with ONLY the targeted field
        // populated. The reducer leaves all other fields untouched. This
        // is the critical correctness property — we never want this tool
        // to clobber `description` when the agent asked for `constraints`.
        let update_cmd = build_update_command(field, output.markdown.clone());
        if let Err(e) = self.actor.send_command(update_cmd).await {
            return Ok(ToolResult::error(format!(
                "field written but UpdateSpecCore failed: {e}"
            )));
        }

        // Record per-call writer usage.
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
                    "failed to record spec_core_field-writer usage event"
                );
            }
        }

        Ok(ToolResult::text(format!(
            "spec_core.{} updated ({} chars).",
            field.as_wire(),
            output.markdown.len()
        )))
    }
}

/// Build an `UpdateSpecCore` command with ONLY the targeted field set so
/// the reducer leaves every other field as-is. Critical: the reducer
/// uses `Option<String>` semantics — `None` means "don't touch", `Some`
/// means "replace". A single-field write must therefore set None on the
/// 7 untouched fields.
fn build_update_command(field: SpecCoreField, markdown: String) -> Command {
    let mut description = None;
    let mut constraints = None;
    let mut success_criteria = None;
    let mut risks = None;
    let mut notes = None;
    match field {
        SpecCoreField::Description => description = Some(markdown),
        SpecCoreField::Constraints => constraints = Some(markdown),
        SpecCoreField::SuccessCriteria => success_criteria = Some(markdown),
        SpecCoreField::Risks => risks = Some(markdown),
        SpecCoreField::Notes => notes = Some(markdown),
    }
    Command::UpdateSpecCore {
        title: None,
        one_liner: None,
        goal: None,
        description,
        constraints,
        success_criteria,
        risks,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_decomposer::DecomposerUsage;
    use crate::spec_core_field_writer::SpecCoreFieldOutput;
    use barnstormer_core::actor;
    use barnstormer_core::state::SpecState;

    #[derive(Debug)]
    struct StubWriter {
        markdown: String,
    }

    #[async_trait::async_trait]
    impl SpecCoreFieldWriter for StubWriter {
        async fn write_field(
            &self,
            _spec_id: Ulid,
            _request: &SpecCoreFieldRequest,
            _related_card_summaries: &[(String, String)],
        ) -> Result<SpecCoreFieldOutput, String> {
            Ok(SpecCoreFieldOutput {
                markdown: self.markdown.clone(),
                usage: vec![DecomposerUsage {
                    agent_id: "spec-core-field-writer".into(),
                    model: "claude-haiku-4-5".into(),
                    input_tokens: 50,
                    output_tokens: 100,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                }],
            })
        }
    }

    async fn make_test_actor() -> (Ulid, SpecActorHandle) {
        let spec_id = Ulid::new();
        let handle = actor::spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "test".into(),
                one_liner: "t".into(),
                goal: "g".into(),
            })
            .await
            .unwrap();
        (spec_id, handle)
    }

    fn make_tool_with(actor: SpecActorHandle, markdown: &str) -> DelegateSpecCoreFieldTool {
        DelegateSpecCoreFieldTool {
            actor: Arc::new(actor),
            writer: Arc::new(StubWriter {
                markdown: markdown.to_string(),
            }),
        }
    }

    #[tokio::test]
    async fn tool_name_and_schema() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle, "md");
        assert_eq!(tool.name(), "delegate_spec_core_field");
        let schema = tool.schema();
        let required = schema
            .get("required")
            .and_then(|v| v.as_array())
            .expect("schema has required");
        let names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"field_name"));
        assert!(names.contains(&"key_points"));
        let enum_arr = schema
            .pointer("/properties/field_name/enum")
            .and_then(|v| v.as_array())
            .expect("field_name enum exists");
        assert_eq!(enum_arr.len(), 5);
    }

    #[tokio::test]
    async fn writes_constraints_and_leaves_other_fields_untouched() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(
            handle.clone(),
            "- GitHub backend\n- Agent-as-connector\n- MCP-first",
        );

        // Seed an existing description that we should NOT clobber.
        handle
            .send_command(Command::UpdateSpecCore {
                title: None,
                one_liner: None,
                goal: None,
                description: Some("Existing description should stay.".into()),
                constraints: None,
                success_criteria: None,
                risks: None,
                notes: None,
            })
            .await
            .unwrap();

        tool.execute(json!({
            "field_name": "constraints",
            "key_points": ["GitHub backend", "Agent-as-connector", "MCP-first"]
        }))
        .await
        .unwrap();

        let state = handle.read_state().await;
        let core = state.core.as_ref().unwrap();
        // Target field written
        assert!(core.constraints.as_deref().unwrap_or("").contains("GitHub backend"));
        // Untouched field preserved
        assert_eq!(
            core.description.as_deref(),
            Some("Existing description should stay.")
        );
        // Other fields stay None
        assert!(core.success_criteria.is_none());
        assert!(core.risks.is_none());
        assert!(core.notes.is_none());
    }

    #[tokio::test]
    async fn each_field_routes_to_its_target() {
        // Pin that field_name → which spec_core field gets written.
        // Catches a swap regression in build_update_command.
        let cases = [
            ("description", "DESC"),
            ("constraints", "CONS"),
            ("success_criteria", "SUCC"),
            ("risks", "RISK"),
            ("notes", "NOTE"),
        ];
        for (field, marker) in cases {
            let (_id, handle) = make_test_actor().await;
            let tool = make_tool_with(handle.clone(), marker);
            tool.execute(json!({
                "field_name": field,
                "key_points": ["x"]
            }))
            .await
            .unwrap();
            let state = handle.read_state().await;
            let core = state.core.as_ref().unwrap();
            let written = match field {
                "description" => core.description.as_deref(),
                "constraints" => core.constraints.as_deref(),
                "success_criteria" => core.success_criteria.as_deref(),
                "risks" => core.risks.as_deref(),
                "notes" => core.notes.as_deref(),
                _ => unreachable!(),
            };
            assert_eq!(
                written,
                Some(marker),
                "field {field} should receive marker {marker}"
            );
        }
    }

    #[tokio::test]
    async fn rejects_unknown_field_name() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle, "md");
        let err = tool
            .execute(json!({
                "field_name": "title",  // valid spec_core field but not in our 5
                "key_points": ["x"]
            }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown field_name"));

        let err = tool
            .execute(json!({
                "field_name": "constraitns",  // typo
                "key_points": ["x"]
            }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown field_name"));
    }

    #[tokio::test]
    async fn rejects_missing_or_empty_key_points() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle, "md");

        // Missing
        let err = tool
            .execute(json!({"field_name": "constraints"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("key_points"));

        // Empty array
        let err = tool
            .execute(json!({"field_name": "constraints", "key_points": []}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("at least one"));

        // Whitespace-only entry
        let err = tool
            .execute(json!({"field_name": "constraints", "key_points": ["  "]}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[tokio::test]
    async fn rejects_malformed_target_length_range() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle, "md");
        let err = tool
            .execute(json!({
                "field_name": "constraints",
                "key_points": ["x"],
                "target_length_range": [1000, 500]
            }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("target_length_range"));
    }

    #[tokio::test]
    async fn writer_errors_propagate_as_tool_errors() {
        #[derive(Debug)]
        struct FailingWriter;
        #[async_trait::async_trait]
        impl SpecCoreFieldWriter for FailingWriter {
            async fn write_field(
                &self,
                _spec_id: Ulid,
                _request: &SpecCoreFieldRequest,
                _related_card_summaries: &[(String, String)],
            ) -> Result<SpecCoreFieldOutput, String> {
                Err("writer upstream failed".into())
            }
        }
        let (_id, handle) = make_test_actor().await;
        let tool = DelegateSpecCoreFieldTool {
            actor: Arc::new(handle),
            writer: Arc::new(FailingWriter),
        };
        let result = tool
            .execute(json!({
                "field_name": "constraints",
                "key_points": ["x"]
            }))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("writer upstream failed"));
    }

    #[tokio::test]
    async fn key_points_propagate_through_to_writer() {
        use std::sync::Mutex;

        #[derive(Debug)]
        struct CapturingWriter {
            captured: Arc<Mutex<Vec<String>>>,
        }
        #[async_trait::async_trait]
        impl SpecCoreFieldWriter for CapturingWriter {
            async fn write_field(
                &self,
                _spec_id: Ulid,
                request: &SpecCoreFieldRequest,
                _related_card_summaries: &[(String, String)],
            ) -> Result<SpecCoreFieldOutput, String> {
                *self.captured.lock().unwrap() = request.key_points.clone();
                Ok(SpecCoreFieldOutput {
                    markdown: "ok".into(),
                    usage: vec![],
                })
            }
        }

        let captured = Arc::new(Mutex::new(Vec::new()));
        let (_id, handle) = make_test_actor().await;
        let tool = DelegateSpecCoreFieldTool {
            actor: Arc::new(handle),
            writer: Arc::new(CapturingWriter {
                captured: Arc::clone(&captured),
            }),
        };
        tool.execute(json!({
            "field_name": "risks",
            "key_points": ["A", "B", "C"]
        }))
        .await
        .unwrap();

        let got = captured.lock().unwrap().clone();
        assert_eq!(got, vec!["A".to_string(), "B".to_string(), "C".to_string()]);
    }

    #[tokio::test]
    async fn related_card_summaries_get_extracted_and_passed() {
        use std::sync::Mutex;

        #[derive(Debug)]
        struct CapturingWriter {
            captured: Arc<Mutex<Vec<(String, String)>>>,
        }
        #[async_trait::async_trait]
        impl SpecCoreFieldWriter for CapturingWriter {
            async fn write_field(
                &self,
                _spec_id: Ulid,
                _request: &SpecCoreFieldRequest,
                related_card_summaries: &[(String, String)],
            ) -> Result<SpecCoreFieldOutput, String> {
                *self.captured.lock().unwrap() = related_card_summaries.to_vec();
                Ok(SpecCoreFieldOutput {
                    markdown: "ok".into(),
                    usage: vec![],
                })
            }
        }

        let (_id, handle) = make_test_actor().await;
        handle
            .send_command(Command::CreateCard {
                card_type: "risk".into(),
                title: "Connector maintenance".into(),
                body: Some("Each harness UI change requires a patch.".into()),
                lane: Some("Ideas".into()),
                created_by: "seed".into(),
                source_attachment_id: None,
            })
            .await
            .unwrap();
        let state = handle.read_state().await;
        let existing_id = *state.cards.keys().next().unwrap();
        drop(state);

        let captured = Arc::new(Mutex::new(Vec::new()));
        let tool = DelegateSpecCoreFieldTool {
            actor: Arc::new(handle),
            writer: Arc::new(CapturingWriter {
                captured: Arc::clone(&captured),
            }),
        };
        tool.execute(json!({
            "field_name": "risks",
            "key_points": ["A"],
            "related_card_ids": [existing_id.to_string()]
        }))
        .await
        .unwrap();

        let got = captured.lock().unwrap().clone();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "Connector maintenance");
        assert!(got[0].1.contains("harness UI change"));
    }

    #[test]
    fn build_update_command_writes_one_field_only() {
        // The reducer treats None as "don't touch". Pin that
        // build_update_command produces Some on exactly one field.
        let cmd = build_update_command(SpecCoreField::Risks, "md".into());
        match cmd {
            Command::UpdateSpecCore {
                title,
                one_liner,
                goal,
                description,
                constraints,
                success_criteria,
                risks,
                notes,
            } => {
                assert!(title.is_none());
                assert!(one_liner.is_none());
                assert!(goal.is_none());
                assert!(description.is_none());
                assert!(constraints.is_none());
                assert!(success_criteria.is_none());
                assert_eq!(risks.as_deref(), Some("md"));
                assert!(notes.is_none());
            }
            _ => panic!("expected UpdateSpecCore"),
        }
    }
}
