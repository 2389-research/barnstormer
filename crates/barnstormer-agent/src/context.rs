// ABOUTME: Provides AgentContext for feeding state and history to LLM-backed agents.
// ABOUTME: Defines AgentRole enum and snapshot serialization for persistent agent memory.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ulid::Ulid;

use barnstormer_core::event::{Event, EventPayload};
use barnstormer_core::transcript::TranscriptMessage;

/// The maximum character length for a rolling summary before compaction triggers.
const ROLLING_SUMMARY_CAP: usize = 2000;

/// The maximum number of key decisions to retain per agent.
const MAX_KEY_DECISIONS: usize = 50;

/// Identifies the functional role an agent plays within the swarm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentRole {
    Manager,
    Brainstormer,
    Planner,
    DotGenerator,
    Critic,
}

impl AgentRole {
    /// Return a human-readable label for this role.
    pub fn label(&self) -> &'static str {
        match self {
            AgentRole::Manager => "manager",
            AgentRole::Brainstormer => "brainstormer",
            AgentRole::Planner => "planner",
            AgentRole::DotGenerator => "dot_generator",
            AgentRole::Critic => "critic",
        }
    }
}

impl std::fmt::Display for AgentRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Contextual information provided to an agent for each reasoning step.
/// Contains the current state summary, recent events, transcript history,
/// and the agent's accumulated memory (rolling summary and key decisions).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentContext {
    pub spec_id: Ulid,
    pub agent_id: String,
    pub agent_role: AgentRole,
    pub state_summary: String,
    pub recent_events: Vec<Event>,
    pub recent_transcript: Vec<TranscriptMessage>,
    pub rolling_summary: String,
    pub key_decisions: Vec<String>,
    pub last_event_seen: u64,
}

impl AgentContext {
    /// Create a fresh context for a given agent with no accumulated memory.
    pub fn new(spec_id: Ulid, agent_id: String, agent_role: AgentRole) -> Self {
        Self {
            spec_id,
            agent_id,
            agent_role,
            state_summary: String::new(),
            recent_events: Vec::new(),
            recent_transcript: Vec::new(),
            rolling_summary: String::new(),
            key_decisions: Vec::new(),
            last_event_seen: 0,
        }
    }

    /// Process new events to update the rolling summary and last_event_seen cursor.
    /// Events with IDs at or below last_event_seen are skipped.
    pub fn update_from_events(&mut self, events: &[Event]) {
        for event in events {
            if event.event_id <= self.last_event_seen {
                continue;
            }
            self.last_event_seen = event.event_id;

            let description = format!(
                "Event #{}: {}",
                event.event_id,
                describe_event_payload(&event.payload)
            );

            if self.rolling_summary.is_empty() {
                self.rolling_summary = description;
            } else {
                self.rolling_summary.push_str("; ");
                self.rolling_summary.push_str(&description);
            }
        }

        self.compact_summary();
    }

    /// Append a key decision to the bounded decision list.
    pub fn add_decision(&mut self, decision: String) {
        self.key_decisions.push(decision);
        if self.key_decisions.len() > MAX_KEY_DECISIONS {
            self.key_decisions
                .drain(0..self.key_decisions.len() - MAX_KEY_DECISIONS);
        }
    }

    /// Serialize this context to a serde_json::Value for inclusion in snapshot data.
    pub fn to_snapshot_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    /// Restore an AgentContext from a previously-serialized snapshot value.
    pub fn from_snapshot_value(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value.clone())
    }

    /// If the rolling summary exceeds the character cap, truncate older content
    /// and prepend a marker indicating that earlier context was compacted.
    pub fn compact_summary(&mut self) {
        let char_count = self.rolling_summary.chars().count();
        if char_count <= ROLLING_SUMMARY_CAP {
            return;
        }

        // Keep the tail portion that fits within the cap, leaving room for the prefix.
        let prefix = "[earlier context compacted] ";
        let prefix_chars = prefix.chars().count();
        let budget = ROLLING_SUMMARY_CAP.saturating_sub(prefix_chars);

        // Take the last `budget` characters using char-safe indexing.
        let skip = char_count.saturating_sub(budget);
        let tail: String = self.rolling_summary.chars().skip(skip).collect();

        // Find a clean break point (semicolon boundary) within the tail.
        let clean_start = tail.find("; ").map(|i| i + 2).unwrap_or(0);
        let trimmed = &tail[clean_start..];

        self.rolling_summary = format!("{}{}", prefix, trimmed);
    }
}

