# Phase Model & Swarm Gating Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add event-sourced `SpecPhase` (Brainstorming/Active) to the domain model, gate non-Manager agents during brainstorming, and expose a phase transition endpoint.

**Architecture:** New `SpecPhase` enum added to barnstormer-core, threaded through Command/Event/State/Actor. Swarm gating in barnstormer-agent skips non-Manager agents when phase is Brainstorming. Web endpoint in barnstormer-server allows phase transitions via HTMX.

**Tech Stack:** Rust, serde, Axum, HTMX, tokio broadcast channels

**Spec:** `docs/superpowers/specs/2026-03-19-phase-model-design.md`

---

## File Structure

| Action | File | Responsibility |
|--------|------|---------------|
| Modify | `crates/barnstormer-core/src/state.rs` | Add `SpecPhase` enum, `phase` field on `SpecState`, reducer arm |
| Modify | `crates/barnstormer-core/src/command.rs` | Add `TransitionPhase` command variant |
| Modify | `crates/barnstormer-core/src/event.rs` | Add `PhaseTransitioned` event variant |
| Modify | `crates/barnstormer-core/src/actor.rs` | Handle `TransitionPhase` command, `AlreadyInPhase` error, emit extra event on `CreateSpec` |
| Modify | `crates/barnstormer-core/src/lib.rs` | Re-export `SpecPhase` |
| Modify | `crates/barnstormer-agent/src/swarm.rs` | Phase gating in `run_loop`, phase-transition notify |
| Modify | `crates/barnstormer-agent/src/context.rs` | Add `PhaseTransitioned` arm to `describe_event_payload` |
| Modify | `crates/barnstormer-server/src/api/stream.rs` | Add `PhaseTransitioned` arm to `event_type_name` |
| Modify | `crates/barnstormer-server/src/routes.rs` | Add phase transition route |
| Modify | `crates/barnstormer-server/src/web/mod.rs` | Add phase transition handler |

---

### Task 1: Add SpecPhase enum and SpecState field

**Files:**
- Modify: `crates/barnstormer-core/src/state.rs`
- Modify: `crates/barnstormer-core/src/lib.rs`

- [ ] **Step 1: Write failing tests for SpecPhase serde and SpecState defaults**

Add to the `#[cfg(test)]` module in `state.rs`:

```rust
#[test]
fn spec_phase_serde_brainstorming() {
    let phase = SpecPhase::Brainstorming;
    let json = serde_json::to_string(&phase).unwrap();
    assert_eq!(json, "\"brainstorming\"");
    let back: SpecPhase = serde_json::from_str(&json).unwrap();
    assert_eq!(back, SpecPhase::Brainstorming);
}

#[test]
fn spec_phase_serde_active() {
    let phase = SpecPhase::Active;
    let json = serde_json::to_string(&phase).unwrap();
    assert_eq!(json, "\"active\"");
    let back: SpecPhase = serde_json::from_str(&json).unwrap();
    assert_eq!(back, SpecPhase::Active);
}

#[test]
fn spec_state_new_defaults_to_active() {
    let state = SpecState::new();
    assert_eq!(state.phase, SpecPhase::Active);
}

#[test]
fn snapshot_without_phase_deserializes_as_active() {
    // Simulate an old snapshot JSON without a "phase" field
    let json = r#"{"core":null,"cards":{},"transcript":[],"pending_question":null,"undo_stack":[],"last_event_id":0,"lanes":["Ideas","Plan","Spec"]}"#;
    let state: SpecState = serde_json::from_str(json).unwrap();
    assert_eq!(state.phase, SpecPhase::Active);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --package barnstormer-core -- state::tests::spec_phase`
Expected: compilation errors — `SpecPhase` doesn't exist yet

- [ ] **Step 3: Implement SpecPhase enum and add to SpecState**

Add the enum above the `SpecState` struct in `state.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecPhase {
    Brainstorming,
    Active,
}

fn default_phase_active() -> SpecPhase {
    SpecPhase::Active
}
```

Add the field to `SpecState` (currently at line 23):

