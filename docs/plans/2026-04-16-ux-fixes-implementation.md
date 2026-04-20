# UX Fixes Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix 14 UX issues found during hands-on testing (all items from `docs/plans/2026-04-16-ux-issues-from-testing.md` except #13 streaming, #5 context loss, #9 backend-confirmed, #11 already fixed).

**Architecture:** All changes are in the `feature/phase-wayfinding` branch, stacked on top of existing commits. Fixes are grouped by dependency: quick HTML/CSS wins first, then template logic changes, then SSE infrastructure improvements.

**Tech Stack:** Rust (Axum, Askama templates), HTML/CSS, JavaScript (HTMX, SSE), no new dependencies.

---

### Task 1: Quick wins — focus "Something else", textarea height, lane tooltips (#3, #4, #14)

Three independent micro-fixes in templates and CSS.

**Files:**
- Modify: `templates/partials/chat_transcript.html:58,61,80,83,100`
- Modify: `templates/partials/board.html:4-6`
- Modify: `static/style.css`

**Step 1: Add focus() call to "Something else" onclick handlers**

In `templates/partials/chat_transcript.html`, the Boolean question "Something else" button (line 58) has an onclick handler. Append `f.querySelector('.chat-else-expand textarea').focus();` to the end of the onclick string.

There are two instances to fix:
1. Boolean question (line 58): change the onclick to:
```
onclick="var f=this.closest('.chat-question-options'); f.querySelectorAll('.chat-option-btn').forEach(function(b){b.style.display='none'}); f.querySelector('.chat-else-expand').style.display='flex'; f.querySelector('.chat-else-expand textarea').name='answer'; f.querySelector('.chat-else-expand textarea').focus();"
```
2. MultipleChoice question (line 80): same change.

**Step 2: Standardize textarea rows to 1**

In `templates/partials/chat_transcript.html`, change all `rows="2"` on the "Something else" textareas to `rows="1"`:
- Line 61: `<textarea placeholder="Describe what you mean..." rows="1" ...>`
- Line 83: same
- Line 100 (Freeform question): change `rows="2"` to `rows="1"`

**Step 3: Add tooltips to lane headers**

In `templates/partials/board.html`, replace line 5:
```html
<h3>{{ lane.name }}</h3>
```
with:
```html
<h3 title="{% if lane.name == "Ideas" %}Raw ideas from brainstorming — unstructured thoughts and suggestions.{% else if lane.name == "Plan" %}Items being refined into actionable tasks for the spec.{% else if lane.name == "Spec" %}Finalized spec items that define the implementation.{% else %}{{ lane.name }}{% endif %}">{{ lane.name }}</h3>
```

**Step 4: Build and verify**

Run: `cargo build --all 2>&1 | tail -5`
Expected: compilation succeeds (templates are compiled at build time in Askama).

**Step 5: Commit**

```bash
git add templates/partials/chat_transcript.html templates/partials/board.html
git commit -m "fix: focus 'something else' textarea, standardize rows, add lane tooltips (#3, #4, #14)"
```

---

### Task 2: Thinking throbber — show activity during agent reasoning (#2, #6, #12)

When agents are working (between `agent_step_started` and `agent_step_finished`), show an animated thinking indicator in the chat transcript.

**Files:**
- Modify: `templates/partials/chat_transcript.html`
- Modify: `static/style.css`

**Step 1: Add throbber element to chat transcript**

At the bottom of the chat messages div (after the `{% endfor %}` on line 30, before the `{% if transcript.is_empty() %}` block on line 31), insert a hidden throbber element:

```html
<div id="{{ container_id }}-throbber" class="chat-throbber" style="display:none;">
    <div class="chat-message">
        <div class="chat-message-header">
            <div class="chat-avatar avatar-manager">T</div>
            <span class="chat-sender">Orchestrator</span>
        </div>
        <div class="chat-body">
            <span class="thinking-dots"><span></span><span></span><span></span></span>
        </div>
    </div>
</div>
```

**Step 2: Add throbber toggle JavaScript**

In the `<script>` block at the bottom of `chat_transcript.html` (inside the IIFE, after the scroll position logic), add SSE listeners that toggle the throbber:

