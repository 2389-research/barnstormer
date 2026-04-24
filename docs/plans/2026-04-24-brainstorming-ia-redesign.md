# Brainstorming IA Redesign Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans or superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** Restructure the brainstorming phase UI so chat is the main area, with a right sidebar that tabs between Cards (captured output) and Context (uploaded reference). Delete the Canvas concept end-to-end. Card/context events update the inactive tab's notification dot so users don't miss captures while focused on conversation.

**Architecture:** Mirror refine's layout pattern, inverted: brainstorm has chat-as-main + sidebar-tabs; refine keeps main-tabs + chat-sidebar. Tab-switching and notification state live client-side in a small JS module; SSE-driven re-fetch keeps each tab's content fresh via the declarative `hx-trigger` pattern we just wired.

**Tech Stack:** Axum handler + Askama partial for cards feed; vanilla JS for tab switching + notification dots; existing CSS patterns (`.view-toggles-capsule`, `.card-type`, `.badge-*`) extended with a `.sidebar-tabs` family; SSE events `card_created/updated/moved/deleted` and `context_attached/summarized/notes_updated/removed`.

---

## Task 1: Cards feed endpoint + partial

Create the server side of the Cards tab — a flat, reverse-chronological list of cards (newest first), type-color-coded, with click-to-expand body via native `<details>`/`<summary>`. No lane/ordering since brainstorming has no structure yet.

**Files:**
- Create: `templates/partials/cards_feed.html`
- Modify: `crates/barnstormer-server/src/web/mod.rs` — add `CardsFeedTemplate` + `cards_feed` handler (near `canvas_fragment`, ~line 988)
- Modify: `crates/barnstormer-server/src/routes.rs` — register `GET /web/specs/{id}/cards-feed`

**Step 1: Write the failing test**

In `crates/barnstormer-server/src/web/mod.rs` tests module:

```rust
#[tokio::test]
async fn cards_feed_returns_empty_state_when_no_cards() {
    let state = test_state();
    let app = create_router(Arc::clone(&state), None);
    app.oneshot(Request::post("/web/specs").header("content-type", MP_CONTENT_TYPE).body(mp_description_body("Cards feed empty")).unwrap()).await.unwrap();
    let spec_id = { let actors = state.actors.read().await; *actors.keys().next().unwrap() };

    let app2 = create_router(Arc::clone(&state), None);
    let resp = app2.oneshot(Request::get(&format!("/web/specs/{}/cards-feed", spec_id)).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let html = String::from_utf8(axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
    assert!(html.contains("No cards captured yet"), "empty state must hint at expected behavior: {}", html);
    assert!(html.contains("sse:card_created"), "must re-trigger on card SSE events");
}

#[tokio::test]
async fn cards_feed_renders_cards_newest_first() {
    let state = test_state();
    let app = create_router(Arc::clone(&state), None);
    app.oneshot(Request::post("/web/specs").header("content-type", MP_CONTENT_TYPE).body(mp_description_body("Cards feed ordering")).unwrap()).await.unwrap();
    let spec_id = { let actors = state.actors.read().await; *actors.keys().next().unwrap() };

    {
        let actors = state.actors.read().await;
        let handle = actors.get(&spec_id).unwrap();
        for title in ["First", "Second", "Third"] {
            handle.send_command(Command::CreateCard {
                card_type: "idea".to_string(),
                title: title.to_string(),
                body: None,
                lane: None,
                created_by: "manager".to_string(),
                source_attachment_id: None,
            }).await.unwrap();
        }
    }

    let app2 = create_router(Arc::clone(&state), None);
    let resp = app2.oneshot(Request::get(&format!("/web/specs/{}/cards-feed", spec_id)).body(Body::empty()).unwrap()).await.unwrap();
    let html = String::from_utf8(axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
    let third_pos = html.find("Third").expect("Third missing");
    let first_pos = html.find("First").expect("First missing");
    assert!(third_pos < first_pos, "newest card must render first (reverse chrono)");
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --package barnstormer-server cards_feed_ -- --nocapture`
Expected: compile error — `cards_feed` handler and route don't exist.

**Step 3: Add the template**

Write `templates/partials/cards_feed.html`:

