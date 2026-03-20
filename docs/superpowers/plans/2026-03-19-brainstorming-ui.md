# Brainstorming UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Provide full-width chat layout during brainstorming phase with board peeking, hidden canvas container, and smooth phase transition UX.

**Architecture:** Conditional rendering in `spec_view.html` based on `state.phase`. Chat partials reused with `container_id` parameter for brainstorming (full-width in `#canvas`) vs active (sidebar in `chat-rail`). Phase transitions trigger full workspace re-fetch via SSE.

**Tech Stack:** Askama templates, HTMX, CSS, SSE

**Spec:** `docs/superpowers/specs/2026-03-19-brainstorming-ui-design.md`

**Prerequisites:** Plan 1 (Phase Model) MUST be fully implemented first. Before starting, verify these exist:
- `SpecPhase` enum in `crates/barnstormer-core/src/state.rs`
- `state.phase` field on `SpecState`
- `Command::TransitionPhase` in `command.rs`
- `POST /web/specs/{id}/phase` endpoint

If any are missing, STOP and implement Plan 1 first.

---

## File Structure

| Action | File | Responsibility |
|--------|------|---------------|
| Modify | `crates/barnstormer-server/src/web/mod.rs` | Pass `phase`, `container_id` to templates; update `sanitize_container_id` |
| Modify | `templates/partials/spec_view.html` | Conditional brainstorming vs active layout, phase badge, View Board/Back to Chat, SSE handler |
| Modify | `templates/partials/chat_panel.html` | Accept `container_id`, `chat-fullwidth` wrapper class |
| Modify | `static/style.css` | Full-width chat, canvas container, phase badge, button styles |

---

### Task 1: Pass phase to spec view template and update SpecViewTemplate

**Files:**
- Modify: `crates/barnstormer-server/src/web/mod.rs`

- [ ] **Step 1: Write failing test for phase in spec view context**

Add to the test module in `web/mod.rs`. Follow existing pattern: `test_state()` + POST to create spec:

```rust
#[tokio::test]
async fn spec_view_brainstorming_contains_phase_marker() {
    let state = test_state();
    let app = create_router(Arc::clone(&state), None);
    app.oneshot(
        Request::post("/web/specs")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("description=Brainstorming+UI+test"))
            .unwrap(),
    ).await.unwrap();

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().unwrap()
    };

    // New specs start in Brainstorming
    let app2 = create_router(Arc::clone(&state), None);
    let resp = app2
        .oneshot(
            Request::get(&format!("/web/specs/{}", spec_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("data-view=\"brainstorming\""), "should have brainstorming marker");
    assert!(html.contains("phase-brainstorming"), "should have brainstorming badge");
    assert!(html.contains("View Board"), "should have View Board button");
    assert!(html.contains("agent-canvas"), "should have agent-canvas container");
    assert!(!html.contains("Resume Brainstorming"), "should not have Resume button in brainstorming");
}

#[tokio::test]
async fn spec_view_active_contains_tab_toggles() {
    let state = test_state();
    let app = create_router(Arc::clone(&state), None);
    app.oneshot(
        Request::post("/web/specs")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("description=Active+UI+test"))
            .unwrap(),
    ).await.unwrap();

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().unwrap()
    };

    // Transition to Active
    let actors = state.actors.read().await;
    let handle = actors.get(&spec_id).unwrap();
    handle.send_command(Command::TransitionPhase {
        target: SpecPhase::Active,
    }).await.unwrap();
    drop(actors);

    let app2 = create_router(Arc::clone(&state), None);
    let resp = app2
        .oneshot(
            Request::get(&format!("/web/specs/{}", spec_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("data-view=\"document\""), "should have document tab toggle");
    assert!(html.contains("Resume Brainstorming"), "should have Resume button");
    assert!(!html.contains("data-view=\"brainstorming\""), "should not have brainstorming marker");
    assert!(!html.contains("phase-brainstorming"), "should not have brainstorming badge");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --package barnstormer-server -- spec_view_brainstorming`
