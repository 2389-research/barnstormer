// ABOUTME: Defines SpecState and UndoEntry for building spec state from an event stream.
// ABOUTME: The apply() method pattern-matches on EventPayload to fold events into current state.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::card::Card;
use crate::event::{Event, EventPayload};
use crate::model::SpecCore;
use crate::transcript::{MessageKind, TranscriptMessage, UserQuestion};

/// Stores the inverse operations needed to undo a mutation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoEntry {
    pub event_id: u64,
    pub inverse: Vec<EventPayload>,
}

/// A file attached as context to the brainstorming phase of a spec.
/// Tracks the original upload metadata plus an optional agent-generated
/// summary and user notes. `removed` is a tombstone flag so event history
/// is preserved when an attachment is taken out of active context.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextAttachment {
    pub attachment_id: Ulid,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub summary: Option<String>,
    pub user_notes: Option<String>,
    pub added_at: DateTime<Utc>,
    pub removed: bool,
}

/// Tracks which lifecycle phase a spec is in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpecPhase {
    Brainstorming,
    #[serde(alias = "Active")]
    Refining,
    Complete,
}

fn default_phase_refining() -> SpecPhase {
    SpecPhase::Refining
}

/// The full materialized state of a spec, built by replaying events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecState {
    pub core: Option<SpecCore>,
    pub cards: BTreeMap<Ulid, Card>,
    pub transcript: Vec<TranscriptMessage>,
    pub pending_question: Option<UserQuestion>,
    pub undo_stack: Vec<UndoEntry>,
    pub last_event_id: u64,
    pub lanes: Vec<String>,
    #[serde(default = "default_phase_refining")]
    pub phase: SpecPhase,
    #[serde(default)]
    pub canvas_content: Option<String>,
    #[serde(default)]
    pub context_attachments: Vec<ContextAttachment>,
}

impl Default for SpecState {
    fn default() -> Self {
        Self {
            core: None,
            cards: BTreeMap::new(),
            transcript: Vec::new(),
            pending_question: None,
            undo_stack: Vec::new(),
            last_event_id: 0,
            lanes: vec!["Ideas".to_string(), "Plan".to_string(), "Spec".to_string()],
            phase: SpecPhase::Refining,
            canvas_content: None,
            context_attachments: Vec::new(),
        }
    }
}

