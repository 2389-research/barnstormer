# Brainstorming Context Files — Implementation Plan (Phase 1: Text)

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add event-sourced file attachments to the brainstorming phase so the Manager agent has reference material (with LLM summaries + user notes) in its context, and replace the separate Import flow with a unified Create flow that optionally accepts files.

**Architecture:** Four new commands/events (Attach/Summarize/UpdateNotes/Remove) extend the existing event-sourced model. Files are stored at `~/.barnstormer/specs/{spec_id}/context/{attachment_id}/{filename}`. A summarizer subagent runs async after upload and emits a `SummarizeContext` command. A new `retrieve_context` mux tool lets agents pull full file contents on demand. The brainstorming UI gains a right-rail context panel that reuses existing `.card`, `.chat-panel`, and `.form-group` primitives. The Create form becomes multipart to accept optional files at spec creation time. The old `/web/specs/import` routes and template are removed.

**Tech Stack:** Rust 2024 workspace (axum 0.8, askama 0.14, tokio, serde, ulid, broadcast channels), HTMX + SSE on the client, `mux-rs` for LLM calls.

**Design doc:** `docs/plans/2026-04-21-brainstorming-context-files-design.md`

---

## Conventions

- All new `.rs` files start with two `ABOUTME:` comment lines.
- TDD: write failing test, verify failure, implement, verify pass, commit.
- After each task: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --all` before commit.
- Commit messages follow existing `feat:` / `fix:` / `refactor:` / `test:` / `docs:` style, and close with the `Co-Authored-By` trailer.
- One logical change per commit.

---

## Task 1: Add `ContextAttachment` type

**Files:**
- Modify: `crates/barnstormer-core/src/state.rs`

**Step 1: Add the struct near the other domain structs** (e.g., next to `Card`)

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextAttachment {
    pub attachment_id: Ulid,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub summary: Option<String>,
    pub user_notes: Option<String>,
    pub added_at: DateTime<Utc>,
    pub removed: bool,
}
```

**Step 2: Verify it compiles**

Run: `cargo check -p barnstormer-core`
Expected: compiles cleanly.

**Step 3: Commit**

```bash
git add crates/barnstormer-core/src/state.rs
git commit -m "feat(core): add ContextAttachment type"
```

---

## Task 2: Add new commands

**Files:**
- Modify: `crates/barnstormer-core/src/command.rs`

**Step 1: Write failing test in command.rs `#[cfg(test)]` module**

```rust
#[test]
fn attach_context_command_serializes() {
    let id = Ulid::new();
    let cmd = Command::AttachContext {
        attachment_id: id,
        filename: "notes.md".to_string(),
        mime_type: "text/markdown".to_string(),
        size_bytes: 1024,
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"type\":\"AttachContext\""));
    assert!(json.contains("\"filename\":\"notes.md\""));
}

#[test]
fn summarize_context_command_serializes() {
    let cmd = Command::SummarizeContext {
        attachment_id: Ulid::new(),
        summary: "Key points...".to_string(),
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"type\":\"SummarizeContext\""));
}

#[test]
fn update_context_notes_command_serializes() {
    let cmd = Command::UpdateContextNotes {
        attachment_id: Ulid::new(),
        notes: "From the kickoff".to_string(),
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"type\":\"UpdateContextNotes\""));
}

#[test]
fn remove_context_command_serializes() {
    let cmd = Command::RemoveContext { attachment_id: Ulid::new() };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"type\":\"RemoveContext\""));
}
```

**Step 2: Run tests — they should fail to compile**

Run: `cargo test -p barnstormer-core --lib command::tests`
Expected: compilation error (variants missing).

**Step 3: Add the four variants to the `Command` enum**

```rust
AttachContext {
    attachment_id: Ulid,
    filename: String,
    mime_type: String,
    size_bytes: u64,
},
SummarizeContext {
    attachment_id: Ulid,
    summary: String,
},
UpdateContextNotes {
    attachment_id: Ulid,
    notes: String,
},
RemoveContext {
    attachment_id: Ulid,
},
```

**Step 4: Run tests — should pass**

Run: `cargo test -p barnstormer-core --lib command::tests`
Expected: all four new tests pass.

**Step 5: Commit**

```bash
git add crates/barnstormer-core/src/command.rs
git commit -m "feat(core): add context-attachment commands"
```

---

## Task 3: Add event payloads

**Files:**
- Modify: `crates/barnstormer-core/src/event.rs`

**Step 1: Write failing tests**

```rust
#[test]
fn context_attached_event_serializes() {
    let payload = EventPayload::ContextAttached {
        attachment: ContextAttachment {
            attachment_id: Ulid::new(),
            filename: "a.md".to_string(),
            mime_type: "text/markdown".to_string(),
            size_bytes: 10,
            summary: None,
            user_notes: None,
            added_at: Utc::now(),
            removed: false,
        },
    };
    let s = serde_json::to_string(&payload).unwrap();
    assert!(s.contains("\"type\":\"ContextAttached\""));
}

#[test]
fn context_summarized_event_serializes() {
    let payload = EventPayload::ContextSummarized {
        attachment_id: Ulid::new(),
        summary: "sum".to_string(),
    };
    let s = serde_json::to_string(&payload).unwrap();
    assert!(s.contains("\"type\":\"ContextSummarized\""));
}

#[test]
fn context_notes_updated_event_serializes() {
    let payload = EventPayload::ContextNotesUpdated {
        attachment_id: Ulid::new(),
        notes: "n".to_string(),
    };
    let s = serde_json::to_string(&payload).unwrap();
    assert!(s.contains("\"type\":\"ContextNotesUpdated\""));
}

#[test]
fn context_removed_event_serializes() {
    let payload = EventPayload::ContextRemoved { attachment_id: Ulid::new() };
    let s = serde_json::to_string(&payload).unwrap();
    assert!(s.contains("\"type\":\"ContextRemoved\""));
}
```

**Step 2: Run tests — should fail to compile**

Run: `cargo test -p barnstormer-core --lib event::tests`
Expected: compilation error.

**Step 3: Add the four variants to `EventPayload`**

```rust
ContextAttached {
    attachment: ContextAttachment,
},
ContextSummarized {
    attachment_id: Ulid,
    summary: String,
},
ContextNotesUpdated {
    attachment_id: Ulid,
    notes: String,
},
ContextRemoved {
    attachment_id: Ulid,
},
```

(Add the import for `ContextAttachment` at the top of `event.rs` if not already there.)

**Step 4: Run tests — should pass**

Run: `cargo test -p barnstormer-core --lib event::tests`
Expected: tests pass.

**Step 5: Commit**

```bash
git add crates/barnstormer-core/src/event.rs
git commit -m "feat(core): add context-attachment event payloads"
```

---

## Task 4: Extend `SpecState` with attachments + apply + undo

