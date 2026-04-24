# Phase Wayfinding Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the overloaded command bar with a layered layout: simplified command bar, linear phase stepper, and contextual view toggles.

**Architecture:** Three horizontal layers replace the single command bar. The phase stepper is a new Askama partial that renders Brainstorm → Refine → Complete as a clickable linear progression. View toggles become a reusable partial that only renders when the active phase has sub-views. Phase transition buttons are eliminated — clicking stepper steps IS the transition.

**Tech Stack:** Rust/Axum backend, Askama templates, HTMX, vanilla CSS, SSE for live updates.

**Design doc:** `docs/plans/2026-04-17-phase-wayfinding-design.md`

---

### Task 1: Phase Stepper CSS

Add CSS for the stepper component before touching any templates.

**Files:**
- Modify: `static/style.css`

**Step 1: Add stepper CSS after the view-toggle rules (~line 216)**

Add these rules after the existing `.view-toggle-label` media query block:

```css
/* --- Phase stepper --- */
.phase-stepper {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0;
    padding: 12px 20px;
    background: var(--bg-card);
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
}

.phase-step {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 16px;
    border: none;
    background: none;
    cursor: pointer;
    font-size: 13px;
    font-weight: 500;
    font-family: var(--font-body);
    color: var(--text-muted);
    transition: all 0.2s;
    white-space: nowrap;
    position: relative;
}

.phase-step-number {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border-radius: 50%;
    font-size: 12px;
    font-weight: 600;
    border: 2px solid var(--border);
    color: var(--text-muted);
    background: var(--bg-card);
    transition: all 0.2s;
    flex-shrink: 0;
}

.phase-step-label {
    transition: color 0.2s;
}

/* Connector line between steps */
.phase-step-connector {
    width: 48px;
    height: 2px;
    background: var(--border);
    flex-shrink: 0;
    transition: background 0.2s;
}

/* Active step */
.phase-step.step-active {
    cursor: default;
    color: var(--text-primary);
}
.phase-step.step-active .phase-step-number {
    background: var(--text-primary);
    border-color: var(--text-primary);
    color: var(--bg-card);
}

/* Completed step */
.phase-step.step-completed {
    color: var(--text-primary);
}
.phase-step.step-completed:hover {
    opacity: 0.7;
}
.phase-step.step-completed .phase-step-number {
    background: var(--success, #22c55e);
    border-color: var(--success, #22c55e);
    color: #fff;
}

/* Completed connector */
.phase-step-connector.connector-completed {
    background: var(--success, #22c55e);
}

/* Disabled/upcoming step */
.phase-step.step-disabled {
    cursor: not-allowed;
    opacity: 0.5;
}
.phase-step.step-disabled .phase-step-number {
    border-style: dashed;
}
```

**Step 2: Verify CSS is valid**

Run: `cargo build --all 2>&1 | head -5`
Expected: build succeeds (CSS is static, not compiled, but this verifies nothing else broke)

**Step 3: Commit**

```bash
git add static/style.css
git commit -m "style: add phase stepper CSS"
```

---

### Task 2: Phase Stepper Template

Create the Askama partial for the phase stepper.

**Files:**
- Create: `templates/partials/phase_stepper.html`
- Modify: `crates/barnstormer-server/src/web/mod.rs` (add template struct)

**Step 1: Create the stepper template**

Create `templates/partials/phase_stepper.html`:

```html
{# ABOUTME: Linear phase stepper showing Brainstorm → Refine → Complete progression. #}
{# ABOUTME: Clickable steps trigger phase transitions via HTMX POST. #}

{% let phases = [("brainstorming", "Brainstorm", "1"), ("refining", "Refine", "2"), ("complete", "Complete", "3")] %}

<nav class="phase-stepper" aria-label="Spec phases">
    {% for (phase_id, label, number) in phases %}
    {% if !loop.first %}
    <div class="phase-step-connector{% if self.is_completed(phase_id) %} connector-completed{% endif %}"></div>
    {% endif %}

    {% if *phase_id == phase %}
    <div class="phase-step step-active" aria-current="step">
        <span class="phase-step-number">{{ number }}</span>
        <span class="phase-step-label">{{ label }}</span>
    </div>
    {% else if self.is_completed(phase_id) %}
    <button class="phase-step step-completed"
            hx-post="/web/specs/{{ spec_id }}/phase"
            hx-vals='{"target":"{{ phase_id }}"}'
            hx-swap="none"
            title="Return to {{ label }}">
        <span class="phase-step-number">✓</span>
        <span class="phase-step-label">{{ label }}</span>
    </button>
    {% else %}
    <div class="phase-step step-disabled"
         title="{{ self.disabled_tooltip(phase_id) }}">
        <span class="phase-step-number">{{ number }}</span>
        <span class="phase-step-label">{{ label }}</span>
    </div>
    {% endif %}
    {% endfor %}
</nav>
```

**Step 2: Add the template struct and helper methods in Rust**

In `crates/barnstormer-server/src/web/mod.rs`, add this struct near the other template structs (around line 579):

```rust
/// Phase stepper navigation partial.
#[derive(Template)]
#[template(path = "partials/phase_stepper.html")]
pub struct PhaseStepperTemplate {
    pub spec_id: String,
    pub phase: String,
}

impl PhaseStepperTemplate {
    /// A phase is "completed" if the current phase is further along in the lifecycle.
    fn is_completed(&self, phase_id: &str) -> bool {
        let order = |p: &str| match p {
            "brainstorming" => 0,
            "refining" => 1,
            "complete" => 2,
            _ => 99,
        };
        order(phase_id) < order(&self.phase)
    }

    /// Tooltip text explaining why a future phase is disabled.
    fn disabled_tooltip(&self, phase_id: &str) -> &'static str {
        match phase_id {
            "refining" => "Complete brainstorming to unlock refining",
            "complete" => "Refine the spec before finalizing",
            _ => "",
        }
    }
}
```

**Step 3: Verify it compiles**

Run: `cargo build --all 2>&1 | tail -3`
Expected: build succeeds

**Step 4: Commit**

```bash
git add templates/partials/phase_stepper.html crates/barnstormer-server/src/web/mod.rs
git commit -m "feat: add phase stepper template and helpers"
```

---

### Task 3: View Toggles Reusable Partial

Extract the hardcoded view toggles from `spec_view.html` into a reusable partial.

**Files:**
- Create: `templates/partials/view_toggles.html`
- Modify: `crates/barnstormer-server/src/web/mod.rs` (add template struct)

**Step 1: Create the view toggles partial template**

Create `templates/partials/view_toggles.html`:

```html
{# ABOUTME: Reusable view toggle capsule for phase-specific sub-navigation. #}
{# ABOUTME: Takes a list of toggles and renders a pill-capsule selector bar. #}

{% if !toggles.is_empty() %}
<div class="view-toggles-row">
    <div class="view-toggles-capsule">
        {% for toggle in toggles %}
        <button class="view-toggle{% if toggle.id == active_view %} active{% endif %}"
                data-view="{{ toggle.id }}"
                hx-get="/web/specs/{{ spec_id }}/{{ toggle.endpoint }}"
                hx-target="#canvas" hx-swap="innerHTML">
            {{ toggle.icon|safe }}
            <span class="view-toggle-label">{{ toggle.label }}</span>
        </button>
        {% endfor %}
    </div>
</div>
{% endif %}
```

**Step 2: Add the template struct and toggle data type**

In `crates/barnstormer-server/src/web/mod.rs`, add near the other template structs:

```rust
/// A single toggle option for the view toggles bar.
pub struct ViewToggle {
    pub id: &'static str,
    pub label: &'static str,
    pub endpoint: &'static str,
    pub icon: &'static str,
}

/// View toggles partial — reusable sub-navigation capsule.
#[derive(Template)]
#[template(path = "partials/view_toggles.html")]
pub struct ViewTogglesTemplate {
    pub spec_id: String,
    pub active_view: String,
    pub toggles: Vec<ViewToggle>,
}
```

