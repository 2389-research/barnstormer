// ABOUTME: delegate_card_body mux tool — writes card bodies via a Haiku writer
// ABOUTME: when the Sonnet SubAgent has already decided what card(s) to create.
// ABOUTME: Accepts either a single-card shape or a {cards:[...]} batch shape so
// ABOUTME: agents can fan out multiple cards from a single user response without
// ABOUTME: cycling through the Sonnet tool-use loop N times.

use std::sync::Arc;

use async_trait::async_trait;
use mux::tool::{Tool, ToolResult};
use serde_json::json;
use ulid::Ulid;

use barnstormer_core::actor::SpecActorHandle;
use barnstormer_core::command::Command;

use crate::card_body_writer::{CardBodyContext, CardBodyRequest, CardBodyWriter, CardKind};

/// Tool that writes card bodies via a Haiku writer and applies the
/// resulting `CreateCard` commands to the actor.
///
/// Two call shapes:
///
/// 1. **Single-card**: `{card_type, title, scope, key_points, …}` — author
///    ONE card. Use when responding to a single user request that produces
///    one card, or when iteratively deciding cards one at a time based on
///    state observations.
///
/// 2. **Batch**: `{cards: [{card_type, title, scope, key_points, …}, …]}`
///    — author N cards in a single tool call. Use when ONE user message
///    triggers multiple cards you've already decided on (adversarial
///    review, "add the missing risk + constraint + task for X", etc.).
///    The writer runs each card's LLM call in parallel; the tool applies
///    the CreateCard commands sequentially in input order.
///
/// The two shapes are mutually exclusive — providing both `cards` and a
/// top-level `card_type` is an error.
#[derive(Clone)]
pub struct DelegateCardBodyTool {
    pub(crate) actor: Arc<SpecActorHandle>,
    pub(crate) agent_id: String,
    pub(crate) writer: Arc<dyn CardBodyWriter>,
}

#[async_trait]
impl Tool for DelegateCardBodyTool {
    fn name(&self) -> &str {
        "delegate_card_body"
    }

    fn description(&self) -> &str {
        "Author card bodies via a faster writer model. Two call shapes:\n\n\
         SINGLE: {card_type, title, scope, key_points?, lane?, ...} — author one card. \
         Use when one user request produces one card, or when iteratively deciding cards \
         one at a time based on state observations.\n\n\
         BATCH: {cards: [{card_type, title, scope, ...}, ...]} — author N cards in a \
         single tool call. STRONGLY PREFER this when one user message triggers multiple \
         cards you've already decided on (adversarial review, 'add the missing risk + \
         constraint + task for X', user said 'add 4 cards covering Y'). The writer runs \
         all card LLM calls in parallel, and YOU avoid N sequential tool-use loop iterations. \
         Each batched card is independent — they don't see each other's results. Don't batch \
         when card B depends on what card A's body actually says.\n\n\
         Either shape applies the CreateCard command(s) itself; no follow-up write_commands \
         call is needed."
    }

