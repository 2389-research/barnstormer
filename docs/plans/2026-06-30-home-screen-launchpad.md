# Home Screen Launchpad + Project Management + Google Drive — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace barnstormer's empty welcome page with a Claude-style launchpad composer as the default screen, give every spec rename/pin/delete with a Pinned/Recent rail, and let users attach files via multi-upload, drag-drop, paste, and Google Drive.

**Architecture:** Pin and Archive are both event-sourced (new commands → events → a `pinned` / `archived` flag on `SpecCore`, replayed on load) — identical patterns. **There is no destructive delete.** Archived specs are filtered out of the rail and surfaced in a Settings → Archive view where they can be restored (unarchive). Rename reuses the existing `UpdateSpecCore` command. The launchpad promotes the existing `create_spec_form` to `/`. Google Drive uses the **client-side Picker**: the browser obtains a short-lived `drive.file` token, downloads the picked file's bytes, and POSTs them to the existing `/context` endpoint — the server never stores Google tokens and the whole summarize/attach pipeline is reused.

**Tech Stack:** Rust, Axum, Askama templates, HTMX, event-sourced core (`barnstormer-core`), Google Picker JS API.

---

## Conventions for every task

- **TDD rhythm:** write the failing test → run it, confirm it fails → minimal impl → run, confirm pass → commit. Backend logic (commands/events/reducer/handlers) is unit-tested. Pure-template/JS tasks are verified with `cargo build` + an Askama render test where one exists + manual check (noted per task).
- Every new file starts with the 2-line `ABOUTME:` comment (see CLAUDE.md).
- Run the full suite before each commit: `cargo test --all` and `cargo clippy --all-targets -- -D warnings`.
- Commit messages: casual is fine, present tense.

---

## Phases (mergeable in order; each phase is independently shippable)

1. **Pin/Unpin** — backend event + flag
2. **Rename** — web route over existing `UpdateSpecCore`
3. **Archive/Unarchive** — event + flag (mirror of Pin); filtered from rail; **Settings → Archive view** with restore. No destructive delete.
4. **Rail UI** — Pinned/Recent grouping + ⋯ menu wiring (depends on 1–3)
5. **Launchpad home screen** — `/` renders the composer
6. **Composer attach parity** — multi-file, drag-drop, paste, chips (+ fix `upload_context` to accept multiple files)
7. **Google Drive Picker** — client-side picker → existing `/context` endpoint
8. **Spec-view cleanups (confirmed)** — drop the duplicate command-bar tooltip; simplify context file cards to a clean ×
9. **Polish & fixes** — timestamps without seconds; hide the brainstorming sidebar until there's something to show; fix the duplicated-streamed-text bug

> **Declined / not built:** the phase-tracker cleanup (turning the "Return to Brainstorm/Refine" links into chips) — leave the stepper exactly as-is, per review.

---

# Phase 1 — Pin / Unpin

### Task 1.1: Add `pinned` field to `SpecCore`

**Files:**
- Modify: `crates/barnstormer-core/src/model.rs:9-22`

**Step 1: Write the failing test** (append to the `#[cfg(test)]` module in `model.rs`)

```rust
#[test]
fn spec_core_defaults_pinned_false_on_deserialize() {
    // Older snapshots written before `pinned` existed must still load.
    let json = r#"{
        "spec_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "title":"t","one_liner":"o","goal":"g",
        "description":null,"constraints":null,"success_criteria":null,
        "risks":null,"notes":null,
        "created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"
    }"#;
    let core: SpecCore = serde_json::from_str(json).unwrap();
    assert!(!core.pinned);
}
```

**Step 2: Run, confirm fail**

Run: `cargo test -p barnstormer-core spec_core_defaults_pinned_false`
Expected: FAIL (no field `pinned`).

**Step 3: Minimal impl** — add the field with a serde default so old snapshots load:

```rust
pub struct SpecCore {
    pub spec_id: Ulid,
    pub title: String,
    pub one_liner: String,
    pub goal: String,
    pub description: Option<String>,
    pub constraints: Option<String>,
    pub success_criteria: Option<String>,
    pub risks: Option<String>,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub pinned: bool,
}
```

Then fix the one construction site flagged by the compiler: `crates/barnstormer-core/src/state.rs:101-118` (the `SpecCreated` reducer arm) — add `pinned: false,` to the `SpecCore { … }` literal.

**Step 4: Run, confirm pass.** `cargo test -p barnstormer-core` (also fixes the build).