**Files:**
- Modify: `crates/barnstormer-core/src/state.rs`

**Step 1: Write failing tests (append to existing `#[cfg(test)]` block)**

```rust
#[test]
fn apply_context_attached_adds_attachment() {
    let mut state = SpecState::new();
    let attachment_id = Ulid::new();
    let event = make_event(
        1,
        make_spec_id(),
        EventPayload::ContextAttached {
            attachment: ContextAttachment {
                attachment_id,
                filename: "a.md".to_string(),
                mime_type: "text/markdown".to_string(),
                size_bytes: 42,
                summary: None,
                user_notes: None,
                added_at: Utc::now(),
                removed: false,
            },
        },
    );
    state.apply(&event);
    assert_eq!(state.context_attachments.len(), 1);
    assert_eq!(state.context_attachments[0].attachment_id, attachment_id);
}

#[test]
fn apply_context_summarized_updates_summary() {
    let mut state = SpecState::new();
    let attachment_id = Ulid::new();
    state.apply(&make_event(1, make_spec_id(), EventPayload::ContextAttached {
        attachment: ContextAttachment {
            attachment_id, filename: "a".into(), mime_type: "text/plain".into(),
            size_bytes: 1, summary: None, user_notes: None,
            added_at: Utc::now(), removed: false,
        },
    }));
    state.apply(&make_event(2, make_spec_id(), EventPayload::ContextSummarized {
        attachment_id, summary: "brief".into(),
    }));
    assert_eq!(state.context_attachments[0].summary.as_deref(), Some("brief"));
}

#[test]
fn apply_context_notes_updated_sets_notes() {
    let mut state = SpecState::new();
    let attachment_id = Ulid::new();
    state.apply(&make_event(1, make_spec_id(), EventPayload::ContextAttached {
        attachment: ContextAttachment {
            attachment_id, filename: "a".into(), mime_type: "text/plain".into(),
            size_bytes: 1, summary: None, user_notes: None,
            added_at: Utc::now(), removed: false,
        },
    }));
    state.apply(&make_event(2, make_spec_id(), EventPayload::ContextNotesUpdated {
        attachment_id, notes: "my note".into(),
    }));
    assert_eq!(state.context_attachments[0].user_notes.as_deref(), Some("my note"));
}

#[test]
fn apply_context_removed_marks_removed() {
    let mut state = SpecState::new();
    let attachment_id = Ulid::new();
    state.apply(&make_event(1, make_spec_id(), EventPayload::ContextAttached {
        attachment: ContextAttachment {
            attachment_id, filename: "a".into(), mime_type: "text/plain".into(),
            size_bytes: 1, summary: None, user_notes: None,
            added_at: Utc::now(), removed: false,
        },
    }));
    state.apply(&make_event(2, make_spec_id(), EventPayload::ContextRemoved { attachment_id }));
    assert!(state.context_attachments[0].removed);
}

#[test]
fn undo_context_attached_marks_removed() {
    let mut state = SpecState::new();
    let attachment_id = Ulid::new();
    state.apply(&make_event(1, make_spec_id(), EventPayload::ContextAttached {
        attachment: ContextAttachment {
            attachment_id, filename: "a".into(), mime_type: "text/plain".into(),
            size_bytes: 1, summary: None, user_notes: None,
            added_at: Utc::now(), removed: false,
        },
    }));
    // Simulate undo by applying UndoApplied with the inverse the attach event produced.
    let top = state.undo_stack.last().expect("undo entry pushed");
    let inverse = top.inverse.clone();
    state.apply(&make_event(2, make_spec_id(), EventPayload::UndoApplied {
        target_event_id: 1,
        inverse_events: inverse,
    }));
    assert!(state.context_attachments[0].removed);
}
```

**Step 2: Run tests — should fail**

Run: `cargo test -p barnstormer-core --lib state::tests -- context`
Expected: compilation error (`context_attachments` field missing).

**Step 3: Add field to `SpecState`**

```rust
pub struct SpecState {
    // ... existing fields ...
    pub context_attachments: Vec<ContextAttachment>,
}
```

Initialize in `SpecState::new()` with `context_attachments: Vec::new()`.

**Step 4: Add match arms to `apply`**

```rust
EventPayload::ContextAttached { attachment } => {
    let inverse = vec![EventPayload::ContextRemoved {
        attachment_id: attachment.attachment_id,
    }];
    self.undo_stack.push(UndoEntry { event_id: event.event_id, inverse });
    self.context_attachments.push(attachment.clone());
}
EventPayload::ContextSummarized { attachment_id, summary } => {
    if let Some(att) = self.context_attachments.iter_mut()
        .find(|a| a.attachment_id == *attachment_id)
    {
        // no undo for summarization — it's idempotent replacement from the summarizer
        att.summary = Some(summary.clone());
    }
}
EventPayload::ContextNotesUpdated { attachment_id, notes } => {
    if let Some(att) = self.context_attachments.iter_mut()
        .find(|a| a.attachment_id == *attachment_id)
    {
        let prior = att.user_notes.clone().unwrap_or_default();
        self.undo_stack.push(UndoEntry {
            event_id: event.event_id,
            inverse: vec![EventPayload::ContextNotesUpdated {
                attachment_id: *attachment_id,
                notes: prior,
            }],
        });
        att.user_notes = Some(notes.clone());
    }
}
EventPayload::ContextRemoved { attachment_id } => {
    if let Some(att) = self.context_attachments.iter_mut()
        .find(|a| a.attachment_id == *attachment_id)
    {
        // Inverse is ContextAttached with the same attachment (un-removed).
        let mut restored = att.clone();
        restored.removed = false;
        self.undo_stack.push(UndoEntry {
            event_id: event.event_id,
            inverse: vec![EventPayload::ContextAttached { attachment: restored }],
        });
        att.removed = true;
    }
}
```

Note: the existing `UndoApplied` handler already loops over `inverse_events` and applies each as a payload — the above inverses slot into that mechanism.

**Step 5: Run tests**

Run: `cargo test -p barnstormer-core --lib state::tests`
Expected: all tests pass.

**Step 6: Commit**

```bash
git add crates/barnstormer-core/src/state.rs
git commit -m "feat(core): apply and undo for context-attachment events"
```

---

## Task 5: Actor Command → Event for new commands

**Files:**
- Modify: `crates/barnstormer-core/src/actor.rs`

**Step 1: Write failing tests**

