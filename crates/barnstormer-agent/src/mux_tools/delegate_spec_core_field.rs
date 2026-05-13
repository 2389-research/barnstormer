// ABOUTME: delegate_spec_core_field mux tool — writes spec_core prose fields
// ABOUTME: via a Haiku writer. Accepts single-field or {fields:[...]} batch
// ABOUTME: so an agent that decides to fill out multiple fields from one user
// ABOUTME: response does it in ONE tool call instead of N sequential ones.

use std::sync::Arc;

use async_trait::async_trait;
use mux::tool::{Tool, ToolResult};
use serde_json::json;
use ulid::Ulid;

use barnstormer_core::actor::SpecActorHandle;
use barnstormer_core::command::Command;

use crate::spec_core_field_writer::{
    SpecCoreField, SpecCoreFieldContext, SpecCoreFieldRequest, SpecCoreFieldWriter,
};

/// Tool that authors prose fields on the spec_core via a Haiku writer
/// and applies the resulting `UpdateSpecCore` commands to the actor.
///
/// Covers the 5 prose-bearing fields: `description`, `constraints`,
/// `success_criteria`, `risks`, `notes`. The 3 short fields (`title`,
/// `one_liner`, `goal`) intentionally aren't delegate-able — short
/// enough Sonnet writes them inline.
///
/// Two call shapes:
///
/// 1. **Single**: `{field_name, key_points, …}` — author ONE field.
///
/// 2. **Batch**: `{fields: [{field_name, key_points, …}, …]}` — author
///    multiple fields in a single tool call. Use when ONE user message
///    triggers updates to several fields (after refinement, after
///    adversarial review, when filling out spec_core from scratch).
///    Writer runs each field's LLM call in parallel.
///
/// The two shapes are mutually exclusive — providing both `fields` and a
/// top-level `field_name` is an error.
///
/// Each successful field-write produces an `UpdateSpecCore` command
/// touching ONLY that field. Other spec_core fields stay untouched.
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
        "Author prose fields on the spec_core via a faster writer model. Covers description / \
         constraints / success_criteria / risks / notes. Two call shapes:\n\n\
         SINGLE: {field_name, key_points, ...} — author ONE field.\n\
         BATCH:  {fields: [{field_name, key_points, ...}, ...]} — author N fields in one tool call. \
         STRONGLY PREFER batch when ONE user message triggers updates to multiple spec_core fields \
         (post-refinement, post-adversarial-review, initial spec_core fill-out). The writer runs \
         each field's call in parallel; you skip N sequential tool-use loop iterations.\n\n\
         Both shapes apply UpdateSpecCore internally, touching only the targeted field(s). Other \
         spec_core fields stay untouched. No follow-up write_commands call needed.\n\n\
         For the short fields (title, one_liner, goal), use write_commands.UpdateSpecCore directly."
    }

    fn schema(&self) -> serde_json::Value {
        let single_props = single_field_properties();
        json!({
            "type": "object",
            "properties": {
                "field_name":           single_props["field_name"],
                "key_points":           single_props["key_points"],
                "related_card_ids":     single_props["related_card_ids"],
                "free_text_context":    single_props["free_text_context"],
                "target_length_range":  single_props["target_length_range"],
                "fields": {
                    "type": "array",
                    "description": "BATCH MODE. Use when one user message triggers updates to multiple spec_core fields. Each element has the same fields as the single-field form (field_name + key_points required; others optional). Mutually exclusive with the top-level field_name.",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": single_props,
                        "required": ["field_name", "key_points"]
                    }
                }
            }
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult, anyhow::Error> {
        let has_top_field_name = params.get("field_name").is_some();
        let has_fields = params.get("fields").is_some();

        if has_top_field_name && has_fields {
            return Err(anyhow::anyhow!(
                "provide EITHER single-field args (field_name/key_points/...) OR fields:[...] batch, not both"
            ));
        }
        if !has_top_field_name && !has_fields {
            return Err(anyhow::anyhow!(
                "missing args: provide either field_name+key_points (single) or fields:[...] (batch)"
            ));
        }

        let requests: Vec<SpecCoreFieldRequest> = if has_fields {
            let arr = params
                .get("fields")
                .and_then(|v| v.as_array())
                .ok_or_else(|| anyhow::anyhow!("'fields' must be an array"))?;
            if arr.is_empty() {
                return Err(anyhow::anyhow!("'fields' must contain at least one field"));
            }
            let mut parsed = Vec::with_capacity(arr.len());
            for (i, entry) in arr.iter().enumerate() {
                let req = parse_single_field(entry)
                    .map_err(|e| anyhow::anyhow!("fields[{i}]: {e}"))?;
                parsed.push(req);
            }
            parsed
        } else {
            vec![parse_single_field(&params)?]
        };

        // Resolve per-request related-card context from state in one read.
        let contexts = self.resolve_contexts(&requests).await;

        let spec_id = self.actor.spec_id;
        let results = match self.writer.write_fields(spec_id, &requests, &contexts).await {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolResult::error(format!("spec_core write failed: {e}")));
            }
        };

        if results.len() != requests.len() {
            return Ok(ToolResult::error(format!(
                "writer returned {} results for {} requests; refusing to apply (shape contract violated)",
                results.len(),
                requests.len()
            )));
        }

        // Apply each successful field-write as its own UpdateSpecCore
        // command, touching only that field. Partial failures don't
        // abort the batch.
        let mut applied = 0usize;
        let mut errors: Vec<(usize, String, String)> = Vec::new();
        for (i, (req, result)) in requests.iter().zip(results.iter()).enumerate() {
            match result {
                Err(e) => {
                    errors.push((i, req.field.as_wire().to_string(), e.clone()));
                    continue;
                }
                Ok(output) => {
                    let cmd = build_update_command(req.field, output.markdown.clone());
                    if let Err(e) = self.actor.send_command(cmd).await {
                        errors.push((
                            i,
                            req.field.as_wire().to_string(),
                            format!("UpdateSpecCore failed: {e}"),
                        ));
                        continue;
                    }
                    applied += 1;
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
                }
            }
        }

        let summary = if errors.is_empty() {
            if requests.len() == 1 {
                format!(
                    "spec_core.{} updated.",
                    requests[0].field.as_wire()
                )
            } else {
                let names: Vec<&str> = requests.iter().map(|r| r.field.as_wire()).collect();
                format!(
                    "{} spec_core fields updated: {}",
                    applied,
                    names.join(", ")
                )
            }
        } else {
            let mut s = format!(
                "{}/{} spec_core fields updated. Failures:\n",
                applied,
                requests.len()
            );
            for (i, field, err) in &errors {
                s.push_str(&format!("  - fields[{i}] '{field}': {err}\n"));
            }
            s
        };
        Ok(ToolResult::text(summary))
    }
}

