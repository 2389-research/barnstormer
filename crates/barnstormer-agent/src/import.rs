// ABOUTME: LLM-powered spec import — parses arbitrary text into structured spec commands.
// ABOUTME: Sends content to an LLM, extracts JSON with spec metadata and cards, converts to Commands.

use std::sync::Arc;

use mux::llm::{LlmClient, Message, Request};
use serde::{Deserialize, Serialize};

use barnstormer_core::Command;

/// Result of parsing input content via the LLM. Contains the core spec
/// metadata and any cards extracted from the source material.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub spec: ImportSpec,
    #[serde(default)]
    pub update: Option<ImportUpdate>,
    #[serde(default)]
    pub cards: Vec<ImportCard>,
}

/// Core spec identity fields extracted from the input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportSpec {
    pub title: String,
    pub one_liner: String,
    pub goal: String,
}

/// Optional extended spec metadata extracted from the input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportUpdate {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub constraints: Option<String>,
    #[serde(default)]
    pub success_criteria: Option<String>,
    #[serde(default)]
    pub risks: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// A card extracted from the input material.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportCard {
    pub card_type: String,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub lane: Option<String>,
}

/// Send content to an LLM and parse the response into an ImportResult.
///
/// `source_hint` is an optional format hint (e.g. "dot", "yaml", "markdown")
/// that helps the LLM understand the input format.
pub async fn parse_with_llm(
    content: &str,
    source_hint: Option<&str>,
    client: &Arc<dyn LlmClient>,
    model: &str,
) -> Result<ImportResult, anyhow::Error> {
    let system_prompt = build_import_system_prompt(source_hint);
    let req = Request::new(model)
        .system(system_prompt)
        .message(Message::user(content))
        .max_tokens(4096);

    let response = client.create_message(&req).await?;
    let text = response.text();

    if text.is_empty() {
        return Err(anyhow::anyhow!("LLM returned empty response"));
    }

    extract_json(&text)
}

/// Build the system prompt that instructs the LLM to extract spec structure.
pub fn build_import_system_prompt(source_hint: Option<&str>) -> String {
    let format_note = match source_hint {
        Some(hint) => format!("The input is in {} format. ", hint),
        None => String::new(),
    };

    format!(
        r#"You are a spec structure extractor. {format_note}Analyze the input and extract a structured specification.

Output ONLY valid JSON with this exact schema (no markdown, no commentary):

{{
  "spec": {{
    "title": "Short title for the spec",
    "one_liner": "One sentence summary",
    "goal": "What this spec aims to achieve"
  }},
  "update": {{
    "description": "Detailed description (optional, null if not present)",
    "constraints": "Known constraints (optional)",
    "success_criteria": "How to measure success (optional)",
    "risks": "Known risks (optional)",
    "notes": "Additional notes (optional)"
  }},
  "cards": [
    {{
      "card_type": "idea|task|plan|decision|constraint|risk",
      "title": "Card title",
      "body": "Card body/details (optional)",
      "lane": "Ideas|Spec|Backlog (optional, defaults to Ideas)"
    }}
  ]
}}

Card type guidance:
- "idea": Creative concepts, features, possibilities
- "task": Concrete work items, implementation steps
- "plan": High-level strategies or approaches
- "decision": Choices that have been made or need to be made
- "constraint": Limitations or requirements that must be honored
- "risk": Potential problems or concerns

Extract as many meaningful cards as the input warrants. Every distinct idea, task, requirement, or concern should be its own card.

If the input is minimal, create at least one card capturing the core concept."#
    )
}

/// Parse JSON from LLM output using a 3-tier strategy:
/// 1. Try parsing the entire text as JSON
/// 2. Strip markdown code fences and try again
/// 3. Find the first `{` to last `}` and try that substring
pub fn extract_json(text: &str) -> Result<ImportResult, anyhow::Error> {
    // Tier 1: Try raw JSON parse
    if let Ok(result) = serde_json::from_str::<ImportResult>(text) {
        return Ok(result);
    }

    // Tier 2: Strip code fences
    let stripped = strip_code_fences(text);
    if let Ok(result) = serde_json::from_str::<ImportResult>(&stripped) {
        return Ok(result);
    }

    // Tier 3: Find first { to last }
    let first_brace = text.find('{');
    let last_brace = text.rfind('}');
    if let (Some(start), Some(end)) = (first_brace, last_brace)
        && start < end
    {
        let substring = &text[start..=end];
        if let Ok(result) = serde_json::from_str::<ImportResult>(substring) {
            return Ok(result);
        }
    }

    Err(anyhow::anyhow!(
        "failed to parse LLM response as ImportResult JSON"
    ))
}

/// Strip markdown code fences from text.
fn strip_code_fences(text: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    let mut in_fence = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || !trimmed.is_empty() {
            lines.push(line);
        }
    }

    lines.join("\n")
}