impl SpecState {
    /// Create an empty SpecState with default lanes.
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a single event to mutate this state. Each event payload variant
    /// is handled to update the corresponding state fields. Undo entries are
    /// pushed for reversible mutations.
    pub fn apply(&mut self, event: &Event) {
        self.last_event_id = event.event_id;

        match &event.payload {
            EventPayload::SpecCreated {
                title,
                one_liner,
                goal,
            } => {
                self.core = Some(SpecCore {
                    spec_id: event.spec_id,
                    title: title.clone(),
                    one_liner: one_liner.clone(),
                    goal: goal.clone(),
                    description: None,
                    constraints: None,
                    success_criteria: None,
                    risks: None,
                    notes: None,
                    created_at: event.timestamp,
                    updated_at: event.timestamp,
                });
            }

            EventPayload::SpecCoreUpdated {
                title,
                one_liner,
                goal,
                description,
                constraints,
                success_criteria,
                risks,
                notes,
            } => {
                if let Some(ref mut core) = self.core {
                    if let Some(t) = title {
                        core.title = t.clone();
                    }
                    if let Some(o) = one_liner {
                        core.one_liner = o.clone();
                    }
                    if let Some(g) = goal {
                        core.goal = g.clone();
                    }
                    if let Some(d) = description {
                        core.description = Some(d.clone());
                    }
                    if let Some(c) = constraints {
                        core.constraints = Some(c.clone());
                    }
                    if let Some(s) = success_criteria {
                        core.success_criteria = Some(s.clone());
                    }
                    if let Some(r) = risks {
                        core.risks = Some(r.clone());
                    }
                    if let Some(n) = notes {
                        core.notes = Some(n.clone());
                    }
                    core.updated_at = event.timestamp;
                }
            }

            EventPayload::CardCreated { card } => {
                let inverse = vec![EventPayload::CardDeleted {
                    card_id: card.card_id,
                }];
                self.undo_stack.push(UndoEntry {
                    event_id: event.event_id,
                    inverse,
                });
                self.cards.insert(card.card_id, card.clone());
            }

            EventPayload::CardUpdated {
                card_id,
                title,
                body,
                card_type,
                refs,
            } => {
                if let Some(card) = self.cards.get_mut(card_id) {
                    // Build inverse from old values before mutating
                    let inverse = vec![EventPayload::CardUpdated {
                        card_id: *card_id,
                        title: title.as_ref().map(|_| card.title.clone()),
                        body: body.as_ref().map(|_| card.body.clone()),
                        card_type: card_type.as_ref().map(|_| card.card_type.clone()),
                        refs: refs.as_ref().map(|_| card.refs.clone()),
                    }];
                    self.undo_stack.push(UndoEntry {
                        event_id: event.event_id,
                        inverse,
                    });

                    if let Some(t) = title {
                        card.title = t.clone();
                    }
                    if let Some(b) = body {
                        card.body = b.clone();
                    }
                    if let Some(ct) = card_type {
                        card.card_type = ct.clone();
                    }
                    if let Some(r) = refs {
                        card.refs = r.clone();
                    }
                    card.updated_at = event.timestamp;
                }
            }

            EventPayload::CardMoved {
                card_id,
                lane,
                order,
            } => {
                if let Some(card) = self.cards.get_mut(card_id) {
                    let inverse = vec![EventPayload::CardMoved {
                        card_id: *card_id,
                        lane: card.lane.clone(),
                        order: card.order,
                    }];
                    self.undo_stack.push(UndoEntry {
                        event_id: event.event_id,
                        inverse,
                    });

                    card.lane = lane.clone();
                    card.order = *order;
                    card.updated_at = event.timestamp;
                }
            }

            EventPayload::CardDeleted { card_id } => {
                if let Some(card) = self.cards.remove(card_id) {
                    let inverse = vec![EventPayload::CardCreated { card }];
                    self.undo_stack.push(UndoEntry {
                        event_id: event.event_id,
                        inverse,
                    });
                }
            }

            EventPayload::TranscriptAppended { message } => {
                self.transcript.push(message.clone());
            }

            EventPayload::QuestionAsked { question } => {
                self.pending_question = Some(question.clone());
            }

            EventPayload::QuestionAnswered {
                question_id,
                answer,
            } => {
                self.pending_question = None;
                self.canvas_content = None;
                self.transcript.push(TranscriptMessage {
                    message_id: *question_id,
                    sender: "human".to_string(),
                    content: answer.clone(),
                    kind: MessageKind::Chat,
                    timestamp: event.timestamp,
                });
            }

            EventPayload::AgentStepStarted {
                agent_id,
                description,
            } => {
                self.transcript.push(TranscriptMessage {
                    message_id: Ulid::new(),
                    sender: agent_id.clone(),
                    content: description.clone(),
                    kind: MessageKind::StepStarted,
                    timestamp: event.timestamp,
                });
            }

            EventPayload::AgentStepFinished {
                agent_id,
                diff_summary,
            } => {
                self.transcript.push(TranscriptMessage {
                    message_id: Ulid::new(),
                    sender: agent_id.clone(),
                    content: diff_summary.clone(),
                    kind: MessageKind::StepFinished,
                    timestamp: event.timestamp,
                });
            }

            EventPayload::UndoApplied { inverse_events, .. } => {
                // Apply inverse events without pushing further undo entries
                for inverse_payload in inverse_events {
                    let synthetic_event = Event {
                        event_id: event.event_id,
                        spec_id: event.spec_id,
                        timestamp: event.timestamp,
                        payload: inverse_payload.clone(),
                    };
                    self.apply_without_undo(&synthetic_event);
                }
                self.undo_stack.pop();
                // Clear stale canvas content after undo
                self.canvas_content = None;
            }

            EventPayload::SnapshotWritten { .. } => {
                // No-op on state
            }

            EventPayload::CanvasUpdated { content } => {
                if content.is_empty() {
                    self.canvas_content = None;
                } else {
                    self.canvas_content = Some(content.clone());
                }
                // No undo entry — canvas content is regenerated by agents
            }

            EventPayload::PhaseTransitioned { phase } => {
                self.phase = phase.clone();
                // No undo entry — phase transitions are lifecycle events
            }

            EventPayload::ContextAttached { attachment } => {
                let inverse = vec![EventPayload::ContextRemoved {
                    attachment_id: attachment.attachment_id,
                }];
                self.undo_stack.push(UndoEntry {
                    event_id: event.event_id,
                    inverse,
                });
                self.context_attachments.push(attachment.clone());
            }

            EventPayload::ContextSummarized {
                attachment_id,
                summary,
            } => {
                if let Some(att) = self
                    .context_attachments
                    .iter_mut()
                    .find(|a| a.attachment_id == *attachment_id)
                {
                    // no undo for summarization — it's idempotent replacement from the summarizer
                    att.summary = Some(summary.clone());
                }
            }

            EventPayload::ContextNotesUpdated {
                attachment_id,
                notes,
            } => {
                if let Some(att) = self
                    .context_attachments
                    .iter_mut()
                    .find(|a| a.attachment_id == *attachment_id)
                {
                    let prior = att.user_notes.clone().unwrap_or_default();
                    self.undo_stack.push(UndoEntry {
                        event_id: event.event_id,
                        inverse: vec![EventPayload::ContextNotesUpdated {
                            attachment_id: *attachment_id,
                            notes: prior,
                        }],
                    });
                    att.user_notes = if notes.is_empty() {
                        None
                    } else {
                        Some(notes.clone())
                    };
                }
            }

            EventPayload::ContextRemoved { attachment_id } => {
                if let Some(att) = self
                    .context_attachments
                    .iter_mut()
                    .find(|a| a.attachment_id == *attachment_id)
                {
                    // Inverse is ContextAttached with the same attachment (un-removed).
                    let mut restored = att.clone();
                    restored.removed = false;
                    self.undo_stack.push(UndoEntry {
                        event_id: event.event_id,
                        inverse: vec![EventPayload::ContextAttached {
                            attachment: restored,
                        }],
                    });
                    att.removed = true;
                }
            }

            EventPayload::StreamingDelta { .. } => {
                // Ephemeral — no state mutation
            }

            EventPayload::StreamingToolActivity { .. } => {
                // Ephemeral — no state mutation
            }
        }
    }