```javascript
var compositor = document.querySelector('.spec-compositor');
var throbberId = '{{ container_id }}-throbber';
if (compositor) {
    compositor.addEventListener('sse:agent_step_started', function() {
        var th = document.getElementById(throbberId);
        if (th) { th.style.display = ''; }
        var f = document.getElementById(feedId);
        if (f) { f.scrollTop = f.scrollHeight; }
    });
    compositor.addEventListener('sse:agent_step_finished', function() {
        var th = document.getElementById(throbberId);
        if (th) { th.style.display = 'none'; }
    });
}
```

**Step 3: Add throbber CSS animation**

Append to `static/style.css`:

```css
/* Thinking throbber — three bouncing dots */
.chat-throbber {
    padding: 8px 20px;
}
.thinking-dots {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    height: 20px;
}
.thinking-dots span {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--text-muted);
    animation: throbberBounce 1.4s ease-in-out infinite;
}
.thinking-dots span:nth-child(2) { animation-delay: 0.2s; }
.thinking-dots span:nth-child(3) { animation-delay: 0.4s; }
@keyframes throbberBounce {
    0%, 80%, 100% { opacity: 0.3; transform: scale(0.8); }
    40% { opacity: 1; transform: scale(1); }
}
```

**Step 4: Build and verify**

Run: `cargo build --all 2>&1 | tail -5`
Expected: compilation succeeds.

**Step 5: Commit**

```bash
git add templates/partials/chat_transcript.html static/style.css
git commit -m "feat: add thinking throbber during agent reasoning steps (#2, #6, #12)"
```

---

### Task 3: Multi-select checkboxes for allow_multi questions (#1)

When `allow_multi` is true on a MultipleChoice question, render checkboxes with a Submit button instead of individual submit buttons.

**Files:**
- Modify: `templates/partials/chat_transcript.html:69-89`

**Step 1: Replace the MultipleChoice form with conditional rendering**

Replace the entire MultipleChoice block (lines 69-89) with:

```html
{% when QuestionData::MultipleChoice { question_id, question, choices, allow_multi } %}
<div class="chat-question-body">{{ question|safe }}</div>
<form hx-post="/web/specs/{{ spec_id }}/answer"
      hx-target="#{{ container_id }}"
      hx-swap="outerHTML"
      autocomplete="off"
      class="chat-question-options">
    <input type="hidden" name="question_id" value="{{ question_id }}" data-1p-ignore>
    {% if allow_multi %}
    {% for choice in choices %}
    <label class="chat-option-checkbox">
        <input type="checkbox" name="answer" value="{{ choice }}">
        <span>{{ choice }}</span>
    </label>
    {% endfor %}
    <button type="submit" class="chat-option-btn chat-option-submit" onclick="var f=this.closest('form'); var checked=f.querySelectorAll('input[name=answer]:checked'); if(checked.length===0){event.preventDefault(); return;} var vals=[]; checked.forEach(function(c){vals.push(c.value);}); var h=document.createElement('input'); h.type='hidden'; h.name='answer'; h.value=vals.join(', '); f.appendChild(h); checked.forEach(function(c){c.disabled=true;});">Submit</button>
    {% else %}
    {% for choice in choices %}
    <button type="submit" name="answer" value="{{ choice }}" class="chat-option-btn">{{ choice }}</button>
    {% endfor %}
    {% endif %}
    <button type="button" class="chat-option-btn chat-option-else" onclick="var f=this.closest('.chat-question-options'); f.querySelectorAll('.chat-option-btn, .chat-option-checkbox').forEach(function(b){b.style.display='none'}); f.querySelector('.chat-else-expand').style.display='flex'; f.querySelector('.chat-else-expand textarea').name='answer'; f.querySelector('.chat-else-expand textarea').focus();">Something else&hellip;</button>
    <div class="chat-else-expand" style="display:none;">
        <div class="chat-input-row">
            <textarea placeholder="Describe what you mean..." rows="1" data-1p-ignore required></textarea>
            <button type="submit" class="btn btn-send" title="Send">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="19" x2="12" y2="5"/><polyline points="5 12 12 5 19 12"/></svg>
            </button>
        </div>
    </div>
</form>
```

**Step 2: Add checkbox styling to CSS**

Append to `static/style.css`:

```css
/* Multi-select checkboxes in question cards */
.chat-option-checkbox {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    border-radius: var(--radius-xl);
    background: var(--bg-card);
    border: 1px solid var(--border);
    padding: 12px 16px;
    font-size: 14px;
    color: var(--text-primary);
    cursor: pointer;
    transition: all 0.15s;
}
.chat-option-checkbox:hover {
    border-color: var(--text-muted);
}
.chat-option-checkbox:has(input:checked) {
    border-color: var(--text-primary);
    background: var(--bg-secondary);
}
.chat-option-checkbox input[type="checkbox"] {
    accent-color: var(--text-primary);
    width: 16px;
    height: 16px;
}
.chat-option-submit {
    background: var(--text-primary);
    color: var(--bg-card);
    text-align: center;
    font-weight: 600;
}
.chat-option-submit:hover {
    opacity: 0.85;
}
```

**Step 3: Build and verify**

Run: `cargo build --all 2>&1 | tail -5`
Expected: compilation succeeds.

**Step 4: Commit**

```bash
git add templates/partials/chat_transcript.html static/style.css
git commit -m "feat: render checkboxes for multi-select questions (#1)"
```

---

### Task 4: Fix chat width after board toggle in brainstorming (#7)

When toggling from "View Board" back to "Back to Chat" in brainstorming mode, the chat renders full-viewport instead of the constrained center column.

**Files:**
- Modify: `templates/partials/spec_view.html:56-70`

**Step 1: Fix the toggle script to restore canvas overflow style**

The issue is that when the board loads, it replaces the chat panel inside `#canvas`. When "Back to Chat" fetches the chat panel again via HTMX, the `.canvas` element still has board-related sizing. The fix is to ensure the canvas styling resets properly.

In `templates/partials/spec_view.html`, replace the brainstorming toggle script (lines 55-101) with a version that also resets canvas overflow:

After `btnChat.addEventListener('click', function() {` (line 66), the HTMX fetch already handles content replacement. The real issue is the `.canvas` selector in CSS needs `overflow-y: hidden` for brainstorming (since the chat panel manages its own scroll) but the board needs `overflow-y: auto`. Add an explicit class toggle:

```javascript
btnBoard.addEventListener('click', function() {
    btnBoard.style.display = 'none';
    btnChat.style.display = '';
    document.getElementById('canvas').classList.add('canvas-board');
});
```
```javascript
btnChat.addEventListener('click', function() {
    btnChat.style.display = 'none';
    btnBoard.style.display = '';
    document.getElementById('canvas').classList.remove('canvas-board');
});
```

**Step 2: Add CSS for canvas-board state**

In `static/style.css`, after the existing `.canvas` rule, add:

```css
.canvas.canvas-board {
    overflow-y: auto;
    padding: var(--spacing-lg);
}
```

And update the brainstorming canvas rule to be explicit:

```css
[data-view="brainstorming"] .spec-body > .canvas {
    flex: 1;
    min-width: 300px;
    overflow-y: hidden;
    padding: 0;
    display: flex;
    flex-direction: column;
}
[data-view="brainstorming"] .spec-body > .canvas.canvas-board {
    overflow-y: auto;
    padding: var(--spacing-lg);
    display: block;
}
```

**Step 3: Build and verify**

Run: `cargo build --all 2>&1 | tail -5`
Expected: compilation succeeds.

**Step 4: Commit**

```bash
git add templates/partials/spec_view.html static/style.css
git commit -m "fix: restore chat column width after board toggle in brainstorming (#7)"
```

---

### Task 5: Phase transition rendering fallback (#8)

The `sse:phase_transitioned` event handler does `htmx.ajax()` to reload the workspace, but if the SSE connection drops, the UI never updates. Add a polling fallback that checks current phase periodically and reloads if it changed.

**Files:**
- Modify: `templates/partials/spec_view.html` (all three phase blocks)
- Modify: `crates/barnstormer-server/src/web/mod.rs` (add phase-check endpoint)

**Step 1: Add a lightweight phase-check API endpoint**

In `crates/barnstormer-server/src/web/mod.rs`, add a new handler near the other spec endpoints:

```rust
/// Returns the current phase as plain text — used by the client-side
/// polling fallback when SSE might be disconnected.
pub async fn phase_check(
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
        None => {
            return (StatusCode::NOT_FOUND, "not_found").into_response();
        }
    };
    let spec_state = handle.read_state().await;
    let phase_str = match spec_state.phase {
        crate::state::SpecPhase::Brainstorming => "brainstorming",
        crate::state::SpecPhase::Refining => "refining",
        crate::state::SpecPhase::Complete => "complete",
    };
    phase_str.into_response()
}
```