```rust
#[tokio::test]
async fn actor_processes_attach_context() {
    let spec_id = Ulid::new();
    let handle = spawn(spec_id, SpecState::new());
    handle.send_command(Command::CreateSpec {
        title: "t".into(), one_liner: "o".into(), goal: "g".into(),
    }).await.unwrap();

    let attachment_id = Ulid::new();
    let events = handle.send_command(Command::AttachContext {
        attachment_id,
        filename: "notes.md".into(),
        mime_type: "text/markdown".into(),
        size_bytes: 42,
    }).await.unwrap();

    assert_eq!(events.len(), 1);
    match &events[0].payload {
        EventPayload::ContextAttached { attachment } => {
            assert_eq!(attachment.attachment_id, attachment_id);
            assert_eq!(attachment.filename, "notes.md");
        }
        _ => panic!("expected ContextAttached"),
    }
}

#[tokio::test]
async fn actor_processes_summarize_context() {
    let spec_id = Ulid::new();
    let handle = spawn(spec_id, SpecState::new());
    handle.send_command(Command::CreateSpec {
        title: "t".into(), one_liner: "o".into(), goal: "g".into(),
    }).await.unwrap();
    let attachment_id = Ulid::new();
    handle.send_command(Command::AttachContext {
        attachment_id, filename: "a".into(),
        mime_type: "text/plain".into(), size_bytes: 1,
    }).await.unwrap();
    let events = handle.send_command(Command::SummarizeContext {
        attachment_id, summary: "brief".into(),
    }).await.unwrap();
    assert!(matches!(&events[0].payload, EventPayload::ContextSummarized { .. }));
}

// similar tests for UpdateContextNotes, RemoveContext
```

**Step 2: Run tests — should fail to compile**

Expected: missing match arms on `command_to_events`.

**Step 3: Add match arms to `command_to_events`**

```rust
Command::AttachContext { attachment_id, filename, mime_type, size_bytes } => {
    let core = state.core.as_ref();
    if core.is_none() {
        return Err(ActorError::InvalidCommand("spec not created yet".into()));
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
Command::SummarizeContext { attachment_id, summary } => {
    if !state.context_attachments.iter().any(|a| a.attachment_id == attachment_id) {
        return Err(ActorError::InvalidCommand(format!(
            "unknown attachment {attachment_id}"
        )));
    }
    vec![EventPayload::ContextSummarized { attachment_id, summary }]
}
Command::UpdateContextNotes { attachment_id, notes } => {
    if !state.context_attachments.iter().any(|a| a.attachment_id == attachment_id) {
        return Err(ActorError::InvalidCommand(format!(
            "unknown attachment {attachment_id}"
        )));
    }
    vec![EventPayload::ContextNotesUpdated { attachment_id, notes }]
}
Command::RemoveContext { attachment_id } => {
    let Some(att) = state.context_attachments.iter().find(|a| a.attachment_id == attachment_id) else {
        return Err(ActorError::InvalidCommand(format!(
            "unknown attachment {attachment_id}"
        )));
    };
    if att.removed {
        return Err(ActorError::InvalidCommand("attachment already removed".into()));
    }
    vec![EventPayload::ContextRemoved { attachment_id }]
}
```

**Step 4: Run tests**

Run: `cargo test -p barnstormer-core --lib actor::tests`
Expected: pass.

**Step 5: Commit**

```bash
git add crates/barnstormer-core/src/actor.rs
git commit -m "feat(core): actor handlers for context-attachment commands"
```

---

## Task 6: SSE event-type mapping

**Files:**
- Modify: `crates/barnstormer-server/src/api/stream.rs`

**Step 1: Add match arms in `event_type_name`**

```rust
EventPayload::ContextAttached { .. } => "context_attached",
EventPayload::ContextSummarized { .. } => "context_summarized",
EventPayload::ContextNotesUpdated { .. } => "context_notes_updated",
EventPayload::ContextRemoved { .. } => "context_removed",
```

**Step 2: Verify compile + existing tests still pass**

Run: `cargo test -p barnstormer-server`
Expected: all tests pass.

**Step 3: Commit**

```bash
git add crates/barnstormer-server/src/api/stream.rs
git commit -m "feat(server): map context events to SSE names"
```

---

## Task 7: Disk storage helper module

**Files:**
- Create: `crates/barnstormer-server/src/context_storage.rs`
- Modify: `crates/barnstormer-server/src/lib.rs` (add `pub mod context_storage;`)

**Purpose:** centralize path logic + sanitize filenames + utf-8 check. No async — pure fs + utf-8 validation. Keeps upload handlers focused on HTTP.

**Step 1: Create the file with ABOUTME header and failing tests**

```rust
// ABOUTME: Disk storage helpers for context attachments — path layout, filename
// ABOUTME: sanitization, UTF-8 detection, and read/write helpers.

use std::io;
use std::path::{Path, PathBuf};
use ulid::Ulid;

pub fn sanitize_filename(raw: &str) -> String {
    // Strip any directory components, then replace control chars and known
    // path-dangerous chars with '_'. Empty result becomes "file".
    let base = Path::new(raw).file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file");
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_control() || matches!(c, '/' | '\\' | '\0') { '_' } else { c })
        .collect();
    if cleaned.trim().is_empty() { "file".to_string() } else { cleaned }
}

pub fn attachment_dir(home: &Path, spec_id: Ulid, attachment_id: Ulid) -> PathBuf {
    home.join("specs").join(spec_id.to_string()).join("context").join(attachment_id.to_string())
}

pub fn attachment_path(home: &Path, spec_id: Ulid, attachment_id: Ulid, filename: &str) -> PathBuf {
    attachment_dir(home, spec_id, attachment_id).join(filename)
}

pub fn is_utf8_text(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok()
}

pub fn write_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)
}

pub fn read_text(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_path_components() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("a/b/c.md"), "c.md");
        assert_eq!(sanitize_filename("normal.txt"), "normal.txt");
    }

    #[test]
    fn sanitize_handles_empty() {
        assert_eq!(sanitize_filename(""), "file");
        assert_eq!(sanitize_filename("   "), "file");
    }

    #[test]
    fn sanitize_replaces_control_chars() {
        assert_eq!(sanitize_filename("a\nb.txt"), "a_b.txt");
    }

    #[test]
    fn utf8_detection_works() {
        assert!(is_utf8_text(b"hello"));
        assert!(is_utf8_text("héllo".as_bytes()));
        assert!(!is_utf8_text(&[0xff, 0xfe, 0x00, 0x01]));
    }

    #[test]
    fn write_and_read_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a/b/c.txt");
        write_bytes(&path, b"hi").unwrap();
        let got = read_text(&path).unwrap();
        assert_eq!(got, "hi");
    }
}
```

**Step 2: Add `tempfile` to dev-dependencies**

Modify `crates/barnstormer-server/Cargo.toml`:

```toml
[dev-dependencies]
tempfile = "3"
```