    fn schema(&self) -> serde_json::Value {
        // Note: we don't use JSON-Schema `oneOf` because Anthropic's tool
        // schemas + many model versions handle it inconsistently. Instead
        // we mark all top-level fields optional and validate the shape
        // (one or the other, not both) inside execute().
        let single_card_props = single_card_properties();
        json!({
            "type": "object",
            "properties": {
                "card_type": single_card_props["card_type"],
                "lane":      single_card_props["lane"],
                "title":     single_card_props["title"],
                "scope":     single_card_props["scope"],
                "key_points": single_card_props["key_points"],
                "source_attachment_id": single_card_props["source_attachment_id"],
                "related_card_ids":     single_card_props["related_card_ids"],
                "free_text_context":    single_card_props["free_text_context"],
                "target_length_range":  single_card_props["target_length_range"],
                "cards": {
                    "type": "array",
                    "description": "BATCH MODE. Use when ONE user message triggers multiple cards you've already decided on. Each element has the same fields as the single-card form (card_type + title + scope required; others optional). Mutually exclusive with the top-level card_type field.",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": single_card_props,
                        "required": ["card_type", "title", "scope"]
                    }
                }
            }
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult, anyhow::Error> {
        let has_top_card_type = params.get("card_type").is_some();
        let has_cards = params.get("cards").is_some();

        if has_top_card_type && has_cards {
            return Err(anyhow::anyhow!(
                "provide EITHER single-card args (card_type/title/scope/...) OR cards:[...] batch, not both"
            ));
        }
        if !has_top_card_type && !has_cards {
            return Err(anyhow::anyhow!(
                "missing args: provide either card_type+title+scope (single-card) or cards:[...] (batch)"
            ));
        }

        // Normalize both shapes into a Vec<CardBodyRequest>.
        let requests: Vec<CardBodyRequest> = if has_cards {
            let arr = params
                .get("cards")
                .and_then(|v| v.as_array())
                .ok_or_else(|| anyhow::anyhow!("'cards' must be an array"))?;
            if arr.is_empty() {
                return Err(anyhow::anyhow!("'cards' must contain at least one card"));
            }
            let mut parsed = Vec::with_capacity(arr.len());
            for (i, entry) in arr.iter().enumerate() {
                let req = parse_single_card(entry)
                    .map_err(|e| anyhow::anyhow!("cards[{i}]: {e}"))?;
                parsed.push(req);
            }
            parsed
        } else {
            vec![parse_single_card(&params)?]
        };

        // Resolve per-request context from spec state in one read.
        let contexts = self.resolve_contexts(&requests).await?;

        let spec_id = self.actor.spec_id;
        let results = match self.writer.write_bodies(spec_id, &requests, &contexts).await {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolResult::error(format!("card-body write failed: {e}")));
            }
        };

        if results.len() != requests.len() {
            return Ok(ToolResult::error(format!(
                "writer returned {} results for {} requests; refusing to apply (shape contract violated)",
                results.len(),
                requests.len()
            )));
        }

        // Apply CreateCard commands sequentially in input order, and emit
        // AgentStepUsage events for each successful write. Per-card
        // failures are surfaced in the summary but don't abort the batch.
        let mut created = 0usize;
        let mut card_errors: Vec<(usize, String, String)> = Vec::new();
        for (i, (req, result)) in requests.iter().zip(results.iter()).enumerate() {
            match result {
                Err(e) => {
                    card_errors.push((i, req.title.clone(), e.clone()));
                    continue;
                }
                Ok(output) => {
                    let lane_final = req
                        .lane
                        .clone()
                        .or_else(|| default_lane_for(req.kind));
                    let create_cmd = Command::CreateCard {
                        card_type: req.kind.as_wire().to_string(),
                        title: req.title.clone(),
                        body: Some(output.body.clone()),
                        lane: lane_final,
                        created_by: self.agent_id.clone(),
                        source_attachment_id: req.source_attachment_id,
                    };
                    if let Err(e) = self.actor.send_command(create_cmd).await {
                        card_errors.push((
                            i,
                            req.title.clone(),
                            format!("CreateCard failed: {e}"),
                        ));
                        continue;
                    }
                    created += 1;
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
                                "failed to record card-body-writer usage event"
                            );
                        }
                    }
                }
            }
        }

        let summary = if card_errors.is_empty() {
            if requests.len() == 1 {
                format!(
                    "Card '{}' ({}) created.",
                    requests[0].title,
                    requests[0].kind.as_wire()
                )
            } else {
                format!(
                    "{} cards created ({}/{} succeeded).",
                    created,
                    created,
                    requests.len()
                )
            }
        } else {
            let mut s = format!(
                "{}/{} cards created. Failures:\n",
                created,
                requests.len()
            );
            for (i, title, err) in &card_errors {
                s.push_str(&format!("  - cards[{i}] '{title}': {err}\n"));
            }
            s
        };
        Ok(ToolResult::text(summary))
    }
}

