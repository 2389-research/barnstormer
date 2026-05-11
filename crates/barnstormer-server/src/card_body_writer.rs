// ABOUTME: Server-side impl of barnstormer_agent::CardBodyWriter that runs a
// ABOUTME: single Haiku executor call (no architect step — the SubAgent is the
// ABOUTME: architect). Voice library is per-card_type and lives in the system
// ABOUTME: prompt.

use async_trait::async_trait;
use barnstormer_agent::{
    CardBodyOutput, CardBodyRequest, CardBodyWriter, CardKind, DecomposerUsage,
};
use std::path::PathBuf;
use ulid::Ulid;

const DEFAULT_WRITER_MODEL: &str = "claude-haiku-4-5";
const WRITER_MAX_TOKENS: u32 = 1000;

/// Synthetic agent_id recorded against the per-call AgentStepUsage event.
/// Keeps cost-attribution clean by distinguishing card-body writes from
/// the decomposer's architect/executor calls and from narration tokens.
const WRITER_AGENT_ID: &str = "card-body-writer";

/// Server-side impl. Bypasses mux's LlmClient for the same reason
/// `ServerCardDecomposer` does: cache_control isn't exposed on mux's
/// Request type, and caching the system prompt across multiple
/// `delegate_card_body` calls in a session is a meaningful saving when
/// agents fire the tool repeatedly (e.g. Planner adding 4 cards from
/// an adversarial review pass).
#[derive(Debug)]
pub struct ServerCardBodyWriter {
    pub home: PathBuf,
}

#[async_trait]
impl CardBodyWriter for ServerCardBodyWriter {
    async fn write_body(
        &self,
        spec_id: Ulid,
        request: &CardBodyRequest,
        attachment_summary: Option<&str>,
        related_card_summaries: &[(String, String)],
    ) -> Result<CardBodyOutput, String> {
        let model = std::env::var("BARNSTORMER_CARD_BODY_MODEL")
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

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;

        // Resolve source brief text (if any) — re-uses the same fallback
        // pattern as the decomposer: UTF-8 text → summary → clean error.
        let brief_block = match request.source_attachment_id {
            None => None,
            Some(att_id) => Some(
                resolve_attachment_text(
                    &self.home,
                    spec_id,
                    att_id,
                    attachment_summary,
                )
                .await?,
            ),
        };

        let req_body = build_request(
            &model,
            request,
            brief_block.as_deref(),
            related_card_summaries,
        );

        let resp = client
            .post(&endpoint)
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
        let body = extract_text(&resp_value)?;

        Ok(CardBodyOutput {
            body,
            usage: vec![DecomposerUsage {
                agent_id: WRITER_AGENT_ID.to_string(),
                model: model.clone(),
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_read_tokens: usage.cache_read_tokens,
                cache_write_tokens: usage.cache_write_tokens,
            }],
        })
    }
}

/// Resolve an attachment's text content from disk, falling back to the
/// stored summary when the bytes aren't UTF-8. Mirrors the decomposer's
/// `read_brief` behavior so PDFs/images/etc. are decomposable here too.
async fn resolve_attachment_text(
    home: &std::path::Path,
    spec_id: Ulid,
    attachment_id: Ulid,
    attachment_summary: Option<&str>,
) -> Result<String, String> {
    let dir = home
        .join("specs")
        .join(spec_id.to_string())
        .join("context")
        .join(attachment_id.to_string());
    let mut primary: Option<std::path::PathBuf> = None;
    let mut entries = tokio::fs::read_dir(&dir)
        .await
        .map_err(|e| format!("could not list attachment dir {dir:?}: {e}"))?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".summary.txt") || lower == "metadata.json" || lower.starts_with('.') {
            continue;
        }
        primary = Some(path);
        break;
    }
    let path = primary
        .ok_or_else(|| format!("no readable brief file found in {dir:?}"))?;
    match tokio::fs::read_to_string(&path).await {
        Ok(text) => Ok(text),
        Err(_) => {
            if let Some(summary) = attachment_summary
                && !summary.trim().is_empty()
            {
                return Ok(format!(
                    "[NOTE: original attachment ({}) is binary/non-text. \
                     Scope card content to what's actually captured in this summary.]\n\n{}",
                    path.file_name().and_then(|s| s.to_str()).unwrap_or("(unknown)"),
                    summary
                ));
            }
            Err(format!(
                "attachment {path:?} is not UTF-8 text and no summary is available"
            ))
        }
    }
}