```html
{# ABOUTME: Cards feed for the brainstorming sidebar: reverse-chrono list of all captured cards. #}
{# ABOUTME: Self-refreshing on card SSE events via hx-trigger on the wrapper element. #}

<div id="cards-feed"
     class="cards-feed"
     hx-get="/web/specs/{{ spec_id }}/cards-feed"
     hx-trigger="sse:card_created, sse:card_updated, sse:card_moved, sse:card_deleted"
     hx-swap="outerHTML">
    {% if cards.is_empty() %}
    <div class="cards-feed-empty">
        <p>No cards captured yet.</p>
        <p class="cards-feed-empty-hint">As you brainstorm, the Manager will capture ideas, questions, and constraints as cards.</p>
    </div>
    {% else %}
    {% for card in cards %}
    <details class="card-feed-item" id="card-feed-{{ card.card_id }}">
        <summary class="card-feed-head">
            <span class="card-type badge-{{ card.card_type }}">{{ card.card_type }}</span>
            <span class="card-feed-title">{{ card.title }}</span>
        </summary>
        {% if let Some(html) = card.body_html %}
        <div class="card-feed-body">{{ html|safe }}</div>
        {% endif %}
        <div class="card-feed-meta">by {{ card.created_by }} &middot; {{ card.updated_at }}</div>
    </details>
    {% endfor %}
    {% endif %}
</div>
```

**Step 4: Add the handler + struct**

In `crates/barnstormer-server/src/web/mod.rs`, near the existing `AgentCanvasTemplate` (~line 990):

```rust
/// Cards feed partial: reverse-chronological list of all captured cards for the
/// brainstorming sidebar. Self-refreshes on card SSE events.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/cards_feed.html")]
pub struct CardsFeedTemplate {
    pub spec_id: String,
    pub cards: Vec<CardData>,
}

/// GET /web/specs/{id}/cards-feed - Render the flat card list for the brainstorm sidebar.
pub async fn cards_feed(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let spec_id = match parse_spec_id(&id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };
    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h,
        None => return (StatusCode::NOT_FOUND, Html("<p class=\"error-msg\">Spec not found.</p>".to_string())).into_response(),
    };
    let spec_state = handle.read_state().await;
    // Newest-first: sort by updated_at descending.
    let mut cards: Vec<CardData> = spec_state.cards.iter().map(CardData::from_card).collect();
    cards.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    CardsFeedTemplate { spec_id: id, cards }.into_response()
}
```

**Step 5: Register the route**

In `crates/barnstormer-server/src/routes.rs`, under the other `/web/specs/{id}/*` routes:

```rust
.route("/web/specs/{id}/cards-feed", get(web::cards_feed))
```

**Step 6: Run tests to verify they pass**

Run: `cargo test --package barnstormer-server cards_feed_`
Expected: both tests pass.

**Step 7: Commit**

```bash
git add templates/partials/cards_feed.html crates/barnstormer-server/src/web/mod.rs crates/barnstormer-server/src/routes.rs
git commit -m "feat(web): cards-feed endpoint for brainstorm sidebar"
```

---

## Task 2: Restructure brainstorming layout — chat main, sidebar tabs

Swap the brainstorming phase's layout so the main canvas area holds the chat, and the right rail becomes a tab panel that switches between Cards feed (default) and Context panel. Delete the `{% include "partials/agent_canvas.html" %}` — canvas is gone.

**Files:**
- Modify: `templates/partials/spec_view.html` — brainstorming branch body (`{% if phase == "brainstorming" %}` block)
- Modify: `static/style.css` — add `.sidebar-tabs`, `.sidebar-tab-toggle`, `.sidebar-tab-content`, `.card-feed-*` rules

**Step 1: Write the failing test**

Add to `crates/barnstormer-server/src/web/mod.rs` tests:

```rust
#[tokio::test]
async fn brainstorming_layout_has_sidebar_tabs_and_no_canvas() {
    let state = test_state();
    let app = create_router(Arc::clone(&state), None);
    app.oneshot(Request::post("/web/specs").header("content-type", MP_CONTENT_TYPE).body(mp_description_body("Sidebar tabs test")).unwrap()).await.unwrap();
    let spec_id = { let actors = state.actors.read().await; *actors.keys().next().unwrap() };

    let app2 = create_router(Arc::clone(&state), None);
    let resp = app2.oneshot(Request::get(&format!("/web/specs/{}", spec_id)).header("HX-Request", "true").body(Body::empty()).unwrap()).await.unwrap();
    let html = String::from_utf8(axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();

    assert!(html.contains("sidebar-tab-toggle"), "must render tab toggles");
    assert!(html.contains("data-tab=\"cards\""), "must have cards tab button");
    assert!(html.contains("data-tab=\"context\""), "must have context tab button");
    assert!(html.contains("cards-feed"), "cards panel must load feed");
    assert!(!html.contains("agent-canvas"), "canvas is deleted — element must not render");
}
```