**Step 3: Add a helper function that returns the refining toggles**

This avoids repeating the SVG icons everywhere:

```rust
/// Returns the view toggles for the Refining phase.
pub fn refining_toggles() -> Vec<ViewToggle> {
    vec![
        ViewToggle {
            id: "document",
            label: "Document",
            endpoint: "document",
            icon: r#"<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z"/><polyline points="14 2 14 8 20 8"/></svg>"#,
        },
        ViewToggle {
            id: "board",
            label: "Board",
            endpoint: "board",
            icon: r#"<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/></svg>"#,
        },
        ViewToggle {
            id: "spec",
            label: "Workflow",
            endpoint: "spec",
            icon: r#"<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/><line x1="10" y1="9" x2="8" y2="9"/></svg>"#,
        },
    ]
}
```

**Step 4: Add CSS for the view-toggles-row wrapper**

In `static/style.css`, add after the existing `.view-toggle.active` rules (~line 209):

```css
/* View toggles row — sits below stepper when phase has sub-views */
.view-toggles-row {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 8px 20px;
    background: var(--bg-card);
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
}
```

**Step 5: Verify it compiles**

Run: `cargo build --all 2>&1 | tail -3`
Expected: build succeeds

**Step 6: Commit**

```bash
git add templates/partials/view_toggles.html crates/barnstormer-server/src/web/mod.rs static/style.css
git commit -m "feat: add reusable view toggles partial"
```

---

### Task 4: Refactor spec_view.html — Brainstorming Phase

Replace the brainstorming section of `spec_view.html` with the new layered layout.

**Files:**
- Modify: `templates/partials/spec_view.html:4-117` (brainstorming block)

**Step 1: Replace the brainstorming phase block**

Replace lines 4-117 of `spec_view.html` (the `{% if phase == "brainstorming" %}` block through its closing script tag) with:

```html
{# Brainstorming layout: simplified command bar + stepper + full-width chat #}
<div class="spec-compositor" hx-ext="sse" sse-connect="/api/specs/{{ spec_id }}/events/stream"
     data-view="brainstorming">

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
</header>

{% include "partials/phase_stepper.html" %}

<div class="spec-body">
    <main class="canvas" id="canvas"
          hx-get="/web/specs/{{ spec_id }}/chat-panel"
          hx-trigger="load" hx-swap="innerHTML">
    </main>
    {% if has_pending_question %}
    {% match canvas_content %}
    {% when Some with (content) %}
    <div id="agent-canvas" style="display:block;">{{ content|safe }}</div>
    {% when None %}
    <div id="agent-canvas" style="display:none;"></div>
    {% endmatch %}
    {% else %}
    <div id="agent-canvas" style="display:none;"></div>
    {% endif %}
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
    (function() {
        var compositor = document.querySelector('.spec-compositor');
        if (compositor) {
            compositor.addEventListener('sse:phase_transitioned', function() {
                htmx.ajax('GET', '/web/specs/{{ spec_id }}', {target: '#workspace', swap: 'innerHTML'});
            });

            compositor.addEventListener('sse:canvas_updated', function(evt) {
                var data = JSON.parse(evt.detail.data);
                var canvas = document.getElementById('agent-canvas');
                var content = data.payload && data.payload.content;
                if (content && content.trim() !== '') {
                    canvas.innerHTML = content;
                    canvas.style.display = 'block';
                } else {
                    canvas.innerHTML = '';
                    canvas.style.display = 'none';
                }
            });

            compositor.addEventListener('sse:question_answered', function() {
                var canvas = document.getElementById('agent-canvas');
                if (canvas) {
                    canvas.innerHTML = '';
                    canvas.style.display = 'none';
                }
            });

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
        }
    })();
</script>
```

