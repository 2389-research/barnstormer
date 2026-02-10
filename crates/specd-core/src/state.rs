// ABOUTME: Defines SpecState and UndoEntry for building spec state from an event stream.
// ABOUTME: The apply() method pattern-matches on EventPayload to fold events into current state.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::card::Card;
use crate::event::{Event, EventPayload};
use crate::model::SpecCore;
use crate::transcript::{TranscriptMessage, UserQuestion};

/// Stores the inverse operations needed to undo a mutation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoEntry {
    pub event_id: u64,
    pub inverse: Vec<EventPayload>,
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
            lanes: vec![
                "Ideas".to_string(),
                "Plan".to_string(),
                "Done".to_string(),
            ],
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
                self.transcript.push(TranscriptMessage {
                    message_id: *question_id,
                    sender: "human".to_string(),
                    content: answer.clone(),
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
                    content: format!("[step started] {}", description),
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
                    content: format!("[step finished] {}", diff_summary),
                    timestamp: event.timestamp,
                });
            }

            EventPayload::UndoApplied {
                inverse_events, ..
            } => {
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
            }

            EventPayload::SnapshotWritten { .. } => {
                // No-op on state
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

        state.apply(&make_event(
            1,
            spec_id,
            EventPayload::CardCreated { card },
        ));

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

        state.apply(&make_event(
            1,
            spec_id,
            EventPayload::CardCreated { card },
        ));

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

        state.apply(&make_event(
            1,
            spec_id,
            EventPayload::CardCreated { card },
        ));

        state.apply(&make_event(
            2,
            spec_id,
            EventPayload::CardMoved {
                card_id,
                lane: "Done".to_string(),
                order: 3.5,
            },
        ));

        let card = &state.cards[&card_id];
        assert_eq!(card.lane, "Done");
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

        state.apply(&make_event(
            1,
            spec_id,
            EventPayload::CardCreated { card },
        ));
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
        let card = Card::new(
            "idea".to_string(),
            "Test".to_string(),
            "human".to_string(),
        );
        let card_id = card.card_id;

        // CardCreated should push an undo entry
        state.apply(&make_event(
            1,
            spec_id,
            EventPayload::CardCreated { card },
        ));
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
            vec!["Ideas".to_string(), "Plan".to_string(), "Done".to_string()]
        );
    }
}