**Step 2: Run to verify fail**

Run: `cargo test --package barnstormer-server brainstorming_layout_has_sidebar_tabs`
Expected: fail on missing `sidebar-tab-toggle` marker.

**Step 3: Rewrite the brainstorming body**

Replace the brainstorming layout block in `templates/partials/spec_view.html` (the section between `{% if phase == "brainstorming" %}` and `{% else if phase == "refining" %}`):

```html
{% if phase == "brainstorming" %}
{# Brainstorming layout: chat dominant, right sidebar with Cards | Context tabs. #}
<div class="spec-compositor" hx-ext="sse" sse-connect="/api/specs/{{ spec_id }}/events/stream"
     data-view="brainstorming">
{# Hidden sentinel: re-fetches the whole workspace when the phase changes.
   Kept separate so hx-target="#workspace" does not inherit onto siblings. #}
<span id="sse-phase-sub" style="display:none"
      hx-trigger="sse:phase_transitioned"
      hx-get="/web/specs/{{ spec_id }}"
      hx-target="#workspace"
      hx-swap="innerHTML"></span>

<header class="command-bar">
    <div class="command-bar-left">
        <span class="command-bar-title">{{ title }}</span>
        <span class="command-bar-chevron">&#8250;</span>
        <span class="command-bar-subtitle">{{ one_liner }}</span>
    </div>
    <div class="command-bar-right">
        <div id="agent-controls" hx-get="/web/specs/{{ spec_id }}/agents/status"
             hx-trigger="load, sse:agent_step_started, sse:agent_step_finished, refreshAgents from:body"
             hx-swap="innerHTML"></div>
    </div>
    <span class="tooltip command-bar-tooltip">{{ one_liner }}</span>
</header>
{% include "partials/phase_stepper.html" %}

<div class="spec-body">
    <main class="canvas" id="canvas"
          hx-get="/web/specs/{{ spec_id }}/chat-panel"
          hx-trigger="load" hx-swap="innerHTML">
    </main>
    <aside class="sidebar-tabs" id="brainstorm-sidebar">
        <div class="sidebar-tab-toggles" role="tablist">
            <button type="button" class="sidebar-tab-toggle active" data-tab="cards" role="tab" aria-selected="true">
                <span class="sidebar-tab-label">Cards</span>
                <span class="sidebar-tab-badge" aria-hidden="true"></span>
            </button>
            <button type="button" class="sidebar-tab-toggle" data-tab="context" role="tab" aria-selected="false">
                <span class="sidebar-tab-label">Context</span>
                <span class="sidebar-tab-badge" aria-hidden="true"></span>
            </button>
        </div>
        <div class="sidebar-tab-panel" data-panel="cards"
             hx-get="/web/specs/{{ spec_id }}/cards-feed"
             hx-trigger="load" hx-swap="innerHTML">
        </div>
        <div class="sidebar-tab-panel" data-panel="context" style="display:none;"
             hx-get="/web/specs/{{ spec_id }}/context-panel"
             hx-trigger="load" hx-swap="innerHTML">
        </div>
    </aside>
</div>
<div id="agents-offline-banner" class="agents-offline-banner">
    <button class="agents-offline-dismiss" onclick="this.parentElement.style.display='none'" title="Dismiss">&times;</button>
    <span>Agents are not running.</span>
    <button class="btn btn-start-agents"
            hx-post="/web/specs/{{ spec_id }}/agents/start"
            hx-target="#agent-controls"
            hx-swap="innerHTML">Start Agents</button>
</div>

</div>

<script>
    // Polling fallback: if SSE drops, check phase every 15 seconds
    (function() {
        var currentPhase = '{{ phase }}';
        setInterval(function() {
            fetch('/web/specs/{{ spec_id }}/phase-check')
                .then(function(r) { return r.text(); })
                .then(function(serverPhase) {
                    if (serverPhase !== currentPhase) {
                        htmx.ajax('GET', '/web/specs/{{ spec_id }}', {target: '#workspace', swap: 'innerHTML'});
                    }
                })
                .catch(function() {});
        }, 15000);
    })();
</script>
```

Note: This task focuses on markup. Tab switching + notification JS comes in Task 3.

**Step 4: Minimum CSS for tabs**