**Key changes:**
- Phase badge removed from command bar
- "View Board" / "Back to Chat" buttons removed (will become view toggles later)
- Phase stepper included via `{% include %}`
- No view toggles row (brainstorming has none yet)

**Step 2: Verify it compiles**

Run: `cargo build --all 2>&1 | tail -5`
Expected: build succeeds

**Step 3: Verify tests pass**

Run: `cargo test --all 2>&1 | tail -10`
Expected: all tests pass

**Step 4: Commit**

```bash
git add templates/partials/spec_view.html
git commit -m "refactor: brainstorming phase uses layered command bar + stepper"
```

---

### Task 5: Refactor spec_view.html — Refining Phase

Replace the refining section with the new layered layout.

**Files:**
- Modify: `templates/partials/spec_view.html` (refining block, currently `{% else if phase == "refining" %}`)

**Step 1: Replace the refining phase block**

Replace the entire `{% else if phase == "refining" %}` block (currently lines 119-245 area) with:

```html
{% else if phase == "refining" %}
{# Refining layout: command bar + stepper + view toggles + canvas + chat rail #}
<div class="spec-compositor" hx-ext="sse" sse-connect="/api/specs/{{ spec_id }}/events/stream">

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
</header>

{% include "partials/phase_stepper.html" %}

<div class="view-toggles-row">
    <div class="view-toggles-capsule">
        <button class="view-toggle active" data-view="document"
                hx-get="/web/specs/{{ spec_id }}/document"
                hx-target="#canvas" hx-swap="innerHTML">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z"/><polyline points="14 2 14 8 20 8"/></svg>
            <span class="view-toggle-label">Document</span>
        </button>
        <button class="view-toggle" data-view="board"
                hx-get="/web/specs/{{ spec_id }}/board"
                hx-target="#canvas" hx-swap="innerHTML">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/></svg>
            <span class="view-toggle-label">Board</span>
        </button>
        <button class="view-toggle" data-view="spec"
                hx-get="/web/specs/{{ spec_id }}/spec"
                hx-target="#canvas" hx-swap="innerHTML">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/><line x1="10" y1="9" x2="8" y2="9"/></svg>
            <span class="view-toggle-label">Workflow</span>
        </button>
    </div>
</div>

<div class="spec-body">
    <main class="canvas" id="canvas"
          hx-get="/web/specs/{{ spec_id }}/document"
          hx-trigger="load" hx-swap="innerHTML">
    </main>
    <aside class="chat-rail" id="chat-rail"
           hx-get="/web/specs/{{ spec_id }}/chat-panel"
           hx-trigger="load" hx-swap="innerHTML">
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
    // View toggle active state management
    document.querySelectorAll('.view-toggles-capsule .view-toggle').forEach(function(btn) {
        btn.addEventListener('click', function() {
            document.querySelectorAll('.view-toggles-capsule .view-toggle').forEach(function(b) {
                b.classList.remove('active');
            });
            btn.classList.add('active');
        });
    });

    document.body.addEventListener('htmx:afterSwap', function(evt) {
        if (evt.detail.target && evt.detail.target.id === 'agent-status') {
            document.body.dispatchEvent(new CustomEvent('refreshAgents'));
        }
    });

    (function() {
        var refreshTimer = null;
        var sseEvents = ['card_created', 'card_updated', 'card_moved', 'card_deleted', 'spec_core_updated'];
        var compositor = document.querySelector('.spec-compositor');
        if (!compositor) return;

        sseEvents.forEach(function(eventName) {
            compositor.addEventListener('sse:' + eventName, function() {
                if (refreshTimer) clearTimeout(refreshTimer);
                refreshTimer = setTimeout(function() {
                    var canvas = document.getElementById('canvas');
                    if (!canvas) return;
                    var activeBtn = document.querySelector('.view-toggle.active');
                    if (activeBtn) {
                        activeBtn.click();
                    }
                }, 800);
            });
        });

        compositor.addEventListener('sse:phase_transitioned', function() {
            htmx.ajax('GET', '/web/specs/{{ spec_id }}', {target: '#workspace', swap: 'innerHTML'});
        });

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

**Key changes:**
- Phase badge removed
- "Finalize Spec" and "Resume Brainstorming" buttons removed
- View toggles capsule moved from command bar to its own row below stepper
- Phase stepper included via `{% include %}`
- Agent controls is the only thing in command-bar-right

**Note:** We inline the view toggles here rather than using the `ViewTogglesTemplate` partial because Askama `{% include %}` shares the parent template's variables. The reusable partial struct from Task 3 is available for programmatic use (e.g., HTMX fragment responses) but the inline approach is simpler for the main spec_view template.

**Step 2: Verify it compiles**

Run: `cargo build --all 2>&1 | tail -5`
Expected: build succeeds

**Step 3: Verify tests pass**

Run: `cargo test --all 2>&1 | tail -10`
Expected: all tests pass

**Step 4: Commit**

```bash
git add templates/partials/spec_view.html
git commit -m "refactor: refining phase uses layered command bar + stepper + toggle row"
```

---

### Task 6: Refactor spec_view.html — Complete Phase

Replace the complete section with the new layered layout. Move the Download button into the document template.

**Files:**
- Modify: `templates/partials/spec_view.html` (complete block, `{% else %}`)
- Modify: `templates/partials/document.html` (add download button)

**Step 1: Add a download action bar to the document template**

In `templates/partials/document.html`, add a download bar inside the existing `.document-notice` div. Replace the current notice div (lines 2-19) with:

```html
    <div class="document-notice">
        <span class="notice-icon">&#9432;</span>
        Auto-generated from spec data. Edit cards on the Board to update this document.
        <button class="btn btn-sm btn-regen"
                hx-get="/web/specs/{{ spec_id }}/document"
                hx-target="#canvas" hx-swap="innerHTML"
                title="Refresh document from current spec data">
            Regenerate
        </button>
        <button class="btn btn-sm btn-export"
                hx-post="/web/specs/{{ spec_id }}/regenerate"
                hx-target=".regen-status" hx-swap="innerHTML"
                hx-indicator=".btn-export"
                title="Save exports to disk">
            Export to Disk
        </button>
        <a href="/web/specs/{{ spec_id }}/export/markdown" download class="btn btn-sm">Download .md</a>
        <span class="regen-status"></span>
    </div>
