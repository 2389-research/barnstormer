# Chat Input, Provider Status & Agent Kickoff — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** Add a free-text chat input to the activity panel, show model connection status, and wire agent kickoff so the brainstorming flow works end-to-end.

**Architecture:** Three independent features layered on existing infrastructure. Chat input uses existing `AppendTranscript` command. Provider status reads env vars at startup. Agent kickoff stores `SwarmOrchestrator` handles in `AppState` alongside actor handles, auto-starting agents when a spec is created and a provider is available.

**Tech Stack:** Axum + Askama + HTMX (existing), barnstormer-agent SwarmOrchestrator, barnstormer-core Command::AppendTranscript

---

## Phase 1: Chat Input (foundational, no dependencies)

### Task 1: Add chat form to activity template

**Files:**
- Modify: `templates/partials/activity.html`

**Step 1: Add chat input form**

Add a chat input form between the `agent-controls` div and the question widget section. The form posts to `/web/specs/{spec_id}/chat` with a simple text input. Place it at the bottom of the activity panel as a persistent "message bar":

```html
<div class="chat-input" id="chat-input">
    <form hx-post="/web/specs/{{ spec_id }}/chat"
          hx-target="#activity-container"
          hx-swap="innerHTML">
        <div class="chat-input-row">
            <input type="text" name="message" placeholder="Type a message..." autocomplete="off" required>
            <button type="submit" class="btn btn-primary btn-sm">Send</button>
        </div>
    </form>
</div>
```

Insert this BEFORE the `agent-controls` div (line 71), so the layout is:
1. Activity feed (scrollable)
2. Question widget (if pending)
3. Chat input (always present)
4. Agent controls (undo, etc.)

**Step 2: Add CSS for chat input**

Add to `static/style.css`:

```css
/* --- Chat input --- */
.chat-input {
    padding: var(--spacing-sm) var(--spacing-md);
    border-top: 1px solid var(--border);
    background: var(--bg-secondary);
}

.chat-input-row {
    display: flex;
    gap: var(--spacing-sm);
}

.chat-input-row input {
    flex: 1;
    padding: var(--spacing-sm);
    background: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--text-primary);
    font-size: 0.85rem;
    font-family: inherit;
}

.chat-input-row input:focus {
    outline: none;
    border-color: var(--accent);
}

.chat-input-row button {
    flex-shrink: 0;
}
```

### Task 2: Add chat route handler

**Files:**
- Modify: `crates/barnstormer-server/src/web/mod.rs`
- Modify: `crates/barnstormer-server/src/routes.rs`

**Step 1: Add ChatForm struct and handler in web/mod.rs**

Add a new `ChatForm` struct:
```rust
#[derive(Deserialize)]
pub struct ChatForm {
    pub message: String,
}
```

Add a new handler `chat`:
```rust
/// POST /web/specs/{id}/chat - Send a free-text message as the human.
pub async fn chat(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Form(form): Form<ChatForm>,
) -> impl IntoResponse {
    // Parse spec_id, get actor handle
    // Send Command::AppendTranscript { sender: "human".to_string(), content: form.message }
    // Persist events
    // Return refreshed activity panel
}
```

**Step 2: Wire the route in routes.rs**

Add: `.route("/web/specs/{id}/chat", post(web::chat))`

### Task 3: Tests for chat functionality

**Files:**
- Modify: `crates/barnstormer-server/src/web/mod.rs` (tests module)

Write tests:
1. `chat_form_submits_transcript` - POST to /web/specs/{id}/chat returns 200 with refreshed activity
2. `chat_form_message_appears_in_transcript` - Verify the message shows up in the transcript
3. Template test: activity template renders with chat input form

---

## Phase 2: Provider Status (standalone, no dependencies)

### Task 4: Add ProviderStatus detection

**Files:**
- Create: `crates/barnstormer-server/src/providers.rs`

Create a `ProviderStatus` struct that detects available LLM providers from environment:

```rust
/// Status of a single LLM provider.
pub struct ProviderInfo {
    pub name: String,          // "anthropic", "openai", "gemini"
    pub has_api_key: bool,
    pub model: String,         // resolved model name
    pub base_url: Option<String>,
}

/// Overall provider status for the UI.
pub struct ProviderStatus {
    pub default_provider: String,
    pub providers: Vec<ProviderInfo>,
    pub any_available: bool,
}
```

Add `detect()` method that reads env vars:
- Check ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY
- Read SPECD_DEFAULT_PROVIDER, SPECD_DEFAULT_MODEL
- Read {PROVIDER}_MODEL and {PROVIDER}_BASE_URL
- Never expose actual API key values - only whether they are set

### Task 5: Add provider status to AppState and routes

**Files:**
- Modify: `crates/barnstormer-server/src/app_state.rs`
- Modify: `crates/barnstormer-server/src/routes.rs`
- Modify: `crates/barnstormer-server/src/lib.rs`

Add `provider_status: ProviderStatus` field to AppState. Computed once at startup (env vars don't change at runtime).

Add web route: `GET /web/provider-status` - returns a partial HTML indicator.

### Task 6: Add provider status UI to index template

**Files:**
- Modify: `templates/index.html`
- Modify: `static/style.css`

Add a provider status indicator in the left rail, below the spec list and above the "+ New Spec" button. Shows:
- Provider name + model (e.g., "anthropic / claude-sonnet-4-5-20250929")
- Green dot if API key present, red dot if missing
- "No provider configured" warning if none available

CSS for status indicator:
```css
.provider-status { ... }
.status-dot { ... }
.status-dot.connected { background: var(--success); }
.status-dot.disconnected { background: var(--danger); }
```

### Task 7: Tests for provider status

Tests:
1. `ProviderStatus::detect()` with no env vars → no providers available
2. `ProviderStatus::detect()` with ANTHROPIC_API_KEY → anthropic available
3. Provider status template renders correctly
4. GET /web/provider-status returns HTML partial

---

## Phase 3: Agent Kickoff (builds on Phase 1 & 2)

### Task 8: Add swarm handles to AppState

**Files:**
- Modify: `crates/barnstormer-server/src/app_state.rs`

Add a field for swarm orchestrator handles per spec:
```rust
pub swarms: Arc<RwLock<HashMap<Ulid, Arc<SwarmOrchestrator>>>>,
```

Update `AppState::new()` to initialize the empty map.

### Task 9: Add agent start/pause/resume routes

**Files:**
- Modify: `crates/barnstormer-server/src/web/mod.rs`
- Modify: `crates/barnstormer-server/src/routes.rs`

Add handlers:
- `POST /web/specs/{id}/agents/start` → Create SwarmOrchestrator, store in state, spawn agent loop tasks
- `POST /web/specs/{id}/agents/pause` → Pause swarm
- `POST /web/specs/{id}/agents/resume` → Resume swarm

Each returns a partial HTML showing agent status (running/paused/stopped).

Add an "agent status" partial template:
- Create: `templates/partials/agent_status.html`

### Task 10: Add Start Agents button to spec view

**Files:**
- Modify: `templates/partials/spec_view.html`
- Modify: `templates/partials/activity.html`

Add a "Start Agents" button in the tab bar or agent-controls area. When agents are running, show Pause/Resume instead.

### Task 11: Auto-start agents on spec creation (optional)

**Files:**
- Modify: `crates/barnstormer-server/src/web/mod.rs` (create_spec handler)

After creating a spec, if `provider_status.any_available` is true, auto-create the swarm and start agents. The brainstormer agent will ask the first question, which shows up in the activity panel via SSE.

### Task 12: Tests for agent wiring

Tests using StubRuntime:
1. Start agents for a spec → swarm stored in state
2. Pause/resume toggle works
3. Agent produces transcript entries visible in activity
4. Auto-start on spec creation when provider available