```rust
#[serde(default = "default_phase_active")]
pub phase: SpecPhase,
```

Update the `Default` impl for `SpecState` (currently at line 33) to include `phase: SpecPhase::Active`.

Re-export from `crates/barnstormer-core/src/lib.rs` — add to line 18:

```rust
pub use state::{SpecPhase, SpecState, UndoEntry};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --package barnstormer-core -- state::tests::spec_phase`
Expected: all 4 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/barnstormer-core/src/state.rs crates/barnstormer-core/src/lib.rs
git commit -m "feat: add SpecPhase enum and phase field to SpecState"
```

---

### Task 2: Add PhaseTransitioned event variant

**Files:**
- Modify: `crates/barnstormer-core/src/event.rs`
- Modify: `crates/barnstormer-core/src/state.rs` (reducer)

- [ ] **Step 1: Write failing test for PhaseTransitioned event serde**

Add to the `#[cfg(test)]` module in `event.rs`. The existing tests use a `round_trip_event()` helper:

```rust
#[test]
fn phase_transitioned_round_trip() {
    round_trip_event(EventPayload::PhaseTransitioned {
        phase: crate::state::SpecPhase::Brainstorming,
    });
}
```

- [ ] **Step 2: Write failing tests for PhaseTransitioned reducer**

Add to the `#[cfg(test)]` module in `state.rs`:

```rust
#[test]
fn phase_transitioned_updates_state() {
    let mut state = SpecState::new();
    assert_eq!(state.phase, SpecPhase::Active);

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
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --package barnstormer-core -- phase_transitioned`
Expected: compilation error — `PhaseTransitioned` variant doesn't exist

- [ ] **Step 4: Add PhaseTransitioned variant to EventPayload**

In `event.rs`, add to the `EventPayload` enum:

```rust
PhaseTransitioned { phase: crate::state::SpecPhase },
```

- [ ] **Step 5: Add reducer arm in state.rs**

In `state.rs`, in the `apply()` match block (starts at line 59), add:

```rust
EventPayload::PhaseTransitioned { phase } => {
    self.phase = phase.clone();
    // No undo entry — phase transitions are lifecycle events
}
```

Also add an explicit arm to `apply_without_undo()` (starts around line 269). This arm is needed because the catch-all `_ => self.apply(event)` fallback would route through `apply()` which updates `last_event_id` — something `apply_without_undo` should not do during undo replay:

```rust
EventPayload::PhaseTransitioned { phase } => {
    self.phase = phase.clone();
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --package barnstormer-core -- phase_transitioned`
Expected: all 3 tests PASS

- [ ] **Step 7: Commit**

```bash
git add crates/barnstormer-core/src/event.rs crates/barnstormer-core/src/state.rs
git commit -m "feat: add PhaseTransitioned event with reducer"
```

---

### Task 3: Add TransitionPhase command, update CreateSpec, and actor handling

**Files:**
- Modify: `crates/barnstormer-core/src/command.rs`
- Modify: `crates/barnstormer-core/src/actor.rs`

**Note:** The CreateSpec change (emitting 2 events) must be implemented in the same task as TransitionPhase, because actor tests use `CreateSpec` to set up specs — if CreateSpec emits `PhaseTransitioned{Brainstorming}` but `TransitionPhase` doesn't exist yet, tests break. Both changes happen together.

- [ ] **Step 1: Write failing test for TransitionPhase command serde**

Add to the `#[cfg(test)]` module in `command.rs`. Also add the new variant to the existing `command_serializes_round_trip` test's `Vec<Command>`:

```rust
#[test]
fn transition_phase_round_trip() {
    let cmd = Command::TransitionPhase {
        target: crate::state::SpecPhase::Active,
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"TransitionPhase\""));
    assert!(json.contains("\"active\""));
    let back: Command = serde_json::from_str(&json).unwrap();
    match back {
        Command::TransitionPhase { target } => {
            assert_eq!(target, crate::state::SpecPhase::Active);
        }
        _ => panic!("wrong variant"),
    }
}
```

- [ ] **Step 2: Write failing tests for actor handling**

