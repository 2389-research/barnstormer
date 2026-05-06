// ABOUTME: Async actor for processing spec commands and publishing events via tokio channels.
// ABOUTME: Provides SpecActorHandle for sending commands, subscribing to events, and reading state.

use std::sync::Arc;

use chrono::Utc;
use thiserror::Error;
use tokio::sync::{RwLock, broadcast, mpsc, oneshot};
use ulid::Ulid;

use crate::card::Card;
use crate::command::Command;
use crate::event::{Event, EventPayload};
use crate::state::{ContextAttachment, SpecPhase, SpecState};
use crate::transcript::TranscriptMessage;

/// Errors that can occur when processing commands in the actor.
#[derive(Debug, Error)]
pub enum ActorError {
    #[error("spec not yet created")]
    SpecNotCreated,

    #[error("card not found: {0}")]
    CardNotFound(Ulid),

    #[error("attachment not found: {0}")]
    AttachmentNotFound(Ulid),

    #[error("attachment already removed: {0}")]
    AttachmentAlreadyRemoved(Ulid),

    #[error("attachment already exists: {0}")]
    AttachmentAlreadyExists(Ulid),

    #[error("a question is already pending")]
    QuestionAlreadyPending,

    #[error("no pending question to answer")]
    NoPendingQuestion,

    #[error("question id mismatch: expected {expected}, got {got}")]
    QuestionIdMismatch { expected: Ulid, got: Ulid },

    #[error("nothing to undo")]
    NothingToUndo,

    #[error("already in target phase")]
    AlreadyInPhase,

    #[error("actor channel closed")]
    ChannelClosed,
}

/// Message type sent through the command channel: a command paired with
/// a oneshot sender for the response.
type CommandMessage = (Command, oneshot::Sender<Result<Vec<Event>, ActorError>>);

/// Public handle for interacting with a SpecActor. Supports sending commands,
/// subscribing to events, and reading the current state.
#[derive(Clone)]
pub struct SpecActorHandle {
    cmd_tx: mpsc::Sender<CommandMessage>,
    event_tx: broadcast::Sender<Event>,
    state: Arc<RwLock<SpecState>>,
    pub spec_id: Ulid,
}

impl SpecActorHandle {
    /// Send a command to the actor and await the resulting events.
    pub async fn send_command(&self, cmd: Command) -> Result<Vec<Event>, ActorError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send((cmd, tx))
            .await
            .map_err(|_| ActorError::ChannelClosed)?;
        rx.await.map_err(|_| ActorError::ChannelClosed)?
    }

    /// Subscribe to the event broadcast stream.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.event_tx.subscribe()
    }

    /// Get a read-only reference to the shared state.
    pub async fn read_state(&self) -> tokio::sync::RwLockReadGuard<'_, SpecState> {
        self.state.read().await
    }
}

/// Spawn a new SpecActor task and return the handle for interacting with it.
/// The actor processes commands sequentially, converts them to events,
/// applies them to state, and broadcasts them to subscribers.
pub fn spawn(spec_id: Ulid, initial_state: SpecState) -> SpecActorHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel::<CommandMessage>(64);
    let (event_tx, _) = broadcast::channel::<Event>(4096);
    let last_event_id = initial_state.last_event_id;
    let state = Arc::new(RwLock::new(initial_state));

    let handle = SpecActorHandle {
        cmd_tx,
        event_tx: event_tx.clone(),
        state: Arc::clone(&state),
        spec_id,
    };

    let actor = SpecActor {
        state,
        cmd_rx,
        event_tx,
        next_event_id: last_event_id + 1,
        spec_id,
    };

    tokio::spawn(actor.run());

    handle
}

/// The internal actor that processes commands in a loop.
struct SpecActor {
    state: Arc<RwLock<SpecState>>,
    cmd_rx: mpsc::Receiver<CommandMessage>,
    event_tx: broadcast::Sender<Event>,
    next_event_id: u64,
    spec_id: Ulid,
}

impl SpecActor {
    async fn run(mut self) {
        while let Some((cmd, reply_tx)) = self.cmd_rx.recv().await {
            let result = self.process_command(cmd).await;
            // Ignore send error — the caller may have dropped their receiver
            let _ = reply_tx.send(result);
        }
    }

    async fn process_command(&mut self, cmd: Command) -> Result<Vec<Event>, ActorError> {
        let events = self.command_to_events(cmd).await?;

        // Apply events to state under write lock
        {
            let mut state = self.state.write().await;
            for event in &events {
                state.apply(event);
            }
        }

        // Broadcast events to subscribers
        for event in &events {
            // Ignore broadcast errors (no active subscribers is fine)
            let _ = self.event_tx.send(event.clone());
        }

        Ok(events)
    }

