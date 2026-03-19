# Spec 1: Phase Model & Swarm Gating

**Goal:** Add a persisted, event-sourced `SpecPhase` to the domain model so the system knows whether a spec is in brainstorming mode (Manager-only Q&A) or active mode (full agent swarm). Gate non-Manager agents during brainstorming.

**Context:** This is the first of three specs that together build a guided brainstorming flow:
- **Spec 1 (this):** Phase model plumbing — domain types, swarm gating, API endpoint
- **Spec 2:** Brainstorming UI — full-width chat layout, canvas panel, transition animation
- **Spec 3:** Manager intelligence — reworked prompts, `show_canvas` tool, `propose_transition` tool

## Domain Model

### SpecPhase enum

New type in `barnstormer-core`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecPhase {
    Brainstorming,
    Active,
}
```

No `Default` impl — the phase is always set explicitly. `SpecState::new()` defaults to `Active` for backward compatibility. New specs get `Brainstorming` via an explicit `PhaseTransitioned` event emitted during creation.

Serde representation: `"brainstorming"` and `"active"` (snake_case). This matches the web API input format and the JSONL event log format — no mismatch.

### SpecState field

```rust
pub struct SpecState {
    #[serde(default = "default_phase_active")]
    pub phase: SpecPhase,  // new field, defaults to Active for snapshot compat
    pub core: Option<SpecCore>,
    pub cards: BTreeMap<Ulid, Card>,
    pub transcript: Vec<TranscriptMessage>,
    pub pending_question: Option<UserQuestion>,
    pub undo_stack: Vec<UndoEntry>,
    pub last_event_id: u64,
    pub lanes: Vec<String>,
}

fn default_phase_active() -> SpecPhase {
    SpecPhase::Active
}
```

The `#[serde(default)]` annotation ensures existing snapshots that lack a `phase` field deserialize with `Active`, maintaining backward compatibility.

### Command

```rust
Command::TransitionPhase { target: SpecPhase }
```

Serde: inherits `#[serde(tag = "type")]` from Command enum. Serializes as `{"type": "TransitionPhase", "target": "brainstorming"}`.

### Event

```rust
EventPayload::PhaseTransitioned { phase: SpecPhase }
```

Serde: inherits `#[serde(tag = "type")]` from EventPayload enum. Serializes as `{"type": "PhaseTransitioned", "phase": "brainstorming"}`.

### Actor validation

- `TransitionPhase` where `target == state.phase` is rejected with a new `ActorError::AlreadyInPhase` error. This prevents duplicate transition events.
- All other transitions are valid (Brainstorming → Active, Active → Brainstorming).

### CreateSpec change

`Command::CreateSpec` now emits two events: `SpecCreated` followed by `PhaseTransitioned { Brainstorming }`. This ensures new specs start in brainstorming mode via an explicit event in the log.

**Existing test impact:** The test at `actor.rs:377` that asserts `events.len() == 1` for `CreateSpec` will need updating to expect 2 events. Command and event serde round-trip tests need new variants added.

### Reducer

```rust
EventPayload::PhaseTransitioned { phase } => {
    self.phase = phase.clone();
}
```

`PhaseTransitioned` does NOT push an undo entry. Phase transitions are not undoable — they are lifecycle events, not content mutations.

## Swarm Gating

In `SwarmOrchestrator::run_loop`, before each agent step:

1. Read `state.phase` from the actor handle.
2. If `Brainstorming`: `continue` past non-Manager agents in the loop. The agents remain in the `agents` Vec — they are not removed or destroyed, just skipped.
3. If `Active`: run all agents (existing behavior).

The phase check goes inside the agent iteration loop, right before calling the agent's step function. This is a simple `if phase == Brainstorming && role != Manager { continue; }` guard.

**Wake on transition:** The swarm's main loop uses `tokio::select!` on a sleep timer and the `human_message_notify`. Add a third branch: an `mpsc` or `Notify` that fires when a `PhaseTransitioned { Active }` event is seen on the broadcast channel. This reuses the existing pattern — the swarm already subscribes to the broadcast channel for event processing. When it sees `PhaseTransitioned { Active }`, it triggers the notify to break out of the sleep and immediately start the agent cycle.

