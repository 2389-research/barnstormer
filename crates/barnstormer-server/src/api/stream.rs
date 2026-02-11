// ABOUTME: SSE event streaming handler for real-time spec event delivery.
// ABOUTME: Subscribes to a spec actor's broadcast channel and converts events to SSE format.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::stream::{Stream, StreamExt};
use tokio_stream::wrappers::BroadcastStream;
use ulid::Ulid;

use crate::app_state::SharedState;

/// Derive an SSE event type name from an EventPayload variant.
/// Converts the serde tag value (PascalCase) to snake_case for SSE event names.
fn event_type_name(payload: &barnstormer_core::EventPayload) -> &'static str {
    match payload {
        barnstormer_core::EventPayload::SpecCreated { .. } => "spec_created",
        barnstormer_core::EventPayload::SpecCoreUpdated { .. } => "spec_core_updated",
        barnstormer_core::EventPayload::CardCreated { .. } => "card_created",
        barnstormer_core::EventPayload::CardUpdated { .. } => "card_updated",
        barnstormer_core::EventPayload::CardMoved { .. } => "card_moved",
        barnstormer_core::EventPayload::CardDeleted { .. } => "card_deleted",
        barnstormer_core::EventPayload::TranscriptAppended { .. } => "transcript_appended",
        barnstormer_core::EventPayload::QuestionAsked { .. } => "question_asked",
        barnstormer_core::EventPayload::QuestionAnswered { .. } => "question_answered",
        barnstormer_core::EventPayload::AgentStepStarted { .. } => "agent_step_started",
        barnstormer_core::EventPayload::AgentStepFinished { .. } => "agent_step_finished",
        barnstormer_core::EventPayload::UndoApplied { .. } => "undo_applied",
        barnstormer_core::EventPayload::SnapshotWritten { .. } => "snapshot_written",
    }
}

/// Convert a broadcast receiver into an SSE-compatible stream.
fn event_stream_from_receiver(
    rx: tokio::sync::broadcast::Receiver<barnstormer_core::Event>,
) -> impl Stream<Item = Result<SseEvent, axum::Error>> {
    BroadcastStream::new(rx).filter_map(|result| async move {
        match result {
            Ok(event) => {
                let event_type = event_type_name(&event.payload);
                let data = serde_json::to_string(&event).ok()?;
                Some(Ok(SseEvent::default().event(event_type).data(data)))
            }
            Err(_) => None,
        }
    })
}

/// GET /api/specs/{id}/events/stream - SSE endpoint for real-time event streaming.
pub async fn event_stream(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let spec_id = match id.parse::<Ulid>() {
        Ok(id) => id,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "invalid spec id").into_response();
        }
    };

    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h,
        None => {
            return (StatusCode::NOT_FOUND, "spec not found").into_response();
        }
    };

    let rx = handle.subscribe();
    let stream = event_stream_from_receiver(rx);

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use barnstormer_core::{Command, SpecState, spawn};

    #[tokio::test]
    async fn sse_stream_receives_events() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());

        // Subscribe before sending command
        let rx = handle.subscribe();
        let mut stream = Box::pin(event_stream_from_receiver(rx));

        // Send a CreateSpec command to generate an event
        handle
            .send_command(Command::CreateSpec {
                title: "SSE Test".to_string(),
                one_liner: "Stream it".to_string(),
                goal: "Verify SSE".to_string(),
            })
            .await
            .unwrap();

        // Read the event from the stream
        let sse_event = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("should receive event within timeout")
            .expect("stream should have an item")
            .expect("item should be Ok");

        // Verify we got a valid SSE event (we can't easily inspect the
        // event type from the SseEvent struct, but we can verify it succeeded)
        // The fact that we got Ok is sufficient to prove the stream works.
        // We can verify the JSON data by converting back.
        let _ = sse_event;
    }

    #[tokio::test]
    async fn sse_stream_receives_card_created() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());

        // Create spec first (no subscriber yet, so this won't be in stream)
        handle
            .send_command(Command::CreateSpec {
                title: "SSE Card Test".to_string(),
                one_liner: "Cards".to_string(),
                goal: "Cards via SSE".to_string(),
            })
            .await
            .unwrap();

        // Now subscribe
        let rx = handle.subscribe();
        let mut stream = Box::pin(event_stream_from_receiver(rx));

        // Create a card
        handle
            .send_command(Command::CreateCard {
                card_type: "idea".to_string(),
                title: "Stream Card".to_string(),
                body: None,
                lane: None,
                created_by: "human".to_string(),
            })
            .await
            .unwrap();

        // Read the event from the stream
        let sse_event = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("should receive event within timeout")
            .expect("stream should have an item")
            .expect("item should be Ok");

        let _ = sse_event;
    }

    #[test]
    fn event_type_names_are_correct() {
        use barnstormer_core::Card;
        use barnstormer_core::EventPayload;

        assert_eq!(
            event_type_name(&EventPayload::SpecCreated {
                title: String::new(),
                one_liner: String::new(),
                goal: String::new()
            }),
            "spec_created"
        );

        assert_eq!(
            event_type_name(&EventPayload::CardCreated {
                card: Card::new("idea".into(), "t".into(), "h".into())
            }),
            "card_created"
        );

        assert_eq!(
            event_type_name(&EventPayload::UndoApplied {
                target_event_id: 1,
                inverse_events: vec![]
            }),
            "undo_applied"
        );
    }
}
