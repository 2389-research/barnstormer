# Multimodal Context Files — Design (Phase 2)

**Date:** 2026-05-07
**Status:** Approved, ready for implementation planning
**Scope:** Phase 2 — extend Phase 1 (text-only) with images, PDFs, audio, and video

## Overview

Phase 2 of the brainstorming context-files feature: extend the existing
text-only attachment system to accept images, PDFs, audio, and video. The key
architectural choice is that **multimodal content lives entirely inside a
one-shot summarizer subagent**; the brainstorming Manager swarm stays
text-only and consumes the resulting text summary like it does today.

Two new affordances:

- **Notes drive re-summarization.** Editing the user-notes field on an
  attachment automatically re-fires the summarizer, so the user can steer the
  summary toward what they care about ("here's an example of the vibes we
  want" → vibe-focused summary).
- **Manual Resummarize button.** Per attachment. Useful after switching
  providers (when capability gating previously prevented summarization), or
  whenever the user wants to refresh the summary.

The agent's `retrieve_context` tool gains an optional `question` parameter that
fires a fresh summarizer call against the attachment with a targeted question,
rather than just returning the stored summary.

This design intentionally requires **no changes to mux-rs**.

## Goals

- Users can attach images, PDFs, audio, and video to a spec during brainstorming
- The Manager agent reads multimodal-derived summaries injected as text in its
  prompt — no per-turn token bloat
- The agent can ask focused questions about an attachment via
  `retrieve_context(id, question)`
- Notes drive re-summarization automatically; manual Resummarize is the
  recovery affordance after provider swaps
- Provider capability is gated gracefully: uploads succeed, summaries degrade
  when the configured provider can't handle the media kind
- Event-sourced so all changes participate in history, replay, and undo

## Non-Goals

- Changes to mux-rs (deliberately none — the summarizer-as-subagent design
  sidesteps the need for tool-result media support)
- Per-spec storage quotas
- PDF text extraction client-side (the multimodal LLM handles it)
- Long-form audio/video transcription pipelines (recommend transcribe-then-
  upload-as-text for content longer than the 20MB cap)
- A separate `BARNSTORMER_VISION_PROVIDER` env var
- Direct multimodal context for the Manager agent itself (it stays text-only)

## Why a summarizer subagent

mux v0.13.0 supports media in **user-message content blocks** (via
`Message::user_with` and `SubAgent::run_with_blocks`) but **not in tool
results** — `ToolResult.content` is `String`, and the agent runner converts
that into `ContentBlock::ToolResult { content: String, is_error: bool }`
before sending to the provider. Pixels can't flow through the tool-call
return path.

The summarizer-as-subagent architecture sidesteps this: pixels travel through
a fresh user message in a fresh sub-agent invocation (which mux supports
today), and only text crosses back into the Manager's context. Benefits:

- No mux change required
- Provider capability gating is contained to the summarizer
- Pixels are paid for at summarize time only — no per-Manager-turn token bloat
- Notes-driven re-summarization fits naturally on the same call path
- The summarizer can run on a different model than the Manager (vision-capable
  for the summarizer, fast-text for the Manager, configurable later)

The trade-off: the Manager never directly "sees" pixels. For a structured-Q&A
brainstorming Manager, that's the right trade.

## Whitelist

| Kind | Formats |
|------|---------|
| Image | PNG, JPEG, WebP, GIF, HEIC, HEIF, SVG (rasterized to PNG via `resvg`) |
| Document | PDF |
| Audio | WAV, MP3, M4A, AIFF, FLAC |
| Video | MP4, MOV, M4V, WebM |
| Text | (existing UTF-8 path — markdown, code, yaml, etc.) |

Anything outside the whitelist is rejected with 415.

**Provider support is uneven:**

| Provider  | Image | Document | Audio | Video |
|-----------|:-----:|:--------:|:-----:|:-----:|
| Anthropic | ✅    | ✅       | ❌    | ❌    |
| OpenAI    | ✅    | ✅       | ✅    | ❌    |
| Gemini    | ✅    | ✅       | ✅    | ✅    |
| Ollama    | ✅    | ❌       | ❌    | ❌    |

When the configured provider doesn't support the uploaded kind, the upload
still succeeds; the summarize attempt fails gracefully via
`MarkContextSummarizeFailed` with a reason like "current provider (anthropic)
doesn't support audio — switch providers and click Resummarize."

## Domain model changes

Existing `ContextAttachment` already covers most of what's needed
(`mime_type`, `size_bytes`, `summary: Option<String>`, `user_notes`, etc.).
Minimum additions:

```rust
pub struct ContextAttachment {
    // ... existing fields ...
    pub summary_error: Option<String>,  // last failure reason; cleared on next attempt
}
```

`summary_error` lets the UI distinguish "pending" from "failed." Today both
collapse to `summary: None` and the UI spins forever on permanent failures.

```rust
Command::MarkContextSummarizeFailed { attachment_id: Ulid, reason: String }
Event::ContextSummarizeFailed { attachment_id: Ulid, reason: String }
```

Sent by the summarizer task when capability gating or LLM error means we
can't produce a summary. Participates in the undo stack.

**Deliberately not added:**

- No `media_kind` field — derived at runtime from `mime_type` via a small helper
- No `ResummarizeContext` command — manual Resummarize is just a server-side
  trigger that re-fires the summarizer; only persists state when the summary
  returns or fails (existing commands handle both)
- No "Generating" state event — kept transient on the server. Crash mid-
  summarize leaves the attachment in pending state and the user can hit
  Resummarize.
- No `summary_generated_at` — can add later if needed

## Storage layout

Unchanged, except for SVG which gets a cached rasterized PNG:

```
~/.barnstormer/specs/{spec_id}/context/{attachment_id}/
    {filename}              # original bytes
    rasterized.png          # SVG-only, cached at upload time
```

## Upload pipeline

1. Multipart in → ULID gen
2. Read bytes (single-shot, bounded by 20MB cap)
3. **MIME detection via magic-byte sniff** using the `infer` crate
   - Browser-supplied Content-Type is ignored — user-controllable, not
     trustworthy for a whitelist gate
   - For text: if `infer` returns nothing, attempt UTF-8 read; valid UTF-8 →
     text path
   - For SVG: UTF-8 + content starts with `<svg` or `<?xml ... <svg` → SVG path
   - Whitelist match → if not matched → 415 with friendly per-kind message
4. 20MB size check → 413
5. **SVG branch:** rasterize via `resvg` → write `rasterized.png` next to the
   original file. Failure path: log + degrade to markup-only at summarize time.
6. Write original bytes to disk at `{att_dir}/{filename}` (existing layout)
7. `AttachContext` command sent with the **server-sniffed** `mime_type` (not the
   browser-claimed one). SSE pushes `context_attached`.
8. Dispatch summarizer based on detected `MediaKind` (see Summarizer section).

**One small new helper:** `media_kind_from_mime(&str) -> Option<MediaKind>` in
barnstormer-server (mux doesn't expose this directly).

## Summarizer

`summarizer.rs` gets one structural change (one input type, three shapes) and
grows two new entry points:

```rust
pub enum SummarizerInput {
    Text  { content: String },
    Media { kind: MediaKind, mime: String, path: PathBuf },
    Svg   { markup: String, raster_path: PathBuf },
}

// Fire-and-forget. Used by upload, notes-change, and the manual Resummarize
// button. On failure, sends MarkContextSummarizeFailed instead of silent drop.
pub fn spawn_summarize(
    actor: SpecActorHandle,
    attachment_id: Ulid,
    filename: String,
    notes: Option<String>,
    input: SummarizerInput,
);

// Awaitable. Used by retrieve_context(question?). When `question` is Some,
// replaces the summary prompt with the question.
pub async fn summarize_now(
    filename: &str,
    notes: Option<&str>,
    input: &SummarizerInput,
    question: Option<&str>,
) -> anyhow::Result<String>;
```

`spawn_summarize` is a thin wrapper that calls `summarize_now` with
`question=None` and routes the result back to the actor as a command. All
three flows share the same core. The request construction itself is extracted
into a pure helper `build_summarize_request(filename, notes, input, question)
-> mux::llm::Request` so it can be unit-tested without an LLM call.

**Prompt construction:**

```text
SYSTEM: <existing summarization prompt — refined to mention images/audio/video/documents>
USER:   <filename>{filename}</filename>
        {<user_notes>{notes}</user_notes> if notes are present}
        {content_block | media_block | svg_dual_block}
```

- **content_block** (text): existing 64KB-truncated text wrapped in `<content>` tags
- **media_block**: `ContentBlock::Media { kind, source: MediaSource::Path(path), mime_type }`
- **svg_dual_block**: media block (rasterized PNG) + text block with the
  original markup wrapped in `<svg_markup>...</svg_markup>`. Markup truncated
  at 64KB. If raster missing, degrades to markup-only.

**Why send both for SVG:** the rasterized PNG and the markup carry different
information. The PNG gives the LLM visual perception (layout, colors,
composition); the markup gives it semantic structure (element IDs, classes,
embedded text data, gradient definitions). For wireframes and diagrams,
combining them produces a substantially richer summary than either alone.

**Question mode (used by `retrieve_context`):** the default summary prompt is
replaced by the agent's question. Output stays plain text. The same media
block is sent. Capability gating is the same.

**Capability gating:** before constructing the request, check
`client.supports_media(kind)` for media inputs. On failure, return an error
with a reason naming the provider and the unsupported kind.
`spawn_summarize` translates that into `MarkContextSummarizeFailed`;
`summarize_now` returns it to `retrieve_context` which surfaces it as a tool
error.

**Concurrency:** latest-wins. Multiple summarize tasks for the same
attachment can race (notes change while a summarize is in flight, or rapid
manual Resummarize clicks). Each spawns a new task; whichever lands last is
the recorded summary. No generation counter for v1 — YAGNI.

## Re-summarize triggers

Two new triggers, both flow into `spawn_summarize`. A server-side helper
handles disk-to-input routing:

```rust
fn build_summarizer_input(
    home: &Path,
    spec_id: Ulid,
    attachment: &ContextAttachment,
) -> Result<SummarizerInput>
```

Reads from disk and branches on the stored `mime_type`:

- `text/*` (or any UTF-8 stored thing) → `Text { content: read_to_string(...) }`
- `image/svg+xml` → `Svg { markup: read_to_string(svg_path), raster_path: {att_dir}/rasterized.png }`
  (or markup-only if raster missing)
- Other `image/*` → `Media { Image, mime, path }`
- `application/pdf` → `Media { Document, mime, path }`
- `audio/*` → `Media { Audio, mime, path }`
- `video/*` → `Media { Video, mime, path }`

### Notes-change re-summarize

`update_context_notes` handler (`PATCH /web/specs/{id}/context/{att_id}/notes`)
gains one extra step *after* the existing `Command::UpdateContextNotes` is
accepted by the actor:

1. (existing) Send `UpdateContextNotes` command
2. (new) Read the updated attachment from state, build `SummarizerInput`,
   call `spawn_summarize` with the new notes

Always re-summarize on every notes update — UI auto-saves on blur, so each
PATCH represents a deliberate user action, not a keystroke. No additional
debounce needed. Latest-wins concurrency covers any rapid-fire edge case.

### Manual Resummarize button

New endpoint: `POST /web/specs/{id}/context/{att_id}/resummarize`

1. Look up attachment in actor state. 404 if not found, 410 if soft-removed
2. Build `SummarizerInput` from disk via the helper
3. `spawn_summarize(...)` with current `user_notes`
4. Return an HTMX partial: re-rendered attachment card with the
   "Summarizing…" spinner state. SSE will swap it again when the new summary
   lands (or fails).

**Phase gating:** unchanged from Phase 1. New attachments are 409 outside
brainstorming; resummarize and notes-change work in any phase.

## `retrieve_context` tool changes

The existing tool stays mostly as-is; gains an optional `question` parameter
and per-kind dispatch.

**Schema:**

```json
{
  "type": "object",
  "properties": {
    "attachment_id": {
      "type": "string",
      "description": "ULID of the attachment to retrieve"
    },
    "question": {
      "type": "string",
      "description": "Optional. When provided, dispatches a fresh summarizer call with this question against the attachment. Use when you need a targeted answer the existing summary doesn't cover."
    }
  },
  "required": ["attachment_id"]
}
```

**Description:**

> Retrieve content from a context file attachment, or ask a focused question
> about it. For text files, returns the full content (or a question-targeted
> answer). For images, PDFs, audio, and video, returns the stored summary (or
> a fresh summary answering your question).

**Execute logic:**

| Kind  | `question` is None | `question` is Some |
|---|---|---|
| Text  | Read file → return full text *(today's behavior)* | `summarize_now(Text{content}, question)` |
| Media | Return stored `summary` text (or stored `summary_error` as a tool error) | `summarize_now(Media/Svg, question)` |

`summarize_now` is the awaitable core from the Summarizer section.

**Edge cases:**

- Media attachment with no summary yet (`summary` and `summary_error` both
  `None`) and no `question`: return text like `"(summary still being
  generated — check back shortly, or pass a 'question' parameter to fetch a
  fresh answer now)"`.
- Soft-removed attachment: `attachment not found` (existing behavior).
- `summarize_now` fails on capability: tool returns `ToolResult::error(reason)`.

## UI / preview rendering

Each attachment card grows a per-kind preview block and a Resummarize button.
All built on existing `.card` primitives.

**Per-kind preview block:**

| Kind  | Preview |
|---|---|
| Text  | None (filename + size only — current behavior) |
| Image (raster) | `<img src="…/raw" alt="{filename}" class="context-preview-image" />` |
| Image (SVG) | `<img>` with the original SVG (browser renders natively; `X-Content-Type-Options: nosniff` keeps it sandboxed) |
| PDF   | Filename + PDF glyph; no inline preview |
| Audio | `<audio controls preload="metadata"><source src="…/raw" type="{mime}"></audio>` |
| Video | `<video controls preload="metadata" class="context-preview-video"><source src="…/raw" type="{mime}"></video>` |

CSS for new previews: `max-width: 100%; height: auto;` for images, sensible
max-height (~200px) for videos so cards don't dominate the rail. Reuse
existing `.card` body padding.

**Badge variants:** add `.badge-image`, `.badge-doc`, `.badge-audio`,
`.badge-video` (single-line CSS additions). Keep `.badge-note` for plain text.

**Resummarize button:**

```html
<button class="btn btn-sm"
        hx-post="/web/specs/{id}/context/{att_id}/resummarize"
        hx-target="closest .card"
        hx-swap="outerHTML">
  Resummarize
</button>
```

POST returns the freshly-rendered card with "Summarizing…" state. SSE then
pushes `context_summarized` (or `context_summarize_failed`) to swap it again
when the result lands.

**Summary state machine in templates:**

| State | Render |
|---|---|
| `summary: None, summary_error: None` | Spinner: "Summarizing…" |
| `summary: Some(text), summary_error: None` | Rendered markdown summary *(existing)* |
| `summary: None, summary_error: Some(reason)` | `.card-error` block + reason + Resummarize button |
| `summary: Some(stale), summary_error: Some(reason)` | Rendered prior summary + small "(last attempt failed: {reason})" line + Resummarize button |

The fourth state happens when you have a working summary and a subsequent
re-attempt fails. Keep the prior summary visible — better UX than blanking it.

**SSE events:** add `context_summarize_failed` to the existing `context_*`
set. Wire it into the cards-feed and context-panel partials' `hx-trigger`
lists alongside the existing `context_attached / context_summarized /
context_notes_updated / context_removed`.

## Error handling

| Condition | Surface | State |
|---|---|---|
| File > 20MB | 413; "File exceeds 20MB. For longer audio/video, transcribe and upload as text." | No state mutation |
| MIME not in whitelist | 415; per-kind hint listing allowed formats | No state mutation |
| File write fails on disk | 500; surfaced to UI as upload error | No state mutation |
| `infer` sniff returns nothing AND content not UTF-8 | 415 with "couldn't identify file type" | No state mutation |
| SVG rasterization fails | Upload succeeds with original SVG only; summarizer falls back to markup-only | Logged; attachment created normally |
| Provider doesn't support detected media kind | Upload succeeds; summarizer fires `MarkContextSummarizeFailed` | `summary_error: Some(reason)`; file on disk |
| LLM call fails (network, 5xx, timeout) | `MarkContextSummarizeFailed` with the underlying error | `summary_error: Some(...)` |
| LLM returns empty/whitespace summary | `MarkContextSummarizeFailed` with `"empty summary from LLM"` | `summary_error: Some(...)` |
| Notes update on missing attachment | 404 | No state mutation |
| Resummarize on missing attachment | 404 | No state mutation |
| Resummarize on soft-removed attachment | 410 Gone | No state mutation |
| Concurrent resummarize requests | All fire; latest-wins | Acceptable race |
| `retrieve_context` on missing/removed attachment | `ToolResult::error("attachment not found")` | — |
| `retrieve_context(question)` capability gate fails | `ToolResult::error(reason)` | — |
| `retrieve_context` on still-pending media (no `question`) | `ToolResult::text("(summary still being generated — retry, or pass a 'question' parameter)")` | — |
| Crash mid-summarize | On recovery: `summary: None, summary_error: None`. UI shows spinner. User clicks Resummarize → fresh attempt | Pending state survives restart cleanly |
| Orphaned files (event references a file not on disk) | Logged at startup; attachment kept in state with synthetic `summary_error` | Doesn't block recovery |
| Browser Content-Type vs. server-sniffed mismatch | Server-sniffed wins; mismatch logged at debug level | Stored mime is the sniffed one |

**Logging level guide:**

- `tracing::warn!` — capability misses, LLM failures, SVG raster failures
- `tracing::info!` — successful summarize, resummarize triggers
- `tracing::debug!` — MIME sniff results, mismatches, truncation events
- `tracing::error!` — disk write failures, panics in summarizer task

## Testing

Following the existing project pattern (no LLM mocks per CLAUDE.md — request
construction is tested as a pure function; end-to-end LLM flow is exercised
manually or via opt-in live-LLM tests).

**`barnstormer-core` (unit):**

- Reducer for new `ContextSummarizeFailed` event sets `summary_error`, clears
  it on the next `ContextSummarized`
- Undo behavior for `ContextSummarizeFailed`
- `summary_error` field serialization round-trip

**`barnstormer-server` (unit):**

- `media_kind_from_mime` helper — every whitelisted MIME maps correctly;
  unknowns return `None`
- MIME sniffing via `infer` — fixture bytes for every accepted format produce
  the expected MIME
- SVG detection helper — `<svg>` and `<?xml ... <svg>` both recognized
- Server-sniff-wins when browser Content-Type disagrees
- SVG rasterization (`resvg` integration) — valid SVG → PNG bytes; malformed
  SVG → error returned cleanly
- `build_summarize_request` extracted as a pure function and unit-tested for
  every input shape (text, image, SVG dual, audio, video, with/without notes,
  with/without question)

**`barnstormer-server` (integration — `tests/`):**

- Upload one fixture per accepted format → `AttachContext` event with sniffed
  MIME, file on disk; SVG additionally has `rasterized.png`
- Upload >20MB → 413
- Upload `.exe` and `.zip` → 415
- Upload with mismatched browser Content-Type → server-sniffed wins
- `POST .../resummarize` returns card partial with spinner state
- `POST .../resummarize` on soft-removed → 410
- `POST .../resummarize` on missing → 404
- `PATCH .../notes` triggers a new summarize task spawn
- Capability-gated failure path: `MarkContextSummarizeFailed` lands; UI
  partial renders the error state

**`barnstormer-agent` (unit):**

- `retrieve_context(id)` no question on text → returns full text *(existing)*
- `retrieve_context(id)` no question on media + summary → returns summary
- `retrieve_context(id)` no question on media + summary_error → tool error
- `retrieve_context(id)` no question on still-pending media → "still being
  generated" hint
- `retrieve_context(id, question)` builds a `summarize_now` call with the
  right input shape
- Removed and missing attachments → error *(existing)*

**Smoke (`tests/smoke.rs`):**

- Full path with one image fixture, gated on `BARNSTORMER_LIVE_LLM=1`:
  upload → SSE `context_attached` → manual resummarize → eventual summary
  lands

**Manual / dev verification:**

- Each format with a real provider: upload → preview renders → summary lands
  → edit notes → summary regenerates → click Resummarize → spinner + new
  summary
- Capability gating: configure Anthropic, upload audio → "current provider
  doesn't support audio"; switch provider, click Resummarize → succeeds
- Agent calls `retrieve_context(question)` and gets a useful answer

## Dependencies

New Rust crates:

- `infer` — magic-byte MIME detection. Lightweight, no system deps.
- `resvg` (and `tiny-skia` / `usvg` from the same family) — pure-Rust SVG
  rasterization. No system deps.

No mux-rs version bump required.

## Scope summary

**In scope:**

- Whitelist: PNG, JPEG, WebP, GIF, HEIC, HEIF, SVG, PDF, WAV, MP3, M4A, AIFF,
  FLAC, MP4, MOV, M4V, WebM
- Multimodal-aware summarizer (subagent pattern, no mux change)
- Notes-driven re-summarization
- Manual Resummarize endpoint + UI button
- `retrieve_context(id, question?)` enhancement
- UI preview rendering for images / audio / video; PDF icon-only
- Capability-gated graceful degradation per provider
- New `summary_error` field + `MarkContextSummarizeFailed` command/event

**Out of scope:**

- mux-rs changes
- Per-spec storage quotas
- PDF text extraction client-side
- Long-form audio/video transcription pipelines
- Separate vision-provider env var
- Direct multimodal context for the Manager agent itself

## Open questions

None — all resolved during brainstorming.

## Next step

Invoke the `writing-plans` skill to produce a detailed implementation plan.
