// ABOUTME: Provides AgentContext for feeding state and history to LLM-backed agents.
// ABOUTME: Defines AgentRole enum and snapshot serialization for persistent agent memory.

use serde::{Deserialize, Serialize};
use ulid::Ulid;

use specd_core::event::Event;
use specd_core::transcript::TranscriptMessage;

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
                "Event #{}: {:?}",
                event.event_id,
                std::mem::discriminant(&event.payload)
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
        if self.rolling_summary.len() <= ROLLING_SUMMARY_CAP {
            return;
        }

        // Keep the tail portion that fits within the cap, leaving room for the prefix.
        let prefix = "[earlier context compacted] ";
        let budget = ROLLING_SUMMARY_CAP.saturating_sub(prefix.len());

        // Find a clean break point (semicolon boundary) within the tail.
        let tail = &self.rolling_summary[self.rolling_summary.len().saturating_sub(budget)..];
        let clean_start = tail.find("; ").map(|i| i + 2).unwrap_or(0);
        let trimmed = &tail[clean_start..];

        self.rolling_summary = format!("{}{}", prefix, trimmed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use specd_core::event::{Event, EventPayload};

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
        assert!(ctx.rolling_summary.starts_with("[earlier context compacted]"));
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
        assert!(ctx.rolling_summary.contains("Event #1"));
        assert!(ctx.rolling_summary.contains("Event #2"));
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
}