    /// Convert a command into one or more events, performing validation
    /// against the current state.
    async fn command_to_events(&mut self, cmd: Command) -> Result<Vec<Event>, ActorError> {
        let state = self.state.read().await;

        let payloads = match cmd {
            Command::CreateSpec {
                title,
                one_liner,
                goal,
            } => {
                vec![
                    EventPayload::SpecCreated {
                        title,
                        one_liner,
                        goal,
                    },
                    EventPayload::PhaseTransitioned {
                        phase: SpecPhase::Brainstorming,
                    },
                ]
            }

            Command::UpdateSpecCore {
                title,
                one_liner,
                goal,
                description,
                constraints,
                success_criteria,
                risks,
                notes,
            } => {
                if state.core.is_none() {
                    return Err(ActorError::SpecNotCreated);
                }
                vec![EventPayload::SpecCoreUpdated {
                    title,
                    one_liner,
                    goal,
                    description,
                    constraints,
                    success_criteria,
                    risks,
                    notes,
                }]
            }

            Command::CreateCard {
                card_type,
                title,
                body,
                lane,
                created_by,
                source_attachment_id,
            } => {
                // If the card claims to come from an attachment, that
                // attachment must exist and not be tombstoned. Rejecting
                // here prevents dangling provenance links if the Manager
                // invents or misremembers an ID.
                if let Some(att_id) = source_attachment_id {
                    let att = state
                        .context_attachments
                        .iter()
                        .find(|a| a.attachment_id == att_id);
                    match att {
                        None => return Err(ActorError::AttachmentNotFound(att_id)),
                        Some(a) if a.removed => {
                            return Err(ActorError::AttachmentNotFound(att_id));
                        }
                        _ => {}
                    }
                }
                let now = Utc::now();
                let card = Card {
                    card_id: Ulid::new(),
                    card_type,
                    title,
                    body,
                    lane: lane.unwrap_or_else(|| "Ideas".to_string()),
                    order: 0.0,
                    refs: Vec::new(),
                    created_at: now,
                    updated_at: now,
                    created_by: created_by.clone(),
                    updated_by: created_by,
                    source_attachment_id,
                };
                vec![EventPayload::CardCreated { card }]
            }

            Command::UpdateCard {
                card_id,
                title,
                body,
                card_type,
                refs,
                updated_by: _,
            } => {
                if !state.cards.contains_key(&card_id) {
                    return Err(ActorError::CardNotFound(card_id));
                }
                vec![EventPayload::CardUpdated {
                    card_id,
                    title,
                    body,
                    card_type,
                    refs,
                }]
            }

            Command::MoveCard {
                card_id,
                lane,
                order,
                updated_by: _,
            } => {
                if !state.cards.contains_key(&card_id) {
                    return Err(ActorError::CardNotFound(card_id));
                }
                vec![EventPayload::CardMoved {
                    card_id,
                    lane,
                    order,
                }]
            }

            Command::DeleteCard {
                card_id,
                updated_by: _,
            } => {
                if !state.cards.contains_key(&card_id) {
                    return Err(ActorError::CardNotFound(card_id));
                }
                vec![EventPayload::CardDeleted { card_id }]
            }

            Command::AppendTranscript { sender, content } => {
                let message = TranscriptMessage::new(sender, content);
                vec![EventPayload::TranscriptAppended { message }]
            }

            Command::AskQuestion { question } => {
                if state.pending_question.is_some() {
                    return Err(ActorError::QuestionAlreadyPending);
                }
                vec![EventPayload::QuestionAsked { question }]
            }

            Command::AnswerQuestion {
                question_id,
                answer,
            } => {
                match &state.pending_question {
                    None => return Err(ActorError::NoPendingQuestion),
                    Some(q) => {
                        let pending_id = question_id_of(q);
                        if pending_id != question_id {
                            return Err(ActorError::QuestionIdMismatch {
                                expected: pending_id,
                                got: question_id,
                            });
                        }
                    }
                }
                vec![EventPayload::QuestionAnswered {
                    question_id,
                    answer,
                }]
            }

            Command::StartAgentStep {
                agent_id,
                description,
            } => {
                vec![EventPayload::AgentStepStarted {
                    agent_id,
                    description,
                }]
            }

            Command::FinishAgentStep {
                agent_id,
                diff_summary,
            } => {
                vec![EventPayload::AgentStepFinished {
                    agent_id,
                    diff_summary,
                }]
            }

            Command::TransitionPhase { target } => {
                if state.phase == target {
                    return Err(ActorError::AlreadyInPhase);
                }
                vec![EventPayload::PhaseTransitioned { phase: target }]
            }

            Command::UpdateCanvas { content } => {
                vec![EventPayload::CanvasUpdated { content }]
            }

            Command::AttachContext {
                attachment_id,
                filename,
                mime_type,
                size_bytes,
            } => {
                if state.core.is_none() {
                    return Err(ActorError::SpecNotCreated);
                }
                // Reject duplicate IDs up front. The follow-up commands
                // (Summarize/UpdateNotes/Remove/CreateCard with source) all use
                // `find(...)` over context_attachments, which would only ever
                // hit the first entry — leaving any duplicate orphaned in
                // state and on disk.
                if state
                    .context_attachments
                    .iter()
                    .any(|a| a.attachment_id == attachment_id)
                {
                    return Err(ActorError::AttachmentAlreadyExists(attachment_id));
                }
                let attachment = ContextAttachment {
                    attachment_id,
                    filename,
                    mime_type,
                    size_bytes,
                    summary: None,
                    user_notes: None,
                    added_at: Utc::now(),
                    removed: false,
                };
                vec![EventPayload::ContextAttached { attachment }]
            }

            Command::SummarizeContext {
                attachment_id,
                summary,
            } => {
                let Some(att) = state
                    .context_attachments
                    .iter()
                    .find(|a| a.attachment_id == attachment_id)
                else {
                    return Err(ActorError::AttachmentNotFound(attachment_id));
                };
                if att.removed {
                    return Err(ActorError::AttachmentAlreadyRemoved(attachment_id));
                }
                vec![EventPayload::ContextSummarized {
                    attachment_id,
                    summary,
                }]
            }

            Command::UpdateContextNotes {
                attachment_id,
                notes,
            } => {
                let Some(att) = state
                    .context_attachments
                    .iter()
                    .find(|a| a.attachment_id == attachment_id)
                else {
                    return Err(ActorError::AttachmentNotFound(attachment_id));
                };
                if att.removed {
                    return Err(ActorError::AttachmentAlreadyRemoved(attachment_id));
                }
                vec![EventPayload::ContextNotesUpdated {
                    attachment_id,
                    notes,
                }]
            }

            Command::RemoveContext { attachment_id } => {
                let Some(att) = state
                    .context_attachments
                    .iter()
                    .find(|a| a.attachment_id == attachment_id)
                else {
                    return Err(ActorError::AttachmentNotFound(attachment_id));
                };
                if att.removed {
                    return Err(ActorError::AttachmentAlreadyRemoved(attachment_id));
                }
                vec![EventPayload::ContextRemoved { attachment_id }]
            }

            Command::StreamDelta { agent_id, text } => {
                vec![EventPayload::StreamingDelta { agent_id, text }]
            }

            Command::StreamToolActivity { agent_id, activity } => {
                vec![EventPayload::StreamingToolActivity { agent_id, activity }]
            }

            Command::Undo => {
                if state.undo_stack.is_empty() {
                    return Err(ActorError::NothingToUndo);
                }
                let entry = state.undo_stack.last().unwrap();
                let target_event_id = entry.event_id;
                let inverse_events = entry.inverse.clone();
                vec![EventPayload::UndoApplied {
                    target_event_id,
                    inverse_events,
                }]
            }
        };

        // Drop the read lock before creating events
        drop(state);

        let now = Utc::now();
        let events = payloads
            .into_iter()
            .map(|payload| {
                // Ephemeral events (streaming deltas, tool activity) get event_id 0
                // and do not consume a monotonic ID. This avoids gaps in the
                // persisted JSONL log since ephemeral events are never written.
                let event_id = if payload.is_ephemeral() {
                    0
                } else {
                    let id = self.next_event_id;
                    self.next_event_id += 1;
                    id
                };
                Event {
                    event_id,
                    spec_id: self.spec_id,
                    timestamp: now,
                    payload,
                }
            })
            .collect();

        Ok(events)
    }
}