(If it's already there, skip.)

**Step 3: Run tests**

Run: `cargo test -p barnstormer-server --lib context_storage::tests`
Expected: pass.

**Step 4: Commit**

```bash
git add crates/barnstormer-server/src/context_storage.rs crates/barnstormer-server/src/lib.rs crates/barnstormer-server/Cargo.toml
git commit -m "feat(server): context_storage module with path + utf-8 helpers"
```

---

## Task 8: Upload endpoint — `POST /web/specs/{id}/context`

**Files:**
- Modify: `crates/barnstormer-server/Cargo.toml` (enable axum `multipart` feature)
- Modify: `crates/barnstormer-server/src/web/mod.rs` (add handler)
- Modify: `crates/barnstormer-server/src/routes.rs` (register route)

**Step 1: Enable multipart feature in `barnstormer-server/Cargo.toml`**

Change `axum.workspace = true` to `axum = { workspace = true, features = ["multipart"] }` OR add multipart feature in workspace Cargo.toml under axum if preferred. Verify with `grep axum` in Cargo.toml files and match convention.

**Step 2: Write failing integration test**

Create test in `crates/barnstormer-server/tests/context_upload.rs`:

```rust
// ABOUTME: Integration test for context file upload endpoint — verifies multipart
// ABOUTME: parsing, disk write, and ContextAttached event emission.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;
// ... use existing test fixtures from the server crate to spin up a router with a
// temp BARNSTORMER_HOME directory.

#[tokio::test]
async fn upload_text_file_emits_attached_event() {
    let (router, state, spec_id) = test_fixtures::setup_with_spec_in_brainstorming().await;

    let boundary = "----BarnstormerTest";
    let body = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"notes.md\"\r\n\
         Content-Type: text/markdown\r\n\r\n\
         # Hello\n\r\n\
         --{boundary}--\r\n"
    );

    let req = Request::builder()
        .method("POST")
        .uri(format!("/web/specs/{}/context", spec_id))
        .header("content-type", format!("multipart/form-data; boundary={boundary}"))
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify the event landed in state.
    let actors = state.actors.read().await;
    let handle = actors.get(&spec_id).unwrap();
    let spec_state = handle.read_state().await;
    assert_eq!(spec_state.context_attachments.len(), 1);
    assert_eq!(spec_state.context_attachments[0].filename, "notes.md");
}

#[tokio::test]
async fn upload_binary_file_returns_415() {
    let (router, _state, spec_id) = test_fixtures::setup_with_spec_in_brainstorming().await;
    let boundary = "----BarnstormerTest";
    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"a.bin\"\r\n\
         Content-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(&[0xff, 0xfe, 0x00, 0x01]);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let req = Request::builder()
        .method("POST")
        .uri(format!("/web/specs/{}/context", spec_id))
        .header("content-type", format!("multipart/form-data; boundary={boundary}"))
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn upload_outside_brainstorming_returns_409() {
    let (router, _state, spec_id) = test_fixtures::setup_with_spec_in_active().await;
    // ... same multipart body as first test, expect 409 CONFLICT
}
```

**Note on `test_fixtures`:** Look at existing tests in the server crate. Reuse the pattern; extract a shared helper module if needed. The design says "real data/APIs" — use a real temp dir for `BARNSTORMER_HOME`. No LLM call is made during attach (summarizer runs separately).

**Step 3: Run tests — should fail (no handler)**

Run: `cargo test -p barnstormer-server --test context_upload`
Expected: 404 or compile error.

**Step 4: Add the handler in `web/mod.rs`**

```rust
pub async fn upload_context(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    mut multipart: axum::extract::Multipart,
) -> impl IntoResponse {
    let spec_id = match parse_spec_id(&id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    // Locate the actor.
    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h.clone(),
        None => return (StatusCode::NOT_FOUND, "spec not found").into_response(),
    };
    drop(actors);

    // Gate on brainstorming.
    let phase = handle.read_state().await.phase.clone();
    if phase != SpecPhase::Brainstorming {
        return (StatusCode::CONFLICT, "context files can only be attached during brainstorming").into_response();
    }

    // Read one file (first file part) from the multipart body.
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut filename: Option<String> = None;
    let mut mime: Option<String> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            filename = field.file_name().map(str::to_string);
            mime = field.content_type().map(str::to_string);
            let bytes = match field.bytes().await {
                Ok(b) => b,
                Err(e) => return (StatusCode::BAD_REQUEST, format!("read error: {e}")).into_response(),
            };
            file_bytes = Some(bytes.to_vec());
            break;
        }
    }

    let Some(bytes) = file_bytes else {
        return (StatusCode::BAD_REQUEST, "missing file part").into_response();
    };
    const MAX_BYTES: usize = 20 * 1024 * 1024;
    if bytes.len() > MAX_BYTES {
        return (StatusCode::PAYLOAD_TOO_LARGE, "file exceeds 20MB").into_response();
    }
    if !crate::context_storage::is_utf8_text(&bytes) {
        return (StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "binary files not yet supported — text files only for now").into_response();
    }

    let filename = crate::context_storage::sanitize_filename(
        filename.as_deref().unwrap_or("file")
    );
    let mime = mime.unwrap_or_else(|| "text/plain".to_string());
    let attachment_id = Ulid::new();
    let path = crate::context_storage::attachment_path(
        &state.home, spec_id, attachment_id, &filename,
    );
    if let Err(e) = crate::context_storage::write_bytes(&path, &bytes) {
        tracing::error!("failed to write attachment: {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, "write failed").into_response();
    }

    let size_bytes = bytes.len() as u64;
    let cmd = Command::AttachContext {
        attachment_id, filename: filename.clone(), mime_type: mime, size_bytes,
    };
    if let Err(e) = handle.send_command(cmd).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("command failed: {e}")).into_response();
    }

    // Spawn summarizer (Task 12 will define this function).
    let content = String::from_utf8(bytes).expect("utf-8 verified above");
    crate::summarizer::spawn_summarize(handle.clone(), state.clone(), attachment_id, filename, content);

    (StatusCode::OK, "ok").into_response()
}
```

(The reference to `crate::summarizer::spawn_summarize` will fail to compile until Task 12 — comment it out for now, or wire it in as a no-op stub that Task 12 fills in.)

**Step 5: Register the route in `routes.rs`**

```rust
.route("/web/specs/{id}/context", post(web::upload_context))
```

Also add `.layer(DefaultBodyLimit::max(25 * 1024 * 1024))` scoped to the upload route if needed to override axum's default 2MB body limit. (Check axum 0.8 docs — typical pattern: `.route_layer(DefaultBodyLimit::max(...))` on a nested sub-router, or inline via `Router::new().route(...).layer(...)`.)

**Step 6: Run tests**

Run: `cargo test -p barnstormer-server --test context_upload`
Expected: the non-summarizer-dependent tests pass.

**Step 7: Commit**

```bash
git add crates/barnstormer-server/Cargo.toml crates/barnstormer-server/src/web/mod.rs crates/barnstormer-server/src/routes.rs crates/barnstormer-server/tests/context_upload.rs
git commit -m "feat(server): multipart upload endpoint for context files"
```

---

## Task 9: PATCH notes endpoint — `PATCH /web/specs/{id}/context/{att_id}/notes`

**Files:**
- Modify: `crates/barnstormer-server/src/web/mod.rs`
- Modify: `crates/barnstormer-server/src/routes.rs`

**Step 1: Write failing integration test** (append to `context_upload.rs` or create `context_notes.rs`)

```rust
#[tokio::test]
async fn patch_notes_updates_attachment() {
    let (router, state, spec_id) = test_fixtures::setup_with_attachment().await; // returns attachment_id too
    // PATCH /web/specs/{spec_id}/context/{attachment_id}/notes with form body "notes=hello"
    // Assert 200, then read state and check user_notes == "hello"
}
```

**Step 2: Add handler**

```rust
pub async fn update_context_notes(
    State(state): State<SharedState>,
    Path((id, att_id)): Path<(String, String)>,
    Form(form): Form<NotesForm>,
) -> impl IntoResponse {
    let spec_id = match parse_spec_id(&id) { Ok(id) => id, Err(r) => return *r };
    let attachment_id = match att_id.parse::<Ulid>() {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "bad attachment id").into_response(),
    };
    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h.clone(),
        None => return (StatusCode::NOT_FOUND, "spec not found").into_response(),
    };
    drop(actors);
    let cmd = Command::UpdateContextNotes { attachment_id, notes: form.notes };
    match handle.send_command(cmd).await {
        Ok(_) => (StatusCode::OK, "ok").into_response(),
        Err(e) => (StatusCode::NOT_FOUND, format!("{e}")).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct NotesForm { pub notes: String }
```

**Step 3: Register route**

```rust
.route("/web/specs/{id}/context/{att_id}/notes", axum::routing::patch(web::update_context_notes))
```

**Step 4: Run tests + commit**

```bash
cargo test -p barnstormer-server
git add -u
git commit -m "feat(server): PATCH endpoint for context-attachment notes"
```

---

## Task 10: DELETE endpoint — `DELETE /web/specs/{id}/context/{att_id}`

**Files:**
- Modify: `crates/barnstormer-server/src/web/mod.rs`, `routes.rs`

**Step 1: Write failing test**

Similar pattern: set up attachment, DELETE, assert `removed: true` in state, assert file still exists on disk (soft-delete).

**Step 2: Add handler**

```rust
pub async fn remove_context(
    State(state): State<SharedState>,
    Path((id, att_id)): Path<(String, String)>,
) -> impl IntoResponse {
    // parse ids, look up handle, send Command::RemoveContext, return 200.
}
```

**Step 3: Register route**

```rust
.route("/web/specs/{id}/context/{att_id}", axum::routing::delete(web::remove_context))
```

**Step 4: Run tests + commit**

```bash
git commit -m "feat(server): DELETE endpoint for context attachments (soft-remove)"
```

---

## Task 11: GET raw file endpoint — `GET /web/specs/{id}/context/{att_id}/raw`

**Files:**
- Modify: `crates/barnstormer-server/src/web/mod.rs`, `routes.rs`

**Step 1: Write failing test**

Upload a file, GET its raw endpoint, assert body matches original content.

**Step 2: Add handler**

```rust
pub async fn download_context(
    State(state): State<SharedState>,
    Path((id, att_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let spec_id = ...; let attachment_id = ...;
    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) { ... };
    drop(actors);
    let spec_state = handle.read_state().await;
    let att = match spec_state.context_attachments.iter()
        .find(|a| a.attachment_id == attachment_id && !a.removed)
    {
        Some(a) => a,
        None => return (StatusCode::NOT_FOUND, "attachment not found").into_response(),
    };
    let path = crate::context_storage::attachment_path(&state.home, spec_id, attachment_id, &att.filename);
    match crate::context_storage::read_text(&path) {
        Ok(text) => (
            [(header::CONTENT_TYPE, att.mime_type.as_str())],
            text,
        ).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "file not found on disk").into_response(),
    }
}
```

**Step 3: Register route + tests + commit**

```bash
git commit -m "feat(server): raw download endpoint for context attachments"
```

---

## Task 12: Summarizer subagent

**Files:**
- Create: `crates/barnstormer-server/src/summarizer.rs`
- Modify: `crates/barnstormer-server/src/lib.rs`

This ties together: take file content, call the LLM client (single-shot), send `SummarizeContext` command when done. Mirrors the shape of `barnstormer-agent::import::parse_with_llm` but simpler output (plain text, not JSON).

**Step 1: Write the module scaffold with `ABOUTME:` header**

```rust
// ABOUTME: Async summarizer for uploaded context files — sends content to the LLM,
// ABOUTME: then emits SummarizeContext when the summary comes back.

use std::sync::Arc;
use barnstormer_core::{Command, SpecActorHandle};
use mux::llm::{Request, Message};
use ulid::Ulid;

use crate::state::SharedState;

const SUMMARY_SYSTEM_PROMPT: &str = "Summarize this document concisely (4-8 sentences), \
focusing on what would be relevant for building a software specification. \
Preserve key technical details, names, and constraints.";

pub fn spawn_summarize(
    actor: Arc<SpecActorHandle>,
    state: SharedState,
    attachment_id: Ulid,
    filename: String,
    content: String,
) {
    tokio::spawn(async move {
        if let Err(e) = summarize_and_record(actor, state, attachment_id, filename, content).await {
            tracing::warn!("summarization failed: {e}");
        }
    });
}

async fn summarize_and_record(
    actor: Arc<SpecActorHandle>,
    state: SharedState,
    attachment_id: Ulid,
    filename: String,
    content: String,
) -> anyhow::Result<()> {
    let (client, model) = state.llm_client.clone(); // shared LLM client on state
    let req = Request::new(&model)
        .system(SUMMARY_SYSTEM_PROMPT)
        .message(Message::user(format!("File: {filename}\n\n{content}")))
        .max_tokens(512);
    let resp = client.create_message(&req).await?;
    let summary = resp.text();
    if summary.trim().is_empty() {
        anyhow::bail!("empty summary");
    }
    actor.send_command(Command::SummarizeContext { attachment_id, summary }).await?;
    Ok(())
}
```

**Step 2: Ensure `SharedState` has an `llm_client` field**

Check `crates/barnstormer-server/src/state.rs` (or wherever `SharedState` is defined). If no LLM client is already on state, add one — the swarm code creates one per spec; we want a server-level one for summarizer. Alternative: create a client on demand inside `summarize_and_record` by calling the factory. **Prefer on-demand to avoid scope creep.** Revise the function:

```rust
async fn summarize_and_record(...) -> anyhow::Result<()> {
    let (client, model) = barnstormer_agent::client::create_llm_client(
        &std::env::var("BARNSTORMER_DEFAULT_PROVIDER").unwrap_or_else(|_| "anthropic".into()),
        None,
    )?;
    // ...
}
```

**Step 3: Wire into upload handler (uncomment the call from Task 8)**

**Step 4: Commit**

```bash
git add crates/barnstormer-server/src/summarizer.rs crates/barnstormer-server/src/lib.rs crates/barnstormer-server/src/web/mod.rs
git commit -m "feat(server): summarizer subagent for context files"
```

**Note on testing:** Summarization involves real LLM calls. Gate a smoke test behind `#[ignore]` or an `ANTHROPIC_API_KEY` env check so CI can skip it.

---

## Task 13: `retrieve_context` mux tool

**Files:**
- Create: `crates/barnstormer-agent/src/mux_tools/retrieve_context.rs`
- Modify: `crates/barnstormer-agent/src/mux_tools/mod.rs` (register)

**Step 1: Create the tool with ABOUTME header**

```rust
// ABOUTME: retrieve_context mux tool — lets agents fetch the full text of a
// ABOUTME: context attachment by ID when a summary isn't enough.

use std::path::PathBuf;
use std::sync::Arc;
use async_trait::async_trait;
use barnstormer_core::SpecActorHandle;
use mux::tools::{Tool, ToolResult};
use serde_json::json;
use ulid::Ulid;

#[derive(Clone)]
pub struct RetrieveContextTool {
    pub(crate) actor: Arc<SpecActorHandle>,
    pub(crate) home: PathBuf,
}

#[async_trait]
impl Tool for RetrieveContextTool {
    fn name(&self) -> &str { "retrieve_context" }
    fn description(&self) -> &str {
        "Retrieve the full text of a context file attachment by ID. Use this when \
         the summary isn't enough and you need to see the actual content."
    }
    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "attachment_id": {
                    "type": "string",
                    "description": "The ULID of the attachment to retrieve"
                }
            },
            "required": ["attachment_id"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult, anyhow::Error> {
        let id_str = params.get("attachment_id").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing attachment_id"))?;
        let attachment_id: Ulid = id_str.parse().map_err(|e| anyhow::anyhow!("bad id: {e}"))?;

        let state = self.actor.read_state().await;
        let att = state.context_attachments.iter()
            .find(|a| a.attachment_id == attachment_id && !a.removed)
            .ok_or_else(|| anyhow::anyhow!("attachment not found"))?;
        let filename = att.filename.clone();
        let spec_id = self.actor.spec_id;
        drop(state);

        let path = self.home.join("specs").join(spec_id.to_string())
            .join("context").join(attachment_id.to_string()).join(&filename);
        let text = tokio::fs::read_to_string(&path).await?;
        Ok(ToolResult::text(text))
    }
}
```

**Step 2: Register in `build_registry`**

`build_registry` currently takes `actor, question_pending, pending_transition_question, agent_id`. Add a `home: PathBuf` parameter and pass it through from the caller (swarm.rs).

```rust
registry.register(RetrieveContextTool { actor: Arc::clone(&actor), home: home.clone() }).await;
```

**Step 3: Update swarm.rs call site to pass `home`**

Check `swarm.rs` — it needs access to the data home directory. Likely pass via config or `AgentContext`. Thread it through.

**Step 4: Unit test for the tool**

```rust
#[tokio::test]
async fn retrieve_context_reads_file() {
    // Set up actor with one attachment, write file to temp dir,
    // instantiate tool, execute, assert content.
}
```

**Step 5: Commit**

```bash
git add crates/barnstormer-agent/src/mux_tools/retrieve_context.rs crates/barnstormer-agent/src/mux_tools/mod.rs crates/barnstormer-agent/src/swarm.rs
git commit -m "feat(agent): retrieve_context tool for accessing attachment contents"
```

---

## Task 14: Inject context into agent task prompt

**Files:**
- Modify: `crates/barnstormer-agent/src/swarm.rs` (in `build_task_prompt`)

**Step 1: Write a failing test near the prompt builder**

```rust
#[test]
fn task_prompt_includes_context_files_section_when_present() {
    let mut ctx = make_ctx_with_attachments(vec![
        ("requirements.md", Some("summary text"), Some("user note")),
    ]);
    let prompt = build_task_prompt(&ctx);
    assert!(prompt.contains("## Context Files"));
    assert!(prompt.contains("requirements.md"));
    assert!(prompt.contains("summary text"));
    assert!(prompt.contains("user note"));
}

#[test]
fn task_prompt_omits_context_section_when_empty() {
    let ctx = make_ctx_with_attachments(vec![]);
    let prompt = build_task_prompt(&ctx);
    assert!(!prompt.contains("## Context Files"));
}
```

(The test fixture `make_ctx_with_attachments` should construct an `AgentContext` with a synthetic `state_summary` that already has attachments rendered, OR we pass attachments separately. Look at the existing `AgentContext` shape and decide where attachments live. Simplest: extend `AgentContext` with a `context_attachments: Vec<ContextAttachment>` field populated when building the context for a step.)

**Step 2: Extend `AgentContext`**

In `crates/barnstormer-agent/src/context.rs`:

```rust
pub struct AgentContext {
    // existing fields ...
    pub context_attachments: Vec<ContextAttachment>,
}
```

Wherever `AgentContext` is built (in `swarm.rs`), copy the non-removed attachments from the current `SpecState`:

```rust
let attachments = state.context_attachments.iter()
    .filter(|a| !a.removed)
    .cloned()
    .collect();
```

**Step 3: Render the section in `build_task_prompt`**

```rust
if !ctx.context_attachments.is_empty() {
    prompt.push_str("\n## Context Files\n\n");
    prompt.push_str(
        "The user has attached the following reference materials. \
         Use these to inform your work. If a summary isn't enough, \
         call the `retrieve_context` tool with the attachment ID to read the full text.\n\n"
    );
    for (i, att) in ctx.context_attachments.iter().enumerate() {
        let size_kb = att.size_bytes as f64 / 1024.0;
        prompt.push_str(&format!(
            "### {}. {} ({:.0}KB)\n**attachment_id:** `{}`\n",
            i + 1, att.filename, size_kb, att.attachment_id
        ));
        if let Some(notes) = &att.user_notes {
            if !notes.is_empty() {
                prompt.push_str(&format!("**User notes:** {}\n", notes));
            }
        }
        match &att.summary {
            Some(s) if !s.is_empty() => prompt.push_str(&format!("**Summary:** {}\n\n", s)),
            _ => prompt.push_str("**Summary:** _(being summarized...)_\n\n"),
        }
    }
}
```

**Step 4: Run tests — should pass**

Run: `cargo test -p barnstormer-agent`

**Step 5: Commit**

```bash
git add crates/barnstormer-agent/src/context.rs crates/barnstormer-agent/src/swarm.rs
git commit -m "feat(agent): inject context files into manager task prompt"
```

---

## Task 15: Render context panel partial

**Files:**
- Create: `templates/partials/context_panel.html`
- Modify: `crates/barnstormer-server/src/web/mod.rs` (add `GET /web/specs/{id}/context-panel` returning the rendered panel)
- Modify: `crates/barnstormer-server/src/routes.rs`

**Step 1: Create the template**

```html
{# Context panel — right rail of brainstorming view. Reuses .chat-panel, .card, .form-group. #}
<div class="chat-panel" id="context-panel">
    <div class="chat-panel-header">
        <span class="chat-panel-title">Context</span>
        <form id="context-upload-form"
              hx-post="/web/specs/{{ spec_id }}/context"
              hx-encoding="multipart/form-data"
              hx-target="#context-panel"
              hx-swap="outerHTML"
              style="margin-left:auto;">
            <label class="btn btn-sm" style="cursor:pointer; margin:0;">
                + Add
                <input type="file" name="file" style="display:none;"
                       onchange="this.form.requestSubmit()">
            </label>
        </form>
    </div>

    <div class="chat-transcript" style="overflow-y:auto; flex:1; padding: var(--spacing-md);">
        {% for att in attachments %}
        <div class="card" id="att-{{ att.attachment_id }}" style="margin-bottom: var(--spacing-sm);">
            <div style="display:flex; align-items:center; gap:var(--spacing-sm); margin-bottom:var(--spacing-xs);">
                <span class="card-type badge-note">{{ att.extension }}</span>
                <span style="flex:1; font-weight:500;">{{ att.filename }}</span>
                <button class="btn btn-sm btn-danger"
                        hx-delete="/web/specs/{{ spec_id }}/context/{{ att.attachment_id }}"
                        hx-target="#context-panel" hx-swap="outerHTML"
                        hx-confirm="Remove this file?"
                        title="Remove">×</button>
            </div>
            <div style="font-size:0.75rem; color:var(--text-muted); margin-bottom:var(--spacing-sm);">
                {{ att.size_display }} · added {{ att.added_display }}
            </div>
            {% match att.summary %}
            {% when Some with (s) %}
                <div style="font-size:0.82rem; color:var(--text-secondary); margin-bottom:var(--spacing-sm);">
                    {{ s }}
                </div>
            {% when None %}
                <div style="font-size:0.82rem; color:var(--text-muted); font-style:italic; margin-bottom:var(--spacing-sm);">
                    Summarizing…
                </div>
            {% endmatch %}
            <form hx-patch="/web/specs/{{ spec_id }}/context/{{ att.attachment_id }}/notes"
                  hx-trigger="change from:find textarea, blur from:find textarea"
                  hx-swap="none">
                <div class="form-group" style="margin:0;">
                    <textarea name="notes" rows="2"
                              placeholder="Add notes about this file…"
                              style="font-size:0.82rem;">{{ att.user_notes|default("") }}</textarea>
                </div>
            </form>
        </div>
        {% endfor %}
        {% if attachments.is_empty() %}
        <div style="color:var(--text-muted); font-size:0.82rem; text-align:center; padding: var(--spacing-lg);">
            No context files yet. Drop a file above to get started.
        </div>
        {% endif %}
    </div>
</div>
```

**Step 2: Add the Askama template struct and handler**

```rust
#[derive(Template)]
#[template(path = "partials/context_panel.html")]
struct ContextPanelTemplate {
    spec_id: String,
    attachments: Vec<ContextPanelItem>,
}

struct ContextPanelItem {
    attachment_id: String,
    filename: String,
    extension: String,
    size_display: String,
    added_display: String,
    summary: Option<String>,
    user_notes: Option<String>,
}

pub async fn context_panel(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let spec_id = match parse_spec_id(&id) { Ok(id) => id, Err(r) => return *r };
    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h.clone(),
        None => return (StatusCode::NOT_FOUND, "spec not found").into_response(),
    };
    drop(actors);

    let spec_state = handle.read_state().await;
    let attachments = spec_state.context_attachments.iter()
        .filter(|a| !a.removed)
        .map(|a| ContextPanelItem {
            attachment_id: a.attachment_id.to_string(),
            filename: a.filename.clone(),
            extension: std::path::Path::new(&a.filename).extension()
                .and_then(|e| e.to_str()).unwrap_or("txt").to_string(),
            size_display: format_size(a.size_bytes),
            added_display: a.added_at.format("%H:%M").to_string(),
            summary: a.summary.clone(),
            user_notes: a.user_notes.clone(),
        })
        .collect();

    Html(ContextPanelTemplate { spec_id: spec_id.to_string(), attachments }
        .render().unwrap()).into_response()
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 { format!("{bytes} B") }
    else if bytes < 1024 * 1024 { format!("{:.1} KB", bytes as f64 / 1024.0) }
    else { format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0)) }
}
```

**Step 3: Register route**

```rust
.route("/web/specs/{id}/context-panel", get(web::context_panel))
```

**Step 4: Upload handler should return the re-rendered panel**

Revise Task 8's handler: instead of `(StatusCode::OK, "ok")`, return the rendered panel so HTMX swaps it in. Also update the upload test to assert the returned HTML contains the filename.

**Step 5: Commit**

```bash
git commit -m "feat(web): context panel partial and GET endpoint"
```

---

## Task 16: Wire context panel into brainstorming view

**Files:**
- Modify: `templates/partials/spec_view.html`

**Step 1: Revise the brainstorming branch of `spec_view.html` to use the three-column layout (canvas + context rail)**

Replace the current `.spec-body` block in the `phase == "brainstorming"` branch:

```html
<div class="spec-body">
    <main class="canvas" id="canvas"
          hx-get="/web/specs/{{ spec_id }}/chat-panel"
          hx-trigger="load" hx-swap="innerHTML"></main>
    <aside class="chat-rail" id="context-rail"
           hx-get="/web/specs/{{ spec_id }}/context-panel"
           hx-trigger="load" hx-swap="innerHTML"></aside>
</div>
```

(Preserve the existing SSE canvas handler by moving `#agent-canvas` inside the `canvas` main if it was in `spec-body`. Read the current file carefully before editing.)

**Step 2: Add SSE listeners for context events**

In the existing `<script>` block:

```js
['context_attached', 'context_summarized', 'context_notes_updated', 'context_removed']
    .forEach(function(evt) {
        compositor.addEventListener('sse:' + evt, function() {
            htmx.ajax('GET', '/web/specs/{{ spec_id }}/context-panel',
                      { target: '#context-rail', swap: 'innerHTML' });
        });
    });
```

**Step 3: Manually verify in browser**

Run `cargo run -- start`. Create a spec, upload a text file via the side panel, check that the summary arrives (after a beat), edit notes, remove. Verify the card updates via SSE.

**Step 4: Commit**

```bash
git commit -m "feat(web): brainstorming view uses context rail"
```

---

## Task 17: Add collapsible toggle for context rail

**Files:**
- Modify: `templates/partials/spec_view.html`
- Modify: `static/style.css` only if absolutely needed (try to reuse `display: none` pattern)

**Step 1: Add toggle button to command bar (brainstorming branch)**

```html
<button id="btn-toggle-context" class="btn btn-sm"
        onclick="document.getElementById('context-rail').classList.toggle('hidden');
                 this.textContent = document.getElementById('context-rail').classList.contains('hidden')
                     ? 'Show context' : 'Hide context';">Hide context</button>
```

If `.hidden { display: none; }` doesn't already exist in `style.css`, add it at the end:

```css
.hidden { display: none; }
```

(This is a widely useful utility; acceptable addition. If an existing equivalent is already present, reuse it.)

**Step 2: Manual verification**

Check toggle behaves as expected.

**Step 3: Commit**

```bash
git commit -m "feat(web): collapsible context rail"
```

---

## Task 18: Create form — accept optional files

**Files:**
- Modify: `templates/partials/create_spec_form.html`
- Modify: `crates/barnstormer-server/src/web/mod.rs` (the existing `create_spec` handler)

**Step 1: Update template**

Convert the form to multipart and add a file input that stages files client-side. Minimal JS for the staging list.

```html
{# Form for creating a new spec, optionally with context files #}
<div class="create-spec-form" style="padding: var(--spacing-lg); max-width: 500px; margin: 0 auto;">
    <h2>What do you want to build?</h2>
    <p style="color: var(--text-secondary); margin-bottom: var(--spacing-md); font-size: 0.85rem;">
        Describe your idea in your own words. Optionally attach reference material — notes, existing specs, transcripts — and the Manager will use them to help.
    </p>
    <form hx-post="/web/specs" hx-target="#workspace" hx-swap="innerHTML" hx-push-url="true"
          hx-encoding="multipart/form-data">
        <div class="form-group">
            <textarea id="description" name="description" required rows="6"
                placeholder="e.g. I want to build..."></textarea>
        </div>
        <div class="form-group">
            <label for="files" style="font-size:0.8rem; color:var(--text-secondary);">Context files (optional)</label>
            <input type="file" id="files" name="files" multiple
                   style="margin-top: var(--spacing-xs);">
        </div>
        <button type="submit" class="btn btn-primary">Start Building</button>
    </form>
</div>
```

**Step 2: Update `create_spec` handler to accept multipart**

The handler previously took `Form<CreateSpecForm>`. Change signature to `axum::extract::Multipart`. Iterate fields:
- If `name == "description"`, collect as the description text
- If `name == "files"`, for each file part: validate UTF-8, write to disk, queue `AttachContext` (after the actor is created)

After spec creation, loop through staged files, send `AttachContext` for each, spawn summarizer for each. Continue with the existing redirect logic.

Add tests to `context_upload.rs`:

```rust
#[tokio::test]
async fn create_spec_with_one_file_attaches_it() {
    // POST /web/specs multipart with description + a single text file.
    // Follow the redirect, read state, assert one attachment exists.
}
```

**Step 3: Run tests + commit**

```bash
git commit -m "feat(web): create form accepts optional context files"
```

---

## Task 19: Remove Import flow

**Files:**
- Delete: `templates/partials/import_spec_form.html`
- Modify: `crates/barnstormer-server/src/web/mod.rs` (remove `import_spec_form`, `import_spec` functions + the `ImportSpecFormTemplate` struct)
- Modify: `crates/barnstormer-server/src/api/import.rs` — delete file
- Modify: `crates/barnstormer-server/src/api/mod.rs` — remove the `pub mod import`
- Modify: `crates/barnstormer-server/src/routes.rs` — drop the two import routes
- Modify: `crates/barnstormer-agent/src/import.rs` — leave for now (still referenced by cli/tests? check `grep -r import::` to decide) or delete if truly orphaned after the server routes are removed
- Search-and-remove any "Import" link in nav templates (`grep -n -r "Import" templates/`)

**Step 1: Remove routes**

Delete:

```rust
.route("/web/specs/import", get(web::import_spec_form).post(web::import_spec))
.route("/api/specs/import", post(api::import::import_spec))
```

**Step 2: Delete handler functions and templates**

Remove `ImportSpecFormTemplate`, `import_spec_form`, `import_spec` from `web/mod.rs`. Remove the template file. Remove `api/import.rs`.

**Step 3: Check for orphaned code**

Run `grep -r "import_spec" crates/ templates/` — clean up any remaining references. For `barnstormer-agent/src/import.rs`, run `grep -r "barnstormer_agent::import" crates/`. If nothing outside the crate itself references it, delete the file and remove the `pub mod import` line.

**Step 4: Remove nav link**

Find "Import" in `templates/` and remove the nav item. (Likely in the nav rail template.)

**Step 5: Run full test suite**

Run: `cargo test --all && cargo clippy --all-targets -- -D warnings`
Expected: pass.

**Step 6: Commit**

```bash
git commit -m "refactor: remove separate Import flow (absorbed by Create)"
```

---

## Task 20: Integration smoke test

**Files:**
- Modify / Create: `crates/barnstormer-server/tests/context_upload.rs` or a new `tests/smoke_context.rs`

**Purpose:** end-to-end path — create spec via form, upload file via panel, update notes, remove, undo. Exercises actors, state, events, HTTP, templates.

**Step 1: Write the test**

```rust
#[tokio::test]
async fn smoke_full_context_lifecycle() {
    let (router, state) = test_fixtures::router_with_tempdir().await;
    // 1. Create spec via POST /web/specs with description + no files.
    // 2. POST /web/specs/{id}/context with a text file → assert attached.
    // 3. PATCH notes → assert notes present in state.
    // 4. DELETE → assert removed=true.
    // 5. POST /web/specs/{id}/undo → assert removed=false (via existing undo endpoint).
    // 6. GET /web/specs/{id}/context-panel → assert HTML contains filename.
}
```

**Step 2: Run + commit**

```bash
cargo test --all
git commit -m "test: smoke test for full context-file lifecycle"
```

---

## Task 21: Final verification + PR

**Steps:**

1. `cargo fmt --all`
2. `cargo clippy --all-targets -- -D warnings`
3. `cargo test --all`
4. `cargo run -- start` → manually exercise:
   - Create a new spec with one text file attached on the form
   - Upload a second file from the panel
   - See the summary appear
   - Edit notes, blur, confirm they persist (refresh page)
   - Remove, confirm soft-delete
   - Transition to Active phase, confirm context rail disappears but `retrieve_context` still works (verify via Active-phase agent usage if reachable, or via a quick unit check)
5. Open PR with link back to the design doc.

---

## Out of scope (Phase 2)

- Multimodal content blocks in `mux-rs`
- Binary / image / PDF uploads
- Summary retry UI
- Per-spec storage quotas
- RAG chunking
- Preview rendering

See the design doc for rationale.