    /// Apply an event's payload effects without pushing undo entries.
    /// Used internally for applying inverse events during undo.
    fn apply_without_undo(&mut self, event: &Event) {
        match &event.payload {
            EventPayload::CardCreated { card } => {
                self.cards.insert(card.card_id, card.clone());
            }
            EventPayload::CardUpdated {
                card_id,
                title,
                body,
                card_type,
                refs,
            } => {
                if let Some(card) = self.cards.get_mut(card_id) {
                    if let Some(t) = title {
                        card.title = t.clone();
                    }
                    if let Some(b) = body {
                        card.body = b.clone();
                    }
                    if let Some(ct) = card_type {
                        card.card_type = ct.clone();
                    }
                    if let Some(r) = refs {
                        card.refs = r.clone();
                    }
                    card.updated_at = event.timestamp;
                }
            }
            EventPayload::CardMoved {
                card_id,
                lane,
                order,
            } => {
                if let Some(card) = self.cards.get_mut(card_id) {
                    card.lane = lane.clone();
                    card.order = *order;
                    card.updated_at = event.timestamp;
                }
            }
            EventPayload::CardDeleted { card_id } => {
                self.cards.remove(card_id);
            }
            EventPayload::PhaseTransitioned { phase } => {
                self.phase = phase.clone();
            }
            EventPayload::CanvasUpdated { content } => {
                if content.is_empty() {
                    self.canvas_content = None;
                } else {
                    self.canvas_content = Some(content.clone());
                }
            }
            EventPayload::ContextAttached { attachment } => {
                // During undo of a ContextRemoved, we get a ContextAttached inverse whose
                // attachment_id already exists in state — un-tombstone rather than duplicate.
                if let Some(existing) = self
                    .context_attachments
                    .iter_mut()
                    .find(|a| a.attachment_id == attachment.attachment_id)
                {
                    *existing = attachment.clone();
                } else {
                    self.context_attachments.push(attachment.clone());
                }
            }
            EventPayload::ContextSummarized {
                attachment_id,
                summary,
            } => {
                if let Some(att) = self
                    .context_attachments
                    .iter_mut()
                    .find(|a| a.attachment_id == *attachment_id)
                {
                    att.summary = Some(summary.clone());
                }
            }
            EventPayload::ContextNotesUpdated {
                attachment_id,
                notes,
            } => {
                if let Some(att) = self
                    .context_attachments
                    .iter_mut()
                    .find(|a| a.attachment_id == *attachment_id)
                {
                    att.user_notes = if notes.is_empty() {
                        None
                    } else {
                        Some(notes.clone())
                    };
                }
            }
            EventPayload::ContextRemoved { attachment_id } => {
                if let Some(att) = self
                    .context_attachments
                    .iter_mut()
                    .find(|a| a.attachment_id == *attachment_id)
                {
                    att.removed = true;
                }
            }
            EventPayload::StreamingDelta { .. } => {
                // Ephemeral — no state mutation
            }
            EventPayload::StreamingToolActivity { .. } => {
                // Ephemeral — no state mutation
            }
            // Other event types during undo are applied normally
            _ => {
                self.apply(event);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::Card;
    use chrono::Utc;

    fn make_spec_id() -> Ulid {
        Ulid::new()
    }

    fn make_event(event_id: u64, spec_id: Ulid, payload: EventPayload) -> Event {
        Event {
            event_id,
            spec_id,
            timestamp: Utc::now(),
            payload,
        }
    }

    #[test]
    fn apply_spec_created_sets_core_fields() {
        let mut state = SpecState::new();
        let spec_id = make_spec_id();
        let event = make_event(
            1,
            spec_id,
            EventPayload::SpecCreated {
                title: "My Spec".to_string(),
                one_liner: "A thing".to_string(),
                goal: "Build it".to_string(),
            },
        );

        state.apply(&event);

        let core = state.core.as_ref().expect("core should be set");
        assert_eq!(core.spec_id, spec_id);
        assert_eq!(core.title, "My Spec");
        assert_eq!(core.one_liner, "A thing");
        assert_eq!(core.goal, "Build it");
        assert!(core.description.is_none());
        assert_eq!(state.last_event_id, 1);
    }

    #[test]
    fn apply_spec_core_updated_modifies_fields() {
        let mut state = SpecState::new();
        let spec_id = make_spec_id();

        // First create the spec
        state.apply(&make_event(
            1,
            spec_id,
            EventPayload::SpecCreated {
                title: "Original".to_string(),
                one_liner: "First".to_string(),
                goal: "Initial goal".to_string(),
            },
        ));

        // Then update it
        state.apply(&make_event(
            2,
            spec_id,
            EventPayload::SpecCoreUpdated {
                title: Some("Updated Title".to_string()),
                one_liner: None,
                goal: None,
                description: Some("A description".to_string()),
                constraints: None,
                success_criteria: None,
                risks: None,
                notes: None,
            },
        ));

        let core = state.core.as_ref().expect("core should exist");
        assert_eq!(core.title, "Updated Title");
        assert_eq!(core.one_liner, "First"); // unchanged
        assert_eq!(core.description, Some("A description".to_string()));
        assert_eq!(state.last_event_id, 2);
    }

    #[test]
    fn apply_card_created_adds_card() {
        let mut state = SpecState::new();
        let spec_id = make_spec_id();
        let card = Card::new(
            "idea".to_string(),
            "Test Card".to_string(),
            "agent-1".to_string(),
        );
        let card_id = card.card_id;

        state.apply(&make_event(1, spec_id, EventPayload::CardCreated { card }));

        assert_eq!(state.cards.len(), 1);
        assert!(state.cards.contains_key(&card_id));
        assert_eq!(state.cards[&card_id].title, "Test Card");
    }

    #[test]
    fn apply_card_updated_modifies_card() {
        let mut state = SpecState::new();
        let spec_id = make_spec_id();
        let card = Card::new(
            "idea".to_string(),
            "Original Title".to_string(),
            "agent-1".to_string(),
        );
        let card_id = card.card_id;

        state.apply(&make_event(1, spec_id, EventPayload::CardCreated { card }));

        state.apply(&make_event(
            2,
            spec_id,
            EventPayload::CardUpdated {
                card_id,
                title: Some("Renamed Card".to_string()),
                body: Some(Some("New body".to_string())),
                card_type: None,
                refs: None,
            },
        ));

        let card = &state.cards[&card_id];
        assert_eq!(card.title, "Renamed Card");
        assert_eq!(card.body, Some("New body".to_string()));
        assert_eq!(card.card_type, "idea"); // unchanged
    }

    #[test]
    fn apply_card_moved_changes_lane_and_order() {
        let mut state = SpecState::new();
        let spec_id = make_spec_id();
        let card = Card::new(
            "task".to_string(),
            "Move Me".to_string(),
            "human".to_string(),
        );
        let card_id = card.card_id;

        state.apply(&make_event(1, spec_id, EventPayload::CardCreated { card }));

        state.apply(&make_event(
            2,
            spec_id,
            EventPayload::CardMoved {
                card_id,
                lane: "Spec".to_string(),
                order: 3.5,
            },
        ));

        let card = &state.cards[&card_id];
        assert_eq!(card.lane, "Spec");
        assert_eq!(card.order, 3.5);
    }

    #[test]
    fn apply_card_deleted_removes_card() {
        let mut state = SpecState::new();
        let spec_id = make_spec_id();
        let card = Card::new(
            "idea".to_string(),
            "Delete Me".to_string(),
            "human".to_string(),
        );
        let card_id = card.card_id;

        state.apply(&make_event(1, spec_id, EventPayload::CardCreated { card }));
        assert_eq!(state.cards.len(), 1);

        state.apply(&make_event(
            2,
            spec_id,
            EventPayload::CardDeleted { card_id },
        ));
        assert_eq!(state.cards.len(), 0);
        assert!(!state.cards.contains_key(&card_id));
    }

    #[test]
    fn apply_question_asked_sets_pending() {
        let mut state = SpecState::new();
        let spec_id = make_spec_id();
        let question = UserQuestion::Boolean {
            question_id: Ulid::new(),
            question: "Continue?".to_string(),
            default: Some(true),
        };

        state.apply(&make_event(
            1,
            spec_id,
            EventPayload::QuestionAsked {
                question: question.clone(),
            },
        ));

        assert!(state.pending_question.is_some());
    }

    #[test]
    fn apply_question_answered_clears_pending() {
        let mut state = SpecState::new();
        let spec_id = make_spec_id();
        let q_id = Ulid::new();
        let question = UserQuestion::Boolean {
            question_id: q_id,
            question: "Continue?".to_string(),
            default: None,
        };

        state.apply(&make_event(
            1,
            spec_id,
            EventPayload::QuestionAsked { question },
        ));
        assert!(state.pending_question.is_some());

        state.apply(&make_event(
            2,
            spec_id,
            EventPayload::QuestionAnswered {
                question_id: q_id,
                answer: "Yes".to_string(),
            },
        ));
        assert!(state.pending_question.is_none());
        // The answer should be in transcript
        assert_eq!(state.transcript.len(), 1);
        assert_eq!(state.transcript[0].content, "Yes");
    }

    #[test]
    fn undo_entry_created_on_card_mutation() {
        let mut state = SpecState::new();
        let spec_id = make_spec_id();
        let card = Card::new("idea".to_string(), "Test".to_string(), "human".to_string());
        let card_id = card.card_id;

        // CardCreated should push an undo entry
        state.apply(&make_event(1, spec_id, EventPayload::CardCreated { card }));
        assert_eq!(state.undo_stack.len(), 1);
        assert_eq!(state.undo_stack[0].event_id, 1);

        // CardUpdated should push another undo entry
        state.apply(&make_event(
            2,
            spec_id,
            EventPayload::CardUpdated {
                card_id,
                title: Some("Renamed".to_string()),
                body: None,
                card_type: None,
                refs: None,
            },
        ));
        assert_eq!(state.undo_stack.len(), 2);
        assert_eq!(state.undo_stack[1].event_id, 2);

        // CardMoved should push another undo entry
        state.apply(&make_event(
            3,
            spec_id,
            EventPayload::CardMoved {
                card_id,
                lane: "Plan".to_string(),
                order: 1.0,
            },
        ));
        assert_eq!(state.undo_stack.len(), 3);

        // CardDeleted should push another undo entry
        state.apply(&make_event(
            4,
            spec_id,
            EventPayload::CardDeleted { card_id },
        ));
        assert_eq!(state.undo_stack.len(), 4);
    }

    #[test]
    fn undo_applied_pops_undo_stack() {
        let mut state = SpecState::new();
        let spec_id = make_spec_id();

        // Create a card (pushes 1 undo entry)
        let card = Card::new(
            "idea".to_string(),
            "Undo Test".to_string(),
            "human".to_string(),
        );
        let card_id = card.card_id;
        state.apply(&make_event(1, spec_id, EventPayload::CardCreated { card }));
        assert_eq!(
            state.undo_stack.len(),
            1,
            "undo_stack should have 1 entry after card creation"
        );

        // Apply UndoApplied (should apply inverse and pop the entry)
        state.apply(&make_event(
            2,
            spec_id,
            EventPayload::UndoApplied {
                target_event_id: 1,
                inverse_events: vec![EventPayload::CardDeleted { card_id }],
            },
        ));

        assert_eq!(state.cards.len(), 0, "card should be removed after undo");
        assert_eq!(
            state.undo_stack.len(),
            0,
            "undo_stack should be empty after UndoApplied"
        );
    }

    #[test]
    fn apply_agent_step_started_sets_step_started_kind() {
        let mut state = SpecState::new();
        let spec_id = make_spec_id();
        state.apply(&make_event(
            1,
            spec_id,
            EventPayload::AgentStepStarted {
                agent_id: "manager-01HTEST".to_string(),
                description: "Manager reasoning step".to_string(),
            },
        ));
        assert_eq!(state.transcript.len(), 1);
        assert_eq!(
            state.transcript[0].kind,
            crate::transcript::MessageKind::StepStarted
        );
        assert_eq!(state.transcript[0].content, "Manager reasoning step");
        assert!(!state.transcript[0].content.contains("[step started]"));
    }

    #[test]
    fn apply_agent_step_finished_sets_step_finished_kind() {
        let mut state = SpecState::new();
        let spec_id = make_spec_id();
        state.apply(&make_event(
            1,
            spec_id,
            EventPayload::AgentStepFinished {
                agent_id: "manager-01HTEST".to_string(),
                diff_summary: "Updated goal and added 3 cards".to_string(),
            },
        ));
        assert_eq!(state.transcript.len(), 1);
        assert_eq!(
            state.transcript[0].kind,
            crate::transcript::MessageKind::StepFinished
        );
        assert_eq!(
            state.transcript[0].content,
            "Updated goal and added 3 cards"
        );
        assert!(!state.transcript[0].content.contains("[step finished]"));
    }

    #[test]
    fn apply_multiple_events_builds_full_state() {
        let mut state = SpecState::new();
        let spec_id = make_spec_id();

        // Create a spec
        state.apply(&make_event(
            1,
            spec_id,
            EventPayload::SpecCreated {
                title: "Full Spec".to_string(),
                one_liner: "Complete test".to_string(),
                goal: "Verify full state build".to_string(),
            },
        ));

        // Add two cards
        let card_a = Card::new(
            "idea".to_string(),
            "Card A".to_string(),
            "human".to_string(),
        );
        let card_b = Card::new(
            "task".to_string(),
            "Card B".to_string(),
            "agent-1".to_string(),
        );
        let card_a_id = card_a.card_id;

        state.apply(&make_event(
            2,
            spec_id,
            EventPayload::CardCreated { card: card_a },
        ));
        state.apply(&make_event(
            3,
            spec_id,
            EventPayload::CardCreated { card: card_b },
        ));

        // Move card A
        state.apply(&make_event(
            4,
            spec_id,
            EventPayload::CardMoved {
                card_id: card_a_id,
                lane: "Plan".to_string(),
                order: 1.0,
            },
        ));

        // Append transcript
        let msg = TranscriptMessage::new("system".to_string(), "Spec initialized".to_string());
        state.apply(&make_event(
            5,
            spec_id,
            EventPayload::TranscriptAppended { message: msg },
        ));

        // Verify full state
        assert!(state.core.is_some());
        assert_eq!(state.core.as_ref().unwrap().title, "Full Spec");
        assert_eq!(state.cards.len(), 2);
        assert_eq!(state.cards[&card_a_id].lane, "Plan");
        assert_eq!(state.transcript.len(), 1);
        assert_eq!(state.last_event_id, 5);
        assert_eq!(state.undo_stack.len(), 3); // 2 creates + 1 move
        assert_eq!(
            state.lanes,
            vec!["Ideas".to_string(), "Plan".to_string(), "Spec".to_string()]
        );
    }

    #[test]
    fn spec_phase_serde_brainstorming() {
        let phase = SpecPhase::Brainstorming;
        let json = serde_json::to_string(&phase).unwrap();
        assert_eq!(json, "\"Brainstorming\"");
        let back: SpecPhase = serde_json::from_str(&json).unwrap();
        assert_eq!(back, SpecPhase::Brainstorming);
    }

    #[test]
    fn spec_phase_serde_refining() {
        let phase = SpecPhase::Refining;
        let json = serde_json::to_string(&phase).unwrap();
        assert_eq!(json, "\"Refining\"");
        let back: SpecPhase = serde_json::from_str(&json).unwrap();
        assert_eq!(back, SpecPhase::Refining);
    }

    #[test]
    fn spec_phase_serde_active_alias_deserializes_as_refining() {
        // Backwards compat: persisted events with "Active" should deserialize as Refining
        let back: SpecPhase = serde_json::from_str("\"Active\"").unwrap();
        assert_eq!(back, SpecPhase::Refining);
    }

    #[test]
    fn spec_state_new_defaults_to_refining() {
        let state = SpecState::new();
        assert_eq!(state.phase, SpecPhase::Refining);
    }

    #[test]
    fn phase_transitioned_updates_state() {
        let mut state = SpecState::new();
        assert_eq!(state.phase, SpecPhase::Refining);

        let event = Event {
            event_id: 1,
            spec_id: Ulid::new(),
            timestamp: Utc::now(),
            payload: EventPayload::PhaseTransitioned {
                phase: SpecPhase::Brainstorming,
            },
        };
        state.apply(&event);
        assert_eq!(state.phase, SpecPhase::Brainstorming);
    }

    #[test]
    fn phase_transitioned_does_not_push_undo() {
        let mut state = SpecState::new();
        let event = Event {
            event_id: 1,
            spec_id: Ulid::new(),
            timestamp: Utc::now(),
            payload: EventPayload::PhaseTransitioned {
                phase: SpecPhase::Brainstorming,
            },
        };
        state.apply(&event);
        assert!(state.undo_stack.is_empty());
    }

    #[test]
    fn canvas_updated_sets_content() {
        let mut state = SpecState::new();
        let event = Event {
            event_id: 1,
            spec_id: Ulid::new(),
            timestamp: Utc::now(),
            payload: EventPayload::CanvasUpdated {
                content: "<h1>Hello</h1>".to_string(),
            },
        };
        state.apply(&event);
        assert_eq!(state.canvas_content, Some("<h1>Hello</h1>".to_string()));
    }

    #[test]
    fn canvas_updated_empty_clears_content() {
        let mut state = SpecState::new();
        state.canvas_content = Some("old".to_string());
        let event = Event {
            event_id: 1,
            spec_id: Ulid::new(),
            timestamp: Utc::now(),
            payload: EventPayload::CanvasUpdated {
                content: String::new(),
            },
        };
        state.apply(&event);
        assert_eq!(state.canvas_content, None);
    }

    #[test]
    fn canvas_updated_does_not_push_undo() {
        let mut state = SpecState::new();
        let event = Event {
            event_id: 1,
            spec_id: Ulid::new(),
            timestamp: Utc::now(),
            payload: EventPayload::CanvasUpdated {
                content: "html".to_string(),
            },
        };
        state.apply(&event);
        assert!(state.undo_stack.is_empty());
    }

    #[test]
    fn undo_applied_clears_canvas_content() {
        let mut state = SpecState::new();
        state.canvas_content = Some("stale diagram".to_string());
        let event = Event {
            event_id: 2,
            spec_id: Ulid::new(),
            timestamp: Utc::now(),
            payload: EventPayload::UndoApplied {
                target_event_id: 1,
                inverse_events: vec![],
            },
        };
        state.apply(&event);
        assert_eq!(state.canvas_content, None);
    }

    #[test]
    fn canvas_content_serde_round_trip() {
        let mut state = SpecState::new();
        state.canvas_content = Some("<div>test</div>".to_string());
        let json = serde_json::to_string(&state).unwrap();
        let back: SpecState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.canvas_content, Some("<div>test</div>".to_string()));
    }

    #[test]
    fn snapshot_without_canvas_content_deserializes_as_none() {
        let json = serde_json::to_string(&SpecState::new()).unwrap();
        let back: SpecState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.canvas_content, None);
    }

    #[test]
    fn apply_streaming_delta_is_noop() {
        let mut state = SpecState::new();
        let spec_id = make_spec_id();
        let before_cards = state.cards.len();
        let before_transcript = state.transcript.len();
        state.apply(&make_event(
            1,
            spec_id,
            EventPayload::StreamingDelta {
                agent_id: "manager-1".to_string(),
                text: "Hello".to_string(),
            },
        ));
        assert_eq!(state.cards.len(), before_cards);
        assert_eq!(state.transcript.len(), before_transcript);
        assert!(state.core.is_none());
    }

    #[test]
    fn spec_phase_serde_complete() {
        let phase = SpecPhase::Complete;
        let json = serde_json::to_string(&phase).unwrap();
        assert_eq!(json, "\"Complete\"");
        let back: SpecPhase = serde_json::from_str(&json).unwrap();
        assert_eq!(back, SpecPhase::Complete);
    }

    #[test]
    fn snapshot_without_phase_deserializes_as_refining() {
        // Simulate an old snapshot JSON without a "phase" field
        let json = r#"{"core":null,"cards":{},"transcript":[],"pending_question":null,"undo_stack":[],"last_event_id":0,"lanes":["Ideas","Plan","Spec"]}"#;
        let state: SpecState = serde_json::from_str(json).unwrap();
        assert_eq!(state.phase, SpecPhase::Refining);
    }

    #[test]
    fn apply_context_attached_adds_attachment() {
        let mut state = SpecState::new();
        let attachment_id = Ulid::new();
        let event = make_event(
            1,
            make_spec_id(),
            EventPayload::ContextAttached {
                attachment: ContextAttachment {
                    attachment_id,
                    filename: "a.md".to_string(),
                    mime_type: "text/markdown".to_string(),
                    size_bytes: 42,
                    summary: None,
                    user_notes: None,
                    added_at: Utc::now(),
                    removed: false,
                },
            },
        );
        state.apply(&event);
        assert_eq!(state.context_attachments.len(), 1);
        assert_eq!(state.context_attachments[0].attachment_id, attachment_id);
    }

    #[test]
    fn apply_context_summarized_updates_summary() {
        let mut state = SpecState::new();
        let attachment_id = Ulid::new();
        state.apply(&make_event(
            1,
            make_spec_id(),
            EventPayload::ContextAttached {
                attachment: ContextAttachment {
                    attachment_id,
                    filename: "a".into(),
                    mime_type: "text/plain".into(),
                    size_bytes: 1,
                    summary: None,
                    user_notes: None,
                    added_at: Utc::now(),
                    removed: false,
                },
            },
        ));
        state.apply(&make_event(
            2,
            make_spec_id(),
            EventPayload::ContextSummarized {
                attachment_id,
                summary: "brief".into(),
            },
        ));
        assert_eq!(
            state.context_attachments[0].summary.as_deref(),
            Some("brief")
        );
    }

    #[test]
    fn apply_context_notes_updated_sets_notes() {
        let mut state = SpecState::new();
        let attachment_id = Ulid::new();
        state.apply(&make_event(
            1,
            make_spec_id(),
            EventPayload::ContextAttached {
                attachment: ContextAttachment {
                    attachment_id,
                    filename: "a".into(),
                    mime_type: "text/plain".into(),
                    size_bytes: 1,
                    summary: None,
                    user_notes: None,
                    added_at: Utc::now(),
                    removed: false,
                },
            },
        ));
        state.apply(&make_event(
            2,
            make_spec_id(),
            EventPayload::ContextNotesUpdated {
                attachment_id,
                notes: "my note".into(),
            },
        ));
        assert_eq!(
            state.context_attachments[0].user_notes.as_deref(),
            Some("my note")
        );
    }

    #[test]
    fn apply_context_removed_marks_removed() {
        let mut state = SpecState::new();
        let attachment_id = Ulid::new();
        state.apply(&make_event(
            1,
            make_spec_id(),
            EventPayload::ContextAttached {
                attachment: ContextAttachment {
                    attachment_id,
                    filename: "a".into(),
                    mime_type: "text/plain".into(),
                    size_bytes: 1,
                    summary: None,
                    user_notes: None,
                    added_at: Utc::now(),
                    removed: false,
                },
            },
        ));
        state.apply(&make_event(
            2,
            make_spec_id(),
            EventPayload::ContextRemoved { attachment_id },
        ));
        assert!(state.context_attachments[0].removed);
    }

    #[test]
    fn undo_context_attached_marks_removed() {
        let mut state = SpecState::new();
        let attachment_id = Ulid::new();
        state.apply(&make_event(
            1,
            make_spec_id(),
            EventPayload::ContextAttached {
                attachment: ContextAttachment {
                    attachment_id,
                    filename: "a".into(),
                    mime_type: "text/plain".into(),
                    size_bytes: 1,
                    summary: None,
                    user_notes: None,
                    added_at: Utc::now(),
                    removed: false,
                },
            },
        ));
        // Simulate undo by applying UndoApplied with the inverse the attach event produced.
        let top = state.undo_stack.last().expect("undo entry pushed");
        let inverse = top.inverse.clone();
        state.apply(&make_event(
            2,
            make_spec_id(),
            EventPayload::UndoApplied {
                target_event_id: 1,
                inverse_events: inverse,
            },
        ));
        assert!(state.context_attachments[0].removed);
    }

    #[test]
    fn undo_context_removed_restores_attachment() {
        let mut state = SpecState::new();
        let attachment_id = Ulid::new();
        state.apply(&make_event(
            1,
            make_spec_id(),
            EventPayload::ContextAttached {
                attachment: ContextAttachment {
                    attachment_id,
                    filename: "a".into(),
                    mime_type: "text/plain".into(),
                    size_bytes: 1,
                    summary: None,
                    user_notes: None,
                    added_at: Utc::now(),
                    removed: false,
                },
            },
        ));
        state.apply(&make_event(
            2,
            make_spec_id(),
            EventPayload::ContextRemoved { attachment_id },
        ));
        assert!(state.context_attachments[0].removed);

        let inverse = state.undo_stack.last().unwrap().inverse.clone();
        state.apply(&make_event(
            3,
            make_spec_id(),
            EventPayload::UndoApplied {
                target_event_id: 2,
                inverse_events: inverse,
            },
        ));

        assert_eq!(state.context_attachments.len(), 1, "no duplicate entry");
        assert!(
            !state.context_attachments[0].removed,
            "removed flag cleared"
        );
    }

    #[test]
    fn context_summarized_does_not_push_undo() {
        let mut state = SpecState::new();
        let attachment_id = Ulid::new();
        state.apply(&make_event(
            1,
            make_spec_id(),
            EventPayload::ContextAttached {
                attachment: ContextAttachment {
                    attachment_id,
                    filename: "a".into(),
                    mime_type: "text/plain".into(),
                    size_bytes: 1,
                    summary: None,
                    user_notes: None,
                    added_at: Utc::now(),
                    removed: false,
                },
            },
        ));
        let undo_len_after_attach = state.undo_stack.len();
        state.apply(&make_event(
            2,
            make_spec_id(),
            EventPayload::ContextSummarized {
                attachment_id,
                summary: "brief".into(),
            },
        ));
        assert_eq!(
            state.undo_stack.len(),
            undo_len_after_attach,
            "summarization should not push an undo entry"
        );
    }

    #[test]
    fn undo_context_notes_updated_restores_none_when_prior_was_none() {
        let mut state = SpecState::new();
        let attachment_id = Ulid::new();
        // Attach (no notes)
        state.apply(&make_event(
            1,
            make_spec_id(),
            EventPayload::ContextAttached {
                attachment: ContextAttachment {
                    attachment_id,
                    filename: "a".into(),
                    mime_type: "text/plain".into(),
                    size_bytes: 1,
                    summary: None,
                    user_notes: None,
                    added_at: Utc::now(),
                    removed: false,
                },
            },
        ));
        // Add notes
        state.apply(&make_event(
            2,
            make_spec_id(),
            EventPayload::ContextNotesUpdated {
                attachment_id,
                notes: "hello".into(),
            },
        ));
        assert_eq!(
            state.context_attachments[0].user_notes.as_deref(),
            Some("hello")
        );

        // Undo
        let inverse = state.undo_stack.last().unwrap().inverse.clone();
        state.apply(&make_event(
            3,
            make_spec_id(),
            EventPayload::UndoApplied {
                target_event_id: 2,
                inverse_events: inverse,
            },
        ));

        // Prior was None — should be restored to None, not Some("")
        assert_eq!(state.context_attachments[0].user_notes, None);
    }
}