/// Extract the question_id from any UserQuestion variant.
fn question_id_of(q: &crate::transcript::UserQuestion) -> Ulid {
    match q {
        crate::transcript::UserQuestion::Boolean { question_id, .. } => *question_id,
        crate::transcript::UserQuestion::MultipleChoice { question_id, .. } => *question_id,
        crate::transcript::UserQuestion::Freeform { question_id, .. } => *question_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::SpecPhase;
    use crate::transcript::UserQuestion;

    #[tokio::test]
    async fn actor_processes_create_spec() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());

        let events = handle
            .send_command(Command::CreateSpec {
                title: "Test Spec".to_string(),
                one_liner: "A test".to_string(),
                goal: "Verify actor".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_id, 1);
        assert_eq!(events[0].spec_id, spec_id);
        match &events[0].payload {
            EventPayload::SpecCreated { .. } => {}
            _ => panic!("first event should be SpecCreated"),
        }
        match &events[1].payload {
            EventPayload::PhaseTransitioned { phase } => {
                assert_eq!(*phase, SpecPhase::Brainstorming);
            }
            _ => panic!("second event should be PhaseTransitioned"),
        }

        let state = handle.read_state().await;
        let core = state.core.as_ref().expect("spec should be created");
        assert_eq!(core.title, "Test Spec");
    }

    #[tokio::test]
    async fn actor_processes_create_card() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());

        // Create spec first
        handle
            .send_command(Command::CreateSpec {
                title: "Spec".to_string(),
                one_liner: "One".to_string(),
                goal: "Goal".to_string(),
            })
            .await
            .unwrap();

        // Create a card
        let events = handle
            .send_command(Command::CreateCard {
                card_type: "idea".to_string(),
                title: "My Card".to_string(),
                body: None,
                lane: None,
                created_by: "human".to_string(),
                source_attachment_id: None,
            })
            .await
            .unwrap();

        assert_eq!(events.len(), 1);
        match &events[0].payload {
            EventPayload::CardCreated { card } => {
                assert_eq!(card.title, "My Card");
                assert_eq!(card.lane, "Ideas");
                assert!(card.source_attachment_id.is_none());
            }
            _ => panic!("expected CardCreated event"),
        }

        let state = handle.read_state().await;
        assert_eq!(state.cards.len(), 1);
    }

    #[tokio::test]
    async fn actor_accepts_create_card_with_valid_source_attachment_id() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());

        handle
            .send_command(Command::CreateSpec {
                title: "s".into(),
                one_liner: "o".into(),
                goal: "g".into(),
            })
            .await
            .unwrap();

        let att_id = Ulid::new();
        handle
            .send_command(Command::AttachContext {
                attachment_id: att_id,
                filename: "vibes.md".into(),
                mime_type: "text/markdown".into(),
                size_bytes: 128,
            })
            .await
            .unwrap();

        let events = handle
            .send_command(Command::CreateCard {
                card_type: "constraint".to_string(),
                title: "Quiet competence".to_string(),
                body: Some("From vibes.md".to_string()),
                lane: None,
                created_by: "manager-1".to_string(),
                source_attachment_id: Some(att_id),
            })
            .await
            .unwrap();

        assert_eq!(events.len(), 1);
        match &events[0].payload {
            EventPayload::CardCreated { card } => {
                assert_eq!(card.source_attachment_id, Some(att_id));
            }
            _ => panic!("expected CardCreated"),
        }
    }

    #[tokio::test]
    async fn actor_rejects_create_card_with_unknown_source_attachment_id() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "s".into(),
                one_liner: "o".into(),
                goal: "g".into(),
            })
            .await
            .unwrap();

        let bogus = Ulid::new();
        let result = handle
            .send_command(Command::CreateCard {
                card_type: "idea".to_string(),
                title: "Ghost source".to_string(),
                body: None,
                lane: None,
                created_by: "manager-1".to_string(),
                source_attachment_id: Some(bogus),
            })
            .await;

        assert!(matches!(
            result,
            Err(ActorError::AttachmentNotFound(id)) if id == bogus
        ));
    }

    #[tokio::test]
    async fn actor_rejects_create_card_when_source_attachment_was_removed() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "s".into(),
                one_liner: "o".into(),
                goal: "g".into(),
            })
            .await
            .unwrap();

        let att_id = Ulid::new();
        handle
            .send_command(Command::AttachContext {
                attachment_id: att_id,
                filename: "gone.md".into(),
                mime_type: "text/markdown".into(),
                size_bytes: 10,
            })
            .await
            .unwrap();
        handle
            .send_command(Command::RemoveContext {
                attachment_id: att_id,
            })
            .await
            .unwrap();

        let result = handle
            .send_command(Command::CreateCard {
                card_type: "idea".to_string(),
                title: "Too late".to_string(),
                body: None,
                lane: None,
                created_by: "manager-1".to_string(),
                source_attachment_id: Some(att_id),
            })
            .await;

        assert!(matches!(
            result,
            Err(ActorError::AttachmentNotFound(id)) if id == att_id
        ));
    }

    #[tokio::test]
    async fn actor_broadcasts_events() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        let mut rx = handle.subscribe();

        handle
            .send_command(Command::CreateSpec {
                title: "Broadcast Test".to_string(),
                one_liner: "One".to_string(),
                goal: "Goal".to_string(),
            })
            .await
            .unwrap();

        let event = rx.recv().await.expect("should receive broadcast event");
        assert_eq!(event.event_id, 1);
        match &event.payload {
            EventPayload::SpecCreated { title, .. } => {
                assert_eq!(title, "Broadcast Test");
            }
            _ => panic!("expected SpecCreated"),
        }
    }

    #[tokio::test]
    async fn actor_rejects_second_pending_question() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());

        let q1 = UserQuestion::Boolean {
            question_id: Ulid::new(),
            question: "First?".to_string(),
            default: None,
        };

        handle
            .send_command(Command::AskQuestion { question: q1 })
            .await
            .unwrap();

        let q2 = UserQuestion::Boolean {
            question_id: Ulid::new(),
            question: "Second?".to_string(),
            default: None,
        };

        let result = handle
            .send_command(Command::AskQuestion { question: q2 })
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, ActorError::QuestionAlreadyPending),
            "expected QuestionAlreadyPending, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn actor_allows_question_after_answer() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());

        let q_id = Ulid::new();
        let q1 = UserQuestion::Boolean {
            question_id: q_id,
            question: "First?".to_string(),
            default: None,
        };

        handle
            .send_command(Command::AskQuestion { question: q1 })
            .await
            .unwrap();

        handle
            .send_command(Command::AnswerQuestion {
                question_id: q_id,
                answer: "Yes".to_string(),
            })
            .await
            .unwrap();

        // Now a second question should be allowed
        let q2 = UserQuestion::Freeform {
            question_id: Ulid::new(),
            question: "Second?".to_string(),
            placeholder: None,
            validation_hint: None,
        };

        let result = handle
            .send_command(Command::AskQuestion { question: q2 })
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn actor_rejects_command_on_nonexistent_card() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());

        let bad_id = Ulid::new();
        let result = handle
            .send_command(Command::UpdateCard {
                card_id: bad_id,
                title: Some("Ghost".to_string()),
                body: None,
                card_type: None,
                refs: None,
                updated_by: "human".to_string(),
            })
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, ActorError::CardNotFound(id) if id == bad_id),
            "expected CardNotFound, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn actor_event_id_continues_from_recovered_state() {
        let spec_id = Ulid::new();

        // Simulate recovered state with last_event_id = 50
        let mut recovered_state = SpecState::new();
        recovered_state.last_event_id = 50;

        let handle = spawn(spec_id, recovered_state);

        let events = handle
            .send_command(Command::CreateSpec {
                title: "Recovered Spec".to_string(),
                one_liner: "After crash".to_string(),
                goal: "Verify event IDs continue".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0].event_id, 51,
            "event_id should continue from last_event_id (50) + 1"
        );
        assert_eq!(events[1].event_id, 52);
    }

    #[tokio::test]
    async fn actor_undo_reverses_card_creation() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());

        // Create spec first
        handle
            .send_command(Command::CreateSpec {
                title: "Spec".to_string(),
                one_liner: "One".to_string(),
                goal: "Goal".to_string(),
            })
            .await
            .unwrap();

        // Create a card
        handle
            .send_command(Command::CreateCard {
                card_type: "idea".to_string(),
                title: "Undo Me".to_string(),
                body: None,
                lane: None,
                created_by: "human".to_string(),
                source_attachment_id: None,
            })
            .await
            .unwrap();

        // Verify card exists
        {
            let state = handle.read_state().await;
            assert_eq!(state.cards.len(), 1);
        }

        // Undo should remove the card
        handle.send_command(Command::Undo).await.unwrap();

        let state = handle.read_state().await;
        assert_eq!(state.cards.len(), 0, "card should be removed after undo");
    }

    #[tokio::test]
    async fn actor_double_undo_returns_nothing_to_undo() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());

        // Create spec first
        handle
            .send_command(Command::CreateSpec {
                title: "Spec".to_string(),
                one_liner: "One".to_string(),
                goal: "Goal".to_string(),
            })
            .await
            .unwrap();

        // Create a card (single undoable operation)
        handle
            .send_command(Command::CreateCard {
                card_type: "idea".to_string(),
                title: "Single Card".to_string(),
                body: None,
                lane: None,
                created_by: "human".to_string(),
                source_attachment_id: None,
            })
            .await
            .unwrap();

        // First undo should succeed
        handle.send_command(Command::Undo).await.unwrap();

        // Second undo should fail with NothingToUndo
        let result = handle.send_command(Command::Undo).await;
        assert!(result.is_err(), "second undo should fail");
        let err = result.unwrap_err();
        assert!(
            matches!(err, ActorError::NothingToUndo),
            "expected NothingToUndo, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn transition_phase_produces_event() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        // CreateSpec puts spec into Brainstorming
        handle
            .send_command(Command::CreateSpec {
                title: "Test".to_string(),
                one_liner: "t".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();

        let events = handle
            .send_command(Command::TransitionPhase {
                target: SpecPhase::Refining,
            })
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        match &events[0].payload {
            EventPayload::PhaseTransitioned { phase } => {
                assert_eq!(*phase, SpecPhase::Refining);
            }
            _ => panic!("wrong event"),
        }
    }

    #[tokio::test]
    async fn transition_phase_already_in_phase_rejected() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "Test".to_string(),
                one_liner: "t".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();

        // Brainstorming -> Refining
        handle
            .send_command(Command::TransitionPhase {
                target: SpecPhase::Refining,
            })
            .await
            .unwrap();

        // Refining -> Refining should fail
        let err = handle
            .send_command(Command::TransitionPhase {
                target: SpecPhase::Refining,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ActorError::AlreadyInPhase));
    }

    #[tokio::test]
    async fn transition_phase_brainstorming_refining_brainstorming() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "Test".to_string(),
                one_liner: "t".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();

        // Brainstorming -> Refining -> Brainstorming
        handle
            .send_command(Command::TransitionPhase {
                target: SpecPhase::Refining,
            })
            .await
            .unwrap();
        handle
            .send_command(Command::TransitionPhase {
                target: SpecPhase::Brainstorming,
            })
            .await
            .unwrap();
        let state = handle.read_state().await;
        assert_eq!(state.phase, SpecPhase::Brainstorming);
    }

    #[tokio::test]
    async fn actor_processes_attach_context() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "t".into(),
                one_liner: "o".into(),
                goal: "g".into(),
            })
            .await
            .unwrap();

        let attachment_id = Ulid::new();
        let events = handle
            .send_command(Command::AttachContext {
                attachment_id,
                filename: "notes.md".into(),
                mime_type: "text/markdown".into(),
                size_bytes: 42,
            })
            .await
            .unwrap();

        assert_eq!(events.len(), 1);
        match &events[0].payload {
            EventPayload::ContextAttached { attachment } => {
                assert_eq!(attachment.attachment_id, attachment_id);
                assert_eq!(attachment.filename, "notes.md");
                assert_eq!(attachment.mime_type, "text/markdown");
                assert_eq!(attachment.size_bytes, 42);
                assert!(attachment.summary.is_none());
                assert!(attachment.user_notes.is_none());
                assert!(!attachment.removed);
            }
            _ => panic!("expected ContextAttached"),
        }

        let state = handle.read_state().await;
        assert_eq!(state.context_attachments.len(), 1);
    }

    #[tokio::test]
    async fn actor_rejects_duplicate_attach_context_id() {
        // Regression: AttachContext used to trust the caller-supplied
        // attachment_id and always append. A duplicate id leaves the second
        // entry orphaned because every follow-up path (Summarize/UpdateNotes/
        // Remove, plus CreateCard's source check) uses `find(...)` and only
        // ever hits the first entry.
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "t".into(),
                one_liner: "o".into(),
                goal: "g".into(),
            })
            .await
            .unwrap();
        let attachment_id = Ulid::new();
        handle
            .send_command(Command::AttachContext {
                attachment_id,
                filename: "notes.md".into(),
                mime_type: "text/markdown".into(),
                size_bytes: 1,
            })
            .await
            .unwrap();

        let dup = handle
            .send_command(Command::AttachContext {
                attachment_id,
                filename: "notes-again.md".into(),
                mime_type: "text/markdown".into(),
                size_bytes: 2,
            })
            .await;
        assert!(
            matches!(dup, Err(ActorError::AttachmentAlreadyExists(id)) if id == attachment_id),
            "second AttachContext with the same id must fail with AttachmentAlreadyExists, got {dup:?}"
        );

        let state = handle.read_state().await;
        assert_eq!(
            state.context_attachments.len(),
            1,
            "rejected duplicate must not leave an extra entry in state"
        );
        assert_eq!(state.context_attachments[0].filename, "notes.md");
    }

    #[tokio::test]
    async fn actor_rejects_attach_context_after_remove_with_same_id() {
        // Even after a soft-remove, re-attaching with the same id stays
        // forbidden — the find() pattern in follow-up commands still wouldn't
        // disambiguate which entry future operations should target.
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "t".into(),
                one_liner: "o".into(),
                goal: "g".into(),
            })
            .await
            .unwrap();
        let attachment_id = Ulid::new();
        handle
            .send_command(Command::AttachContext {
                attachment_id,
                filename: "x.md".into(),
                mime_type: "text/markdown".into(),
                size_bytes: 1,
            })
            .await
            .unwrap();
        handle
            .send_command(Command::RemoveContext { attachment_id })
            .await
            .unwrap();

        let dup = handle
            .send_command(Command::AttachContext {
                attachment_id,
                filename: "x-again.md".into(),
                mime_type: "text/markdown".into(),
                size_bytes: 2,
            })
            .await;
        assert!(
            matches!(dup, Err(ActorError::AttachmentAlreadyExists(id)) if id == attachment_id),
            "re-attach after remove with the same id must fail with AttachmentAlreadyExists, got {dup:?}"
        );
    }

    #[tokio::test]
    async fn actor_processes_summarize_context() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "t".into(),
                one_liner: "o".into(),
                goal: "g".into(),
            })
            .await
            .unwrap();
        let attachment_id = Ulid::new();
        handle
            .send_command(Command::AttachContext {
                attachment_id,
                filename: "a".into(),
                mime_type: "text/plain".into(),
                size_bytes: 1,
            })
            .await
            .unwrap();

        let events = handle
            .send_command(Command::SummarizeContext {
                attachment_id,
                summary: "brief".into(),
            })
            .await
            .unwrap();

        assert_eq!(events.len(), 1);
        match &events[0].payload {
            EventPayload::ContextSummarized {
                attachment_id: a,
                summary,
            } => {
                assert_eq!(*a, attachment_id);
                assert_eq!(summary, "brief");
            }
            _ => panic!("expected ContextSummarized"),
        }
    }

    #[tokio::test]
    async fn actor_processes_update_context_notes() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "t".into(),
                one_liner: "o".into(),
                goal: "g".into(),
            })
            .await
            .unwrap();
        let attachment_id = Ulid::new();
        handle
            .send_command(Command::AttachContext {
                attachment_id,
                filename: "a".into(),
                mime_type: "text/plain".into(),
                size_bytes: 1,
            })
            .await
            .unwrap();

        let events = handle
            .send_command(Command::UpdateContextNotes {
                attachment_id,
                notes: "my note".into(),
            })
            .await
            .unwrap();

        assert_eq!(events.len(), 1);
        match &events[0].payload {
            EventPayload::ContextNotesUpdated {
                attachment_id: a,
                notes,
            } => {
                assert_eq!(*a, attachment_id);
                assert_eq!(notes, "my note");
            }
            _ => panic!("expected ContextNotesUpdated"),
        }
    }

    #[tokio::test]
    async fn actor_processes_remove_context() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "t".into(),
                one_liner: "o".into(),
                goal: "g".into(),
            })
            .await
            .unwrap();
        let attachment_id = Ulid::new();
        handle
            .send_command(Command::AttachContext {
                attachment_id,
                filename: "a".into(),
                mime_type: "text/plain".into(),
                size_bytes: 1,
            })
            .await
            .unwrap();

        let events = handle
            .send_command(Command::RemoveContext { attachment_id })
            .await
            .unwrap();

        assert_eq!(events.len(), 1);
        match &events[0].payload {
            EventPayload::ContextRemoved { attachment_id: a } => {
                assert_eq!(*a, attachment_id);
            }
            _ => panic!("expected ContextRemoved"),
        }

        let state = handle.read_state().await;
        assert!(state.context_attachments[0].removed);
    }

    #[tokio::test]
    async fn actor_rejects_summarize_on_unknown_attachment() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "t".into(),
                one_liner: "o".into(),
                goal: "g".into(),
            })
            .await
            .unwrap();

        let bad_id = Ulid::new();
        let result = handle
            .send_command(Command::SummarizeContext {
                attachment_id: bad_id,
                summary: "brief".into(),
            })
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, ActorError::AttachmentNotFound(id) if id == bad_id),
            "expected AttachmentNotFound, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn actor_rejects_remove_on_already_removed_attachment() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "t".into(),
                one_liner: "o".into(),
                goal: "g".into(),
            })
            .await
            .unwrap();
        let attachment_id = Ulid::new();
        handle
            .send_command(Command::AttachContext {
                attachment_id,
                filename: "a".into(),
                mime_type: "text/plain".into(),
                size_bytes: 1,
            })
            .await
            .unwrap();

        // First remove succeeds
        handle
            .send_command(Command::RemoveContext { attachment_id })
            .await
            .unwrap();

        // Second remove should fail
        let result = handle
            .send_command(Command::RemoveContext { attachment_id })
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, ActorError::AttachmentAlreadyRemoved(id) if id == attachment_id),
            "expected AttachmentAlreadyRemoved, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn actor_rejects_summarize_removed_attachment() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "t".into(),
                one_liner: "o".into(),
                goal: "g".into(),
            })
            .await
            .unwrap();
        let attachment_id = Ulid::new();
        handle
            .send_command(Command::AttachContext {
                attachment_id,
                filename: "a".into(),
                mime_type: "text/plain".into(),
                size_bytes: 1,
            })
            .await
            .unwrap();
        handle
            .send_command(Command::RemoveContext { attachment_id })
            .await
            .unwrap();
        let result = handle
            .send_command(Command::SummarizeContext {
                attachment_id,
                summary: "late".into(),
            })
            .await;
        assert!(
            matches!(result, Err(ActorError::AttachmentAlreadyRemoved(id)) if id == attachment_id)
        );
    }

    #[tokio::test]
    async fn actor_rejects_update_notes_on_removed_attachment() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "t".into(),
                one_liner: "o".into(),
                goal: "g".into(),
            })
            .await
            .unwrap();
        let attachment_id = Ulid::new();
        handle
            .send_command(Command::AttachContext {
                attachment_id,
                filename: "a".into(),
                mime_type: "text/plain".into(),
                size_bytes: 1,
            })
            .await
            .unwrap();
        handle
            .send_command(Command::RemoveContext { attachment_id })
            .await
            .unwrap();
        let result = handle
            .send_command(Command::UpdateContextNotes {
                attachment_id,
                notes: "late".into(),
            })
            .await;
        assert!(
            matches!(result, Err(ActorError::AttachmentAlreadyRemoved(id)) if id == attachment_id)
        );
    }

    #[tokio::test]
    async fn actor_broadcasts_streaming_delta() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        let mut rx = handle.subscribe();

        let events = handle
            .send_command(Command::StreamDelta {
                agent_id: "manager-1".to_string(),
                text: "Hi".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(events.len(), 1);
        match &events[0].payload {
            EventPayload::StreamingDelta { agent_id, text } => {
                assert_eq!(agent_id, "manager-1");
                assert_eq!(text, "Hi");
            }
            _ => panic!("expected StreamingDelta"),
        }

        let broadcast = rx.recv().await.unwrap();
        match &broadcast.payload {
            EventPayload::StreamingDelta { .. } => {}
            _ => panic!("expected StreamingDelta broadcast"),
        }
    }

    #[tokio::test]
    async fn ephemeral_events_get_zero_event_id() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());

        // Create spec first (uses event IDs 1 and 2)
        handle
            .send_command(Command::CreateSpec {
                title: "Ephemeral Test".to_string(),
                one_liner: "t".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();

        // Send a streaming delta — ephemeral, should get event_id 0
        let events = handle
            .send_command(Command::StreamDelta {
                agent_id: "manager-1".to_string(),
                text: "Hello".to_string(),
            })
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].event_id, 0,
            "ephemeral events should get event_id 0"
        );

        // Send a durable event — should get event_id 3 (no gap from ephemeral)
        let events = handle
            .send_command(Command::UpdateCanvas {
                content: "<p>test</p>".to_string(),
            })
            .await
            .unwrap();
        assert_eq!(
            events[0].event_id, 3,
            "durable event ID should not be affected by ephemeral events"
        );
    }

    #[tokio::test]
    async fn update_canvas_produces_event() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "Test".to_string(),
                one_liner: "t".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();

        let events = handle
            .send_command(Command::UpdateCanvas {
                content: "<h1>Hello</h1>".to_string(),
            })
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        match &events[0].payload {
            EventPayload::CanvasUpdated { content } => {
                assert_eq!(content, "<h1>Hello</h1>");
            }
            _ => panic!("wrong event"),
        }
    }
}