/// Truncate a string to at most `max_chars` characters, appending "..." if truncated.
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{}...", truncated)
    }
}

/// Produce a human-readable description of an event payload for rolling summaries.
fn describe_event_payload(payload: &EventPayload) -> String {
    match payload {
        EventPayload::SpecCreated { title, .. } => {
            format!("spec created: '{}'", title)
        }
        EventPayload::SpecCoreUpdated { title, .. } => {
            if let Some(t) = title {
                format!("spec updated (title -> '{}')", t)
            } else {
                "spec metadata updated".to_string()
            }
        }
        EventPayload::CardCreated { card } => {
            format!("card created: '{}' ({})", card.title, card.card_type)
        }
        EventPayload::CardUpdated { card_id, title, .. } => {
            if let Some(t) = title {
                format!("card {} updated (title -> '{}')", card_id, t)
            } else {
                format!("card {} updated", card_id)
            }
        }
        EventPayload::CardMoved { card_id, lane, .. } => {
            format!("card {} moved to '{}'", card_id, lane)
        }
        EventPayload::CardDeleted { card_id } => {
            format!("card {} deleted", card_id)
        }
        EventPayload::TranscriptAppended { message } => {
            let preview = truncate_chars(&message.content, 50);
            format!("{} said: {}", message.sender, preview)
        }
        EventPayload::QuestionAsked { .. } => "question asked to user".to_string(),
        EventPayload::QuestionAnswered { answer, .. } => {
            let preview = truncate_chars(answer, 50);
            format!("user answered: {}", preview)
        }
        EventPayload::AgentStepStarted {
            agent_id,
            description,
        } => {
            format!("agent {} started: {}", agent_id, description)
        }
        EventPayload::AgentStepFinished {
            agent_id,
            diff_summary,
        } => {
            format!("agent {} finished: {}", agent_id, diff_summary)
        }
        EventPayload::UndoApplied {
            target_event_id, ..
        } => {
            format!("undo applied to event #{}", target_event_id)
        }
        EventPayload::SnapshotWritten { snapshot_id } => {
            format!("snapshot #{} written", snapshot_id)
        }
    }
}

/// Serialize a collection of agent contexts into a HashMap suitable for
/// inclusion in SnapshotData.agent_contexts.
pub fn contexts_to_snapshot_map(contexts: &[AgentContext]) -> HashMap<String, serde_json::Value> {
    contexts
        .iter()
        .map(|ctx| (ctx.agent_id.clone(), ctx.to_snapshot_value()))
        .collect()
}