/// Build the LLM request. System prompt encodes the per-card_type voice
/// library; user message carries the agent's structured intent + any
/// supporting context (brief excerpt, related-card titles).
fn build_request(
    model: &str,
    request: &CardBodyRequest,
    brief_text: Option<&str>,
    related_card_summaries: &[(String, String)],
) -> serde_json::Value {
    let kind_label = request.kind.as_wire();
    let target = request.target_length_range.unwrap_or_else(|| default_range(request.kind));

    let user_msg = build_user_message(request, kind_label, target, related_card_summaries);

    // The system prompt is large + repeated across many card_body writes
    // in a session, so mark it cacheable. Same pattern as the decomposer's
    // executor calls.
    let system_blocks = serde_json::json!([
        {
            "type": "text",
            "text": SYSTEM_PROMPT,
            "cache_control": {"type": "ephemeral"}
        }
    ]);

    // User content: when there's a brief, give it its own cacheable block
    // so multiple card_body calls referencing the same brief in a session
    // share the brief cost. The per-call instructions stay fresh.
    let user_content: Vec<serde_json::Value> = match brief_text {
        Some(text) => vec![
            serde_json::json!({
                "type": "text",
                "text": format!("<source_attachment>\n{text}\n</source_attachment>"),
                "cache_control": {"type": "ephemeral"}
            }),
            serde_json::json!({"type": "text", "text": user_msg}),
        ],
        None => vec![serde_json::json!({"type": "text", "text": user_msg})],
    };

    serde_json::json!({
        "model": model,
        "max_tokens": WRITER_MAX_TOKENS,
        "system": system_blocks,
        "messages": [
            {"role": "user", "content": user_content}
        ]
    })
}

fn build_user_message(
    request: &CardBodyRequest,
    kind_label: &str,
    target: (usize, usize),
    related_card_summaries: &[(String, String)],
) -> String {
    let mut s = String::new();
    s.push_str("Card to author:\n");
    s.push_str(&format!("- card_type: {kind_label}\n"));
    if let Some(lane) = &request.lane {
        s.push_str(&format!("- lane: {lane}\n"));
    }
    s.push_str(&format!("- title: {}\n", request.title));
    s.push_str(&format!("- scope: {}\n", request.scope));
    s.push_str(&format!("- target body length: {}-{} chars\n", target.0, target.1));

    if !request.key_points.is_empty() {
        s.push_str("\nKey points to include (in order):\n");
        for (i, p) in request.key_points.iter().enumerate() {
            s.push_str(&format!("{}. {}\n", i + 1, p));
        }
    }

    if let Some(ctx) = &request.free_text_context {
        s.push_str(&format!("\nContext: {ctx}\n"));
    }

    if !related_card_summaries.is_empty() {
        s.push_str("\nExisting cards on the board (do NOT duplicate their content):\n");
        for (title, excerpt) in related_card_summaries {
            if excerpt.is_empty() {
                s.push_str(&format!("- {title}\n"));
            } else {
                s.push_str(&format!("- {title} — {excerpt}\n"));
            }
        }
    }

    s.push_str("\nWrite the card body now. Output ONLY the markdown body — no preamble, no closing remarks, no surrounding fences.");
    s
}

fn default_range(kind: CardKind) -> (usize, usize) {
    match kind {
        CardKind::Idea => (200, 600),
        CardKind::Task => (600, 1200),
        CardKind::Constraint => (400, 900),
        CardKind::Risk => (400, 1000),
        CardKind::Note => (200, 600),
    }
}

const SYSTEM_PROMPT: &str = r#"You author single card bodies for a spec-builder. The calling agent has already decided what card to create and provides: card_type, title, scope, optional key_points, optional context. You produce ONE markdown body in the right voice for the card_type.

Format conventions (apply to all card_types):
- Start with one declarative framing sentence — no preamble, no "Here is...", no question
- Use **bold-asterisk section headers** like **Implementation:** when the body has multiple distinct concerns
- Use markdown bullets (`- `) within sections; 3-5 per section typical
- Declarative voice. No "I think". No "Let me know if...". No marketing fluff.
- Stay within the target length range supplied per call
- Don't duplicate content from cards listed under "Existing cards on the board"
- Don't extrapolate beyond what the agent supplied — if scope and key_points don't support a section, omit it

Voice library by card_type:

idea — exploratory, "what if" framing. Best for early-stage possibilities, not yet decided.
- Open with a one-sentence framing of the possibility
- Optional **Why it might work:** / **Open questions:** / **What we'd need to validate:** sections
- Keep speculative — no false certainty
- Length 200-600 chars typical