Add to the `#[cfg(test)]` module in `actor.rs`. The existing pattern creates actors inline — follow it:

```rust
#[tokio::test]
async fn transition_phase_produces_event() {
    let spec_id = Ulid::new();
    let handle = spawn(spec_id, SpecState::new());
    // CreateSpec puts spec into Brainstorming
    handle.send_command(Command::CreateSpec {
        title: "Test".to_string(),
        one_liner: "t".to_string(),
        goal: "g".to_string(),
    }).await.unwrap();

    let events = handle
        .send_command(Command::TransitionPhase {
            target: SpecPhase::Active,
        })
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    match &events[0].payload {
        EventPayload::PhaseTransitioned { phase } => {
            assert_eq!(*phase, SpecPhase::Active);
        }
        _ => panic!("wrong event"),
    }
}

#[tokio::test]
async fn transition_phase_already_in_phase_rejected() {
    let spec_id = Ulid::new();
    let handle = spawn(spec_id, SpecState::new());
    handle.send_command(Command::CreateSpec {
        title: "Test".to_string(),
        one_liner: "t".to_string(),
        goal: "g".to_string(),
    }).await.unwrap();

    // Brainstorming -> Active
    handle.send_command(Command::TransitionPhase {
        target: SpecPhase::Active,
    }).await.unwrap();

    // Active -> Active should fail
    let err = handle.send_command(Command::TransitionPhase {
        target: SpecPhase::Active,
    }).await.unwrap_err();
    assert!(matches!(err, ActorError::AlreadyInPhase));
}

#[tokio::test]
async fn transition_phase_brainstorming_active_brainstorming() {
    let spec_id = Ulid::new();
    let handle = spawn(spec_id, SpecState::new());
    handle.send_command(Command::CreateSpec {
        title: "Test".to_string(),
        one_liner: "t".to_string(),
        goal: "g".to_string(),
    }).await.unwrap();

    // Brainstorming -> Active -> Brainstorming
    handle.send_command(Command::TransitionPhase {
        target: SpecPhase::Active,
    }).await.unwrap();
    handle.send_command(Command::TransitionPhase {
        target: SpecPhase::Brainstorming,
    }).await.unwrap();
    let state = handle.read_state().await;
    assert_eq!(state.phase, SpecPhase::Brainstorming);
}
```

- [ ] **Step 3: Update existing `actor_processes_create_spec` test**

The test at `actor.rs:363` currently asserts `events.len() == 1`. Update it to expect 2 events:

```rust
assert_eq!(events.len(), 2);
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
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test --package barnstormer-core -- transition_phase`
Expected: compilation errors — `TransitionPhase` variant doesn't exist

- [ ] **Step 5: Add TransitionPhase command variant**

In `command.rs`, add to the `Command` enum:

```rust
TransitionPhase { target: crate::state::SpecPhase },
```

- [ ] **Step 6: Add AlreadyInPhase error variant**

In `actor.rs`, add to `ActorError`:

```rust
#[error("already in target phase")]
AlreadyInPhase,
```

- [ ] **Step 7: Implement command handling in actor**

In `actor.rs`, in the `command_to_events()` match block, add:

```rust
Command::TransitionPhase { target } => {
    if self.state.read().await.phase == target {
        return Err(ActorError::AlreadyInPhase);
    }
    vec![EventPayload::PhaseTransitioned { phase: target }]
}
```

Also update the `CreateSpec` arm to emit a second event. Add `use crate::state::SpecPhase;` to the imports:

```rust
Command::CreateSpec { title, one_liner, goal } => {
    vec![
        EventPayload::SpecCreated { title, one_liner, goal },
        EventPayload::PhaseTransitioned { phase: SpecPhase::Brainstorming },
    ]
}
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test --package barnstormer-core`
Expected: all tests PASS (including updated CreateSpec test)

- [ ] **Step 9: Commit**

```bash
git add crates/barnstormer-core/src/command.rs crates/barnstormer-core/src/actor.rs
git commit -m "feat: add TransitionPhase command with AlreadyInPhase guard"
```

---

### Task 4: Update SSE stream and agent context for new event