**Step 5: Commit** — `git add -A && git commit -m "feat(core): add pinned flag to SpecCore"`

---

### Task 1.2: Add `PinSpec`/`UnpinSpec` commands and `SpecPinned`/`SpecUnpinned` events

**Files:**
- Modify: `crates/barnstormer-core/src/command.rs` (after `UpdateSpecCore`, ~line 28)
- Modify: `crates/barnstormer-core/src/event.rs` (after `SpecCoreUpdated`, ~line 40)

**Step 1: Failing test** (in `command.rs` test module) — round-trip serde so the variants exist:

```rust
#[test]
fn pin_unpin_commands_round_trip() {
    for c in [Command::PinSpec, Command::UnpinSpec] {
        let s = serde_json::to_string(&c).unwrap();
        let back: Command = serde_json::from_str(&s).unwrap();
        assert_eq!(format!("{back:?}"), format!("{c:?}"));
    }
}
```

**Step 2: Run, confirm fail.**

**Step 3: Impl.** In `command.rs`:

```rust
    /// Pin this spec to the top of the rail.
    PinSpec,
    /// Remove the pin.
    UnpinSpec,
```

In `event.rs`:

```rust
    SpecPinned,
    SpecUnpinned,
```

**Step 4: Run, confirm pass.**

**Step 5: Commit** — `feat(core): add Pin/Unpin commands and events`

---

### Task 1.3: Reducer applies the pin events

**Files:**
- Modify: `crates/barnstormer-core/src/state.rs` (the `apply` match, near the `SpecCoreUpdated` arm ~158)

**Step 1: Failing test** (state.rs test module):

```rust
#[test]
fn apply_spec_pinned_sets_flag() {
    let mut s = SpecState::new();
    apply_created(&mut s); // existing test helper that emits SpecCreated; if absent, build one inline
    s.apply(&event_with(EventPayload::SpecPinned));
    assert!(s.core.as_ref().unwrap().pinned);
    s.apply(&event_with(EventPayload::SpecUnpinned));
    assert!(!s.core.as_ref().unwrap().pinned);
}
```

> If `apply_created`/`event_with` helpers don't exist, mirror the construction used by the nearest existing reducer test in this file.

**Step 2: Run, confirm fail.**

**Step 3: Impl** — add arms:

```rust
EventPayload::SpecPinned => {
    if let Some(ref mut core) = self.core {
        core.pinned = true;
        core.updated_at = event.timestamp;
    }
}
EventPayload::SpecUnpinned => {
    if let Some(ref mut core) = self.core {
        core.pinned = false;
        core.updated_at = event.timestamp;
    }
}
```

**Step 4: Run, confirm pass.**

**Step 5: Commit** — `feat(core): reduce SpecPinned/SpecUnpinned`

---

### Task 1.4: Actor translates the commands to events

**Files:**
- Modify: `crates/barnstormer-core/src/actor.rs` (the command→event match; `CreateSpec`→`PhaseTransitioned` lives ~174)

**Step 1: Failing test** (actor.rs test module): send `Command::PinSpec`, assert the returned events contain `SpecPinned`.

```rust
#[tokio::test]
async fn pin_command_emits_spec_pinned() {
    let handle = spawn(Ulid::new(), SpecState::new());
    handle.send_command(Command::CreateSpec {
        title: "t".into(), one_liner: "o".into(), goal: "g".into(),
    }).await.unwrap();
    let events = handle.send_command(Command::PinSpec).await.unwrap();
    assert!(events.iter().any(|e| matches!(e.payload, EventPayload::SpecPinned)));
}
```

**Step 2: Run, confirm fail.**

**Step 3: Impl** — mirror the simplest existing arm (e.g. how `UpdateSpecCore` maps to one event):

```rust
Command::PinSpec => vec![self.make_event(EventPayload::SpecPinned)],
Command::UnpinSpec => vec![self.make_event(EventPayload::SpecUnpinned)],
```

> Use whatever event-construction helper the surrounding arms use (`make_event`/`emit`/building `Event { … }` inline) — match the file.

**Step 4: Run, confirm pass.**

**Step 5: Commit** — `feat(core): actor maps Pin/Unpin to events`

---

### Task 1.5: Expose `pinned` on `SpecSummary` + pin/unpin web routes

**Files:**
- Modify: `crates/barnstormer-server/src/api/specs.rs:16-22` (add field)
- Modify: `crates/barnstormer-server/src/web/mod.rs` (new handlers `pin_spec`, `unpin_spec`; the `spec_list` handler ~86-103)
- Modify: `crates/barnstormer-server/src/routes.rs` (new routes)