task — concrete, actionable, implementation-ready.
- Open with a one-sentence framing of the work to be done
- Use **Implementation:** / **Acceptance Criteria:** / **Dependencies:** sections as warranted
- Each bullet describes a verifiable outcome or a specific action
- Length 600-1200 chars typical

constraint — normative, binding.
- Open with the constraint stated as a declarative requirement (no MUST/SHOULD verbiage needed unless it adds clarity)
- Add a **Rationale:** section explaining WHY this constraint exists when the reason isn't obvious
- Length 400-900 chars typical

risk — structured risk analysis.
- Open with one sentence naming the risk
- **Likelihood:** one phrase (Low / Medium / High plus reasoning)
- **Impact:** one phrase (severity + scope of impact)
- **Mitigation:** 2-4 concrete steps that reduce the risk
- Optional **Trigger Conditions:** when relevant — what monitoring signal would say "this is materializing"
- Length 400-1000 chars typical

note — question-shaped or annotation.
- Question or open-decision framing; no false resolution
- May be a single paragraph or 2-4 bullets
- Length 200-600 chars typical

Output the body markdown directly. No surrounding code fences. No leading "Body:" label."#;

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

    fn req_for(kind: CardKind) -> CardBodyRequest {
        CardBodyRequest {
            kind,
            lane: None,
            title: "T".into(),
            scope: "S".into(),
            key_points: vec!["a".into(), "b".into()],
            source_attachment_id: None,
            related_card_ids: vec![],
            free_text_context: None,
            target_length_range: None,
        }
    }

    #[test]
    fn system_prompt_covers_all_card_kinds() {
        // Pins that every card_type has its own voice rules in the system
        // prompt. A regression that drops a section would let agents call
        // delegate_card_body with that kind and get unpredictable output.
        for label in &["idea", "task", "constraint", "risk", "note"] {
            assert!(
                SYSTEM_PROMPT.contains(&format!("{label} —")),
                "system prompt missing voice rules for '{label}'; full prompt:\n{}",
                SYSTEM_PROMPT
            );
        }
    }

    #[test]
    fn system_prompt_forbids_preamble_and_marketing() {
        // The format conventions block carries anti-patterns that keep
        // bodies clean. Catch accidental relaxation.
        for forbidden in &[
            "no preamble",
            "Declarative voice",
            "Don't extrapolate",
            "Don't duplicate content",
        ] {
            assert!(
                SYSTEM_PROMPT.contains(forbidden),
                "system prompt should contain '{forbidden}'; full prompt:\n{}",
                SYSTEM_PROMPT
            );
        }
    }

    #[test]
    fn default_length_ranges_per_kind() {
        // The defaults are calibrated against the lengths observed in
        // rescued runs. If we ever change these, surface it explicitly.
        assert_eq!(default_range(CardKind::Idea), (200, 600));
        assert_eq!(default_range(CardKind::Task), (600, 1200));
        assert_eq!(default_range(CardKind::Constraint), (400, 900));
        assert_eq!(default_range(CardKind::Risk), (400, 1000));
        assert_eq!(default_range(CardKind::Note), (200, 600));
    }

    #[test]
    fn build_user_message_includes_all_supplied_fields() {
        let mut r = req_for(CardKind::Task);
        r.lane = Some("Plan".into());
        r.title = "Implement OAuth".into();
        r.scope = "Add OAuth handshake".into();
        r.key_points = vec!["use PKCE".into(), "store token in keychain".into()];
        r.free_text_context = Some("called from adversarial review".into());
        let msg = build_user_message(
            &r,
            "task",
            (600, 1200),
            &[("GitHub backend".to_string(), "Use GitHub for storage".to_string())],
        );
        assert!(msg.contains("- card_type: task"));
        assert!(msg.contains("- lane: Plan"));
        assert!(msg.contains("- title: Implement OAuth"));
        assert!(msg.contains("- scope: Add OAuth handshake"));
        assert!(msg.contains("- target body length: 600-1200 chars"));
        assert!(msg.contains("1. use PKCE"));
        assert!(msg.contains("2. store token in keychain"));
        assert!(msg.contains("Context: called from adversarial review"));
        assert!(msg.contains("Existing cards on the board"));
        assert!(msg.contains("GitHub backend — Use GitHub for storage"));
    }

    #[test]
    fn build_user_message_skips_optional_sections_when_absent() {
        let r = req_for(CardKind::Idea);
        let msg = build_user_message(&r, "idea", (200, 600), &[]);
        // Mandatory fields present
        assert!(msg.contains("- card_type: idea"));
        assert!(msg.contains("- title: T"));
        // Optional sections absent
        assert!(!msg.contains("- lane:"));
        assert!(!msg.contains("Context:"));
        assert!(!msg.contains("Existing cards"));
    }

    #[test]
    fn build_request_marks_system_block_cacheable() {
        let r = req_for(CardKind::Idea);
        let req = build_request("claude-haiku-4-5", &r, None, &[]);
        // System block is an array with one text block carrying cache_control.
        let system = req.get("system").unwrap().as_array().unwrap();
        assert_eq!(system.len(), 1);
        assert!(system[0].get("cache_control").is_some());
    }

    #[test]
    fn build_request_attaches_brief_as_cacheable_block_when_provided() {
        let r = req_for(CardKind::Task);
        let req = build_request("claude-haiku-4-5", &r, Some("This is the brief."), &[]);
        let msgs = req.get("messages").unwrap().as_array().unwrap();
        let content = msgs[0].get("content").unwrap().as_array().unwrap();
        // First block is the cacheable brief, second is the per-call instructions.
        assert_eq!(content.len(), 2);
        assert!(
            content[0].get("cache_control").is_some(),
            "first block should be the cacheable brief"
        );
        let brief_text = content[0].get("text").unwrap().as_str().unwrap();
        assert!(brief_text.contains("<source_attachment>"));
        assert!(brief_text.contains("This is the brief."));
        // Second block has no cache_control (per-call instructions are fresh)
        assert!(content[1].get("cache_control").is_none());
    }

    #[test]
    fn parse_usage_handles_cache_fields() {
        let resp = serde_json::json!({
            "usage": {
                "input_tokens": 100,
                "output_tokens": 200,
                "cache_read_input_tokens": 5000,
                "cache_creation_input_tokens": 0
            }
        });
        let u = parse_usage(&resp);
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.output_tokens, 200);
        assert_eq!(u.cache_read_tokens, 5000);
        assert_eq!(u.cache_write_tokens, 0);
    }

    /// Real non-UTF-8 bytes that will fail `read_to_string`. `\xff\xfe` is an
    /// invalid UTF-8 start byte; previously this test used `\x25PDF binary`
    /// which is actually valid ASCII and silently passed the text path.
    const BINARY_BYTES: &[u8] = b"\xff\xfe\x00 binary garbage \x01\x02\x03";

    #[tokio::test]
    async fn resolve_attachment_text_uses_summary_on_binary() {
        let dir = tempfile::tempdir().unwrap();
        let spec_id = Ulid::new();
        let attachment_id = Ulid::new();
        let attach_dir = dir
            .path()
            .join("specs")
            .join(spec_id.to_string())
            .join("context")
            .join(attachment_id.to_string());
        std::fs::create_dir_all(&attach_dir).unwrap();
        std::fs::write(attach_dir.join("brief.pdf"), BINARY_BYTES).unwrap();

        let text = resolve_attachment_text(
            dir.path(),
            spec_id,
            attachment_id,
            Some("LLM summary of the PDF."),
        )
        .await
        .unwrap();
        assert!(text.contains("[NOTE:"));
        assert!(text.contains("LLM summary of the PDF."));
    }

    #[tokio::test]
    async fn resolve_attachment_text_errors_without_summary_on_binary() {
        let dir = tempfile::tempdir().unwrap();
        let spec_id = Ulid::new();
        let attachment_id = Ulid::new();
        let attach_dir = dir
            .path()
            .join("specs")
            .join(spec_id.to_string())
            .join("context")
            .join(attachment_id.to_string());
        std::fs::create_dir_all(&attach_dir).unwrap();
        std::fs::write(attach_dir.join("brief.pdf"), BINARY_BYTES).unwrap();

        let err = resolve_attachment_text(dir.path(), spec_id, attachment_id, None)
            .await
            .unwrap_err();
        assert!(err.contains("not UTF-8") && err.contains("no summary"));
    }

    #[tokio::test]
    async fn resolve_attachment_text_returns_text_when_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let spec_id = Ulid::new();
        let attachment_id = Ulid::new();
        let attach_dir = dir
            .path()
            .join("specs")
            .join(spec_id.to_string())
            .join("context")
            .join(attachment_id.to_string());
        std::fs::create_dir_all(&attach_dir).unwrap();
        std::fs::write(attach_dir.join("brief.md"), b"# Hello\nplain text").unwrap();

        let text =
            resolve_attachment_text(dir.path(), spec_id, attachment_id, None).await.unwrap();
        assert!(text.contains("# Hello"));
        assert!(!text.starts_with("[NOTE:"));
    }
}