impl DelegateCardBodyTool {
    /// Resolve per-request grounding context from spec state. Reads state
    /// ONCE for the entire batch — much cheaper than locking the actor
    /// per-card.
    async fn resolve_contexts(
        &self,
        requests: &[CardBodyRequest],
    ) -> Result<Vec<CardBodyContext>, anyhow::Error> {
        let state = self.actor.read_state().await;
        let mut contexts = Vec::with_capacity(requests.len());
        for req in requests {
            let mut ctx = CardBodyContext::default();
            if let Some(att_id) = req.source_attachment_id {
                let att = state
                    .context_attachments
                    .iter()
                    .find(|a| a.attachment_id == att_id && !a.removed);
                match att {
                    None => {
                        return Err(anyhow::anyhow!(
                            "source_attachment_id {att_id} not found (card '{}')",
                            req.title
                        ));
                    }
                    Some(a) => {
                        ctx.attachment_summary = a.summary.clone();
                    }
                }
            }
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
            contexts.push(ctx);
        }
        Ok(contexts)
    }
}

/// Single-card property declarations — shared between the top-level
/// schema (single-card path) and the items: schema in the batch path.
fn single_card_properties() -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    m.insert(
        "card_type".into(),
        json!({
            "type": "string",
            "enum": CardKind::all_wire_values(),
            "description": "What kind of card. Drives voice on the writer side: idea (exploratory) | task (concrete actionable) | constraint (normative) | risk (Likelihood/Impact/Mitigation) | note (question-shaped)."
        }),
    );
    m.insert(
        "lane".into(),
        json!({
            "type": "string",
            "enum": ["Ideas", "Plan", "Spec"],
            "description": "Which lane. Optional; defaults per card_type."
        }),
    );
    m.insert(
        "title".into(),
        json!({"type": "string", "description": "Concise card title, 3-8 words typical."}),
    );
    m.insert(
        "scope".into(),
        json!({
            "type": "string",
            "description": "One sentence summarizing what this card covers. Grounds the writer."
        }),
    );
    m.insert(
        "key_points".into(),
        json!({
            "type": "array",
            "items": {"type": "string"},
            "description": "Ordered bullets/claims the body should include. May be empty — writer elaborates from scope."
        }),
    );
    m.insert(
        "source_attachment_id".into(),
        json!({
            "type": "string",
            "description": "Optional ULID of an attached source brief; writer pulls supporting content from it."
        }),
    );
    m.insert(
        "related_card_ids".into(),
        json!({
            "type": "array",
            "items": {"type": "string"},
            "description": "Optional ULIDs of existing cards to ground against (avoid duplication)."
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
            "description": "Optional [min_chars, max_chars]; defaults per card_type."
        }),
    );
    m
}

fn parse_single_card(v: &serde_json::Value) -> Result<CardBodyRequest, anyhow::Error> {
    let card_type_str = v
        .get("card_type")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing 'card_type'"))?;
    let kind = CardKind::from_wire(card_type_str).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown card_type '{}'; valid: {:?}",
            card_type_str,
            CardKind::all_wire_values()
        )
    })?;

    let title = v
        .get("title")
        .and_then(|x| x.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing or empty 'title'"))?
        .to_string();

    let scope = v
        .get("scope")
        .and_then(|x| x.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing or empty 'scope'"))?
        .to_string();

    let lane = v
        .get("lane")
        .and_then(|x| x.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string());

    let key_points: Vec<String> = match v.get("key_points") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .map(|kp| match kp.as_str() {
                Some(s) => Ok(s.to_string()),
                None => Err(anyhow::anyhow!("each element of 'key_points' must be a string")),
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(anyhow::anyhow!("'key_points' must be an array of strings")),
    };

    let source_attachment_id = match v.get("source_attachment_id") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) if s.trim().is_empty() => None,
        Some(serde_json::Value::String(s)) => Some(
            s.parse::<Ulid>()
                .map_err(|e| anyhow::anyhow!("bad source_attachment_id: {e}"))?,
        ),
        Some(_) => return Err(anyhow::anyhow!("'source_attachment_id' must be a string ULID")),
    };

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

    Ok(CardBodyRequest {
        kind,
        lane,
        title,
        scope,
        key_points,
        source_attachment_id,
        related_card_ids,
        free_text_context,
        target_length_range,
    })
}