## Web API

### New route

```
POST /web/specs/{id}/phase
```

Accepts form: `PhaseForm { target: String }` — values `"brainstorming"` or `"active"`.

**Parsing:** Map the string to `SpecPhase`. Invalid values return `400 Bad Request` with error message.

**Success:** Sends `Command::TransitionPhase { target }` to the actor. Returns `200 OK` with an HTML fragment (for HTMX swap). The fragment content is a phase badge element that Spec 2 will define — for now, a simple `<span>` with the current phase text.

**Errors:**
- Nonexistent spec: `404 Not Found`
- Invalid target string: `400 Bad Request`
- Already in target phase (`AlreadyInPhase`): `409 Conflict` with error message
- Invalid spec ID: `400 Bad Request` (existing `parse_spec_id` behavior)

### Existing routes

No changes. The `phase` field is automatically included in `GET /api/specs/{id}/state` since it's part of SpecState.

### SSE

`PhaseTransitioned` events broadcast through the existing channel like all other events. The SSE event name is `phase_transitioned`. The SSE data payload includes the phase value. UI consumption is handled in Spec 2.

## Backward Compatibility

### Event log (JSONL)

Existing specs have JSONL logs without `PhaseTransitioned` events. `SpecState::new()` sets `phase: SpecPhase::Active`. When these specs replay their events, no `PhaseTransitioned` event is encountered, so `phase` stays `Active`. This is correct — existing specs should behave as they always have.

New specs emit `PhaseTransitioned { Brainstorming }` as part of `CreateSpec`, so they start in brainstorming mode.

### Snapshots

Existing snapshots lack a `phase` field. The `#[serde(default = "default_phase_active")]` annotation on `SpecState.phase` ensures these deserialize correctly as `Active`.

### Existing tests

The following existing tests will need updates:
- `actor.rs`: `CreateSpec` event count assertion (1 → 2)
- `command.rs`: serde round-trip test needs `TransitionPhase` variant
- `event.rs`: serde round-trip test needs `PhaseTransitioned` variant
- Any snapshot tests that assert on SpecState structure

## Testing

### Unit tests (barnstormer-core)

- `SpecState::new()` starts with `phase: Active`
- `TransitionPhase { Active }` produces `PhaseTransitioned { Active }` event
- `TransitionPhase { Brainstorming }` produces `PhaseTransitioned { Brainstorming }` event
- `PhaseTransitioned` event updates `state.phase`
- `TransitionPhase` to current phase is rejected (`AlreadyInPhase` error)
- Round-trip: Brainstorming → Active → Brainstorming works
- `CreateSpec` emits 2 events: `SpecCreated` + `PhaseTransitioned { Brainstorming }`
- `PhaseTransitioned` does not push an undo entry
- `SpecPhase` serde: `Brainstorming` serializes as `"brainstorming"`, `Active` as `"active"`
- `TransitionPhase` command serde round-trip
- `PhaseTransitioned` event serde round-trip
- Snapshot deserialization without `phase` field defaults to `Active`

### Unit tests (barnstormer-agent)

- Swarm skips non-Manager agents when phase is `Brainstorming`
- Swarm runs all agents when phase is `Active`

### Integration tests (barnstormer-server)

- `POST /web/specs/{id}/phase` with `target=active` returns 200
- `POST /web/specs/{id}/phase` with `target=brainstorming` returns 200
- `POST /web/specs/{id}/phase` for nonexistent spec returns 404
- `POST /web/specs/{id}/phase` to current phase returns 409
- `POST /web/specs/{id}/phase` with invalid target returns 400
- State API response includes `phase` field

## Out of Scope

- UI layout changes (Spec 2)
- Manager prompt rework (Spec 3)
- `show_canvas` / `propose_transition` tools (Spec 3)
- Canvas infrastructure (Spec 3)