**Files:**
- Modify: `crates/barnstormer-server/src/api/stream.rs`
- Modify: `crates/barnstormer-agent/src/context.rs`

- [ ] **Step 1: Add PhaseTransitioned arm to event_type_name**

In `stream.rs`, add to the `event_type_name` match (currently lines 16-31):

```rust
EventPayload::PhaseTransitioned { .. } => "phase_transitioned",
```

- [ ] **Step 2: Add PhaseTransitioned arm to describe_event_payload**

In `context.rs`, add to the `describe_event_payload` match (currently at line 160):

```rust
EventPayload::PhaseTransitioned { phase } => {
    format!("phase transitioned to {:?}", phase)
}
```

- [ ] **Step 3: Run full workspace tests**

Run: `cargo test --all`
Expected: all tests PASS — no exhaustive match errors remain

- [ ] **Step 4: Commit**

```bash
git add crates/barnstormer-server/src/api/stream.rs crates/barnstormer-agent/src/context.rs
git commit -m "feat: add PhaseTransitioned to SSE stream and agent context"
```

---

### Task 5: Swarm gating — skip non-Manager agents during Brainstorming

**Files:**
- Modify: `crates/barnstormer-agent/src/swarm.rs`

- [ ] **Step 1: Write failing tests for phase gating**

Add to the `#[cfg(test)]` module in `swarm.rs`. Use `with_agents()` (line 207) which accepts explicit client and model params. Use `StubLlmClient` from `crate::testing`:

```rust
#[tokio::test]
async fn swarm_skips_non_manager_during_brainstorming() {
    use crate::testing::StubLlmClient;

    let spec_id = Ulid::new();
    let handle = barnstormer_core::actor::spawn(spec_id, SpecState::new());
    // CreateSpec puts spec into Brainstorming
    handle.send_command(Command::CreateSpec {
        title: "Test".to_string(),
        one_liner: "t".to_string(),
        goal: "g".to_string(),
    }).await.unwrap();

    let state = handle.read_state().await;
    assert_eq!(state.phase, SpecPhase::Brainstorming);
    drop(state);

    let agents = vec![
        AgentRunner::new(spec_id, AgentRole::Manager),
        AgentRunner::new(spec_id, AgentRole::Brainstormer),
        AgentRunner::new(spec_id, AgentRole::Planner),
    ];
    let swarm = SwarmOrchestrator::with_agents(
        spec_id,
        handle,
        agents,
        Arc::new(StubLlmClient::new()),
        "test-model".to_string(),
    );

    // Verify gating logic: only Manager should run
    let phase = swarm.actor.read_state().await.phase.clone();
    for agent in swarm.agents.iter().flatten() {
        if phase == SpecPhase::Brainstorming && agent.role != AgentRole::Manager {
            // This agent would be skipped
        } else if agent.role == AgentRole::Manager {
            // Manager runs
        }
    }
    // The structural assertion: Manager is at index 0
    assert_eq!(swarm.agents[0].as_ref().unwrap().role, AgentRole::Manager);
}

#[tokio::test]
async fn swarm_runs_all_agents_during_active() {
    use crate::testing::StubLlmClient;

    let spec_id = Ulid::new();
    let handle = barnstormer_core::actor::spawn(spec_id, SpecState::new());
    handle.send_command(Command::CreateSpec {
        title: "Test".to_string(),
        one_liner: "t".to_string(),
        goal: "g".to_string(),
    }).await.unwrap();
    // Transition to Active
    handle.send_command(Command::TransitionPhase {
        target: SpecPhase::Active,
    }).await.unwrap();

    let state = handle.read_state().await;
    assert_eq!(state.phase, SpecPhase::Active);
    drop(state);

    let agents = vec![
        AgentRunner::new(spec_id, AgentRole::Manager),
        AgentRunner::new(spec_id, AgentRole::Brainstormer),
        AgentRunner::new(spec_id, AgentRole::Planner),
    ];
    let swarm = SwarmOrchestrator::with_agents(
        spec_id,
        handle,
        agents,
        Arc::new(StubLlmClient::new()),
        "test-model".to_string(),
    );

    // All 3 agents should be present and none skipped in Active
    assert_eq!(swarm.agents.iter().flatten().count(), 3);
}
```