**Step 2: Register the route**

Find the router setup (search for `.route("/web/specs/:id/phase"`) and add a GET route near it:

```rust
.route("/web/specs/:id/phase-check", get(phase_check))
```

**Step 3: Add phase polling fallback to all three phase blocks**

In each of the three `<script>` blocks in `spec_view.html`, after the `sse:phase_transitioned` listener, add:

```javascript
// Polling fallback: if SSE drops, check phase every 15 seconds
var currentPhase = '{{ phase }}';
setInterval(function() {
    fetch('/web/specs/{{ spec_id }}/phase-check')
        .then(function(r) { return r.text(); })
        .then(function(serverPhase) {
            if (serverPhase !== currentPhase) {
                htmx.ajax('GET', '/web/specs/{{ spec_id }}', {target: '#workspace', swap: 'innerHTML'});
            }
        })
        .catch(function() {}); // silently ignore network errors
}, 15000);
```

**Step 4: Build and test**

Run: `cargo build --all 2>&1 | tail -5`
Expected: compilation succeeds.

Run: `cargo test --all 2>&1 | tail -20`
Expected: all tests pass.

**Step 5: Commit**

```bash
git add crates/barnstormer-server/src/web/mod.rs templates/partials/spec_view.html
git commit -m "feat: add phase polling fallback for SSE disconnection (#8)"
```

---

### Task 6: Sidebar refresh on spec creation (#10)

The sidebar spec list loads once and never updates. Add an SSE listener so it refreshes when a spec is created.

**Files:**
- Modify: `templates/partials/spec_list.html`
- Modify: `templates/base.html` (add SSE connection for sidebar)

**Step 1: Add HTMX polling trigger to spec list**

The sidebar needs to know when specs change, but it sits outside the per-spec SSE connection. The simplest approach is a periodic poll (every 30 seconds) plus a manual refresh trigger.

Replace the `spec_list.html` content wrapper to add a refresh trigger:

In `templates/base.html`, find the nav block. The spec list is loaded inside the nav rail. We need to add `hx-trigger="load, every 30s"` to the spec list container.

First, check how the nav is rendered. The spec list div likely already has `hx-get="/web/specs" hx-trigger="load"`. Change its trigger to:

```html
<div class="spec-list" id="spec-list"
     hx-get="/web/specs"
     hx-trigger="load, every 30s"
     hx-swap="innerHTML">
</div>
```

This is in the page template that includes the nav, not `base.html` itself. Search for where the spec list is rendered.

**Step 2: Find and modify the spec list container**

Look for the template that renders the nav rail content with the spec list loading. It should be in the main page template (likely `templates/index.html` or `templates/home.html`). Modify its `hx-trigger` to include `every 30s`.

**Step 3: Build and verify**

Run: `cargo build --all 2>&1 | tail -5`
Expected: compilation succeeds.

**Step 4: Commit**

```bash
git add templates/
git commit -m "feat: auto-refresh sidebar spec list every 30s (#10)"
```

---

### Task 7: Improve manager error messages (#15)

The "[manager] encountered an issue and will retry" message appears repeatedly with no context. Improve the error message and collapse repeated identical errors.

**Files:**
- Modify: `crates/barnstormer-agent/src/swarm.rs` (find the retry message)
- Modify: `templates/partials/chat_transcript.html` (collapse repeated status lines)

**Step 1: Find the retry error message in the agent crate**

Search for "encountered an issue" or "will retry" in the agent crate to find where this message is generated. Update it to include more context about what failed (e.g., "LLM call failed: rate limited" or "response parsing error").

The fix should change the generic message to include the error kind:

```rust
// Instead of:
// "[manager] encountered an issue and will retry on the next cycle"
// Use:
format!("[{}] encountered an issue ({}). Will retry on the next cycle.", agent_id, error_summary)
```

Where `error_summary` is a short description of the error (first 100 chars of the error message, sanitized).

**Step 2: Collapse consecutive identical status lines in the template**

In `templates/partials/chat_transcript.html`, the status line rendering loop (lines 12-17) shows every step event. Add a deduplication check: if the current status line content matches the previous one, skip rendering it and instead show a "repeated N times" indicator.

This requires passing a `repeat_count` field on `TranscriptEntry` from the server side. In `crates/barnstormer-server/src/web/mod.rs`, in the transcript processing logic, before rendering, collapse consecutive identical step messages:

```rust
// After building transcript entries, collapse consecutive identical steps
let mut collapsed: Vec<TranscriptEntry> = Vec::new();
for entry in entries {
    if entry.is_step {
        if let Some(last) = collapsed.last_mut() {
            if last.is_step && last.content == entry.content {
                last.repeat_count += 1;
                continue;
            }
        }
    }
    collapsed.push(entry);
}
```

Add `repeat_count: u32` to `TranscriptEntry` struct (default 1).

In the template, show the repeat count:
```html
{% if entry.repeat_count > 1 %}
<span class="chat-status-repeat">(×{{ entry.repeat_count }})</span>
{% endif %}
```

**Step 3: Add CSS for repeat count**

```css
.chat-status-repeat {
    font-size: 10px;
    color: var(--text-muted);
    opacity: 0.6;
    white-space: nowrap;
}
```

**Step 4: Build and test**

Run: `cargo build --all 2>&1 | tail -5`
Run: `cargo test --all 2>&1 | tail -20`
Expected: compilation and tests pass.

**Step 5: Commit**

```bash
git add crates/barnstormer-agent/src/swarm.rs crates/barnstormer-server/src/web/mod.rs templates/partials/chat_transcript.html static/style.css
git commit -m "fix: add error context to manager retry messages, collapse repeated errors (#15)"
```

---

### Task 8: Fix Export to Disk button (#16)

The "Export to Disk" button appears to do nothing. The handler POSTs to `/web/specs/{id}/regenerate` and targets `.regen-status`, a tiny `<span>`. The response may be working but invisible.

**Files:**
- Modify: `templates/partials/document.html:11-17`
- Modify: `static/style.css`

**Step 1: Make the regen-status feedback visible**

In `templates/partials/document.html`, the `.regen-status` span (line 17) has no styling and is inline. Replace lines 11-17 with a version that provides clear visual feedback:

```html
<button class="btn btn-sm btn-export"
        hx-post="/web/specs/{{ spec_id }}/regenerate"
        hx-target=".regen-status" hx-swap="innerHTML"
        hx-indicator=".btn-export"
        title="Save exports to disk">
    Export to Disk
</button>
<span class="regen-status"></span>
```

**Step 2: Add CSS for the confirmation message and loading state**

Append to `static/style.css`:

```css
/* Export button loading indicator */
.btn-export.htmx-request {
    opacity: 0.5;
    pointer-events: none;
}
/* Regen confirmation flash */
.regen-status {
    font-size: 12px;
    color: var(--success);
    font-weight: 500;
    transition: opacity 0.3s;
}
.regen-confirm {
    animation: regenFlash 3s ease-out forwards;
}
@keyframes regenFlash {
    0% { opacity: 1; }
    70% { opacity: 1; }
    100% { opacity: 0; }
}
```

**Step 3: Build and verify**

Run: `cargo build --all 2>&1 | tail -5`
Expected: compilation succeeds.

**Step 4: Commit**

```bash
git add templates/partials/document.html static/style.css
git commit -m "fix: make Export to Disk feedback visible with loading state (#16)"
```

---

### Task 9: Run full test suite and clippy

**Step 1: Run all tests**

Run: `cargo test --all 2>&1 | tail -30`
Expected: all tests pass (note: `export::dot::tests::long_prompts_are_truncated` may fail intermittently due to pre-existing ULID ordering nondeterminism — this is not caused by our changes).

**Step 2: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -20`
Expected: no warnings.

**Step 3: Fix any issues found, then commit if needed**

---

## Excluded Issues

- **#5** (Context loss): Agent-side architecture issue, not a UI fix.
- **#9** (Backend correct after refresh): Confirms backend is fine, no action needed.
- **#11** (Document/Spec tabs): Already fixed in this feature branch.
- **#13** (Token-by-token streaming): Deferred to separate effort — requires LLM provider → SSE streaming pipeline.

## Task Order & Dependencies

```
Task 1 (quick wins)     ─┐
Task 2 (throbber)        ├── independent, can run in any order
Task 3 (multi-select)    │
Task 4 (chat width)     ─┘
Task 5 (phase polling)  ── depends on knowing route patterns (read Task 5 carefully)
Task 6 (sidebar refresh) ── independent
Task 7 (error messages)  ── touches both agent + server crates
Task 8 (export button)   ── independent
Task 9 (full test suite) ── must be last
```
