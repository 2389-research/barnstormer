// ABOUTME: Server-side impl of barnstormer_agent::CardDecomposer that runs a
// ABOUTME: Haiku architect + executor pipeline against the Anthropic API directly
// ABOUTME: (bypassing mux to gain access to prompt caching via cache_control).

use async_trait::async_trait;
use barnstormer_agent::{CardDecomposer, DecomposedCard, DecomposerOutput, DecomposerUsage};
use serde::Deserialize;
use std::path::PathBuf;
use ulid::Ulid;

/// Default Haiku model for the decomposition pipeline. Both architect and
/// executor use the same model — the 2026-05-09 run-04C experiment showed
/// Haiku is competent at architecting decomposition plans when given a
/// well-formed brief, and gets cheaper per-card than Sonnet by ~5x.
const DEFAULT_DECOMPOSER_MODEL: &str = "claude-haiku-4-5";

/// Hard cap on per-call output. Long enough for the architect to plan ~25
/// cards in JSON, and long enough for executors to produce 700-1500 char
/// bodies even when Haiku is over-verbose (we'd rather see truncated bodies
/// than an unparseable JSON architect plan).
const ARCHITECT_MAX_TOKENS: u32 = 8000;
const EXECUTOR_MAX_TOKENS: u32 = 900;

/// Synthetic agent IDs recorded against per-call AgentStepUsage events so
/// cost-attribution downstream can distinguish architect from executor calls
/// inside a single delegate_card_decomposition tool invocation.
const ARCHITECT_AGENT_ID: &str = "card-decomposer-architect";
const EXECUTOR_AGENT_ID: &str = "card-decomposer-executor";

/// Server-side decomposer. Reads the brief from disk, calls Anthropic
/// directly via reqwest so we can attach `cache_control: ephemeral` to the
/// brief block (mux does not currently expose prompt-caching). Pricing
/// study 2026-05-09 run-02 showed that without caching the split pipeline
/// costs MORE than monolithic Sonnet (+75%); with caching it wins by ~16%.
/// So caching is load-bearing, not a nice-to-have.
#[derive(Debug)]
pub struct ServerCardDecomposer {
    pub home: PathBuf,
}

#[async_trait]
impl CardDecomposer for ServerCardDecomposer {
    async fn decompose(
        &self,
        spec_id: Ulid,
        brief_attachment_id: Ulid,
        target_card_count: u32,
        decomposition_hints: Option<&str>,
        attachment_summary: Option<&str>,
    ) -> Result<DecomposerOutput, String> {
        let model = std::env::var("BARNSTORMER_DECOMPOSER_MODEL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_DECOMPOSER_MODEL.to_string());

        // The brief is stored on disk at:
        //   <home>/specs/<spec_id>/context/<attachment_id>/<filename>
        // We try to find any non-summary file in that dir.
        let brief_dir = self
            .home
            .join("specs")
            .join(spec_id.to_string())
            .join("context")
            .join(brief_attachment_id.to_string());
        let brief_text = read_brief(&brief_dir, attachment_summary).await?;

        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| "ANTHROPIC_API_KEY not set".to_string())?;
        let base_url = std::env::var("ANTHROPIC_BASE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "https://api.anthropic.com".to_string());
        let endpoint = format!("{}/v1/messages", base_url.trim_end_matches('/'));

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;

        let mut usage_log = Vec::new();

        // Step 1: architect produces the decomposition plan as JSON.
        let plan = run_architect(
            &client,
            &endpoint,
            &api_key,
            &model,
            &brief_text,
            target_card_count,
            decomposition_hints,
            &mut usage_log,
        )
        .await?;

        // Step 2: for each planned card, run an executor with the brief
        // block + executor system prompt cached. Sequential — keeps the
        // logic simple; runs in ~6-8 seconds total for typical 20-card plans.
        let mut cards = Vec::with_capacity(plan.cards.len());
        for entry in &plan.cards {
            let body = run_executor(
                &client,
                &endpoint,
                &api_key,
                &model,
                &brief_text,
                entry,
                &mut usage_log,
            )
            .await?;
            cards.push(DecomposedCard {
                title: entry.title.clone(),
                card_type: normalize_card_type(&entry.card_type),
                body,
                lane: entry.lane.clone(),
            });
        }

        Ok(DecomposerOutput {
            cards,
            usage: usage_log,
        })
    }
}

/// Locate and read the brief file inside <home>/specs/<id>/context/<att>/.
/// Picks the first regular file that isn't an obvious sidecar (summary.txt,
/// metadata.json).
///
/// Tries UTF-8 text first — works for .md, .txt, .json, etc. If the file
/// isn't valid UTF-8 (most commonly PDFs, images, audio, video), falls back
/// to the pre-computed `attachment_summary` that barnstormer's multimodal
/// summarizer generated at upload time. If that summary is also missing
/// (e.g. summarizer timed out or failed), returns a clear error so the
/// Manager can route around the decomposer instead of silently producing
/// zero cards.
async fn read_brief(
    dir: &std::path::Path,
    attachment_summary: Option<&str>,
) -> Result<String, String> {
    let mut primary_path: Option<std::path::PathBuf> = None;
    let mut entries = tokio::fs::read_dir(dir)
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
        primary_path = Some(path);
        break;
    }

