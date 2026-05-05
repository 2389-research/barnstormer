# Brainstorming Context Files — Design

**Date:** 2026-04-21
**Status:** Approved, ready for implementation planning
**Scope:** Phase 1 — text files only

## Overview

Allow users to attach files during spec creation and throughout the brainstorming phase to provide the Manager agent with reference material (existing specs, meeting notes, requirements docs, code samples, etc.). Each attachment gets an LLM-generated summary and an editable user note, both injected into the agent's context. Agents can retrieve full file content on demand.

This feature absorbs and replaces the existing Import flow: "create new" becomes the single entry point for starting a spec, optionally with context material.

**Phased:**
- **Phase 1 (this design):** Text files only — anything readable as UTF-8.
- **Phase 2 (future):** Multimodal — images, PDFs, and other binary formats. Requires prerequisite work in `mux-rs` to add image/document content blocks.

## Goals

- Users can drop reference material into a spec at creation time and during brainstorming
- The Manager agent has summaries and user-provided annotations in its context
- Agents can drill into full file content when summaries aren't enough
- Replace the separate Import flow with a unified Create flow
- Event-sourced so changes participate in history, replay, and undo

## Non-Goals (Phase 1)

- Binary formats (images, PDFs) — Phase 2
- RAG / chunked retrieval — using summarize-then-inject instead
- Per-spec storage quotas
- Preview rendering of attachment content

## Domain Model

New type in `barnstormer-core`:

```rust
pub struct ContextAttachment {
    pub attachment_id: Ulid,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub summary: Option<String>,       // LLM-generated, populated async
    pub user_notes: Option<String>,    // user annotation
    pub added_at: DateTime<Utc>,
    pub removed: bool,                 // soft-delete for undo support
}
```

New commands:

- `AttachContext { attachment_id, filename, mime_type, size_bytes }`
- `SummarizeContext { attachment_id, summary }`
- `UpdateContextNotes { attachment_id, notes }`
- `RemoveContext { attachment_id }`

New events (one per command):

- `ContextAttached`
- `ContextSummarized`
- `ContextNotesUpdated`
- `ContextRemoved`

State change: `SpecState` gets `context_attachments: Vec<ContextAttachment>`. All four events are undo-able via the existing undo stack mechanism.

File storage: `~/.barnstormer/specs/{spec_id}/context/{attachment_id}/{filename}`. The per-attachment directory namespaces the filename to avoid collisions while preserving the original name for downloads and display.

## Upload & Summarization Pipeline

New endpoint: `POST /web/specs/{id}/context` accepting `multipart/form-data`.

Flow:

1. Receive file, generate `attachment_id` (ULID)
2. Attempt UTF-8 read of file bytes; if not valid UTF-8, reject with 415 and message "Binary files not yet supported — text files only for now"
3. Enforce per-file size limit (20MB, configurable)
4. Write bytes to disk at `context/{attachment_id}/{filename}`
5. Send `AttachContext` command to actor
6. SSE pushes `context_attached` event → UI shows card with "Summarizing..." state
7. Spawn async task: send file content to LLM with a summarization prompt
8. When summary returns, send `SummarizeContext` command
9. SSE pushes `context_summarized` → UI replaces spinner with summary text

Additional endpoints:

- `PATCH /web/specs/{id}/context/{attachment_id}/notes` — user edits annotation (auto-save on blur)
- `DELETE /web/specs/{id}/context/{attachment_id}` — soft-remove (file stays on disk for undo)
- `GET /web/specs/{id}/context/{attachment_id}/raw` — serves raw file content (used by `retrieve_context` tool)

Summarization prompt (initial draft — refine during implementation):

> Summarize this document concisely, focusing on what would be relevant for building a software specification. Preserve key technical details, names, and constraints.

## Agent Integration

`build_task_prompt` in `barnstormer-agent` gains a "Context Files" section when the spec has any non-removed attachments:

```text
## Context Files

### 1. requirements.md (12KB)
**User notes:** From the kickoff meeting last week
**Summary:** The document outlines three core requirements...

### 2. existing-api.yaml (8KB)
**Summary:** OpenAPI spec for the current REST API with 14 endpoints...
```

New agent tool: `retrieve_context`