/// Default lane suggestion per card_type when the agent doesn't supply
/// `lane`. Intentionally simple — agents should override when they have
/// a stronger opinion.
fn default_lane_for(kind: CardKind) -> Option<String> {
    Some(
        match kind {
            CardKind::Idea => "Ideas",
            CardKind::Task => "Plan",
            CardKind::Constraint => "Spec",
            CardKind::Risk => "Ideas",
            CardKind::Note => "Ideas",
        }
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_body_writer::CardBodyOutput;
    use crate::card_decomposer::DecomposerUsage;
    use barnstormer_core::actor;
    use barnstormer_core::state::SpecState;

    /// A test writer that returns the same body for every request, with
    /// optional per-index failures.
    #[derive(Debug)]
    struct StubWriter {
        body: String,
        fail_indices: Vec<usize>,
    }

    #[async_trait::async_trait]
    impl CardBodyWriter for StubWriter {
        async fn write_bodies(
            &self,
            _spec_id: Ulid,
            requests: &[CardBodyRequest],
            _contexts: &[CardBodyContext],
        ) -> Result<Vec<Result<CardBodyOutput, String>>, String> {
            let mut results = Vec::with_capacity(requests.len());
            for (i, _) in requests.iter().enumerate() {
                if self.fail_indices.contains(&i) {
                    results.push(Err(format!("stub-failure-for-index-{i}")));
                } else {
                    results.push(Ok(CardBodyOutput {
                        body: self.body.clone(),
                        usage: vec![DecomposerUsage {
                            agent_id: "card-body-writer".into(),
                            model: "claude-haiku-4-5".into(),
                            input_tokens: 100,
                            output_tokens: 50,
                            cache_read_tokens: 0,
                            cache_write_tokens: 0,
                        }],
                    }));
                }
            }
            Ok(results)
        }
    }

    fn stub_writer(body: &str) -> Arc<dyn CardBodyWriter> {
        Arc::new(StubWriter {
            body: body.to_string(),
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
        writer: Arc<dyn CardBodyWriter>,
    ) -> DelegateCardBodyTool {
        DelegateCardBodyTool {
            actor: Arc::new(actor),
            agent_id: "test-agent".into(),
            writer,
        }
    }

    #[tokio::test]
    async fn schema_has_both_single_and_batch_shapes() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle, stub_writer("body"));
        let schema = tool.schema();
        let props = schema.get("properties").unwrap().as_object().unwrap();
        // Single-shape fields all present at top level
        for f in &["card_type", "title", "scope", "key_points", "lane",
                  "source_attachment_id", "related_card_ids", "free_text_context",
                  "target_length_range"] {
            assert!(props.contains_key(*f), "schema missing top-level field '{f}'");
        }
        // Batch field present with array shape + inner items having required fields
        let cards = props.get("cards").unwrap();
        assert_eq!(cards.get("type").and_then(|v| v.as_str()), Some("array"));
        let item_required = cards
            .pointer("/items/required")
            .and_then(|v| v.as_array())
            .expect("items.required exists");
        let req_names: Vec<&str> = item_required.iter().filter_map(|v| v.as_str()).collect();
        for f in &["card_type", "title", "scope"] {
            assert!(req_names.contains(f), "batch items.required missing '{f}'");
        }
    }

    #[tokio::test]
    async fn rejects_both_single_and_batch_provided() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle, stub_writer("body"));
        let err = tool
            .execute(json!({
                "card_type": "idea",
                "title": "T",
                "scope": "s",
                "cards": [{"card_type": "task", "title": "U", "scope": "x"}]
            }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("EITHER") || err.to_string().contains("either"));
    }

    #[tokio::test]
    async fn rejects_neither_single_nor_batch_provided() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle, stub_writer("body"));
        let err = tool.execute(json!({})).await.unwrap_err();
        assert!(err.to_string().contains("missing args"));
    }

    #[tokio::test]
    async fn rejects_empty_batch() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle, stub_writer("body"));
        let err = tool
            .execute(json!({"cards": []}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("at least one"));
    }

    #[tokio::test]
    async fn single_shape_creates_one_card() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle.clone(), stub_writer("single body"));
        let result = tool
            .execute(json!({
                "card_type": "risk",
                "title": "Validation cost",
                "scope": "may be expensive at scale",
                "key_points": ["per-skill $0.05-0.50"]
            }))
            .await
            .unwrap();
        assert!(!result.is_error);
        let state = handle.read_state().await;
        assert_eq!(state.cards.len(), 1);
        let c = state.cards.values().next().unwrap();
        assert_eq!(c.title, "Validation cost");
        assert_eq!(c.card_type, "risk");
        assert_eq!(c.lane, "Ideas");
        assert_eq!(c.body.as_deref(), Some("single body"));
    }

    #[tokio::test]
    async fn batch_shape_creates_n_cards_in_input_order() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle.clone(), stub_writer("body"));
        let result = tool
            .execute(json!({
                "cards": [
                    {"card_type": "risk", "title": "Risk A", "scope": "..."},
                    {"card_type": "task", "title": "Task B", "scope": "..."},
                    {"card_type": "constraint", "title": "Constraint C", "scope": "..."},
                    {"card_type": "note", "title": "Note D", "scope": "..."},
                    {"card_type": "idea", "title": "Idea E", "scope": "..."}
                ]
            }))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("5 cards created"));

        let state = handle.read_state().await;
        assert_eq!(state.cards.len(), 5);

        // CardCreated order isn't strictly the same as input order in
        // state.cards (HashMap), but check titles + types are all there.
        let titles: std::collections::HashSet<String> =
            state.cards.values().map(|c| c.title.clone()).collect();
        for t in &["Risk A", "Task B", "Constraint C", "Note D", "Idea E"] {
            assert!(titles.contains(*t), "card '{t}' not in state");
        }
    }

    #[tokio::test]
    async fn batch_default_lanes_per_card_type() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle.clone(), stub_writer("body"));
        tool.execute(json!({
            "cards": [
                {"card_type": "idea", "title": "I", "scope": "s"},
                {"card_type": "task", "title": "T", "scope": "s"},
                {"card_type": "constraint", "title": "C", "scope": "s"},
                {"card_type": "risk", "title": "R", "scope": "s"},
                {"card_type": "note", "title": "N", "scope": "s"}
            ]
        }))
        .await
        .unwrap();
        let state = handle.read_state().await;
        let by_title: std::collections::HashMap<_, _> = state
            .cards
            .values()
            .map(|c| (c.title.clone(), c.lane.clone()))
            .collect();
        assert_eq!(by_title.get("I").map(String::as_str), Some("Ideas"));
        assert_eq!(by_title.get("T").map(String::as_str), Some("Plan"));
        assert_eq!(by_title.get("C").map(String::as_str), Some("Spec"));
        assert_eq!(by_title.get("R").map(String::as_str), Some("Ideas"));
        assert_eq!(by_title.get("N").map(String::as_str), Some("Ideas"));
    }

    #[tokio::test]
    async fn batch_partial_failure_lands_successful_cards_and_summarizes_failures() {
        let (_id, handle) = make_test_actor().await;
        let writer = Arc::new(StubWriter {
            body: "body".into(),
            fail_indices: vec![1, 3], // cards at index 1 and 3 fail
        });
        let tool = make_tool_with(handle.clone(), writer);
        let result = tool
            .execute(json!({
                "cards": [
                    {"card_type": "risk", "title": "Card 0", "scope": "..."},
                    {"card_type": "task", "title": "Card 1 (fails)", "scope": "..."},
                    {"card_type": "idea", "title": "Card 2", "scope": "..."},
                    {"card_type": "note", "title": "Card 3 (fails)", "scope": "..."},
                    {"card_type": "constraint", "title": "Card 4", "scope": "..."}
                ]
            }))
            .await
            .unwrap();
        assert!(!result.is_error, "result shouldn't be a tool-level error");
        assert!(result.content.contains("3/5 cards created"));
        assert!(result.content.contains("Card 1 (fails)"));
        assert!(result.content.contains("Card 3 (fails)"));

        let state = handle.read_state().await;
        assert_eq!(state.cards.len(), 3);
        let titles: std::collections::HashSet<String> =
            state.cards.values().map(|c| c.title.clone()).collect();
        assert!(titles.contains("Card 0"));
        assert!(titles.contains("Card 2"));
        assert!(titles.contains("Card 4"));
        assert!(!titles.contains("Card 1 (fails)"));
        assert!(!titles.contains("Card 3 (fails)"));
    }

    #[tokio::test]
    async fn batch_rejects_unknown_card_type_per_entry() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle, stub_writer("body"));
        let err = tool
            .execute(json!({
                "cards": [
                    {"card_type": "task", "title": "ok", "scope": "..."},
                    {"card_type": "epic", "title": "bad", "scope": "..."}
                ]
            }))
            .await
            .unwrap_err();
        let s = err.to_string();
        assert!(s.contains("cards[1]"), "should name the failing index: {s}");
        assert!(s.contains("epic"), "should mention the bad card_type: {s}");
    }

    #[tokio::test]
    async fn batch_rejects_missing_required_in_entry() {
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle, stub_writer("body"));
        let err = tool
            .execute(json!({
                "cards": [
                    {"card_type": "task", "title": "ok", "scope": "..."},
                    {"card_type": "task", "title": "missing scope"}
                ]
            }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("cards[1]"));
        assert!(err.to_string().contains("scope"));
    }

    #[tokio::test]
    async fn batch_records_per_card_usage_events() {
        // Pin that AgentStepUsage gets emitted for each successful card
        // in a batch — keeps cost attribution clean post-batch-rollout.
        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle.clone(), stub_writer("body"));
        // Subscribe BEFORE executing to capture events in order.
        let mut rx = handle.subscribe();
        tool.execute(json!({
            "cards": [
                {"card_type": "task", "title": "A", "scope": "s"},
                {"card_type": "task", "title": "B", "scope": "s"},
                {"card_type": "task", "title": "C", "scope": "s"}
            ]
        }))
        .await
        .unwrap();
        // Drain the broadcast channel
        let mut usage_count = 0;
        let mut card_count = 0;
        loop {
            match rx.try_recv() {
                Ok(ev) => {
                    let kind = ev
                        .payload
                        .clone();
                    use barnstormer_core::event::EventPayload as P;
                    match kind {
                        P::AgentStepUsage { .. } => usage_count += 1,
                        P::CardCreated { .. } => card_count += 1,
                        _ => {}
                    }
                }
                Err(_) => break,
            }
        }
        assert_eq!(card_count, 3);
        assert_eq!(usage_count, 3, "expected one usage event per card in the batch");
    }

    #[tokio::test]
    async fn writer_returning_wrong_length_is_rejected_safely() {
        // Defensive: if a writer impl returns a mismatched-length Vec,
        // refuse to apply commands rather than guess at the alignment.
        #[derive(Debug)]
        struct WrongLengthWriter;
        #[async_trait::async_trait]
        impl CardBodyWriter for WrongLengthWriter {
            async fn write_bodies(
                &self,
                _spec_id: Ulid,
                requests: &[CardBodyRequest],
                _contexts: &[CardBodyContext],
            ) -> Result<Vec<Result<CardBodyOutput, String>>, String> {
                // Return only ONE result no matter how many requested.
                Ok(vec![Ok(CardBodyOutput {
                    body: "one".into(),
                    usage: vec![],
                })])
            }
        }

        let (_id, handle) = make_test_actor().await;
        let tool = make_tool_with(handle.clone(), Arc::new(WrongLengthWriter));
        let result = tool
            .execute(json!({
                "cards": [
                    {"card_type": "task", "title": "A", "scope": "s"},
                    {"card_type": "task", "title": "B", "scope": "s"}
                ]
            }))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("shape contract violated"));
        let state = handle.read_state().await;
        assert_eq!(state.cards.len(), 0, "should not apply any cards on contract violation");
    }
}