/// Convert an ImportResult into a Vec of Commands suitable for the event-sourcing pipeline.
///
/// Produces: one CreateSpec, optionally one UpdateSpecCore, and one CreateCard per card.
/// All commands use `created_by: "import"`.
pub fn to_commands(result: &ImportResult) -> Vec<Command> {
    let mut commands = Vec::new();

    // CreateSpec command
    commands.push(Command::CreateSpec {
        title: result.spec.title.clone(),
        one_liner: result.spec.one_liner.clone(),
        goal: result.spec.goal.clone(),
    });

    // UpdateSpecCore command (if any update fields are present)
    if let Some(ref update) = result.update {
        let has_content = update.description.is_some()
            || update.constraints.is_some()
            || update.success_criteria.is_some()
            || update.risks.is_some()
            || update.notes.is_some();

        if has_content {
            commands.push(Command::UpdateSpecCore {
                title: None,
                one_liner: None,
                goal: None,
                description: update.description.clone(),
                constraints: update.constraints.clone(),
                success_criteria: update.success_criteria.clone(),
                risks: update.risks.clone(),
                notes: update.notes.clone(),
            });
        }
    }

    // CreateCard commands
    for card in &result.cards {
        commands.push(Command::CreateCard {
            card_type: card.card_type.clone(),
            title: card.title.clone(),
            body: card.body.clone(),
            lane: card.lane.clone(),
            created_by: "import".to_string(),
        });
    }

    commands
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::StubLlmClient;

    fn sample_import_result() -> ImportResult {
        ImportResult {
            spec: ImportSpec {
                title: "Todo App".to_string(),
                one_liner: "A simple task manager".to_string(),
                goal: "Build a CLI todo application".to_string(),
            },
            update: Some(ImportUpdate {
                description: Some("A todo app with persistent storage".to_string()),
                constraints: Some("Must work offline".to_string()),
                success_criteria: None,
                risks: None,
                notes: None,
            }),
            cards: vec![
                ImportCard {
                    card_type: "idea".to_string(),
                    title: "Add tasks".to_string(),
                    body: Some("Users can add tasks with a title".to_string()),
                    lane: Some("Ideas".to_string()),
                },
                ImportCard {
                    card_type: "task".to_string(),
                    title: "Set up CLI framework".to_string(),
                    body: None,
                    lane: Some("Backlog".to_string()),
                },
            ],
        }
    }

    // -- to_commands tests --

    #[test]
    fn to_commands_creates_spec_command() {
        let result = sample_import_result();
        let commands = to_commands(&result);

        assert!(commands.len() >= 1);
        match &commands[0] {
            Command::CreateSpec {
                title,
                one_liner,
                goal,
            } => {
                assert_eq!(title, "Todo App");
                assert_eq!(one_liner, "A simple task manager");
                assert_eq!(goal, "Build a CLI todo application");
            }
            other => panic!("expected CreateSpec, got {:?}", other),
        }
    }

    #[test]
    fn to_commands_creates_update_spec_core() {
        let result = sample_import_result();
        let commands = to_commands(&result);

        // Second command should be UpdateSpecCore
        assert!(commands.len() >= 2);
        match &commands[1] {
            Command::UpdateSpecCore {
                description,
                constraints,
                success_criteria,
                risks,
                notes,
                ..
            } => {
                assert_eq!(
                    description.as_deref(),
                    Some("A todo app with persistent storage")
                );
                assert_eq!(constraints.as_deref(), Some("Must work offline"));
                assert!(success_criteria.is_none());
                assert!(risks.is_none());
                assert!(notes.is_none());
            }
            other => panic!("expected UpdateSpecCore, got {:?}", other),
        }
    }

    #[test]
    fn to_commands_creates_card_commands() {
        let result = sample_import_result();
        let commands = to_commands(&result);

        // CreateSpec + UpdateSpecCore + 2 cards = 4 commands
        assert_eq!(commands.len(), 4);

        match &commands[2] {
            Command::CreateCard {
                card_type,
                title,
                body,
                lane,
                created_by,
            } => {
                assert_eq!(card_type, "idea");
                assert_eq!(title, "Add tasks");
                assert_eq!(body.as_deref(), Some("Users can add tasks with a title"));
                assert_eq!(lane.as_deref(), Some("Ideas"));
                assert_eq!(created_by, "import");
            }
            other => panic!("expected CreateCard, got {:?}", other),
        }

        match &commands[3] {
            Command::CreateCard {
                card_type,
                title,
                body,
                lane,
                created_by,
            } => {
                assert_eq!(card_type, "task");
                assert_eq!(title, "Set up CLI framework");
                assert!(body.is_none());
                assert_eq!(lane.as_deref(), Some("Backlog"));
                assert_eq!(created_by, "import");
            }
            other => panic!("expected CreateCard, got {:?}", other),
        }
    }

    #[test]
    fn to_commands_handles_empty_cards() {
        let result = ImportResult {
            spec: ImportSpec {
                title: "Empty".to_string(),
                one_liner: "Nothing here".to_string(),
                goal: "Test empty".to_string(),
            },
            update: None,
            cards: vec![],
        };
        let commands = to_commands(&result);

        // Just CreateSpec, no UpdateSpecCore (update is None), no cards
        assert_eq!(commands.len(), 1);
        assert!(matches!(&commands[0], Command::CreateSpec { .. }));
    }

    #[test]
    fn to_commands_skips_update_when_all_fields_none() {
        let result = ImportResult {
            spec: ImportSpec {
                title: "Minimal".to_string(),
                one_liner: "Bare bones".to_string(),
                goal: "Test minimal".to_string(),
            },
            update: Some(ImportUpdate {
                description: None,
                constraints: None,
                success_criteria: None,
                risks: None,
                notes: None,
            }),
            cards: vec![],
        };
        let commands = to_commands(&result);

        // Should skip UpdateSpecCore since all fields are None
        assert_eq!(commands.len(), 1);
        assert!(matches!(&commands[0], Command::CreateSpec { .. }));
    }

    // -- extract_json tests --

    #[test]
    fn extract_json_parses_raw_json() {
        let json = serde_json::to_string(&sample_import_result()).unwrap();
        let result = extract_json(&json).unwrap();
        assert_eq!(result.spec.title, "Todo App");
        assert_eq!(result.cards.len(), 2);
    }

    #[test]
    fn extract_json_strips_code_fences() {
        let json = serde_json::to_string(&sample_import_result()).unwrap();
        let fenced = format!("```json\n{}\n```", json);
        let result = extract_json(&fenced).unwrap();
        assert_eq!(result.spec.title, "Todo App");
    }

    #[test]
    fn extract_json_finds_brace_substring() {
        let json = serde_json::to_string(&sample_import_result()).unwrap();
        let wrapped = format!("Here is the extracted spec:\n{}\nHope that helps!", json);
        let result = extract_json(&wrapped).unwrap();
        assert_eq!(result.spec.title, "Todo App");
    }

    #[test]
    fn extract_json_rejects_garbage() {
        let result = extract_json("this is not json at all");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("failed to parse"));
    }

    // -- ImportResult serde round-trip --

    #[test]
    fn import_result_serde_round_trip() {
        let original = sample_import_result();
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: ImportResult = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.spec.title, original.spec.title);
        assert_eq!(deserialized.spec.one_liner, original.spec.one_liner);
        assert_eq!(deserialized.spec.goal, original.spec.goal);
        assert_eq!(deserialized.cards.len(), original.cards.len());
        assert_eq!(deserialized.cards[0].card_type, "idea");
        assert_eq!(deserialized.cards[1].card_type, "task");
    }

    // -- build_import_system_prompt tests --

    #[test]
    fn system_prompt_includes_card_types() {
        let prompt = build_import_system_prompt(None);
        assert!(prompt.contains("idea"));
        assert!(prompt.contains("task"));
        assert!(prompt.contains("plan"));
        assert!(prompt.contains("decision"));
        assert!(prompt.contains("constraint"));
        assert!(prompt.contains("risk"));
    }

    #[test]
    fn system_prompt_includes_source_hint() {
        let prompt = build_import_system_prompt(Some("DOT graph"));
        assert!(prompt.contains("DOT graph"));
    }

    #[test]
    fn system_prompt_without_hint_has_no_format_note() {
        let prompt = build_import_system_prompt(None);
        // Should not contain any "The input is in" phrase when no hint
        assert!(!prompt.contains("The input is in"));
    }

    // -- Integration tests with StubLlmClient --

    #[tokio::test]
    async fn parse_with_llm_produces_valid_result() {
        let import_json = serde_json::to_string(&sample_import_result()).unwrap();
        let client: Arc<dyn LlmClient> = Arc::new(StubLlmClient::new(&import_json));

        let result = parse_with_llm("Build a todo app", None, &client, "stub-model")
            .await
            .unwrap();

        assert_eq!(result.spec.title, "Todo App");
        assert_eq!(result.cards.len(), 2);
    }

    #[tokio::test]
    async fn parse_with_llm_handles_code_fenced_response() {
        let import_json = serde_json::to_string(&sample_import_result()).unwrap();
        let fenced = format!("```json\n{}\n```", import_json);
        let client: Arc<dyn LlmClient> = Arc::new(StubLlmClient::new(&fenced));

        let result = parse_with_llm("Build a todo app", None, &client, "stub-model")
            .await
            .unwrap();

        assert_eq!(result.spec.title, "Todo App");
    }

    #[tokio::test]
    async fn parse_with_llm_propagates_parse_error() {
        let client: Arc<dyn LlmClient> = Arc::new(StubLlmClient::new("not valid json"));

        let result = parse_with_llm("something", None, &client, "stub-model").await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn parse_with_llm_propagates_empty_response() {
        let client: Arc<dyn LlmClient> = Arc::new(StubLlmClient::new(""));

        let result = parse_with_llm("something", None, &client, "stub-model").await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty response"));
    }
}