**Step 1: Failing test** (web/mod.rs test module) — POST pin then assert the rail HTML places the spec under Pinned. (If full HTTP harness is heavy, instead unit-test that `spec_list` partitions a pinned core into the `pinned` vec — see Task 4.1 which makes this concrete. For this task, a handler-level test that POST `/web/specs/{id}/pin` returns 200 and the spec's `read_state().core.pinned == true` is sufficient.)

**Step 3: Impl.**

`api/specs.rs`:
```rust
pub struct SpecSummary {
    pub spec_id: String,
    pub title: String,
    pub one_liner: String,
    pub updated_at: String,
    pub pinned: bool,
}
```

`web/mod.rs` — new handlers (model on existing handlers that take `Path(id)`, look up the actor, send a command, persist events to the spec's `events.jsonl`, then return the re-rendered `#spec-list`):
```rust
pub async fn pin_spec(State(state): State<SharedState>, Path(id): Path<String>) -> Response {
    set_pin(state, id, true).await
}
pub async fn unpin_spec(State(state): State<SharedState>, Path(id): Path<String>) -> Response {
    set_pin(state, id, false).await
}

async fn set_pin(state: SharedState, id: String, pin: bool) -> Response {
    let spec_id = match Ulid::from_string(&id) { Ok(u) => u, Err(_) => return (StatusCode::BAD_REQUEST, "bad id").into_response() };
    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) { Some(h) => h.clone(), None => return (StatusCode::NOT_FOUND, "no such spec").into_response() };
    drop(actors);
    let cmd = if pin { Command::PinSpec } else { Command::UnpinSpec };
    match handle.send_command(cmd).await {
        Ok(events) => { persist_events(&state, spec_id, &events); }   // reuse the existing log-append helper used by other web handlers
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
    // Return the freshly grouped rail so HTMX can swap #spec-list.
    spec_list(State(state)).await.into_response()
}
```
> Replace `persist_events` with the actual append-to-`events.jsonl` pattern used by neighbouring handlers (e.g. how `answer_question`/`transition_phase` persist). Reuse, don't reinvent.

`routes.rs` (alongside the other `/web/specs/{id}/…` routes):
```rust
.route("/web/specs/{id}/pin", post(web::pin_spec))
.route("/web/specs/{id}/unpin", post(web::unpin_spec))
```

**Step 4: Run, confirm pass** (`cargo test --all`, `cargo clippy`).

**Step 5: Commit** — `feat(server): pin/unpin routes + pinned on SpecSummary`

---

# Phase 2 — Rename

### Task 2.1: Rename route over `UpdateSpecCore`

**Files:**
- Modify: `crates/barnstormer-server/src/web/mod.rs` (new `rename_spec` handler)
- Modify: `crates/barnstormer-server/src/routes.rs`

**Step 1: Failing test** — POST `/web/specs/{id}/rename` with `title=New Name`, assert `read_state().core.title == "New Name"` and 200.

**Step 3: Impl** — handler reads a form field `title`, sends `UpdateSpecCore { title: Some(title), ..all None }`, persists, returns the re-rendered rail row (or the whole `#spec-list`):

```rust
#[derive(serde::Deserialize)]
pub struct RenameForm { title: String }

pub async fn rename_spec(
    State(state): State<SharedState>, Path(id): Path<String>, Form(form): Form<RenameForm>,
) -> Response {
    let title = form.title.trim().to_string();
    if title.is_empty() { return (StatusCode::BAD_REQUEST, "title required").into_response(); }
    // …look up actor as in set_pin…
    let cmd = Command::UpdateSpecCore {
        title: Some(title), one_liner: None, goal: None, description: None,
        constraints: None, success_criteria: None, risks: None, notes: None,
    };
    // …send, persist, return spec_list(State(state))…
}
```

`routes.rs`: `.route("/web/specs/{id}/rename", post(web::rename_spec))`

**Step 4/5:** test pass, commit `feat(server): rename spec via UpdateSpecCore`.

---

# Phase 3 — Archive / Unarchive (no delete)

**Design note:** Archive is the exact same shape as Pin (Phase 1): an event-sourced boolean on `SpecCore` that survives restart and is reversible. Archived specs are excluded from the rail's Pinned/Recent and listed in a Settings → Archive view. **No spec dir is ever removed and no destructive delete exists.**

### Task 3.1: Add `archived` field to `SpecCore`

Mirror **Task 1.1** exactly, replacing `pinned`→`archived`:
- Add `#[serde(default)] pub archived: bool,` to `SpecCore` (`model.rs`).
- Add `archived: false,` to the `SpecCreated` reducer literal (`state.rs`).
- Test: an old snapshot without the field deserializes with `archived == false`.

Commit — `feat(core): add archived flag to SpecCore`.

### Task 3.2: `ArchiveSpec`/`UnarchiveSpec` commands + `SpecArchived`/`SpecUnarchived` events

Mirror **Task 1.2** (command.rs + event.rs), with a serde round-trip test. Commit — `feat(core): add Archive/Unarchive commands and events`.

### Task 3.3: Reducer applies the archive events

Mirror **Task 1.3** (`state.rs` apply arms), setting `core.archived = true/false` and bumping `updated_at`. Test both directions. Commit — `feat(core): reduce SpecArchived/SpecUnarchived`.

### Task 3.4: Actor translates the commands

Mirror **Task 1.4** (`actor.rs`): `Command::ArchiveSpec => vec![self.make_event(EventPayload::SpecArchived)]` and the unarchive counterpart. Test the emitted event. Commit — `feat(core): actor maps Archive/Unarchive`.

### Task 3.5: Web routes — archive, unarchive

**Files:** `crates/barnstormer-server/src/web/mod.rs` (handlers `archive_spec`, `unarchive_spec`), `routes.rs`.

Mirror the `set_pin` handler from **Task 1.5** (look up actor → send `Command::ArchiveSpec`/`UnarchiveSpec` → persist events → re-render). `archive_spec` returns the re-rendered `#spec-list` (the spec drops off the rail). `unarchive_spec` returns the re-rendered Archive view (Task 3.6) so the restored spec leaves the list.

```rust
.route("/web/specs/{id}/archive", post(web::archive_spec))
.route("/web/specs/{id}/unarchive", post(web::unarchive_spec))
```

**Step 1 test:** archive a spec → assert `read_state().core.archived == true` and the spec no longer appears in `spec_list`'s output (covered with Task 4.1's filter). Commit — `feat(server): archive/unarchive routes`.

### Task 3.6: Settings → Archive view

**Files:**
- Modify: `web/mod.rs` (new `archive_view` handler), `routes.rs` (`.route("/web/settings/archive", get(web::archive_view))`)
- New: `templates/partials/archive_list.html` (rows of archived specs, each with a **Restore** button → `hx-post /web/specs/{id}/unarchive`, `hx-target="#archive-list"`)
- New/Modify: a Settings entry point in the rail footer (`templates/index.html` + `spec_page.html` nav block) — a small gear button that `hx-get`s the archive view into `#workspace`.

`archive_view` builds a list of **only archived** specs (iterate actors, keep `core.archived == true`), sorted by `updated_at` desc, and renders them. If empty, show "No archived specs." Verify with a render test (archived spec appears; non-archived does not) + manual check against deck slide 4.

Commit — `feat(ui): Settings → Archive view with restore`.

---

# Phase 4 — Rail UI: Pinned / Recent + ⋯ menu

### Task 4.1: `spec_list` returns grouped + sorted lists

**Files:**
- Modify: `crates/barnstormer-server/src/web/mod.rs:86-103` (`spec_list`)
- Modify: the `SpecListTemplate` struct (same file or `api/specs.rs` — wherever it's declared)

**Step 1: Failing test** — two specs, one pinned; assert template’s `pinned` has the pinned one and `recent` has the other, and `recent` is sorted by `updated_at` desc.

**Step 3: Impl:**

```rust
pub async fn spec_list(State(state): State<SharedState>) -> impl IntoResponse {
    let actors = state.actors.read().await;
    let mut all = Vec::new();
    for (spec_id, handle) in actors.iter() {
        let s = handle.read_state().await;
        if let Some(ref core) = s.core {
            if core.archived { continue; }                 // archived specs live in Settings → Archive
            all.push(SpecSummary {
                spec_id: spec_id.to_string(),
                title: core.title.clone(),
                one_liner: core.one_liner.clone(),
                updated_at: core.updated_at.to_rfc3339(),
                pinned: core.pinned,
            });
        }
    }
    all.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));   // newest first
    let (pinned, recent): (Vec<_>, Vec<_>) = all.into_iter().partition(|s| s.pinned);
    SpecListTemplate { pinned, recent }
}
```

Change `SpecListTemplate` from `{ specs: Vec<SpecSummary> }` to `{ pinned: Vec<SpecSummary>, recent: Vec<SpecSummary> }`.

**Step 4/5:** test pass, commit `feat(server): group rail into pinned/recent`.

---

### Task 4.2: Rewrite `spec_list.html` — Pinned/Recent + ⋯ menu

**Files:**
- Modify: `templates/partials/spec_list.html`
- Modify: `static/style.css` (rail item row, kebab, popover — port the mockup styles from `mockups/home-screen/`)

**Verification:** Askama render test (mirror the existing spec_list render test in `web/mod.rs` if present) that a pinned spec produces the `Pinned` label and an unpinned one produces `Recent`; plus `cargo build` and manual check against the mockup at `mockups/home-screen/artifact.html`.

**Template** (uses the `<details>` element for the menu — no custom JS, matching the existing `context_panel.html` pattern):

```html
{% if pinned.is_empty() && recent.is_empty() %}
<p class="empty-state">No specs yet. Create one to get started.</p>
{% else %}
  {% if !pinned.is_empty() %}
  <div class="rail-group-label">Pinned</div>
  {% for spec in pinned %}{% include "partials/spec_list_row.html" %}{% endfor %}
  {% endif %}
  {% if !recent.is_empty() %}
  <div class="rail-group-label">Recent</div>
  {% for spec in recent %}{% include "partials/spec_list_row.html" %}{% endfor %}
  {% endif %}
{% endif %}
```

**New file: `templates/partials/spec_list_row.html`** (ABOUTME header required). One row = the existing link + a ⋯ `<details>` menu. Rename swaps the row into an inline edit form; Pin/Unpin and Delete target `#spec-list`:

```html
<div class="spec-list-item">
  <a class="spec-row-link" hx-get="/web/specs/{{ spec.spec_id }}"
     hx-target="#workspace" hx-swap="innerHTML" hx-push-url="/web/specs/{{ spec.spec_id }}">
    {% if spec.pinned %}<span class="pin-ico" aria-hidden="true"><!-- pin svg --></span>{% endif %}
    <span class="spec-text">
      <span class="spec-title">{{ spec.title }}</span>
      <span class="one-liner">{{ spec.one_liner }}</span>
    </span>
  </a>
  <details class="spec-menu-wrap">
    <summary class="kebab" aria-label="Spec options"><!-- ⋯ svg --></summary>
    <div class="spec-menu">
      <button class="menu-item"
              hx-get="/web/specs/{{ spec.spec_id }}/rename-form"
              hx-target="closest .spec-list-item" hx-swap="outerHTML"><!-- pencil -->Rename</button>
      {% if spec.pinned %}
      <button class="menu-item" hx-post="/web/specs/{{ spec.spec_id }}/unpin"
              hx-target="#spec-list" hx-swap="innerHTML"><!-- pin -->Unpin</button>
      {% else %}
      <button class="menu-item" hx-post="/web/specs/{{ spec.spec_id }}/pin"
              hx-target="#spec-list" hx-swap="innerHTML"><!-- pin -->Pin</button>
      {% endif %}
      <div class="menu-sep"></div>
      <button class="menu-item" hx-post="/web/specs/{{ spec.spec_id }}/archive"
              hx-target="#spec-list" hx-swap="innerHTML"><!-- archive box -->Archive</button>
    </div>
  </details>
</div>
```

> The grouped-list re-render (`hx-target="#spec-list"`) works because the rail's `#spec-list` wrapper already exists in `index.html`/`spec_page.html`. CSS: copy `.spec-list-item` (flex row), `.spec-text`, `.kebab`, `.spec-menu`, `.menu-item`, `.rail-group-label`, `.pin-ico` from the mockup; the kebab is `opacity:0` and reveals on `.spec-list-item:hover/:focus-within`.

**Commit** — `feat(ui): pinned/recent rail with rename/pin/delete menu`

---

### Task 4.3: Inline rename form

**Files:**
- Modify: `web/mod.rs` (new `rename_form` handler returning a one-row partial)
- New: `templates/partials/spec_list_row_edit.html`

`rename_form` returns the row with the title replaced by an input that POSTs to `/web/specs/{id}/rename` (Task 2.1), targeting `#spec-list`:

```html
<div class="spec-list-item editing">
  <form hx-post="/web/specs/{{ spec.spec_id }}/rename" hx-target="#spec-list" hx-swap="innerHTML">
    <input class="rename-input" name="title" value="{{ spec.title }}" autofocus
           hx-get="/web/specs" hx-trigger="keyup[key=='Escape']" hx-target="#spec-list" hx-swap="innerHTML">
  </form>
</div>
```

Escape re-fetches the rail (cancels). Commit — `feat(ui): inline rename in rail`.

---

# Phase 5 — Launchpad home screen

### Task 5.1: `/` renders the composer; share one launchpad partial

**Files:**
- New: `templates/partials/launchpad.html` (the composer — start from `create_spec_form.html`, restyle to the mockup)
- Modify: `templates/index.html` (workspace block → include launchpad)
- Modify: `crates/barnstormer-server/src/web/mod.rs` (`index` handler ~74; and `create_spec_form` handler so the "New spec" button renders the same partial)

**Design note:** Keep `index()` returning a full-page template (`IndexTemplate`) whose workspace block now includes `partials/launchpad.html`. The "New spec" rail button (`hx-get="/web/specs/new"`) should return the same launchpad partial so home and New-spec are identical. DRY: both render `launchpad.html`.

**`launchpad.html`** (composer; replaces the bare textarea form). Fix the stale hint (images/PDFs already work). Multi-file input (`multiple`), chips container, attach + Google Drive buttons, drag-drop target = the `.composer`:

```html
{# ABOUTME: Launchpad composer — the home screen. Free-text idea + multi-source context. #}
{# ABOUTME: Posts to /web/specs (multipart: description + files[]) and drops into brainstorming. #}
<div class="launchpad">
  <h1 class="greeting">What do you want to spec out?</h1>
  <p class="subgreeting">Describe your idea. Drop in any files, code, or docs you want the agents to read.</p>
  <form id="launchpad-form" class="composer" hx-post="/web/specs"
        hx-target="#workspace" hx-swap="innerHTML" hx-push-url="true"
        hx-encoding="multipart/form-data" enctype="multipart/form-data">
    <div class="composer-chips" id="composer-chips"></div>
    <textarea name="description" required placeholder="e.g. A weekly digest that checks which of my Letterboxd watchlist films just hit Netflix UK…"></textarea>
    <div class="composer-bar">
      <label class="attach-btn">
        <!-- paperclip --> Attach
        <input type="file" name="files" multiple class="composer-file-input" style="display:none">
      </label>
      <button type="button" class="attach-btn drive-btn" id="drive-btn"><!-- drive svg --> Google Drive</button>
      <div class="spacer"></div>
      <button type="submit" class="send-btn">Start Building <!-- arrow --></button>
    </div>
  </form>
  <p class="hint">Drag &amp; drop files anywhere · paste screenshots · <b>code, markdown, images, PDFs, Google Drive</b> all welcome</p>
</div>
```

**Verification:** existing `index_template_renders` / `get_index_returns_html` tests (web/mod.rs:3805, 4189) must still pass; update their assertions to look for the composer (e.g. `name="description"`) instead of the old "Select a spec" copy. `cargo build`, manual check.

**Commit** — `feat(ui): launchpad composer as home screen`

---

# Phase 6 — Composer attach parity

### Task 6.1: `upload_context` accepts multiple files

**Files:**
- Modify: `crates/barnstormer-server/src/web/mod.rs:2738-2922` (`upload_context`)

**Problem:** today it `break`s after the first `file` field. The create handler already loops multiple `files`; make the in-spec endpoint match.

**Step 1: Failing test** — POST two `file` parts to `/web/specs/{id}/context`, assert `read_state().context_attachments` gains 2 (non-removed) entries.

**Step 3: Impl** — replace the single-file extract loop with one that processes **every** `file` field (mirror the per-file body of the create handler: sniff mime → whitelist → write to disk → `AttachContext` → queue summarize). Keep the brainstorming-phase gate and the 25 MB per-field cap.

**Step 4/5:** test pass, commit `feat(server): multi-file context upload`.

---

### Task 6.2: Front-end attach UX (chips, drag-drop, paste)

**Files:**
- New: `static/launchpad.js` (loaded by `base.html` or inlined in `launchpad.html`)
- Modify: `templates/partials/launchpad.html` (wire the script)
- Modify: `templates/partials/context_panel.html` (apply the same drag-drop/paste to the in-spec Context panel; its input becomes `multiple`)

**Behavior (vanilla JS, no deps):**
- File input `change` → render a chip per file (type badge from extension, name, size, × to remove). Maintain a `DataTransfer` so the chips and the actual `<input type=file>.files` stay in sync (remove updates the FileList).
- `dragover`/`drop` on `.composer` → add dropped files to the same `DataTransfer`; toggle a `.composer.drag` class for the highlight.
- `paste` on the textarea → if `clipboardData.files` has images, add them as chips (name them `pasted-<n>.png`).
- The HTMX form submit sends `files[]` as-is (multipart). No endpoint change needed beyond Task 6.1 for the in-spec panel.

**Verification:** manual — drag a file onto the composer, paste a screenshot, confirm chips appear and submit creates a spec with those attachments. (No unit test; this is browser glue.)

**Commit** — `feat(ui): drag-drop, paste, and file chips in composer`

---

# Phase 7 — Google Drive Picker

### Task 7.0 (manual prerequisite — do before 7.1): GCP config

Reuse Sophie's existing Google Cloud **project** (she has OAuth clients in `~/.gcalcli`, `~/.config/gws`, `gsuite-mcp`). In that project:
1. Enable **Google Picker API** and **Google Drive API**.
2. Create an **API key** (restrict to Picker API).
3. Create a **Web application** OAuth client ID (the existing ones are Desktop-type and won't work for the Picker). Authorized JS origins must include barnstormer's origins: `http://127.0.0.1:7331` and `https://sophies-mac-mini.folk-fujita.ts.net:8443`.
4. Add to `.env` / `.env.example`:
   ```
   GOOGLE_DRIVE_CLIENT_ID=...
   GOOGLE_DRIVE_API_KEY=...
   ```

> **Decision flagged for Sophie:** reuse the existing GCP project (recommended — no new project, benign `drive.file` read-only scope) vs. a dedicated project for blast-radius isolation. Default: reuse.

### Task 7.1: Surface Drive config to templates

**Files:**
- Modify: `crates/barnstormer-server/src/config.rs` (read the two env vars into config)
- Modify: the launchpad/context templates' parent context so `google_drive_client_id` / `google_drive_api_key` are available; expose a small JSON config in the page, or `data-` attributes on `#drive-btn`.

If the vars are unset, hide the Google Drive button (feature-flag off) so the app runs without Drive configured.

**Commit** — `feat(server): plumb Google Drive config (optional)`

### Task 7.2: Picker integration

**Files:**
- New: `static/drive-picker.js`
- Modify: `templates/base.html` (load `https://apis.google.com/js/api.js` and `https://accounts.google.com/gsi/client` — only when Drive is configured)

**Behavior:**
1. Click `#drive-btn` → load `gapi` picker + GIS token client, request `https://www.googleapis.com/auth/drive.file`.
2. On file pick, get `fileId` + the OAuth `access_token`.
3. `fetch('https://www.googleapis.com/drive/v3/files/'+fileId+'?alt=media', {headers:{Authorization:'Bearer '+token}})` → blob. (For native Google Docs, export via `/export?mimeType=...`.)
4. Wrap the blob in a `File`, push it into the composer's `DataTransfer` (Task 6.2) as a chip labelled "from Google Drive", **or** for the in-spec panel POST it directly to `/web/specs/{id}/context`.

No server token storage. The existing summarize/whitelist pipeline handles it once bytes arrive.

**Verification:** manual — pick a Drive doc, confirm it lands as a chip / attachment and summarizes. Note in `log`/docs that native-Google-format export mime mapping is the one fiddly bit.

**Commit** — `feat: attach files from Google Drive via Picker`

---

# Phase 8 — Spec-view cleanups (confirmed)

Two small cosmetic cleanups on the in-spec screen. (The phase-tracker cleanup was declined — leave the stepper as-is.)

### Task 8.1: Remove the duplicate one-liner tooltip from the command bar

**Files:** `templates/partials/spec_view.html` (the `<span class="tooltip command-bar-tooltip">{{ one_liner }}</span>` appears in **all three** phase branches — brainstorming, refining, complete), `static/style.css` (remove the now-unused `.command-bar-tooltip` rule).

The one-liner already renders inline in `.command-bar-subtitle`; the tooltip just repeats it. Delete the three tooltip spans. Verify manually: hovering the title bar shows no duplicate bubble. Commit — `fix(ui): drop duplicate one-liner tooltip`.

### Task 8.2: Simplify context file cards (clean ×, status only when off)

**Files:** `templates/partials/context_panel.html`, `static/style.css`.

- Remove the always-present **"in context"** badge (the `badge-plan` branch) — a successfully-summarized file shows **no** status badge.
- Remove the **card-count** pill (`{{ att.card_count }} card(s)`) from the card face.
- Replace the per-card **Resummarize / Remove** buttons with a single clean **×** remove control, styled like the composer chip ×.
- Keep a worded badge **only** for the off states: `summarizing…` (pending) and `summary failed`; on failure show a small **Retry** that calls the existing resummarize endpoint.

Verify against deck slide 9: happy files = type badge + name + ×; only in-progress/failed carry a label. Commit — `feat(ui): simplify context file cards`.

---

# Phase 9 — Polish & fixes

### Task 9.1: Drop seconds from UI timestamps

**Files:** `crates/barnstormer-server/src/web/mod.rs:616` (card feed) and `:1285` (chat transcript) — change `format("%H:%M:%S")` → `format("%H:%M")`. (The context "added" time at `:2629` is already `%H:%M`; the markdown export's full ISO timestamp stays.) Update any test asserting the `HH:MM:SS` shape. Commit — `fix(ui): show HH:MM without seconds`.

### Task 9.2: Hide the brainstorming sidebar until there's something to show

**Files:** `templates/partials/spec_view.html` (brainstorming branch + its script), `static/style.css`, and the `spec_view` handler in `web/mod.rs` (pass counts into the template).

**Behavior:** when a spec has **zero cards AND zero context attachments**, the brainstorming layout shows just the chat at full width — no empty Cards|Context sidebar. Context stays attachable at all times via the composer attach affordance (Phase 6), so hiding the panel never removes the ability to add context. The moment the first card or context file exists, the sidebar appears.

**Sketch:** render `#brainstorm-sidebar` only when `cards_count + attachments_count > 0`; on `sse:card_created` / `sse:context_attached`, reveal it (the events already fire — see the existing sidebar notify wiring) without a full reload. Verify manually: fresh spec, chat back-and-forth with no cards → no sidebar; create a card → sidebar appears. Commit — `feat(ui): hide brainstorming sidebar until cards/context exist`.

### Task 9.3 (bug): Duplicated streamed text

**Symptom:** streamed agent text doubles its opening chunk — "I'll read**I'll read** the current state", "I need**I n**…". It's **not** a static string (absent from every template and prompt), so it lives in the streaming path.

**REQUIRED SUB-SKILL:** use superpowers:root-cause-tracing.

**Start points:** `crates/barnstormer-agent/src/streaming_hook.rs` (emits deltas) and the front-end `StreamDelta` token appender (chat transcript SSE handling — see `templates/partials/chat_transcript.html` and the CLAUDE.md note on appending streamed tokens). Most likely the first chunk is emitted/appended twice (an initial full-text emit followed by the same text as the first delta). Write a failing test reproducing the doubling at the isolated layer, then fix. Commit — `fix(stream): stop doubling the first streamed chunk`.

### Task 9.4 (bug): Bottom content clipped — last item needs scrolling

**Symptom (from review screenshot, 2026-06-30):** an agent panel's bottom edge sits oddly against the viewport bottom — the final bullet/item requires scrolling to reach; the panel doesn't size to the visible area.

**Likely area:** one of the brainstorming-view panels has a height/overflow miscalculation — the flex column's last child is pushed below the fold. Candidates: `.chat-panel` / `.chat-transcript`, the agent activity feed, or the agent status/LEDs row in `templates/partials/` + their rules in `static/style.css`. The layout uses `height:100vh` on `.app-layout` with `overflow:hidden` and inner scroll regions, so a child with missing `min-height:0` (a classic flexbox overflow trap) or padding pushing past `100%` is the usual cause.

**Needs:** confirm from Sophie which panel clips (agent activity feed / chat panel / agent status row). Then reproduce at a fixed viewport height, fix the flex sizing (`min-height:0` on the scroll container, ensure the panel's own height is `100%` of its flex parent, not content-driven), and verify the last item is reachable without page-level scroll. Commit — `fix(ui): contain panel scroll so last item is reachable`.

---

## Final verification (before declaring done)

- `cargo test --all` green; `cargo clippy --all-targets -- -D warnings` clean.
- Manual pass against the mockups (`mockups/home-screen/artifact.html` + `deck.html`): open `/` → composer is the home screen; create with a dropped file + a pasted image + a Drive file; pin a spec → Pinned section appears; unpin → it vanishes; rename inline; archive → row leaves the rail and appears under Settings → Archive; restore → it returns. No spec dir is ever removed.
- Old snapshots still load (the `#[serde(default)] pinned` test covers this).