- Input: `{ attachment_id: String }`
- Output: full file content as text
- Available to agents in all phases (so Active-phase agents can still reference context from brainstorming)
- Registered in the tool registry alongside the existing mux tools

## UI / UX

All new UI reuses existing visual primitives from `static/style.css`. No new design language.

### Create New form

- Existing `.form-group` elements for title / one-liner / goal stay unchanged
- New `.form-group` below them: file input with staged-files list
- Each staged file renders as a small `.card` with `.card-type` badge, filename, size, and a `.btn.btn-sm.btn-danger` remove button
- Files are queued client-side until form submit
- On submit: spec is created first, then files upload sequentially, then redirect to brainstorming view

### Brainstorming view — Context panel

- Reuse the Active-phase three-column layout primitive (`.spec-body` → `.canvas` + right rail). In brainstorming, enable the right rail and populate it with a context panel instead of the chat rail.
- Panel structure matches `.chat-panel`:
  - `.chat-panel-header` with title "Context" and an add-file button
  - Scrollable body listing attachments
  - Optional footer showing count / total size
- Each attachment renders as a `.card`:
  - `.card-type` badge for file type (reuse `.badge-*` color variants; `.badge-note` fits well for documents)
  - Filename as title, size + timestamp as subtitle
  - Summary in card body; show a spinner in the same style as existing async states while summarization is pending
  - Inline `.form-group textarea` for user notes (auto-save on blur)
  - `.btn.btn-sm.btn-danger` remove button
- Collapsible via a `.view-toggle` capsule button in the command bar. Hidden = `display: none` on the rail (matches existing `#agent-canvas` pattern).
- Mobile: follow whatever responsive pattern the chat rail uses (established in the recent phase-wayfinding work).

### Active phase

No UI change. Attachments remain in state and accessible via `retrieve_context`. A read-only view can be added later if needed.

### SSE integration

Context panel is an HTMX partial that swaps on `context_*` events, matching the existing canvas refresh and transcript update patterns.

### Import flow removal

- `GET /web/specs/import` and `POST /web/specs/import` routes removed
- `POST /api/specs/import` removed
- `templates/partials/import_spec_form.html` removed
- "Import" link/button in nav removed (Create absorbs it)

## Testing

**Unit tests** (in `barnstormer-core`):
- Reducer behavior for each of the four new events
- Undo behavior for each event
- Filename sanitization (reject path traversal)
- UTF-8 detection helper

**Integration tests** (web/server crate):
- Upload of a text file → `ContextAttached` event present, file on disk
- Upload of binary content → 415, no state mutation
- `SummarizeContext` command → `ContextSummarized` event present
- `PATCH` notes → notes updated, event present
- `DELETE` attachment → soft-removed, file preserved, undo restores it
- `retrieve_context` tool returns expected file content
- Agent `build_task_prompt` includes context section when attachments exist

## Error Handling

| Condition                                | Response                                                         |
|------------------------------------------|------------------------------------------------------------------|
| File exceeds size limit                  | 413; no state mutation                                           |
| File not valid UTF-8                     | 415 with "text files only for now"; no state mutation            |
| File write fails                         | 500; no `AttachContext` sent                                     |
| LLM summarization fails                  | Attachment persists with `summary: None`; UI shows error message |
| Attachment not found (update/remove/raw) | 404                                                              |
| Spec not in brainstorming phase          | 409 on new attachments; existing ones still accessible           |

**Crash recovery:** On startup, verify each `ContextAttached` event has a corresponding file on disk. Log warnings for orphans but do not block.

## Scope Summary

**In scope (Phase 1):**
- Four new commands/events + state field
- Disk storage with namespaced paths
- Upload endpoint + CRUD
- Summarization subagent (text)
- Prompt injection
- `retrieve_context` tool
- Create form dropzone
- Brainstorming context panel
- Removal of Import flow

**Out of scope (Phase 2 or later):**
- Multimodal content blocks in `mux-rs`
- Images, PDFs, binary formats
- Preview rendering
- RAG-style chunking
- Per-spec storage quotas
- Summary retry UI

## Open Questions

None — all resolved during brainstorming.

## Next Step

Invoke the `writing-plans` skill to produce a detailed implementation plan.
