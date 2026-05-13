// ABOUTME: Server-side impl of barnstormer_agent::SpecCoreFieldWriter that
// ABOUTME: runs a single Haiku call to render one spec_core prose field.

use async_trait::async_trait;
use barnstormer_agent::{
    DecomposerUsage, SpecCoreField, SpecCoreFieldContext, SpecCoreFieldOutput,
    SpecCoreFieldRequest, SpecCoreFieldWriter,
};
use std::sync::Arc;
use ulid::Ulid;

const DEFAULT_WRITER_MODEL: &str = "claude-haiku-4-5";
const WRITER_MAX_TOKENS: u32 = 1400;
const WRITER_AGENT_ID: &str = "spec-core-field-writer";

#[derive(Debug)]
pub struct ServerSpecCoreFieldWriter;

#[async_trait]
impl SpecCoreFieldWriter for ServerSpecCoreFieldWriter {
    async fn write_fields(
        &self,
        _spec_id: Ulid,
        requests: &[SpecCoreFieldRequest],
        contexts: &[SpecCoreFieldContext],
    ) -> Result<Vec<Result<SpecCoreFieldOutput, String>>, String> {
        if requests.len() != contexts.len() {
            return Err(format!(
                "write_fields length mismatch: {} requests vs {} contexts",
                requests.len(),
                contexts.len()
            ));
        }

        let model = std::env::var("BARNSTORMER_SPEC_CORE_FIELD_MODEL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_WRITER_MODEL.to_string());
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| "ANTHROPIC_API_KEY not set".to_string())?;
        let base_url = std::env::var("ANTHROPIC_BASE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "https://api.anthropic.com".to_string());
        let endpoint = format!("{}/v1/messages", base_url.trim_end_matches('/'));
        let client = Arc::new(
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .map_err(|e| format!("failed to build HTTP client: {e}"))?,
        );

        let model = Arc::new(model);
        let api_key = Arc::new(api_key);
        let endpoint = Arc::new(endpoint);

        // Fan out: one tokio::spawn task per field, all running in parallel.
        let mut handles = Vec::with_capacity(requests.len());
        for (req, ctx) in requests.iter().zip(contexts.iter()) {
            let req = req.clone();
            let ctx = ctx.clone();
            let client = Arc::clone(&client);
            let model = Arc::clone(&model);
            let api_key = Arc::clone(&api_key);
            let endpoint = Arc::clone(&endpoint);
            handles.push(tokio::spawn(async move {
                write_one_field(&req, &ctx, &client, &model, &api_key, &endpoint).await
            }));
        }
        let mut results = Vec::with_capacity(handles.len());
        for h in handles {
            match h.await {
                Ok(r) => results.push(r),
                Err(e) => results.push(Err(format!("tokio join error: {e}"))),
            }
        }
        Ok(results)
    }
}

async fn write_one_field(
    request: &SpecCoreFieldRequest,
    ctx: &SpecCoreFieldContext,
    client: &reqwest::Client,
    model: &str,
    api_key: &str,
    endpoint: &str,
) -> Result<SpecCoreFieldOutput, String> {
    let req_body = build_request(model, request, &ctx.related_card_summaries);
    let resp = client
        .post(endpoint)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&req_body)
        .send()
        .await
        .map_err(|e| format!("anthropic HTTP request failed: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("could not read anthropic response body: {e}"))?;
    if !status.is_success() {
        return Err(format!("anthropic returned {status}: {text}"));
    }
    let resp_value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("anthropic response was not JSON: {e}; body: {text}"))?;
    let usage = parse_usage(&resp_value);
    let markdown = extract_text(&resp_value)?;
    Ok(SpecCoreFieldOutput {
        markdown,
        usage: vec![DecomposerUsage {
            agent_id: WRITER_AGENT_ID.to_string(),
            model: model.to_string(),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
        }],
    })
}

fn build_request(
    model: &str,
    request: &SpecCoreFieldRequest,
    related_card_summaries: &[(String, String)],
) -> serde_json::Value {
    let target = request
        .target_length_range
        .unwrap_or_else(|| default_range(request.field));

    let user_msg = build_user_message(request, target, related_card_summaries);

    let system_blocks = serde_json::json!([
        {
            "type": "text",
            "text": SYSTEM_PROMPT,
            "cache_control": {"type": "ephemeral"}
        }
    ]);

    serde_json::json!({
        "model": model,
        "max_tokens": WRITER_MAX_TOKENS,
        "system": system_blocks,
        "messages": [
            {"role": "user", "content": user_msg}
        ]
    })
}

fn build_user_message(
    request: &SpecCoreFieldRequest,
    target: (usize, usize),
    related_card_summaries: &[(String, String)],
) -> String {
    let mut s = String::new();
    s.push_str("spec_core field to author:\n");
    s.push_str(&format!("- field_name: {}\n", request.field.as_wire()));
    s.push_str(&format!(
        "- target length: {}-{} chars\n",
        target.0, target.1
    ));
    s.push_str("\nKey points to include (in order):\n");
    for (i, p) in request.key_points.iter().enumerate() {
        s.push_str(&format!("{}. {}\n", i + 1, p));
    }
    if let Some(ctx) = &request.free_text_context {
        s.push_str(&format!("\nContext: {ctx}\n"));
    }
    if !related_card_summaries.is_empty() {
        s.push_str("\nCards on the board (use these as grounding when relevant; don't restate their full content):\n");
        for (title, excerpt) in related_card_summaries {
            if excerpt.is_empty() {
                s.push_str(&format!("- {title}\n"));
            } else {
                s.push_str(&format!("- {title} — {excerpt}\n"));
            }
        }
    }
    s.push_str("\nWrite the field's markdown now. Output ONLY the markdown — no preamble, no field label, no surrounding fences.");
    s
}

fn default_range(field: SpecCoreField) -> (usize, usize) {
    match field {
        SpecCoreField::Description => (400, 1500),
        SpecCoreField::Constraints => (400, 1500),
        SpecCoreField::SuccessCriteria => (300, 1000),
        SpecCoreField::Risks => (600, 2000),
        SpecCoreField::Notes => (200, 1000),
    }
}

const SYSTEM_PROMPT: &str = r#"You author one prose field on the spec_core for a spec-builder. The calling agent supplies: field_name, ordered key_points, and optional grounding context. You produce ONE markdown blob in the right voice for the field.

Voice library:

description — 1-3 paragraphs of declarative product summary. Open with a sentence framing what the product is. Subsequent paragraphs add the "what" and "for whom" / "how it works at a glance". No bullets unless the key_points are clearly listable. No marketing fluff. No CTAs. Target 400-1500 chars typical.

constraints — bullet markdown. Each bullet states a constraint as a declarative requirement, optionally followed by " — rationale" or a short follow-up clause carrying the why when the why isn't obvious. Plain bullets, no nested structure. Target 400-1500 chars typical.

success_criteria — bullet markdown. Each bullet is a measurable outcome, ideally with a number or threshold. Resist marketing language. Use action-verb framing. Target 300-1000 chars typical.

risks — markdown subsections, one per risk. Each subsection is a bolded title line naming the risk, followed by:
  - **Likelihood:** one phrase (Low / Medium / High plus reasoning)
  - **Impact:** one phrase (severity + scope)
  - **Mitigation:** 1-3 concrete mitigations (often bulleted, sometimes inline)
  - **Trigger Conditions:** (optional) what signal would say this is materializing
The opening of each risk subsection is just the bolded title line — no preamble. Target 600-2000 chars typical for the whole field.

notes — bullet markdown. Open questions, deferred decisions, context the team needs to remember. Question-shaped framing or note-shaped framing both fine. Conversational tone allowed; this is the "internal voice" of the spec. Target 200-1000 chars typical.

Format conventions (apply to all fields):
- Don't echo the field_name back as a heading — the markdown goes into a database column whose label is already the field name
- Don't include closing remarks ("Let me know...", "Hope this helps...")
- Don't surround the output with code fences
- Declarative voice. No "I think". No conditional softening unless the key_point explicitly invites it
- Stay within the target length range
- If grounding cards are supplied, reference them by their concept rather than restating their full content"#;

#[derive(Debug, Default)]
struct ResponseUsage {
    input_tokens: u32,
    output_tokens: u32,
    cache_read_tokens: u32,
    cache_write_tokens: u32,
}

fn parse_usage(resp: &serde_json::Value) -> ResponseUsage {
    let u = match resp.get("usage") {
        Some(v) => v,
        None => return ResponseUsage::default(),
    };
    ResponseUsage {
        input_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        output_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        cache_read_tokens: u
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        cache_write_tokens: u
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
    }
}

fn extract_text(resp: &serde_json::Value) -> Result<String, String> {
    let content = resp
        .get("content")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "anthropic response missing content array".to_string())?;
    let mut buf = String::new();
    for block in content {
        if block.get("type").and_then(|v| v.as_str()) == Some("text")
            && let Some(t) = block.get("text").and_then(|v| v.as_str())
        {
            buf.push_str(t);
        }
    }
    if buf.trim().is_empty() {
        return Err("anthropic response had no text content".to_string());
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req_for(field: SpecCoreField) -> SpecCoreFieldRequest {
        SpecCoreFieldRequest {
            field,
            key_points: vec!["A".into(), "B".into()],
            related_card_ids: vec![],
            free_text_context: None,
            target_length_range: None,
        }
    }

    #[test]
    fn system_prompt_covers_all_five_fields() {
        for label in &["description", "constraints", "success_criteria", "risks", "notes"] {
            assert!(
                SYSTEM_PROMPT.contains(&format!("{label} —")),
                "system prompt missing voice rules for '{label}'; full prompt:\n{}",
                SYSTEM_PROMPT
            );
        }
    }

    #[test]
    fn system_prompt_carries_format_conventions() {
        for required in &[
            "Don't echo the field_name",
            "Don't surround the output with code fences",
            "Declarative voice",
        ] {
            assert!(
                SYSTEM_PROMPT.contains(required),
                "system prompt should contain '{required}'; full prompt:\n{}",
                SYSTEM_PROMPT
            );
        }
    }

    #[test]
    fn default_ranges_calibrated_per_field() {
        assert_eq!(default_range(SpecCoreField::Description), (400, 1500));
        assert_eq!(default_range(SpecCoreField::Constraints), (400, 1500));
        assert_eq!(default_range(SpecCoreField::SuccessCriteria), (300, 1000));
        assert_eq!(default_range(SpecCoreField::Risks), (600, 2000));
        assert_eq!(default_range(SpecCoreField::Notes), (200, 1000));
    }

    #[test]
    fn build_user_message_includes_field_and_points() {
        let r = req_for(SpecCoreField::Constraints);
        let msg = build_user_message(&r, (400, 1500), &[]);
        assert!(msg.contains("- field_name: constraints"));
        assert!(msg.contains("- target length: 400-1500 chars"));
        assert!(msg.contains("1. A"));
        assert!(msg.contains("2. B"));
        assert!(!msg.contains("Context:"));
        assert!(!msg.contains("Cards on the board"));
    }

    #[test]
    fn build_user_message_includes_optional_grounding() {
        let mut r = req_for(SpecCoreField::Risks);
        r.free_text_context = Some("after adversarial review".into());
        let related = vec![(
            "Connector maintenance".to_string(),
            "Each harness UI change requires a patch.".to_string(),
        )];
        let msg = build_user_message(&r, (600, 2000), &related);
        assert!(msg.contains("Context: after adversarial review"));
        assert!(msg.contains("Cards on the board"));
        assert!(msg.contains(
            "Connector maintenance — Each harness UI change requires a patch."
        ));
    }

    #[test]
    fn build_request_marks_system_block_cacheable() {
        let r = req_for(SpecCoreField::Risks);
        let req = build_request("claude-haiku-4-5", &r, &[]);
        let system = req.get("system").unwrap().as_array().unwrap();
        assert_eq!(system.len(), 1);
        assert!(system[0].get("cache_control").is_some());
    }

    #[test]
    fn parse_usage_handles_cache_fields() {
        let resp = serde_json::json!({
            "usage": {
                "input_tokens": 200,
                "output_tokens": 400,
                "cache_read_input_tokens": 3000,
                "cache_creation_input_tokens": 0
            }
        });
        let u = parse_usage(&resp);
        assert_eq!(u.input_tokens, 200);
        assert_eq!(u.output_tokens, 400);
        assert_eq!(u.cache_read_tokens, 3000);
    }

    #[test]
    fn extract_text_concatenates_text_blocks() {
        let resp = serde_json::json!({
            "content": [
                {"type": "text", "text": "first "},
                {"type": "text", "text": "second"}
            ]
        });
        assert_eq!(extract_text(&resp).unwrap(), "first second");
    }
}