    let path = match primary_path {
        Some(p) => p,
        None => return Err(format!("no readable brief file found in {dir:?}")),
    };

    // Try UTF-8 text first.
    match tokio::fs::read_to_string(&path).await {
        Ok(text) => Ok(text),
        Err(text_err) => {
            // Bytes aren't valid UTF-8 — almost certainly a PDF/image/audio/
            // video. Use the upload-time summary as the surrogate brief.
            // Prefix with a note so the architect prompt knows it's a
            // summary, not the full text — bullets and decomposition
            // decisions should be appropriately scoped to a summary.
            if let Some(summary) = attachment_summary
                && !summary.trim().is_empty()
            {
                return Ok(format!(
                        "[NOTE: original attachment ({}) is binary/non-text. \
                         Decomposition is operating on the LLM-generated summary below, \
                         not the raw bytes. Scope card content to what's actually \
                         captured in this summary.]\n\n{}",
                        path.file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("(unknown)"),
                        summary
                    ));
            }
            Err(format!(
                "brief file {path:?} is not UTF-8 text and no attachment summary is available; \
                 cannot decompose (text-read error: {text_err}). \
                 The Manager should fall back to retrieve_context + manual write_commands."
            ))
        }
    }
}

#[derive(Debug, Deserialize)]
struct ArchitectPlan {
    cards: Vec<ArchitectPlanCard>,
}