impl DelegateSpecCoreFieldTool {
    /// Resolve per-field grounding context from spec state in a single read.
    async fn resolve_contexts(
        &self,
        requests: &[SpecCoreFieldRequest],
    ) -> Vec<SpecCoreFieldContext> {
        let state = self.actor.read_state().await;
        let mut out = Vec::with_capacity(requests.len());
        for req in requests {
            let mut ctx = SpecCoreFieldContext::default();
            for cid in &req.related_card_ids {
                if let Some(c) = state.cards.get(cid) {
                    let excerpt: String = c
                        .body
                        .as_deref()
                        .map(|b| b.lines().next().unwrap_or("").chars().take(160).collect())
                        .unwrap_or_default();
                    ctx.related_card_summaries
                        .push((c.title.clone(), excerpt));
                }
            }
            out.push(ctx);
        }
        out
    }
}

fn single_field_properties() -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    m.insert(
        "field_name".into(),
        json!({
            "type": "string",
            "enum": SpecCoreField::all_wire_values(),
            "description": "Which prose field. Drives voice: description (1-3 paragraphs), constraints (bullets with rationale), success_criteria (measurable bullets), risks (Likelihood/Impact/Mitigation subsections), notes (open questions / annotations)."
        }),
    );
    m.insert(
        "key_points".into(),
        json!({
            "type": "array",
            "items": {"type": "string"},
            "description": "Ordered bullets/claims the field should include. The writer expands each into the right shape for the field."
        }),
    );
    m.insert(
        "related_card_ids".into(),
        json!({
            "type": "array",
            "items": {"type": "string"},
            "description": "Optional ULIDs of cards on the board that ground this field."
        }),
    );
    m.insert(
        "free_text_context".into(),
        json!({"type": "string", "description": "Optional free-form context."}),
    );
    m.insert(
        "target_length_range".into(),
        json!({
            "type": "array",
            "items": {"type": "integer"},
            "minItems": 2,
            "maxItems": 2,
            "description": "Optional [min_chars, max_chars]; defaults per field."
        }),
    );
    m
}