Append to `static/style.css`:

```css
.sidebar-tabs {
    display: flex;
    flex-direction: column;
    min-width: 320px;
    max-width: 380px;
    border-left: 1px solid var(--border-color);
    background: var(--panel-bg);
}

.sidebar-tab-toggles {
    display: flex;
    border-bottom: 1px solid var(--border-color);
}

.sidebar-tab-toggle {
    flex: 1;
    padding: var(--spacing-sm) var(--spacing-md);
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    cursor: pointer;
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--spacing-xs);
    font-size: 0.9rem;
    color: var(--text-muted);
}

.sidebar-tab-toggle.active {
    color: var(--text-primary);
    border-bottom-color: var(--accent-color);
}

.sidebar-tab-badge {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: transparent;
    transition: background 120ms ease;
}

.sidebar-tab-toggle.has-notification .sidebar-tab-badge {
    background: var(--accent-color);
}

.sidebar-tab-panel {
    flex: 1;
    overflow-y: auto;
    min-height: 0;
}

/* Cards feed items */
.cards-feed { padding: var(--spacing-md); }
.cards-feed-empty { text-align: center; color: var(--text-muted); padding: var(--spacing-lg); }
.cards-feed-empty-hint { font-size: 0.82rem; margin-top: var(--spacing-sm); }
.card-feed-item { margin-bottom: var(--spacing-sm); padding: var(--spacing-sm); border-radius: 4px; background: var(--card-bg); }
.card-feed-head { display: flex; align-items: center; gap: var(--spacing-sm); cursor: pointer; }
.card-feed-title { flex: 1; font-weight: 500; word-break: break-word; }
.card-feed-body { margin-top: var(--spacing-sm); font-size: 0.9rem; }
.card-feed-meta { margin-top: var(--spacing-xs); font-size: 0.75rem; color: var(--text-muted); }
```

**Step 5: Run tests**

Run: `cargo test --package barnstormer-server brainstorming_layout_has_sidebar_tabs`
Expected: pass.