/// Restore agent contexts from a SnapshotData.agent_contexts map.
/// Contexts that fail to deserialize are skipped with a warning.
pub fn contexts_from_snapshot_map(map: &HashMap<String, serde_json::Value>) -> Vec<AgentContext> {
    map.values()
        .filter_map(|value| match AgentContext::from_snapshot_value(value) {
            Ok(ctx) => Some(ctx),
            Err(e) => {
                tracing::warn!(error = %e, "failed to restore agent context from snapshot");
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use barnstormer_core::event::{Event, EventPayload};

    #[test]
    fn agent_context_creation() {
        let spec_id = Ulid::new();
        let ctx = AgentContext::new(
            spec_id,
            "brainstormer-1".to_string(),
            AgentRole::Brainstormer,
        );

        assert_eq!(ctx.spec_id, spec_id);
        assert_eq!(ctx.agent_id, "brainstormer-1");
        assert_eq!(ctx.agent_role, AgentRole::Brainstormer);
        assert!(ctx.state_summary.is_empty());
        assert!(ctx.recent_events.is_empty());
        assert!(ctx.recent_transcript.is_empty());
        assert!(ctx.rolling_summary.is_empty());
        assert!(ctx.key_decisions.is_empty());
        assert_eq!(ctx.last_event_seen, 0);
    }

    #[test]
    fn context_snapshot_round_trip() {
        let spec_id = Ulid::new();
        let mut ctx = AgentContext::new(spec_id, "planner-1".to_string(), AgentRole::Planner);
        ctx.rolling_summary = "Some accumulated context about the spec".to_string();
        ctx.key_decisions
            .push("Decided to use microservices".to_string());
        ctx.key_decisions
            .push("Chose PostgreSQL over SQLite".to_string());
        ctx.last_event_seen = 42;

        let snapshot = ctx.to_snapshot_value();
        assert!(snapshot.is_object());

        let restored = AgentContext::from_snapshot_value(&snapshot).expect("should deserialize");
        assert_eq!(restored.spec_id, spec_id);
        assert_eq!(restored.agent_id, "planner-1");
        assert_eq!(restored.agent_role, AgentRole::Planner);
        assert_eq!(restored.rolling_summary, ctx.rolling_summary);
        assert_eq!(restored.key_decisions, ctx.key_decisions);
        assert_eq!(restored.last_event_seen, 42);
    }

    #[test]
    fn context_compacts_when_too_large() {
        let spec_id = Ulid::new();
        let mut ctx = AgentContext::new(spec_id, "manager-1".to_string(), AgentRole::Manager);

        // Build a rolling summary that exceeds the 2000-char cap.
        let long_entry = "Event #999: SomeVariant";
        for _ in 0..200 {
            if ctx.rolling_summary.is_empty() {
                ctx.rolling_summary = long_entry.to_string();
            } else {
                ctx.rolling_summary.push_str("; ");
                ctx.rolling_summary.push_str(long_entry);
            }
        }

        assert!(ctx.rolling_summary.len() > ROLLING_SUMMARY_CAP);

        ctx.compact_summary();

        assert!(ctx.rolling_summary.len() <= ROLLING_SUMMARY_CAP);
        assert!(
            ctx.rolling_summary
                .starts_with("[earlier context compacted]")
        );
    }

    #[test]
    fn context_updates_from_events() {
        let spec_id = Ulid::new();
        let mut ctx = AgentContext::new(spec_id, "critic-1".to_string(), AgentRole::Critic);

        let events = vec![
            Event {
                event_id: 1,
                spec_id,
                timestamp: Utc::now(),
                payload: EventPayload::SpecCreated {
                    title: "Test".to_string(),
                    one_liner: "A test spec".to_string(),
                    goal: "Verify updates".to_string(),
                },
            },
            Event {
                event_id: 2,
                spec_id,
                timestamp: Utc::now(),
                payload: EventPayload::TranscriptAppended {
                    message: TranscriptMessage::new(
                        "system".to_string(),
                        "Spec created".to_string(),
                    ),
                },
            },
        ];

        ctx.update_from_events(&events);

        assert_eq!(ctx.last_event_seen, 2);
        assert!(!ctx.rolling_summary.is_empty());
        // Event descriptions should be human-readable
        assert!(ctx.rolling_summary.contains("Event #1"));
        assert!(ctx.rolling_summary.contains("spec created: 'Test'"));
        assert!(ctx.rolling_summary.contains("Event #2"));
        assert!(ctx.rolling_summary.contains("system said:"));
    }

    #[test]
    fn context_skips_already_seen_events() {
        let spec_id = Ulid::new();
        let mut ctx = AgentContext::new(spec_id, "critic-1".to_string(), AgentRole::Critic);
        ctx.last_event_seen = 5;

        let events = vec![
            Event {
                event_id: 3,
                spec_id,
                timestamp: Utc::now(),
                payload: EventPayload::SpecCreated {
                    title: "Old".to_string(),
                    one_liner: "Should skip".to_string(),
                    goal: "Skip".to_string(),
                },
            },
            Event {
                event_id: 6,
                spec_id,
                timestamp: Utc::now(),
                payload: EventPayload::TranscriptAppended {
                    message: TranscriptMessage::new(
                        "system".to_string(),
                        "Should process".to_string(),
                    ),
                },
            },
        ];

        ctx.update_from_events(&events);

        assert_eq!(ctx.last_event_seen, 6);
        // Only event #6 should appear in summary
        assert!(!ctx.rolling_summary.contains("Event #3"));
        assert!(ctx.rolling_summary.contains("Event #6"));
    }

    #[test]
    fn add_decision_bounds_list() {
        let spec_id = Ulid::new();
        let mut ctx = AgentContext::new(spec_id, "manager-1".to_string(), AgentRole::Manager);

        for i in 0..60 {
            ctx.add_decision(format!("Decision {}", i));
        }

        assert_eq!(ctx.key_decisions.len(), MAX_KEY_DECISIONS);
        // The oldest decisions should have been drained
        assert_eq!(
            ctx.key_decisions[0],
            format!("Decision {}", 60 - MAX_KEY_DECISIONS)
        );
    }

    #[test]
    fn agent_role_label() {
        assert_eq!(AgentRole::Manager.label(), "manager");
        assert_eq!(AgentRole::Brainstormer.label(), "brainstormer");
        assert_eq!(AgentRole::Planner.label(), "planner");
        assert_eq!(AgentRole::DotGenerator.label(), "dot_generator");
        assert_eq!(AgentRole::Critic.label(), "critic");
    }

    #[test]
    fn multi_context_snapshot_round_trip() {
        let spec_id = Ulid::new();

        let mut ctx_a = AgentContext::new(spec_id, "manager-1".to_string(), AgentRole::Manager);
        ctx_a.rolling_summary = "Manager saw 5 events".to_string();
        ctx_a.last_event_seen = 5;
        ctx_a.add_decision("Use REST API".to_string());

        let mut ctx_b = AgentContext::new(
            spec_id,
            "brainstormer-1".to_string(),
            AgentRole::Brainstormer,
        );
        ctx_b.rolling_summary = "Brainstormer explored ideas".to_string();
        ctx_b.last_event_seen = 3;

        let map = contexts_to_snapshot_map(&[ctx_a.clone(), ctx_b.clone()]);
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("manager-1"));
        assert!(map.contains_key("brainstormer-1"));

        let restored = contexts_from_snapshot_map(&map);
        assert_eq!(restored.len(), 2);

        let restored_manager = restored
            .iter()
            .find(|c| c.agent_id == "manager-1")
            .expect("should find manager");
        assert_eq!(restored_manager.rolling_summary, "Manager saw 5 events");
        assert_eq!(restored_manager.last_event_seen, 5);
        assert_eq!(restored_manager.key_decisions, vec!["Use REST API"]);

        let restored_brainstormer = restored
            .iter()
            .find(|c| c.agent_id == "brainstormer-1")
            .expect("should find brainstormer");
        assert_eq!(
            restored_brainstormer.rolling_summary,
            "Brainstormer explored ideas"
        );
    }

    #[test]
    fn compact_summary_handles_non_ascii() {
        let spec_id = Ulid::new();
        let mut ctx = AgentContext::new(spec_id, "manager-1".to_string(), AgentRole::Manager);

        // Build a summary with multi-byte characters (emoji, CJK) that exceeds the cap.
        // Each emoji is 4 bytes. Repeating enough to exceed ROLLING_SUMMARY_CAP.
        let emoji_entry = "Event #1: \u{1F680}\u{1F525}\u{2728} launched \u{4e16}\u{754c}";
        for _ in 0..200 {
            if ctx.rolling_summary.is_empty() {
                ctx.rolling_summary = emoji_entry.to_string();
            } else {
                ctx.rolling_summary.push_str("; ");
                ctx.rolling_summary.push_str(emoji_entry);
            }
        }

        assert!(ctx.rolling_summary.len() > ROLLING_SUMMARY_CAP);

        // This must not panic on non-ASCII char boundaries
        ctx.compact_summary();

        assert!(ctx.rolling_summary.chars().count() <= ROLLING_SUMMARY_CAP);
        assert!(
            ctx.rolling_summary
                .starts_with("[earlier context compacted]")
        );
    }

    #[test]
    fn describe_event_payload_non_ascii_content() {
        // Verify describe_event_payload doesn't panic on multi-byte content.
        // Build a message with 60 emoji characters to exceed the 50-char truncation limit.
        let emoji_content: String = (0..60).map(|i| {
            // Cycle through a few multi-byte emoji codepoints
            let codepoints = ['\u{1F600}', '\u{1F525}', '\u{2728}', '\u{1F680}', '\u{1F4A5}'];
            codepoints[i % codepoints.len()]
        }).collect();
        let message = barnstormer_core::transcript::TranscriptMessage::new(
            "agent-1".to_string(),
            emoji_content,
        );
        let payload = EventPayload::TranscriptAppended { message };
        // Must not panic
        let desc = describe_event_payload(&payload);
        assert!(desc.contains("agent-1 said:"));
        // Truncated descriptions should end with "..."
        assert!(desc.ends_with("..."));
    }

    #[test]
    fn describe_event_payload_non_ascii_answer() {
        let payload = EventPayload::QuestionAnswered {
            question_id: Ulid::new(),
            answer: "\u{4e16}\u{754c}\u{4f60}\u{597d}".repeat(20), // CJK characters, >50 chars
        };
        // Must not panic
        let desc = describe_event_payload(&payload);
        assert!(desc.contains("user answered:"));
        assert!(desc.ends_with("..."));
    }

    #[test]
    fn contexts_from_snapshot_map_skips_invalid() {
        let mut map = HashMap::new();
        // Valid context
        let spec_id = Ulid::new();
        let ctx = AgentContext::new(spec_id, "valid-1".to_string(), AgentRole::Planner);
        map.insert("valid-1".to_string(), ctx.to_snapshot_value());

        // Invalid entry (wrong shape)
        map.insert(
            "invalid-1".to_string(),
            serde_json::json!({"not_a_context": true}),
        );

        let restored = contexts_from_snapshot_map(&map);
        // Only the valid one should be restored
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].agent_id, "valid-1");
    }

    #[test]
    fn describe_event_payload_produces_readable_text() {
        let card = barnstormer_core::card::Card::new(
            "idea".to_string(),
            "Cache Layer".to_string(),
            "brainstormer-1".to_string(),
        );
        let card_id = card.card_id;

        let payloads_and_expected: Vec<(EventPayload, &str)> = vec![
            (
                EventPayload::SpecCreated {
                    title: "My App".to_string(),
                    one_liner: "An app".to_string(),
                    goal: "Build it".to_string(),
                },
                "spec created: 'My App'",
            ),
            (
                EventPayload::SpecCoreUpdated {
                    title: Some("Renamed".to_string()),
                    one_liner: None,
                    goal: None,
                    description: None,
                    constraints: None,
                    success_criteria: None,
                    risks: None,
                    notes: None,
                },
                "spec updated (title -> 'Renamed')",
            ),
            (
                EventPayload::CardCreated { card },
                "card created: 'Cache Layer' (idea)",
            ),
            (
                EventPayload::CardMoved {
                    card_id,
                    lane: "Spec".to_string(),
                    order: 1.0,
                },
                "moved to 'Spec'",
            ),
            (EventPayload::CardDeleted { card_id }, "deleted"),
            (
                EventPayload::QuestionAsked {
                    question: barnstormer_core::transcript::UserQuestion::Boolean {
                        question_id: Ulid::new(),
                        question: "Proceed?".to_string(),
                        default: None,
                    },
                },
                "question asked to user",
            ),
            (
                EventPayload::AgentStepStarted {
                    agent_id: "planner-1".to_string(),
                    description: "Planning phase".to_string(),
                },
                "agent planner-1 started: Planning phase",
            ),
            (
                EventPayload::UndoApplied {
                    target_event_id: 7,
                    inverse_events: vec![],
                },
                "undo applied to event #7",
            ),
            (
                EventPayload::SnapshotWritten { snapshot_id: 42 },
                "snapshot #42 written",
            ),
        ];

        for (payload, expected_substr) in &payloads_and_expected {
            let desc = describe_event_payload(payload);
            assert!(
                desc.contains(expected_substr),
                "expected '{}' to contain '{}', got '{}'",
                desc,
                expected_substr,
                desc
            );
        }
    }

    #[test]
    fn compaction_preserves_recent_entries_after_events() {
        let spec_id = Ulid::new();
        let mut ctx = AgentContext::new(spec_id, "manager-1".to_string(), AgentRole::Manager);

        // Feed many events to trigger compaction
        let events: Vec<Event> = (1..=100)
            .map(|i| Event {
                event_id: i,
                spec_id,
                timestamp: Utc::now(),
                payload: EventPayload::TranscriptAppended {
                    message: TranscriptMessage::new(
                        format!("agent-{}", i % 5),
                        format!("Message number {} with some extra padding to fill space", i),
                    ),
                },
            })
            .collect();

        ctx.update_from_events(&events);

        assert_eq!(ctx.last_event_seen, 100);
        assert!(ctx.rolling_summary.len() <= ROLLING_SUMMARY_CAP);
        // Should contain recent event references
        assert!(ctx.rolling_summary.contains("Event #100"));
    }
}