Expected: FAIL — template doesn't render phase-conditional content yet

- [ ] **Step 3: Update SpecViewTemplate struct to include phase**

In `web/mod.rs`, update the `SpecViewTemplate` struct (currently at line 548). Keep ALL existing fields, add `phase`:

```rust
pub struct SpecViewTemplate {
    pub spec_id: String,
    pub title: String,
    pub one_liner: String,
    pub goal: String,
    pub lanes: Vec<LaneData>,
    pub phase: String,  // "brainstorming" or "active"
}
```

Update the `spec_view` handler (line 557) to read phase from state and pass it:

```rust
let phase = match spec_state.phase {
    SpecPhase::Brainstorming => "brainstorming".to_string(),
    SpecPhase::Active => "active".to_string(),
};
```

Update ALL existing `SpecViewTemplate` instantiations in tests (search for `SpecViewTemplate {`) to include `phase: "active".to_string()` for backward compat.

- [ ] **Step 4: Commit handler changes**

```bash
git add crates/barnstormer-server/src/web/mod.rs
git commit -m "feat: pass phase to spec view template"
```

---

### Task 2: Conditional brainstorming vs active layout in spec_view.html

**Files:**
- Modify: `templates/partials/spec_view.html`

- [ ] **Step 1: Add conditional rendering based on phase**

The current template has: `spec-compositor` div with SSE, command bar header, `spec-body` wrapper with `<main class="canvas">` and `<aside class="chat-rail">`, and JavaScript.

Wrap in a phase conditional. Keep the existing active layout intact, add the brainstorming layout. Use `querySelector('.spec-compositor')` consistently (matching existing JS at line 75):

```html
{% if phase == "brainstorming" %}
  <!-- Brainstorming layout -->
  <div class="spec-compositor"
       hx-ext="sse" sse-connect="/api/specs/{{ spec_id }}/events/stream"
       data-view="brainstorming">

    <!-- Command bar -->
    <header class="command-bar">
      <div class="command-bar-left">
        <h2 class="spec-title">{{ title }}</h2>
        <span class="phase-badge phase-brainstorming">Brainstorming</span>
      </div>
      <div class="command-bar-right">
        <button id="btn-view-board" class="btn btn-sm"
                hx-get="/web/specs/{{ spec_id }}/board"
                hx-target="#canvas" hx-swap="innerHTML">View Board</button>
        <button id="btn-back-to-chat" class="btn btn-sm" style="display:none;"
                hx-get="/web/specs/{{ spec_id }}/chat-panel"
                hx-target="#canvas" hx-swap="innerHTML">Back to Chat</button>
      </div>
    </header>

    <div class="spec-body">
      <!-- Main content: full-width chat -->
      <main class="canvas" id="canvas"
            hx-get="/web/specs/{{ spec_id }}/chat-panel"
            hx-trigger="load" hx-swap="innerHTML">
      </main>

      <!-- Hidden canvas container for Spec 3 -->
      <div id="agent-canvas" style="display:none;"></div>
    </div>
  </div>

  <script>
    (function() {
      // View Board / Back to Chat toggle
      var btnBoard = document.getElementById('btn-view-board');
      var btnChat = document.getElementById('btn-back-to-chat');
      if (btnBoard) {
        btnBoard.addEventListener('click', function() {
          btnBoard.style.display = 'none';
          btnChat.style.display = '';
        });
      }
      if (btnChat) {
        btnChat.addEventListener('click', function() {
          btnChat.style.display = 'none';
          btnBoard.style.display = '';
        });
      }

      // Phase transition handler — re-fetch entire workspace
      var compositor = document.querySelector('.spec-compositor');
      if (compositor) {
        compositor.addEventListener('sse:phase_transitioned', function() {
          htmx.ajax('GET', '/web/specs/{{ spec_id }}', {target: '#workspace', swap: 'innerHTML'});
        });
      }
    })();
  </script>

{% else %}
  <!-- Active layout (existing behavior, preserved as-is) -->
  <!-- ... keep the ENTIRE existing spec_view.html content here ... -->
  <!-- Add "Resume Brainstorming" button to the command bar -->
  <!-- Add sse:phase_transitioned listener to existing JS -->
{% endif %}
```

