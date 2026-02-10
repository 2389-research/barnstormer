// ABOUTME: Tool definitions for LLM function calling, expressed as serde_json::Value structs.
// ABOUTME: Each tool maps to an AgentAction and can be sent to any LLM API that supports tools.

use serde_json::{Value, json};

/// Return the complete set of tool definitions that agents can use.
/// These are provider-agnostic JSON schemas; each provider adapter
/// reformats them to match its API's tool specification.
pub fn all_tool_definitions() -> Vec<Value> {
    vec![
        ask_user_boolean(),
        ask_user_multiple_choice(),
        ask_user_freeform(),
        read_state(),
        write_commands(),
        emit_narration(),
        emit_diff_summary(),
    ]
}

/// Tool: ask the user a yes/no question.
fn ask_user_boolean() -> Value {
    json!({
        "name": "ask_user_boolean",
        "description": "Ask the user a yes/no question. Use when you need a simple binary decision from the human.",
        "parameters": {
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The yes/no question to ask the user."
                },
                "default": {
                    "type": "boolean",
                    "description": "Optional default answer (true for yes, false for no)."
                }
            },
            "required": ["question"]
        }
    })
}

/// Tool: ask the user to pick from a list of choices.
fn ask_user_multiple_choice() -> Value {
    json!({
        "name": "ask_user_multiple_choice",
        "description": "Ask the user to choose from a list of options. Use when you have specific alternatives to present.",
        "parameters": {
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to present along with the choices."
                },
                "choices": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of choices for the user to select from."
                },
                "allow_multi": {
                    "type": "boolean",
                    "description": "Whether the user can select multiple choices. Defaults to false."
                }
            },
            "required": ["question", "choices"]
        }
    })
}

/// Tool: ask the user an open-ended question.
fn ask_user_freeform() -> Value {
    json!({
        "name": "ask_user_freeform",
        "description": "Ask the user a free-form question. Use when you need detailed or unstructured input.",
        "parameters": {
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user."
                },
                "placeholder": {
                    "type": "string",
                    "description": "Optional placeholder text for the input field."
                },
                "validation_hint": {
                    "type": "string",
                    "description": "Optional hint about expected format or content."
                }
            },
            "required": ["question"]
        }
    })
}

/// Tool: read the current spec state summary.
fn read_state() -> Value {
    json!({
        "name": "read_state",
        "description": "Read the current spec state summary including cards, transcript, and metadata. Returns a text summary of the spec's current state.",
        "parameters": {
            "type": "object",
            "properties": {},
            "required": []
        }
    })
}

/// Tool: submit one or more commands to modify the spec.
fn write_commands() -> Value {
    json!({
        "name": "write_commands",
        "description": "Submit one or more commands to modify the spec. Commands can create/update/move/delete cards, update spec metadata, or append to the transcript.",
        "parameters": {
            "type": "object",
            "properties": {
                "commands": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "description": "A Command object. Must include a 'type' field matching one of: CreateCard, UpdateCard, MoveCard, DeleteCard, UpdateSpecCore, AppendTranscript."
                    },
                    "description": "List of commands to execute against the spec."
                }
            },
            "required": ["commands"]
        }
    })
}

/// Tool: emit a narration message visible in the transcript.
fn emit_narration() -> Value {
    json!({
        "name": "emit_narration",
        "description": "Emit a narration message to the spec transcript. Use to explain your reasoning or share observations with the user.",
        "parameters": {
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The narration text to add to the transcript."
                }
            },
            "required": ["message"]
        }
    })
}

/// Tool: emit a diff summary describing changes made in this step.
fn emit_diff_summary() -> Value {
    json!({
        "name": "emit_diff_summary",
        "description": "Emit a summary of changes made during this agent step. Used to describe what was added, modified, or removed.",
        "parameters": {
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "A concise summary of the changes made in this step."
                }
            },
            "required": ["summary"]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definitions_are_valid_json() {
        let tools = all_tool_definitions();
        assert_eq!(tools.len(), 7, "should have 7 tool definitions");

        let expected_names = [
            "ask_user_boolean",
            "ask_user_multiple_choice",
            "ask_user_freeform",
            "read_state",
            "write_commands",
            "emit_narration",
            "emit_diff_summary",
        ];

        for (i, tool) in tools.iter().enumerate() {
            // Each tool must be a JSON object
            assert!(tool.is_object(), "tool {} should be an object", i);

            // Each tool must have a name
            let name = tool
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("tool {} missing name", i));
            assert_eq!(name, expected_names[i]);

            // Each tool must have a description
            assert!(
                tool.get("description").and_then(|v| v.as_str()).is_some(),
                "tool {} missing description",
                i
            );

            // Each tool must have a parameters object with type "object"
            let params = tool
                .get("parameters")
                .unwrap_or_else(|| panic!("tool {} missing parameters", i));
            assert!(params.is_object(), "tool {} parameters should be object", i);
            assert_eq!(
                params.get("type").and_then(|v| v.as_str()),
                Some("object"),
                "tool {} parameters should have type 'object'",
                i
            );

            // Each tool must have a required array
            assert!(
                params.get("required").is_some(),
                "tool {} missing required array",
                i
            );
        }
    }

    #[test]
    fn tool_definitions_serialize_to_json_string() {
        let tools = all_tool_definitions();
        let json_str =
            serde_json::to_string_pretty(&tools).expect("should serialize tool definitions");
        assert!(!json_str.is_empty());

        // Verify it can be parsed back
        let parsed: Vec<Value> = serde_json::from_str(&json_str).expect("should parse back");
        assert_eq!(parsed.len(), tools.len());
    }
}