```

**Step 2: Replace the complete phase block in spec_view.html**

Replace the `{% else %}` block (complete phase) with:

```html
{% else %}
{# Complete layout: command bar + stepper + read-only document #}
<div class="spec-compositor" hx-ext="sse" sse-connect="/api/specs/{{ spec_id }}/events/stream">

<header class="command-bar">
    <div class="command-bar-left">
        <span class="command-bar-title">{{ title }}</span>
        <span class="command-bar-chevron">&#8250;</span>
        <span class="command-bar-subtitle">{{ one_liner }}</span>
    </div>
    <div class="command-bar-right">
    </div>
</header>

{% include "partials/phase_stepper.html" %}

<div class="spec-body">
    <main class="canvas" id="canvas"
          hx-get="/web/specs/{{ spec_id }}/document"
          hx-trigger="load" hx-swap="innerHTML">
    </main>
</div>

</div>

<script>
    (function() {
        var compositor = document.querySelector('.spec-compositor');
        if (compositor) {
            compositor.addEventListener('sse:phase_transitioned', function() {
                htmx.ajax('GET', '/web/specs/{{ spec_id }}', {target: '#workspace', swap: 'innerHTML'});
            });

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
        }
    })();
</script>

{% endif %}
```

**Key changes:**
- "Download .md" link removed from command bar (now in document.html notice bar)
- "Keep Refining" button removed (stepper step 2 handles it)
- No agent controls in complete phase (agents aren't relevant for read-only view)
- Phase stepper shows ✓ Brainstorm ✓ Refine ● Complete

**Step 3: Verify it compiles**

Run: `cargo build --all 2>&1 | tail -5`
Expected: build succeeds

**Step 4: Verify tests pass**

Run: `cargo test --all 2>&1 | tail -10`
Expected: all tests pass

**Step 5: Commit**

```bash
git add templates/partials/spec_view.html templates/partials/document.html
git commit -m "refactor: complete phase uses stepper, download moves to document view"
```

---

### Task 7: Clean Up Obsolete CSS

Remove CSS that's no longer used (phase badges in the command bar) and verify nothing breaks.

**Files:**
- Modify: `static/style.css`

**Step 1: Remove the phase badge CSS**

Delete the `.phase-badge`, `.phase-brainstorming`, `.phase-refining`, and `.phase-complete` rules (around lines 1579-1600 in current file).

**Step 2: Verify no templates still reference phase badges**

Run: `grep -r "phase-badge\|phase-brainstorming\|phase-refining\|phase-complete" templates/`
Expected: no matches (the stepper uses different class names)

**Step 3: Check that the phase transition endpoint response still works**

The `transition_phase` handler at `crates/barnstormer-server/src/web/mod.rs:2186-2196` returns `<span class="phase-badge">...</span>`. Since phase transitions now trigger SSE `phase_transitioned` events which reload the entire workspace, the response body isn't displayed. But update it to return a simple success message instead:

In `crates/barnstormer-server/src/web/mod.rs`, replace lines 2186-2196:

```rust
        Ok(_) => {
            // Phase transition triggers SSE phase_transitioned event,
            // which causes the client to reload the entire workspace.
            // Return minimal success response.
            (StatusCode::OK, Html("<span>OK</span>".to_string())).into_response()
        }
```

**Step 4: Verify tests pass**

Run: `cargo test --all 2>&1 | tail -10`
Expected: all tests pass

**Step 5: Commit**

```bash
git add static/style.css crates/barnstormer-server/src/web/mod.rs
git commit -m "chore: remove obsolete phase badge CSS and clean up transition response"
```

---

### Task 8: Visual Verification

Verify the new layout works correctly in the browser across all three phases.

**Files:**
- No file changes — this is a manual verification task

**Step 1: Start the dev server**

Run: `cargo run -- start --no-open`

**Step 2: Verify brainstorming phase**

Open a spec that's in brainstorming phase:
- Command bar shows title + one-liner on left, agent pill on right
- Stepper shows: **● Brainstorm** ——— Refine ——— Complete
- Refine and Complete steps are grayed out / disabled
- Hovering disabled steps shows tooltips
- No phase badge visible
- No "View Board" / "Back to Chat" buttons
- Chat and canvas area work normally

**Step 3: Verify refining phase**

Open a spec that's in refining phase (or transition one):
- Command bar shows title + one-liner on left, agent pill on right
- Stepper shows: **✓ Brainstorm** ——— **● Refine** ——— Complete
- Brainstorm step is clickable (completed), Complete is disabled
- View toggles row shows Document / Board / Workflow below stepper
- No "Finalize Spec" or "Resume Brainstorming" buttons
- Clicking Brainstorm step in stepper transitions back to brainstorming
- Chat rail and canvas work normally

**Step 4: Verify complete phase**

Open a spec in complete phase (or transition one):
- Stepper shows: **✓ Brainstorm** ——— **✓ Refine** ——— **● Complete**
- Both completed steps are clickable
- No view toggles row
- Download .md button is inside the document notice bar
- No agent controls (command-bar-right is empty)

**Step 5: Verify stepper transitions work**

- From refining, click ✓ Brainstorm — page reloads as brainstorming layout
- From brainstorming, verify Refine step is disabled (can't skip ahead)
- From complete, click ✓ Refine — page reloads as refining layout

**Step 6: Commit any fixes if needed**

If visual issues are found, fix and commit with descriptive messages.