In the active (else) branch, add to the command bar's right section:

```html
<button class="btn btn-sm"
        hx-post="/web/specs/{{ spec_id }}/phase"
        hx-vals='{"target":"brainstorming"}'
        hx-swap="none">Resume Brainstorming</button>
```

And add the phase transition SSE listener to existing JS:

```javascript
var compositor = document.querySelector('.spec-compositor');
if (compositor) {
  compositor.addEventListener('sse:phase_transitioned', function() {
    htmx.ajax('GET', '/web/specs/{{ spec_id }}', {target: '#workspace', swap: 'innerHTML'});
  });
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test --package barnstormer-server -- spec_view_brainstorming`
Expected: both tests PASS

- [ ] **Step 3: Commit**

```bash
git add templates/partials/spec_view.html
git commit -m "feat: conditional brainstorming vs active layout in spec view"
```

---

### Task 3: Update chat_panel for full-width mode and fix sanitize_container_id

**Files:**
- Modify: `templates/partials/chat_panel.html`
- Modify: `crates/barnstormer-server/src/web/mod.rs`

- [ ] **Step 1: Write failing tests for chat panel container_id**

```rust
#[tokio::test]
async fn chat_panel_brainstorming_has_fullwidth_class() {
    let state = test_state();
    let app = create_router(Arc::clone(&state), None);
    app.oneshot(
        Request::post("/web/specs")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("description=Chat+fullwidth+test"))
            .unwrap(),
    ).await.unwrap();

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().unwrap()
    };

    // Spec starts in Brainstorming
    let app2 = create_router(Arc::clone(&state), None);
    let resp = app2
        .oneshot(
            Request::get(&format!("/web/specs/{}/chat-panel", spec_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("chat-fullwidth"), "should have fullwidth class in brainstorming");
}

#[tokio::test]
async fn chat_panel_active_no_fullwidth_class() {
    let state = test_state();
    let app = create_router(Arc::clone(&state), None);
    app.oneshot(
        Request::post("/web/specs")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("description=Chat+active+test"))
            .unwrap(),
    ).await.unwrap();

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().unwrap()
    };

    // Transition to Active
    let actors = state.actors.read().await;
    let handle = actors.get(&spec_id).unwrap();
    handle.send_command(Command::TransitionPhase {
        target: SpecPhase::Active,
    }).await.unwrap();
    drop(actors);

    let app2 = create_router(Arc::clone(&state), None);
    let resp = app2
        .oneshot(
            Request::get(&format!("/web/specs/{}/chat-panel", spec_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(!html.contains("chat-fullwidth"), "should not have fullwidth class in active");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --package barnstormer-server -- chat_panel_brainstorming`
Expected: FAIL

- [ ] **Step 3: Update sanitize_container_id allowlist**

In `web/mod.rs`, update `sanitize_container_id` (line 1172) to accept the new values:

```rust
fn sanitize_container_id(raw: &str) -> String {
    match raw {
        "activity-transcript" | "chat-transcript" | "mission-ticker" | "canvas" | "chat-rail" => raw.to_string(),
        _ => "chat-transcript".to_string(),
    }
}
```

- [ ] **Step 4: Update ChatPanelTemplate and handler**

Add `is_fullwidth` to the `ChatPanelTemplate` struct (currently at line 1318):

```rust
pub struct ChatPanelTemplate {
    pub spec_id: String,
    pub container_id: String,
    pub is_fullwidth: bool,
    pub transcript: Vec<TranscriptEntry>,
    pub pending_question: Option<QuestionData>,
}
```