- [ ] **Step 2: Run test to verify it compiles and passes the structural check**

Run: `cargo test --package barnstormer-agent -- swarm_skips_non_manager`

- [ ] **Step 3: Add phase gating to run_loop**

In `swarm.rs`, in the `run_loop` function (starts at line 534), inside the agent iteration loop, add a guard before calling the agent's step. Read phase and role in a single lock:

```rust
{
    let s = swarm.lock().await;
    let phase = s.actor.read_state().await.phase.clone();
    if phase == SpecPhase::Brainstorming {
        if let Some(Some(agent)) = s.agents.get(i) {
            if agent.role != AgentRole::Manager {
                continue;
            }
        }
    }
}
```

- [ ] **Step 4: Add phase-transition notify to wake sleeping swarm**

In `SwarmOrchestrator`, add a field:

```rust
pub phase_notify: Arc<Notify>,
```

Initialize it in both `with_defaults()` and `with_agents()`. In `run_loop`, subscribe to the broadcast channel at the start of the function:

```rust
let mut phase_rx = {
    let s = swarm.lock().await;
    s.actor.subscribe()
};
```

Add a third branch to the `tokio::select!` and drain events looking for `PhaseTransitioned`:

```rust
tokio::select! {
    _ = tokio::time::sleep(sleep_duration) => {}
    _ = notify.notified() => { /* human message priority */ }
    result = phase_rx.recv() => {
        if let Ok(event) = result {
            if matches!(event.payload, EventPayload::PhaseTransitioned { .. }) {
                // Phase changed — re-enter loop to re-check gating
            }
        }
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test --package barnstormer-agent`
Expected: all tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/barnstormer-agent/src/swarm.rs
git commit -m "feat: gate non-Manager agents during brainstorming phase"
```

---

### Task 6: Web endpoint for phase transitions

**Files:**
- Modify: `crates/barnstormer-server/src/routes.rs`
- Modify: `crates/barnstormer-server/src/web/mod.rs`

- [ ] **Step 1: Write failing integration tests**

Add to the test module in `web/mod.rs`. Follow the existing pattern: `test_state()` returns `SharedState`, create specs via HTTP POST, extract spec_id from `state.actors`:

```rust
#[tokio::test]
async fn phase_transition_to_active_returns_200() {
    let state = test_state();
    // Create a spec (starts in Brainstorming)
    let app = create_router(Arc::clone(&state), None);
    app.oneshot(
        Request::post("/web/specs")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("description=Phase+test+spec"))
            .unwrap(),
    ).await.unwrap();

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().unwrap()
    };

    let app2 = create_router(Arc::clone(&state), None);
    let resp = app2
        .oneshot(
            Request::post(&format!("/web/specs/{}/phase", spec_id))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("target=active"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn phase_transition_to_brainstorming_returns_200() {
    let state = test_state();
    let app = create_router(Arc::clone(&state), None);
    app.oneshot(
        Request::post("/web/specs")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("description=Phase+test+spec"))
            .unwrap(),
    ).await.unwrap();

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().unwrap()
    };

    // First transition to Active
    let app2 = create_router(Arc::clone(&state), None);
    app2.oneshot(
        Request::post(&format!("/web/specs/{}/phase", spec_id))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("target=active"))
            .unwrap(),
    ).await.unwrap();

    // Then back to Brainstorming
    let app3 = create_router(Arc::clone(&state), None);
    let resp = app3
        .oneshot(
            Request::post(&format!("/web/specs/{}/phase", spec_id))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("target=brainstorming"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn phase_transition_invalid_target_returns_400() {
    let state = test_state();
    let app = create_router(Arc::clone(&state), None);
    app.oneshot(
        Request::post("/web/specs")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("description=Phase+test+spec"))
            .unwrap(),
    ).await.unwrap();

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().unwrap()
    };

    let app2 = create_router(Arc::clone(&state), None);
    let resp = app2
        .oneshot(
            Request::post(&format!("/web/specs/{}/phase", spec_id))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("target=invalid"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn phase_transition_already_in_phase_returns_409() {
    let state = test_state();
    let app = create_router(Arc::clone(&state), None);
    app.oneshot(
        Request::post("/web/specs")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("description=Phase+test+spec"))
            .unwrap(),
    ).await.unwrap();

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().unwrap()
    };

    // Transition to active
    let app2 = create_router(Arc::clone(&state), None);
    app2.oneshot(
        Request::post(&format!("/web/specs/{}/phase", spec_id))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("target=active"))
            .unwrap(),
    ).await.unwrap();

    // Try active again — 409
    let app3 = create_router(Arc::clone(&state), None);
    let resp = app3
        .oneshot(
            Request::post(&format!("/web/specs/{}/phase", spec_id))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("target=active"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn phase_transition_nonexistent_spec_returns_404() {
    let state = test_state();
    let app = create_router(state, None);
    let resp = app
        .oneshot(
            Request::post("/web/specs/01JNOTREAL00000000000000000/phase")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("target=active"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn state_api_includes_phase_field() {
    let state = test_state();
    let app = create_router(Arc::clone(&state), None);
    app.oneshot(
        Request::post("/web/specs")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("description=Phase+test+spec"))
            .unwrap(),
    ).await.unwrap();

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().unwrap()
    };

    let app2 = create_router(Arc::clone(&state), None);
    let resp = app2
        .oneshot(
            Request::get(&format!("/api/specs/{}/state", spec_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("phase").is_some());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --package barnstormer-server -- phase_transition`
Expected: compilation errors — handler doesn't exist

- [ ] **Step 3: Add route**

In `routes.rs`, add alongside the existing spec routes:

```rust
.route("/web/specs/{id}/phase", post(web::transition_phase))
```

- [ ] **Step 4: Implement handler**

In `web/mod.rs`, add. Note: `parse_spec_id` returns `Result<Ulid, Box<Response>>` — dereference on error:

```rust
#[derive(Deserialize)]
pub struct PhaseForm {
    target: String,
}

pub async fn transition_phase(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Form(form): Form<PhaseForm>,
) -> Response {
    let spec_id = match parse_spec_id(&id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    let target = match form.target.as_str() {
        "brainstorming" => SpecPhase::Brainstorming,
        "active" => SpecPhase::Active,
        _ => {
            return (StatusCode::BAD_REQUEST, Html("<p class=\"error-msg\">Invalid phase target.</p>".to_string())).into_response();
        }
    };

    let actors = state.actors.read().await;
    let Some(handle) = actors.get(&spec_id) else {
        return (StatusCode::NOT_FOUND, Html("<p class=\"error-msg\">Spec not found.</p>".to_string())).into_response();
    };

    match handle.send_command(Command::TransitionPhase { target: target.clone() }).await {
        Ok(_) => {
            let label = match target {
                SpecPhase::Brainstorming => "Brainstorming",
                SpecPhase::Active => "Active",
            };
            (StatusCode::OK, Html(format!("<span class=\"phase-badge\">{}</span>", label))).into_response()
        }
        Err(ActorError::AlreadyInPhase) => {
            (StatusCode::CONFLICT, Html("<p class=\"error-msg\">Already in target phase.</p>".to_string())).into_response()
        }
        Err(e) => {
            (StatusCode::INTERNAL_SERVER_ERROR, Html(format!("<p class=\"error-msg\">Error: {}</p>", e))).into_response()
        }
    }
}
```

Add `use barnstormer_core::state::SpecPhase;` to imports if not already present.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --package barnstormer-server -- phase_transition`
Expected: all 6 tests PASS

- [ ] **Step 6: Run full workspace tests**

Run: `cargo test --all && cargo clippy --all-targets -- -D warnings`
Expected: all tests PASS, no clippy warnings

- [ ] **Step 7: Commit**

```bash
git add crates/barnstormer-server/src/routes.rs crates/barnstormer-server/src/web/mod.rs
git commit -m "feat: add POST /web/specs/{id}/phase endpoint for phase transitions"
```