#[derive(Debug, Deserialize)]
struct ArchitectPlanCard {
    title: String,
    card_type: String,
    #[serde(default)]
    lane: Option<String>,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    avoid: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
async fn run_architect(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    model: &str,
    brief_text: &str,
    target_card_count: u32,
    hints: Option<&str>,
    usage_log: &mut Vec<DecomposerUsage>,
) -> Result<ArchitectPlan, String> {
    let system = format!(
        "You are the architect for a spec-builder. Given a design brief, produce a decomposition plan that an executor model will use to write each card body.\n\n\
         Output ONLY JSON (no markdown fences):\n\n\
         {{\n  \"cards\": [\n    {{\n      \"title\": \"...\",\n      \"card_type\": \"idea|task|constraint|risk|note\",\n      \"lane\": \"Ideas|Plan|Spec\",\n      \"scope\": \"<one sentence — what this card covers>\",\n      \"avoid\": [\"<topic to skip with reason>\"]\n    }}\n  ]\n}}\n\n\
         Decomposition rules:\n\
         - One card per discrete topic. No duplicates.\n\
         - card_type: idea (exploratory) | task (concrete work) | constraint (binding) | risk | note (open question).\n\
         - scope: one sentence describing what this card covers; the executor expands it.\n\
         - avoid: list of topics this card must NOT cover (because in another card or out of scope).\n\n\
         Target: {target_card_count} cards. Stay close to that number — over-decomposition makes the board noisy."
    );

    let user_body = match hints {
        Some(h) => format!(
            "<source_attachment>\n{brief_text}\n</source_attachment>\n\nHints: {h}\n\nProduce the decomposition plan as JSON now."
        ),
        None => format!(
            "<source_attachment>\n{brief_text}\n</source_attachment>\n\nProduce the decomposition plan as JSON now."
        ),
    };

    let req_body = serde_json::json!({
        "model": model,
        "max_tokens": ARCHITECT_MAX_TOKENS,
        "system": system,
        "messages": [{"role": "user", "content": user_body}],
    });

    let resp_value = post_anthropic(client, endpoint, api_key, &req_body).await?;
    let usage = parse_usage(&resp_value);
    usage_log.push(DecomposerUsage {
        agent_id: ARCHITECT_AGENT_ID.to_string(),
        model: model.to_string(),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_write_tokens: usage.cache_write_tokens,
    });

    let text = extract_text(&resp_value)?;
    let json_str = strip_json_fences(&text);
    let plan: ArchitectPlan = serde_json::from_str(&json_str).map_err(|e| {
        format!("architect produced non-JSON output ({e}); first 200 chars: {}", &text.chars().take(200).collect::<String>())
    })?;
    if plan.cards.is_empty() {
        return Err("architect produced zero cards".into());
    }
    Ok(plan)
}

async fn run_executor(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    model: &str,
    brief_text: &str,
    card: &ArchitectPlanCard,
    usage_log: &mut Vec<DecomposerUsage>,
) -> Result<String, String> {
    const EXEC_SYSTEM: &str = "You write card bodies for a spec-builder. The architect provides each card's identity (title, type, lane, scope, avoid list). Read the source attachment and produce a markdown body.\n\n\
        Format:\n\
        - One declarative framing sentence first\n\
        - 3-5 sections with bold-asterisk headers like **Header:**\n\
        - 3-5 bullets per section, drawn from source\n\
        - Declarative voice. Length 700-1500 chars. Compact better than thorough.\n\
        - Don't extrapolate beyond source. Skip topics in the avoid list.\n\n\
        Output ONLY the body markdown — no preamble, no closing remarks.";

    let per_card = format!(
        "Card to write:\n- title: {title}\n- card_type: {card_type}\n- lane: {lane}\n- scope: {scope}\n\nAvoid in this card:\n{avoid}\n\nWrite the card body now. 700-1500 chars.",
        title = card.title,
        card_type = card.card_type,
        lane = card.lane.as_deref().unwrap_or("Ideas"),
        scope = card.scope,
        avoid = serde_json::to_string(&card.avoid).unwrap_or_else(|_| "[]".into()),
    );

    // System block: marked cacheable. Brief block: marked cacheable.
    // Per-card text: stays fresh per call. Two cache_control markers — the
    // system prompt is small (~750 tok, under Haiku's 2048-tok minimum so
    // may not cache) but the brief block is the big win (~5K tokens, easily
    // over the threshold).
    let req_body = serde_json::json!({
        "model": model,
        "max_tokens": EXECUTOR_MAX_TOKENS,
        "system": [
            {"type": "text", "text": EXEC_SYSTEM, "cache_control": {"type": "ephemeral"}}
        ],
        "messages": [
            {
                "role": "user",
                "content": [
                    {
                        "type": "text",
                        "text": format!("<source_attachment>\n{brief_text}\n</source_attachment>"),
                        "cache_control": {"type": "ephemeral"}
                    },
                    {"type": "text", "text": per_card}
                ]
            }
        ],
    });

    let resp_value = post_anthropic(client, endpoint, api_key, &req_body).await?;
    let usage = parse_usage(&resp_value);
    usage_log.push(DecomposerUsage {
        agent_id: EXECUTOR_AGENT_ID.to_string(),
        model: model.to_string(),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_write_tokens: usage.cache_write_tokens,
    });

    extract_text(&resp_value)
}

async fn post_anthropic(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let resp = client
        .post(endpoint)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(body)
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
    serde_json::from_str(&text)
        .map_err(|e| format!("anthropic response was not JSON: {e}; body: {text}"))
}

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

/// Strip ```json ... ``` fences if present so the architect can produce
/// either bare JSON or fenced JSON. We told it not to use fences, but Haiku
/// sometimes adds them anyway — be lenient.
fn strip_json_fences(text: &str) -> String {
    let trimmed = text.trim();
    // First try to find a JSON object span between { and the last }.
    if let Some(start) = trimmed.find('{')
        && let Some(end) = trimmed.rfind('}')
        && end >= start
    {
        return trimmed[start..=end].to_string();
    }
    trimmed.to_string()
}

/// Map architect's `card_type` to the values barnstormer-core accepts.
/// Defends against future architects emitting variants like "Idea" or
/// "constraint_card" — we collapse to lowercase singular and fall back to
/// "note" if nothing matches.
fn normalize_card_type(raw: &str) -> String {
    let lower = raw.trim().to_ascii_lowercase();
    let core = lower
        .trim_start_matches("card_")
        .trim_end_matches("_card")
        .trim_end_matches('s')
        .to_string();
    match core.as_str() {
        "idea" => "idea".into(),
        "task" => "task".into(),
        "constraint" => "constraint".into(),
        "risk" => "risk".into(),
        "note" => "note".into(),
        _ => "note".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_fences_unwraps_json_object_from_markdown() {
        let s = "```json\n{\"cards\": []}\n```";
        assert_eq!(strip_json_fences(s), "{\"cards\": []}");
    }

    #[test]
    fn strip_fences_passes_bare_json() {
        let s = "{\"cards\": [{\"title\":\"x\"}]}";
        assert_eq!(strip_json_fences(s), s);
    }

    #[test]
    fn strip_fences_handles_leading_chatter() {
        // Defensive: architect occasionally prefixes "Here's the plan:" despite the
        // instruction. Strip everything before the first { and after the last }.
        let s = "Sure, here is the plan:\n```json\n{\"cards\":[]}\n```\nLet me know.";
        assert_eq!(strip_json_fences(s), "{\"cards\":[]}");
    }

    #[test]
    fn normalize_card_type_canonical() {
        assert_eq!(normalize_card_type("idea"), "idea");
        assert_eq!(normalize_card_type("Task"), "task");
        assert_eq!(normalize_card_type(" CONSTRAINT "), "constraint");
        assert_eq!(normalize_card_type("Risks"), "risk");
        assert_eq!(normalize_card_type("notes"), "note");
    }

    #[test]
    fn normalize_card_type_falls_back_to_note() {
        // Unknown card_type should become "note" rather than crashing the
        // create-card command later.
        assert_eq!(normalize_card_type("hypothesis"), "note");
        assert_eq!(normalize_card_type(""), "note");
        assert_eq!(normalize_card_type("???"), "note");
    }

    #[test]
    fn parse_usage_handles_full_anthropic_shape() {
        let resp = serde_json::json!({
            "usage": {
                "input_tokens": 100,
                "output_tokens": 200,
                "cache_read_input_tokens": 300,
                "cache_creation_input_tokens": 400
            }
        });
        let u = parse_usage(&resp);
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.output_tokens, 200);
        assert_eq!(u.cache_read_tokens, 300);
        assert_eq!(u.cache_write_tokens, 400);
    }

    #[test]
    fn parse_usage_returns_zeros_when_missing() {
        let resp = serde_json::json!({});
        let u = parse_usage(&resp);
        assert_eq!(u.input_tokens, 0);
        assert_eq!(u.output_tokens, 0);
    }

    #[test]
    fn extract_text_concatenates_text_blocks() {
        let resp = serde_json::json!({
            "content": [
                {"type": "text", "text": "first "},
                {"type": "tool_use", "name": "x"},
                {"type": "text", "text": "second"}
            ]
        });
        assert_eq!(extract_text(&resp).unwrap(), "first second");
    }

    #[test]
    fn extract_text_errors_on_empty_content() {
        let resp = serde_json::json!({
            "content": [{"type": "tool_use", "name": "x"}]
        });
        let err = extract_text(&resp).unwrap_err();
        assert!(err.contains("no text content"));
    }
}