fn parse_single_field(v: &serde_json::Value) -> Result<SpecCoreFieldRequest, anyhow::Error> {
    let field_str = v
        .get("field_name")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing 'field_name'"))?;
    let field = SpecCoreField::from_wire(field_str).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown field_name '{}'; valid: {:?}",
            field_str,
            SpecCoreField::all_wire_values()
        )
    })?;

    let key_points: Vec<String> = match v.get("key_points") {
        None | Some(serde_json::Value::Null) => {
            return Err(anyhow::anyhow!(
                "'key_points' is required and must be a non-empty array"
            ));
        }
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .map(|kp| match kp.as_str() {
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

    let related_card_ids: Vec<Ulid> = match v.get("related_card_ids") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .map(|x| match x.as_str() {
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

    let free_text_context = v
        .get("free_text_context")
        .and_then(|x| x.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string());

    let target_length_range: Option<(usize, usize)> = match v.get("target_length_range") {
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

    Ok(SpecCoreFieldRequest {
        field,
        key_points,
        related_card_ids,
        free_text_context,
        target_length_range,
    })
}

/// Build an `UpdateSpecCore` command with ONLY the targeted field set so
/// the reducer leaves every other field as-is.
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
        fail_indices: Vec<usize>,
    }

    #[async_trait::async_trait]
    impl SpecCoreFieldWriter for StubWriter {
        async fn write_fields(
            &self,
            _spec_id: Ulid,
            requests: &[SpecCoreFieldRequest],
            _contexts: &[SpecCoreFieldContext],
        ) -> Result<Vec<Result<SpecCoreFieldOutput, String>>, String> {
            let mut results = Vec::with_capacity(requests.len());
            for (i, req) in requests.iter().enumerate() {
                if self.fail_indices.contains(&i) {
                    results.push(Err(format!("stub-failure-{i}")));
                } else {
                    // Tag the markdown with the field name so per-field tests
                    // can verify routing.
                    let md = format!("[{}]{}", req.field.as_wire(), self.markdown);
                    results.push(Ok(SpecCoreFieldOutput {
                        markdown: md,
                        usage: vec![DecomposerUsage {
                            agent_id: "spec-core-field-writer".into(),
                            model: "claude-haiku-4-5".into(),
                            input_tokens: 50,
                            output_tokens: 100,
                            cache_read_tokens: 0,
                            cache_write_tokens: 0,
                        }],
                    }));
                }
            }
            Ok(results)
        }
    }

    fn stub_writer(markdown: &str) -> Arc<dyn SpecCoreFieldWriter> {
        Arc::new(StubWriter {
            markdown: markdown.to_string(),
            fail_indices: vec![],
        })
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

    fn make_tool_with(
        actor: SpecActorHandle,
        writer: Arc<dyn SpecCoreFieldWriter>,
    ) -> DelegateSpecCoreFieldTool {
        DelegateSpecCoreFieldTool {
            actor: Arc::new(actor),
            writer,
        }
    }

    #[tokio::test]
    async fn schema_has_both_single_and_batch_shapes() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle, stub_writer("md"));
        let schema = tool.schema();
        let props = schema.get("properties").unwrap().as_object().unwrap();
        for f in &["field_name", "key_points", "related_card_ids",
                  "free_text_context", "target_length_range", "fields"] {
            assert!(props.contains_key(*f), "schema missing field '{f}'");
        }
        let fields = props.get("fields").unwrap();
        assert_eq!(fields.get("type").and_then(|v| v.as_str()), Some("array"));
        let item_required = fields
            .pointer("/items/required")
            .and_then(|v| v.as_array())
            .expect("items.required exists");
        let names: Vec<&str> = item_required.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"field_name"));
        assert!(names.contains(&"key_points"));
    }

    #[tokio::test]
    async fn rejects_both_single_and_batch_provided() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle, stub_writer("md"));
        let err = tool
            .execute(json!({
                "field_name": "constraints",
                "key_points": ["a"],
                "fields": [{"field_name": "risks", "key_points": ["b"]}]
            }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("EITHER") || err.to_string().contains("either"));
    }

    #[tokio::test]
    async fn rejects_neither_single_nor_batch_provided() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle, stub_writer("md"));
        let err = tool.execute(json!({})).await.unwrap_err();
        assert!(err.to_string().contains("missing args"));
    }

    #[tokio::test]
    async fn rejects_empty_batch() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle, stub_writer("md"));
        let err = tool.execute(json!({"fields": []})).await.unwrap_err();
        assert!(err.to_string().contains("at least one"));
    }

    #[tokio::test]
    async fn single_shape_writes_one_field_only() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle.clone(), stub_writer("md"));

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
            "key_points": ["A", "B"]
        }))
        .await
        .unwrap();

        let state = handle.read_state().await;
        let core = state.core.as_ref().unwrap();
        assert!(core.constraints.as_deref().unwrap_or("").contains("[constraints]"));
        assert_eq!(
            core.description.as_deref(),
            Some("Existing description should stay.")
        );
        assert!(core.success_criteria.is_none());
        assert!(core.risks.is_none());
        assert!(core.notes.is_none());
    }

    #[tokio::test]
    async fn batch_shape_writes_n_fields_in_one_call() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle.clone(), stub_writer("md"));
        let result = tool
            .execute(json!({
                "fields": [
                    {"field_name": "description",      "key_points": ["x"]},
                    {"field_name": "constraints",      "key_points": ["x"]},
                    {"field_name": "success_criteria", "key_points": ["x"]},
                    {"field_name": "risks",            "key_points": ["x"]},
                    {"field_name": "notes",            "key_points": ["x"]}
                ]
            }))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("5 spec_core fields updated"));

        let state = handle.read_state().await;
        let core = state.core.as_ref().unwrap();
        // All five prose fields populated with the per-field marker.
        assert!(core.description.as_deref().unwrap_or("").contains("[description]"));
        assert!(core.constraints.as_deref().unwrap_or("").contains("[constraints]"));
        assert!(core.success_criteria.as_deref().unwrap_or("").contains("[success_criteria]"));
        assert!(core.risks.as_deref().unwrap_or("").contains("[risks]"));
        assert!(core.notes.as_deref().unwrap_or("").contains("[notes]"));
        // Short fields untouched.
        assert_eq!(core.title, "test");
        assert_eq!(core.one_liner, "t");
        assert_eq!(core.goal, "g");
    }

    #[tokio::test]
    async fn batch_partial_failure_lands_successful_fields() {
        let (_id, handle) = make_test_actor().await;
        let writer = Arc::new(StubWriter {
            markdown: "md".into(),
            fail_indices: vec![1, 3], // constraints and risks fail
        });
        let tool = make_tool_with(handle.clone(), writer);
        let result = tool
            .execute(json!({
                "fields": [
                    {"field_name": "description",      "key_points": ["x"]},
                    {"field_name": "constraints",      "key_points": ["x"]},
                    {"field_name": "success_criteria", "key_points": ["x"]},
                    {"field_name": "risks",            "key_points": ["x"]},
                    {"field_name": "notes",            "key_points": ["x"]}
                ]
            }))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("3/5 spec_core fields updated"));
        assert!(result.content.contains("'constraints'"));
        assert!(result.content.contains("'risks'"));

        let state = handle.read_state().await;
        let core = state.core.as_ref().unwrap();
        // Successful fields landed
        assert!(core.description.as_deref().unwrap_or("").contains("[description]"));
        assert!(core.success_criteria.as_deref().unwrap_or("").contains("[success_criteria]"));
        assert!(core.notes.as_deref().unwrap_or("").contains("[notes]"));
        // Failed fields are still None
        assert!(core.constraints.is_none());
        assert!(core.risks.is_none());
    }

    #[tokio::test]
    async fn batch_rejects_unknown_field_name_per_entry() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle, stub_writer("md"));
        let err = tool
            .execute(json!({
                "fields": [
                    {"field_name": "constraints", "key_points": ["x"]},
                    {"field_name": "title",       "key_points": ["x"]}
                ]
            }))
            .await
            .unwrap_err();
        let s = err.to_string();
        assert!(s.contains("fields[1]"), "should name failing index: {s}");
        assert!(s.contains("unknown field_name"));
    }

    #[tokio::test]
    async fn writer_returning_wrong_length_is_rejected_safely() {
        #[derive(Debug)]
        struct WrongLengthWriter;
        #[async_trait::async_trait]
        impl SpecCoreFieldWriter for WrongLengthWriter {
            async fn write_fields(
                &self,
                _spec_id: Ulid,
                _requests: &[SpecCoreFieldRequest],
                _contexts: &[SpecCoreFieldContext],
            ) -> Result<Vec<Result<SpecCoreFieldOutput, String>>, String> {
                Ok(vec![Ok(SpecCoreFieldOutput {
                    markdown: "one".into(),
                    usage: vec![],
                })])
            }
        }

        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle.clone(), Arc::new(WrongLengthWriter));
        let result = tool
            .execute(json!({
                "fields": [
                    {"field_name": "constraints", "key_points": ["x"]},
                    {"field_name": "risks",       "key_points": ["x"]}
                ]
            }))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("shape contract violated"));
        let state = handle.read_state().await;
        let core = state.core.as_ref().unwrap();
        // No spec_core writes should have landed.
        assert!(core.constraints.is_none());
        assert!(core.risks.is_none());
    }

    #[test]
    fn build_update_command_writes_one_field_only() {
        // Critical correctness property: a build_update_command for one
        // field must leave every other field None.
        for (field, marker) in [
            (SpecCoreField::Description, "DESC"),
            (SpecCoreField::Constraints, "CONS"),
            (SpecCoreField::SuccessCriteria, "SUCC"),
            (SpecCoreField::Risks, "RISK"),
            (SpecCoreField::Notes, "NOTE"),
        ] {
            let cmd = build_update_command(field, marker.into());
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
                    match field {
                        SpecCoreField::Description => {
                            assert_eq!(description.as_deref(), Some(marker));
                            assert!(constraints.is_none());
                            assert!(success_criteria.is_none());
                            assert!(risks.is_none());
                            assert!(notes.is_none());
                        }
                        SpecCoreField::Constraints => {
                            assert!(description.is_none());
                            assert_eq!(constraints.as_deref(), Some(marker));
                            assert!(success_criteria.is_none());
                            assert!(risks.is_none());
                            assert!(notes.is_none());
                        }
                        SpecCoreField::SuccessCriteria => {
                            assert!(description.is_none());
                            assert!(constraints.is_none());
                            assert_eq!(success_criteria.as_deref(), Some(marker));
                            assert!(risks.is_none());
                            assert!(notes.is_none());
                        }
                        SpecCoreField::Risks => {
                            assert!(description.is_none());
                            assert!(constraints.is_none());
                            assert!(success_criteria.is_none());
                            assert_eq!(risks.as_deref(), Some(marker));
                            assert!(notes.is_none());
                        }
                        SpecCoreField::Notes => {
                            assert!(description.is_none());
                            assert!(constraints.is_none());
                            assert!(success_criteria.is_none());
                            assert!(risks.is_none());
                            assert_eq!(notes.as_deref(), Some(marker));
                        }
                    }
                }
                _ => panic!("expected UpdateSpecCore"),
            }
        }
    }
}