Update the `chat_panel` handler (line 1326) to read phase and set container_id:

```rust
let is_fullwidth = spec_state.phase == SpecPhase::Brainstorming;
let container_id = if is_fullwidth {
    "canvas".to_string()
} else {
    "chat-transcript".to_string()
};
```

Update existing `ChatPanelTemplate` instantiations in tests to include `is_fullwidth: false`.

- [ ] **Step 5: Update chat_panel.html template**

Add a wrapper div with conditional class:

```html
<div class="chat-panel {% if is_fullwidth %}chat-fullwidth{% endif %}">
  <!-- existing chat panel content unchanged -->
</div>
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --package barnstormer-server -- chat_panel`
Expected: all chat_panel tests PASS

- [ ] **Step 7: Commit**

```bash
git add templates/partials/chat_panel.html crates/barnstormer-server/src/web/mod.rs
git commit -m "feat: chat panel supports full-width mode during brainstorming"
```

---

### Task 4: CSS for brainstorming layout

**Files:**
- Modify: `static/style.css`

- [ ] **Step 1: Add brainstorming-specific CSS**

Add these styles. Note: use existing CSS variables where available (check `style.css` for `--spacing-sm`, `--text-secondary`, `--border-subtle`, `--agent-accent`):

```css
/* Phase badge */
.phase-badge {
  display: inline-block;
  padding: 2px 10px;
  border-radius: 12px;
  font-size: 0.75rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.5px;
}

.phase-brainstorming {
  background: var(--agent-accent, #7c3aed);
  color: #fff;
}

/* Full-width chat during brainstorming */
.chat-fullwidth {
  max-width: 720px;
  margin: 0 auto;
  padding: var(--spacing-md);
}

.chat-fullwidth .chat-input-area {
  max-width: 720px;
  margin: 0 auto;
}

/* Agent canvas container (hidden by default, Spec 3 wires it up) */
#agent-canvas {
  display: none;
  flex: 1;
  min-width: 300px;
  overflow-y: auto;
  padding: var(--spacing-md);
  border-left: 1px solid var(--border-subtle);
}

/* When agent-canvas is visible, brainstorming body becomes flex row */
[data-view="brainstorming"] .spec-body {
  display: flex;
  flex: 1;
}

[data-view="brainstorming"] .spec-body > .canvas {
  flex: 1;
  min-width: 300px;
}
```

- [ ] **Step 2: Verify no template compilation issues**

Run: `cargo test --all`

- [ ] **Step 3: Commit**

```bash
git add static/style.css
git commit -m "feat: add brainstorming layout CSS, phase badge, canvas container styles"
```

---

### Task 5: Board peeking during brainstorming

**Files:**
- Modify: `crates/barnstormer-server/src/web/mod.rs`

- [ ] **Step 1: Write test documenting board access during brainstorming**

```rust
#[tokio::test]
async fn board_returns_200_during_brainstorming() {
    let state = test_state();
    let app = create_router(Arc::clone(&state), None);
    app.oneshot(
        Request::post("/web/specs")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("description=Board+peek+test"))
            .unwrap(),
    ).await.unwrap();

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().unwrap()
    };

    // Spec starts in Brainstorming — board should still work
    let app2 = create_router(Arc::clone(&state), None);
    let resp = app2
        .oneshot(
            Request::get(&format!("/web/specs/{}/board", spec_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test --package barnstormer-server -- board_returns_200`
Expected: PASS — board endpoint works regardless of phase

- [ ] **Step 3: Commit**

```bash
git add crates/barnstormer-server/src/web/mod.rs
git commit -m "test: verify board endpoint works during brainstorming phase"
```

---

### Task 6: Final integration verification

- [ ] **Step 1: Run full workspace tests**

Run: `cargo test --all`
Expected: all tests PASS

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings

- [ ] **Step 3: Commit if any fixes were needed**

```bash
git commit -m "chore: fix clippy warnings from brainstorming UI implementation"
```