Run full test suite: `cargo test --all`
Expected: no regressions (the old `spec_view_brainstorming_contains_canvas_listener` and `canvas_fragment_*` tests will need updates — they'll be removed in Task 4).

**Step 6: Smoke-test in browser**

Restart server, load a brainstorming spec, confirm:
- Chat renders in main area
- Right sidebar shows Cards tab (active) with feed
- Cards tab is still the only thing visible (tab switch wired next)

**Step 7: Commit**

```bash
git add templates/partials/spec_view.html static/style.css crates/barnstormer-server/src/web/mod.rs
git commit -m "feat(web): restructure brainstorming layout to chat main + sidebar tabs"
```

---

## Task 3: Tab switching + notification badges

Wire client-side tab switching (show/hide the two panels) and SSE-driven notification dots on inactive tabs. When `sse:card_*` fires and Cards tab is inactive, its dot lights up. Same for Context tab on `sse:context_*`. Clicking a tab clears its dot.

**Files:**
- Modify: `templates/partials/spec_view.html` — add `<script>` at end of brainstorming block

**Step 1: Add the JS**

Replace the existing brainstorming `<script>` polling block with this combined version:

```html
<script>
    (function() {
        var sidebar = document.getElementById('brainstorm-sidebar');
        if (!sidebar) return;

        var toggles = sidebar.querySelectorAll('.sidebar-tab-toggle');
        var panels = sidebar.querySelectorAll('.sidebar-tab-panel');

        function activate(tabName) {
            toggles.forEach(function(t) {
                var on = t.getAttribute('data-tab') === tabName;
                t.classList.toggle('active', on);
                t.setAttribute('aria-selected', on ? 'true' : 'false');
                if (on) t.classList.remove('has-notification');
            });
            panels.forEach(function(p) {
                p.style.display = p.getAttribute('data-panel') === tabName ? '' : 'none';
            });
        }

        function notify(tabName) {
            toggles.forEach(function(t) {
                if (t.getAttribute('data-tab') === tabName && !t.classList.contains('active')) {
                    t.classList.add('has-notification');
                }
            });
        }

        toggles.forEach(function(t) {
            t.addEventListener('click', function() { activate(t.getAttribute('data-tab')); });
        });

        // SSE notification wiring — the sidebar panels' hx-trigger already wakes up these event
        // names on the EventSource, so bubbled CustomEvents reach us here.
        var compositor = document.querySelector('.spec-compositor');
        if (compositor) {
            ['card_created', 'card_updated', 'card_moved', 'card_deleted'].forEach(function(e) {
                compositor.addEventListener('sse:' + e, function() { notify('cards'); });
            });
            ['context_attached', 'context_summarized', 'context_notes_updated', 'context_removed'].forEach(function(e) {
                compositor.addEventListener('sse:' + e, function() { notify('context'); });
            });
        }

        // Polling fallback for phase drops
        var currentPhase = '{{ phase }}';
        setInterval(function() {
            fetch('/web/specs/{{ spec_id }}/phase-check')
                .then(function(r) { return r.text(); })
                .then(function(serverPhase) {
                    if (serverPhase !== currentPhase) {
                        htmx.ajax('GET', '/web/specs/{{ spec_id }}', {target: '#workspace', swap: 'innerHTML'});
                    }
                })
                .catch(function() {});
        }, 15000);
    })();
</script>
```

**Step 2: Write a test for the behavior contract**

Add to `crates/barnstormer-server/src/web/mod.rs` tests:

```rust
#[tokio::test]
async fn brainstorming_sidebar_tabs_wire_notification_events() {
    let state = test_state();
    let app = create_router(Arc::clone(&state), None);
    app.oneshot(Request::post("/web/specs").header("content-type", MP_CONTENT_TYPE).body(mp_description_body("Tab notifications")).unwrap()).await.unwrap();
    let spec_id = { let actors = state.actors.read().await; *actors.keys().next().unwrap() };

    let app2 = create_router(Arc::clone(&state), None);
    let resp = app2.oneshot(Request::get(&format!("/web/specs/{}", spec_id)).header("HX-Request", "true").body(Body::empty()).unwrap()).await.unwrap();
    let html = String::from_utf8(axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();

    // All four card events wire Cards dot
    for ev in ["card_created", "card_updated", "card_moved", "card_deleted"] {
        assert!(html.contains(&format!("sse:{}", ev)), "must listen for sse:{}", ev);
    }
    // All four context events wire Context dot
    for ev in ["context_attached", "context_summarized", "context_notes_updated", "context_removed"] {
        assert!(html.contains(&format!("sse:{}", ev)), "must listen for sse:{}", ev);
    }
    assert!(html.contains("has-notification"), "must be able to set notification class");
}
```

**Step 3: Run tests + smoke test**

Run: `cargo test --package barnstormer-server brainstorming_`
Expected: all pass.

Manual smoke test:
- Load brainstorming view → Cards tab active, feed visible
- Click Context → Context panel visible, Cards hidden
- Create a card via Manager chat → Cards tab dot lights up
- Click Cards → dot clears, feed shows new card

**Step 4: Commit**

```bash
git add templates/partials/spec_view.html crates/barnstormer-server/src/web/mod.rs
git commit -m "feat(web): tab switching + notification dots on brainstorm sidebar"
```

---

## Task 4: Remove canvas end-to-end

Delete the canvas UI layer. The `canvas_content` field + `CanvasUpdated` event stay in core for backward-compat replay of old logs, but nothing renders it. The `show_canvas` tool is deregistered from the Manager's toolset and the file deleted.

**Files:**
- Delete: `templates/partials/agent_canvas.html`
- Delete: `crates/barnstormer-agent/src/mux_tools/show_canvas.rs`
- Modify: `crates/barnstormer-server/src/web/mod.rs` — remove `AgentCanvasTemplate`, `canvas_fragment` handler, the three `canvas_fragment_*` tests, and the `spec_view_brainstorming_contains_canvas_listener` + `spec_view_prepopulates_canvas_content` tests. Also amend `spec_view_brainstorming_contains_phase_marker`: drop only the `agent-canvas` assertion block (4 lines); the `data-view="brainstorming"`, `phase-stepper`, `step-active`, and `!view-toggles-row` assertions stay.
- Modify: `crates/barnstormer-server/src/routes.rs` — remove `/canvas-fragment` route
- Modify: `crates/barnstormer-agent/src/mux_tools/mod.rs` — remove `pub mod show_canvas;`, the `pub use` re-export, the `show_canvas::ShowCanvasTool` registration in `build_registry`, and update the two registry tests (tool count 10 → 9; drop `show_canvas` from the name assertions)
- Modify: `crates/barnstormer-agent/src/swarm.rs` — remove the `show_canvas` mention from `MANAGER_BRAINSTORMING_PROMPT` (the tool registration is actually in `mux_tools/mod.rs::build_registry`, not swarm.rs)

**Step 1: Grep for all usages**

Run: `grep -rn "show_canvas\|ShowCanvas\|canvas_fragment\|AgentCanvasTemplate\|agent_canvas.html" crates templates static`

Expected output lists every site needing removal. Confirm against the file list above.

**Step 2: Delete the files**

```bash
rm templates/partials/agent_canvas.html
rm crates/barnstormer-agent/src/mux_tools/show_canvas.rs
```

**Step 3: Remove server-side references**

In `crates/barnstormer-server/src/web/mod.rs`:
- Delete the `AgentCanvasTemplate` struct + `canvas_fragment` handler block.
- Delete these tests: `canvas_fragment_returns_empty_div_when_no_content`, `canvas_fragment_renders_content_when_set`, `canvas_fragment_returns_404_for_unknown_spec`, `spec_view_brainstorming_contains_canvas_listener`, `spec_view_prepopulates_canvas_content`.
- The `has_pending_question` + `canvas_content` fields in `SpecViewTemplate`/`SpecPageTemplate` can stay (backwards-compat shape; removing them is a separate small cleanup commit if desired).

In `crates/barnstormer-server/src/routes.rs`, remove:
```rust
.route("/web/specs/{id}/canvas-fragment", get(web::canvas_fragment))
```

**Step 4: Remove agent-side registration**

In `crates/barnstormer-agent/src/mux_tools/mod.rs`:
- Remove `pub mod show_canvas;`
- Remove any re-export of `ShowCanvasTool`.

In `crates/barnstormer-agent/src/swarm.rs`:
- Grep for `ShowCanvasTool` and `show_canvas` — remove the construction + `registry.register(Box::new(...))` call.
- Remove the import.

**Step 5: Build + test**

Run: `cargo build --all`
Expected: clean.

Run: `cargo test --all`
Expected: all pass. Any test that asserted canvas presence must have been removed in Step 3.

**Step 6: Smoke test**

Restart server. Load a brainstorming spec. Confirm no "canvas" element in DOM (F12 → Elements → search "agent-canvas" → none). Chat + sidebar tabs still work.

**Step 7: Commit**

```bash
git add -A
git commit -m "feat(agent,web): remove canvas end-to-end in favour of cards sidebar"
```

---

## Task 5: Final pass — clean up unused template fields + polish

Optional cleanup: since the brainstorming branch no longer uses `canvas_content` or `has_pending_question` in `SpecViewTemplate`, trim the struct. Also add a final smoke-test walkthrough note so the next agent verifying this knows what "working" looks like.

**Files:**
- Modify: `crates/barnstormer-server/src/web/mod.rs` — drop `canvas_content` + `has_pending_question` from `SpecViewTemplate` and `SpecPageTemplate` if no remaining reference; update callers

**Step 1: Grep for remaining references**

Run: `grep -n "has_pending_question\|canvas_content" crates/barnstormer-server/src/web/mod.rs templates`

If the fields have no template consumers after Task 4, remove them from the structs and from the two call sites (`spec_view`) that populate them.

**Step 2: Build + full test suite**

Run: `cargo test --all && cargo clippy --all-targets -- -D warnings`
Expected: clean.

**Step 3: End-to-end manual verification**

Start server, create a new spec, verify in order:
1. Brainstorming phase loads → chat in main, Cards tab active with empty-state copy, Context tab present
2. Send a message → streaming works in chat
3. Manager creates a card → Cards tab dot lights up if you're on Context; click Cards → dot clears, feed shows new card with type color
4. Upload a context file → Context tab dot lights up if you're on Cards; switch → file appears
5. Transition to refining → full workspace swap, refine layout loads (spec/board/doc tabs with chat sidebar)
6. Go back to brainstorming (click completed step) → brainstorm layout restored
7. Confirm no `agent-canvas` element anywhere in DOM during either phase

**Step 4: Commit**

```bash
git add -A
git commit -m "chore(web): drop unused canvas_content + has_pending_question template fields"
```

---

## Out of scope (file as follow-ups)

- Mobile layout for brainstorming (swipe / stacked layout, matching refine's mobile-content-tabs pattern)
- Keyboard navigation between tabs (arrow keys, per WAI-ARIA tablist spec)
- Card click-through to editing from the feed (today the feed is read-only; editing happens in refining)
- `.tooltip` rollout (issue #8)
- Web search tool investigation (issue #10)
