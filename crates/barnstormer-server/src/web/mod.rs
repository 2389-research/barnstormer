// ABOUTME: Web UI route handlers serving HTML via Askama templates and HTMX.
// ABOUTME: Provides browser-friendly views for spec management, board, documents, and activity.

use std::sync::Arc;

use axum::extract::{Form, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;
use barnstormer_agent::SwarmOrchestrator;
use barnstormer_core::{ActorError, Command, SpecPhase, SpecState, spawn};
use barnstormer_store::{JsonlLog, SnapshotData, save_snapshot};
use chrono::Utc;
use ulid::Ulid;

use pulldown_cmark::{Event, Options, Parser, html};

use crate::api::specs::SpecSummary;
use crate::app_state::SharedState;

use askama::Template;
use askama_derive_axum::IntoResponse as AskamaIntoResponse;

/// Index page showing the spec list and welcome message.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "index.html")]
pub struct IndexTemplate {}

/// GET / - Render the main index page.
pub async fn index() -> IndexTemplate {
    IndexTemplate {}
}

/// Partial: list of specs for the left rail.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/spec_list.html")]
pub struct SpecListTemplate {
    pub specs: Vec<SpecSummary>,
}

/// GET /web/specs - Return the spec list as an HTML partial.
pub async fn spec_list(State(state): State<SharedState>) -> impl IntoResponse {
    let actors = state.actors.read().await;
    let mut specs = Vec::new();

    for (spec_id, handle) in actors.iter() {
        let spec_state = handle.read_state().await;
        if let Some(ref core) = spec_state.core {
            specs.push(SpecSummary {
                spec_id: spec_id.to_string(),
                title: core.title.clone(),
                one_liner: core.one_liner.clone(),
                updated_at: core.updated_at.to_rfc3339(),
            });
        }
    }

    SpecListTemplate { specs }
}

/// Partial: create spec form.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/create_spec_form.html")]
pub struct CreateSpecFormTemplate {}

/// GET /web/specs/new - Render the create spec form.
pub async fn create_spec_form() -> CreateSpecFormTemplate {
    CreateSpecFormTemplate {}
}

/// Partial: import spec form.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/import_spec_form.html")]
pub struct ImportSpecFormTemplate {}

/// GET /web/specs/import - Render the import spec form.
pub async fn import_spec_form() -> ImportSpecFormTemplate {
    ImportSpecFormTemplate {}
}

/// Form data for importing a spec from arbitrary content.
#[derive(Deserialize)]
pub struct ImportSpecForm {
    pub content: String,
    #[serde(default)]
    pub source_format: Option<String>,
}

/// POST /web/specs/import - Import a spec from pasted content via LLM.
pub async fn import_spec(
    State(state): State<SharedState>,
    Form(form): Form<ImportSpecForm>,
) -> Response {
    if form.content.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Html("<p class=\"error-msg\">Content must not be empty.</p>".to_string()),
        )
            .into_response();
    }

    // Create LLM client
    let provider = &state.provider_status.default_provider;
    let (client, model) = match barnstormer_agent::client::create_llm_client(
        provider,
        state.provider_status.default_model.as_deref(),
    ) {
        Ok(pair) => pair,
        Err(e) => {
            tracing::error!("failed to create LLM client for import: {}", e);
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Html(format!(
                    "<p class=\"error-msg\">LLM provider not available: {}</p>",
                    e
                )),
            )
                .into_response();
        }
    };

    // Parse via LLM
    let source_hint = form
        .source_format
        .as_deref()
        .filter(|s| *s != "auto");
    let import_result = match barnstormer_agent::import::parse_with_llm(
        &form.content,
        source_hint,
        &client,
        &model,
    )
    .await
    {
        Ok(result) => result,
        Err(e) => {
            tracing::error!("LLM import failed: {}", e);
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Html(format!(
                    "<p class=\"error-msg\">Failed to parse content: {}</p>",
                    e
                )),
            )
                .into_response();
        }
    };

    let commands = barnstormer_agent::import::to_commands(&import_result);

    // Create spec directory and JSONL log
    let spec_id = Ulid::new();
    let spec_dir = state
        .barnstormer_home
        .join("specs")
        .join(spec_id.to_string());
    if let Err(e) = std::fs::create_dir_all(&spec_dir) {
        tracing::error!("failed to create spec directory: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html("<p class=\"error-msg\">Failed to create spec directory.</p>".to_string()),
        )
            .into_response();
    }
    let log_path = spec_dir.join("events.jsonl");
    let mut log = match JsonlLog::open(&log_path) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("failed to create JSONL log: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html("<p class=\"error-msg\">Failed to create spec storage.</p>".to_string()),
            )
                .into_response();
        }
    };

    // Spawn actor and send all commands
    let handle = spawn(spec_id, SpecState::new());
    for cmd in commands {
        match handle.send_command(cmd).await {
            Ok(events) => {
                for event in &events {
                    if let Err(e) = log.append(event) {
                        tracing::error!("failed to persist event: {}", e);
                    }
                }
            }
            Err(e) => {
                tracing::error!("failed to send import command: {}", e);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Html(format!(
                        "<p class=\"error-msg\">Failed to create spec: {}</p>",
                        e
                    )),
                )
                    .into_response();
            }
        }
    }

    // Subscribe event persister
    let persister_handle = spawn_event_persister(&handle, spec_id, &state.barnstormer_home);
    state
        .event_persisters
        .write()
        .await
        .insert(spec_id, persister_handle);

    state.actors.write().await.insert(spec_id, handle);

    // Auto-start agents if a provider is available
    {
        let actors = state.actors.read().await;
        if let Some(handle_ref) = actors.get(&spec_id) {
            try_start_agents(&state, spec_id, handle_ref).await;
        }
    }

    // Return the spec view so HTMX navigates into the imported spec
    let spec_state = {
        let actors = state.actors.read().await;
        match actors.get(&spec_id) {
            Some(h) => h.read_state().await.clone(),
            None => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Html("<p class=\"error-msg\">Spec imported but not found.</p>".to_string()),
                )
                    .into_response();
            }
        }
    };

    let lanes = cards_by_lane(&spec_state);
    let core = match spec_state.core.as_ref() {
        Some(c) => c,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(
                    "<p class=\"error-msg\">Spec imported but core data is missing.</p>".to_string(),
                ),
            )
                .into_response();
        }
    };
    let spec_id_str = spec_id.to_string();
    let phase = match spec_state.phase {
        SpecPhase::Brainstorming => "brainstorming".to_string(),
        SpecPhase::Refining => "refining".to_string(),
        SpecPhase::Complete => "complete".to_string(),
    };

    let has_pending_question = spec_state.pending_question.is_some();
    let canvas_content = spec_state.canvas_content.clone();

    let mut response = SpecViewTemplate {
        spec_id: spec_id_str.clone(),
        title: core.title.clone(),
        one_liner: core.one_liner.clone(),
        goal: core.goal.clone(),
        phase,
        lanes,
        canvas_content,
        has_pending_question,
    }
    .into_response();

    response.headers_mut().insert(
        axum::http::HeaderName::from_static("hx-push-url"),
        axum::http::HeaderValue::from_str(&format!("/web/specs/{}", spec_id_str)).unwrap(),
    );

    response
}

/// Form data for creating a new spec.
#[derive(Deserialize)]
pub struct CreateSpecForm {
    pub description: String,
}

/// Extract a placeholder title from free-text description.
/// Takes the first sentence (ending in . ! ?) or first 60 chars, whichever is shorter.
fn extract_placeholder_title(description: &str) -> String {
    let trimmed = description.trim();
    if trimmed.is_empty() {
        return String::from("Untitled Spec");
    }
    let sentence_end = trimmed
        .find(['.', '!', '?'])
        .map(|i| i + 1)
        .unwrap_or(trimmed.len());
    // Truncate by character count (not bytes) for consistent title length.
    let char_boundary = trimmed
        .char_indices()
        .nth(60)
        .map(|(i, _)| i)
        .unwrap_or(trimmed.len());
    let end = sentence_end.min(char_boundary);
    let mut title = trimmed[..end].to_string();
    if end < trimmed.len() && !title.ends_with(['.', '!', '?']) {
        title.push_str("...");
    }
    title
}

/// POST /web/specs - Create a spec from free-text description, return spec view.
pub async fn create_spec(
    State(state): State<SharedState>,
    Form(form): Form<CreateSpecForm>,
) -> Response {
    let spec_id = Ulid::new();
    let spec_dir = state.barnstormer_home.join("specs").join(spec_id.to_string());
    if let Err(e) = std::fs::create_dir_all(&spec_dir) {
        tracing::error!("failed to create spec directory: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html("<p class=\"error-msg\">Failed to create spec directory.</p>".to_string()),
        )
            .into_response();
    }
    let log_path = spec_dir.join("events.jsonl");

    let mut log = match JsonlLog::open(&log_path) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("failed to create JSONL log: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html("<p class=\"error-msg\">Failed to create spec storage.</p>".to_string()),
            )
                .into_response();
        }
    };

    let handle = spawn(spec_id, SpecState::new());
    let events = match handle
        .send_command(Command::CreateSpec {
            title: extract_placeholder_title(&form.description),
            one_liner: String::new(),
            goal: String::new(),
        })
        .await
    {
        Ok(events) => events,
        Err(e) => {
            tracing::error!("failed to create spec: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(format!(
                    "<p class=\"error-msg\">Failed to create spec: {}</p>",
                    e
                )),
            )
                .into_response();
        }
    };

    for event in &events {
        if let Err(e) = log.append(event) {
            tracing::error!("failed to persist event: {}", e);
        }
    }

    // Append the user's free-text description to the transcript so the
    // manager agent can read it and parse it into structured fields.
    let transcript_events = match handle
        .send_command(Command::AppendTranscript {
            sender: "human".to_string(),
            content: form.description,
        })
        .await
    {
        Ok(events) => events,
        Err(e) => {
            tracing::error!("failed to append transcript: {}", e);
            vec![]
        }
    };
    for event in &transcript_events {
        if let Err(e) = log.append(event) {
            tracing::error!("failed to persist transcript event: {}", e);
        }
    }

    // Subscribe the event persister BEFORE inserting the actor and starting
    // agents so it catches all subsequent events (agent-produced, etc.).
    // The CreateSpec events above were already persisted inline.
    let persister_handle = spawn_event_persister(&handle, spec_id, &state.barnstormer_home);
    state.event_persisters.write().await.insert(spec_id, persister_handle);

    state.actors.write().await.insert(spec_id, handle);

    // Auto-start agents if a provider is available
    {
        let actors = state.actors.read().await;
        if let Some(handle_ref) = actors.get(&spec_id) {
            try_start_agents(&state, spec_id, handle_ref).await;
        }
    }

    // Return the spec view so HTMX navigates directly into the new spec
    let spec_state = {
        let actors = state.actors.read().await;
        match actors.get(&spec_id) {
            Some(h) => h.read_state().await.clone(),
            None => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Html("<p class=\"error-msg\">Spec created but not found.</p>".to_string()),
                )
                    .into_response();
            }
        }
    };

    let lanes = cards_by_lane(&spec_state);
    let core = match spec_state.core.as_ref() {
        Some(c) => c,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html("<p class=\"error-msg\">Spec created but core data is missing.</p>".to_string()),
            )
                .into_response();
        }
    };
    let spec_id_str = spec_id.to_string();
    let phase = match spec_state.phase {
        SpecPhase::Brainstorming => "brainstorming".to_string(),
        SpecPhase::Refining => "refining".to_string(),
        SpecPhase::Complete => "complete".to_string(),
    };

    let has_pending_question = spec_state.pending_question.is_some();
    let canvas_content = spec_state.canvas_content.clone();

    let mut response = SpecViewTemplate {
        spec_id: spec_id_str.clone(),
        title: core.title.clone(),
        one_liner: core.one_liner.clone(),
        goal: core.goal.clone(),
        phase,
        lanes,
        canvas_content,
        has_pending_question,
    }
    .into_response();

    // Set HX-Push-Url so the browser URL updates to the spec view
    response.headers_mut().insert(
        axum::http::HeaderName::from_static("hx-push-url"),
        axum::http::HeaderValue::from_str(&format!("/web/specs/{}", spec_id_str)).unwrap(),
    );

    response
}

/// Helper to parse a ULID from a path string, returning an error response on failure.
fn parse_spec_id(id: &str) -> Result<Ulid, Box<Response>> {
    id.parse::<Ulid>().map_err(|_| {
        Box::new(
            (
                StatusCode::BAD_REQUEST,
                Html("<p class=\"error-msg\">Invalid spec ID.</p>".to_string()),
            )
                .into_response(),
        )
    })
}

/// Helper to collect cards sorted by lane and order for template rendering.
fn cards_by_lane(spec_state: &SpecState) -> Vec<LaneData> {
    let default_lanes = ["Ideas", "Plan", "Spec"];
    let mut lanes: Vec<LaneData> = Vec::new();

    // Default lanes first
    for lane_name in &default_lanes {
        let mut cards: Vec<CardData> = spec_state
            .cards
            .values()
            .filter(|c| c.lane == *lane_name)
            .map(CardData::from_card)
            .collect();
        cards.sort_by(|a, b| {
            a.order
                .partial_cmp(&b.order)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        lanes.push(LaneData {
            name: lane_name.to_string(),
            cards,
        });
    }

    // Any extra lanes with cards, alphabetically
    let extra_lane_names: Vec<String> = spec_state
        .cards
        .values()
        .map(|c| c.lane.clone())
        .filter(|l| !default_lanes.contains(&l.as_str()))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    for lane_name in extra_lane_names {
        let mut cards: Vec<CardData> = spec_state
            .cards
            .values()
            .filter(|c| c.lane == lane_name)
            .map(CardData::from_card)
            .collect();
        cards.sort_by(|a, b| {
            a.order
                .partial_cmp(&b.order)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        lanes.push(LaneData {
            name: lane_name,
            cards,
        });
    }

    lanes
}

/// Serializable card data for templates.
pub struct CardData {
    pub card_id: String,
    pub card_type: String,
    pub title: String,
    pub body: Option<String>,
    /// Pre-rendered markdown→HTML body for template use with `|safe`.
    pub body_html: Option<String>,
    pub lane: String,
    pub order: f64,
    pub created_by: String,
    pub updated_at: String,
}

impl CardData {
    fn from_card(card: &barnstormer_core::Card) -> Self {
        let body_html = card.body.as_ref().map(|b| render_markdown(b));
        Self {
            card_id: card.card_id.to_string(),
            card_type: card.card_type.clone(),
            title: card.title.clone(),
            body: card.body.clone(),
            body_html,
            lane: card.lane.clone(),
            order: card.order,
            created_by: card.created_by.clone(),
            updated_at: card.updated_at.format("%H:%M:%S").to_string(),
        }
    }
}

/// Lane data for templates: lane name and its sorted cards.
pub struct LaneData {
    pub name: String,
    pub cards: Vec<CardData>,
}

/// Full spec view: header + tab bar + board.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/spec_view.html")]
pub struct SpecViewTemplate {
    pub spec_id: String,
    pub title: String,
    pub one_liner: String,
    pub goal: String,
    pub phase: String,
    pub lanes: Vec<LaneData>,
    pub canvas_content: Option<String>,
    pub has_pending_question: bool,
}

impl SpecViewTemplate {
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

/// Full-page spec view for direct navigation / page reload (non-HTMX requests).
#[derive(Template, AskamaIntoResponse)]
#[template(path = "spec_page.html")]
pub struct SpecPageTemplate {
    pub spec_id: String,
    pub title: String,
    pub one_liner: String,
    pub goal: String,
    pub phase: String,
    pub lanes: Vec<LaneData>,
    pub canvas_content: Option<String>,
    pub has_pending_question: bool,
}

impl SpecPageTemplate {
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

/// GET /web/specs/{id} - Render the spec compositor (command bar + canvas + chat rail).
/// For HTMX requests returns the partial; for full page loads returns the complete shell.
pub async fn spec_view(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let is_htmx = headers.get("HX-Request").is_some();

    let spec_id = match parse_spec_id(&id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let spec_state = handle.read_state().await;
    let core = match &spec_state.core {
        Some(c) => c,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec has no core data.</p>".to_string()),
            )
                .into_response();
        }
    };

    let lanes = cards_by_lane(&spec_state);
    let phase = match spec_state.phase {
        SpecPhase::Brainstorming => "brainstorming".to_string(),
        SpecPhase::Refining => "refining".to_string(),
        SpecPhase::Complete => "complete".to_string(),
    };

    let has_pending_question = spec_state.pending_question.is_some();
    let canvas_content = spec_state.canvas_content.clone();

    if is_htmx {
        SpecViewTemplate {
            spec_id: id,
            title: core.title.clone(),
            one_liner: core.one_liner.clone(),
            goal: core.goal.clone(),
            phase,
            lanes,
            canvas_content,
            has_pending_question,
        }
        .into_response()
    } else {
        SpecPageTemplate {
            spec_id: id,
            title: core.title.clone(),
            one_liner: core.one_liner.clone(),
            goal: core.goal.clone(),
            phase,
            lanes,
            canvas_content,
            has_pending_question,
        }
        .into_response()
    }
}

/// Board partial template.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/board.html")]
pub struct BoardTemplate {
    pub spec_id: String,
    pub lanes: Vec<LaneData>,
}

/// GET /web/specs/{id}/board - Render the board partial.
pub async fn board(State(state): State<SharedState>, Path(id): Path<String>) -> impl IntoResponse {
    let spec_id = match parse_spec_id(&id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let spec_state = handle.read_state().await;
    let lanes = cards_by_lane(&spec_state);

    BoardTemplate { spec_id: id, lanes }.into_response()
}

/// Card edit form template.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/card_form.html")]
pub struct CardFormTemplate {
    pub spec_id: String,
    pub card_id: Option<String>,
    pub title: String,
    pub card_type: String,
    pub body: String,
    pub lane: String,
}

/// GET /web/specs/{id}/cards/new - Render the create card form.
pub async fn create_card_form(Path(id): Path<String>) -> CardFormTemplate {
    CardFormTemplate {
        spec_id: id,
        card_id: None,
        title: String::new(),
        card_type: "idea".to_string(),
        body: String::new(),
        lane: "Ideas".to_string(),
    }
}

/// GET /web/specs/{id}/cards/{card_id}/edit - Render the edit card form.
pub async fn edit_card_form(
    State(state): State<SharedState>,
    Path((id, card_id_str)): Path<(String, String)>,
) -> impl IntoResponse {
    let spec_id = match parse_spec_id(&id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    let card_id = match card_id_str.parse::<Ulid>() {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Html("<p class=\"error-msg\">Invalid card ID.</p>".to_string()),
            )
                .into_response();
        }
    };

    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let spec_state = handle.read_state().await;
    let card = match spec_state.cards.get(&card_id) {
        Some(c) => c,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Card not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    CardFormTemplate {
        spec_id: id,
        card_id: Some(card_id_str),
        title: card.title.clone(),
        card_type: card.card_type.clone(),
        body: card.body.clone().unwrap_or_default(),
        lane: card.lane.clone(),
    }
    .into_response()
}

/// Form data for creating/updating a card.
#[derive(Deserialize)]
pub struct CardForm {
    pub title: String,
    pub card_type: String,
    pub body: Option<String>,
    pub lane: Option<String>,
}

/// POST /web/specs/{id}/cards - Create a card, return updated board.
pub async fn create_card(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Form(form): Form<CardForm>,
) -> impl IntoResponse {
    let spec_id = match parse_spec_id(&id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let cmd = Command::CreateCard {
        card_type: form.card_type,
        title: form.title,
        body: form.body.filter(|b| !b.is_empty()),
        lane: form.lane.filter(|l| !l.is_empty()),
        created_by: "human".to_string(),
    };

    let _events = match handle.send_command(cmd).await {
        Ok(events) => events,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Html(format!(
                    "<p class=\"error-msg\">Failed to create card: {}</p>",
                    e
                )),
            )
                .into_response();
        }
    };

    // Events are persisted by the background broadcast subscriber
    // (spawned via spawn_event_persister when the actor was created).

    // Return refreshed board
    let spec_state = handle.read_state().await;
    let lanes = cards_by_lane(&spec_state);
    BoardTemplate { spec_id: id, lanes }.into_response()
}

/// PUT /web/specs/{id}/cards/{card_id} - Update a card, return the updated card HTML.
pub async fn update_card(
    State(state): State<SharedState>,
    Path((id, card_id_str)): Path<(String, String)>,
    Form(form): Form<CardForm>,
) -> impl IntoResponse {
    let spec_id = match parse_spec_id(&id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    let card_id = match card_id_str.parse::<Ulid>() {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Html("<p class=\"error-msg\">Invalid card ID.</p>".to_string()),
            )
                .into_response();
        }
    };

    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let cmd = Command::UpdateCard {
        card_id,
        title: Some(form.title),
        body: Some(form.body.filter(|b| !b.is_empty())),
        card_type: Some(form.card_type),
        refs: None,
        updated_by: "human".to_string(),
    };

    let _events = match handle.send_command(cmd).await {
        Ok(events) => events,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Html(format!(
                    "<p class=\"error-msg\">Failed to update card: {}</p>",
                    e
                )),
            )
                .into_response();
        }
    };

    // Events are persisted by the background broadcast subscriber.

    // Return the updated card HTML
    let spec_state = handle.read_state().await;
    match spec_state.cards.get(&card_id) {
        Some(card) => {
            let card_data = CardData::from_card(card);
            CardTemplate {
                spec_id: id,
                card: card_data,
            }
            .into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Html("<p class=\"error-msg\">Card not found after update.</p>".to_string()),
        )
            .into_response(),
    }
}

/// Card partial template (single card).
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/card.html")]
pub struct CardTemplate {
    pub spec_id: String,
    pub card: CardData,
}

/// DELETE /web/specs/{id}/cards/{card_id} - Delete a card, return empty.
pub async fn delete_card(
    State(state): State<SharedState>,
    Path((id, card_id_str)): Path<(String, String)>,
) -> impl IntoResponse {
    let spec_id = match parse_spec_id(&id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    let card_id = match card_id_str.parse::<Ulid>() {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Html("<p class=\"error-msg\">Invalid card ID.</p>".to_string()),
            )
                .into_response();
        }
    };

    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let cmd = Command::DeleteCard {
        card_id,
        updated_by: "human".to_string(),
    };

    let _events = match handle.send_command(cmd).await {
        Ok(events) => events,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Html(format!(
                    "<p class=\"error-msg\">Failed to delete card: {}</p>",
                    e
                )),
            )
                .into_response();
        }
    };

    // Events are persisted by the background broadcast subscriber.

    // Return empty content so HTMX removes the card element
    Html(String::new()).into_response()
}

/// Document view template.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/document.html")]
pub struct DocumentTemplate {
    pub spec_id: String,
    pub title: String,
    pub one_liner: String,
    pub goal: String,
    pub goal_html: String,
    pub description: Option<String>,
    pub description_html: Option<String>,
    pub constraints: Option<String>,
    pub constraints_html: Option<String>,
    pub success_criteria: Option<String>,
    pub success_criteria_html: Option<String>,
    pub risks: Option<String>,
    pub risks_html: Option<String>,
    pub notes: Option<String>,
    pub notes_html: Option<String>,
    pub lanes: Vec<LaneData>,
}

/// GET /web/specs/{id}/document - Render the spec as a narrative document.
pub async fn document(
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
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let spec_state = handle.read_state().await;
    let core = match &spec_state.core {
        Some(c) => c,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec has no core data.</p>".to_string()),
            )
                .into_response();
        }
    };

    let lanes = cards_by_lane(&spec_state);

    DocumentTemplate {
        spec_id: id,
        title: core.title.clone(),
        one_liner: core.one_liner.clone(),
        goal: core.goal.clone(),
        goal_html: render_markdown(&core.goal),
        description: core.description.clone(),
        description_html: core.description.as_ref().map(|d| render_markdown(d)),
        constraints: core.constraints.clone(),
        constraints_html: core.constraints.as_ref().map(|c| render_markdown(c)),
        success_criteria: core.success_criteria.clone(),
        success_criteria_html: core.success_criteria.as_ref().map(|s| render_markdown(s)),
        risks: core.risks.clone(),
        risks_html: core.risks.as_ref().map(|r| render_markdown(r)),
        notes: core.notes.clone(),
        notes_html: core.notes.as_ref().map(|n| render_markdown(n)),
        lanes,
    }
    .into_response()
}

/// Activity transcript data for templates.
pub struct TranscriptEntry {
    pub sender: String,
    pub sender_label: String,
    pub initial: String,
    pub is_human: bool,
    pub is_step: bool,
    pub is_continuation: bool,
    pub role_class: String,
    pub content: String,
    /// Pre-rendered markdown→HTML for template use with `|safe`.
    pub content_html: String,
    pub timestamp: String,
    /// Number of consecutive identical step messages collapsed into this one.
    pub repeat_count: u32,
}

/// Render markdown content to HTML, stripping raw HTML tags from input
/// to prevent XSS. Handles paragraphs, bold, italic, lists, code blocks,
/// and links.
fn render_markdown(content: &str) -> String {
    let options = Options::empty();
    let parser = Parser::new_ext(content, options).filter(|event| {
        !matches!(event, Event::Html(_) | Event::InlineHtml(_))
    });
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    html_output
}

/// Convert a TranscriptMessage to a TranscriptEntry for template rendering.
fn to_transcript_entry(m: &barnstormer_core::TranscriptMessage) -> TranscriptEntry {
    let (sender_label, is_human, role_class) = sender_display(&m.sender);
    let initial = sender_label.chars().next().unwrap_or('?').to_string();
    let content_html = render_markdown(&m.content);
    TranscriptEntry {
        sender: m.sender.clone(),
        sender_label,
        initial,
        is_human,
        is_step: m.kind.is_step(),
        is_continuation: false,
        role_class,
        content: m.content.clone(),
        content_html,
        timestamp: m.timestamp.format("%H:%M:%S").to_string(),
        repeat_count: 1,
    }
}

/// Mark consecutive entries from the same sender as continuations.
/// The first entry in a run keeps `is_continuation = false`; subsequent
/// entries from the same sender get `is_continuation = true` so the
/// template can skip the avatar/name row.
fn mark_continuations(entries: &mut [TranscriptEntry]) {
    for i in 1..entries.len() {
        if entries[i].sender == entries[i - 1].sender && !entries[i].is_step && !entries[i - 1].is_step {
            entries[i].is_continuation = true;
        }
    }
}

/// Collapse consecutive identical step messages into a single entry with
/// a repeat_count, so the UI can show "(x3)" instead of three identical lines.
fn collapse_repeated_steps(entries: &mut Vec<TranscriptEntry>) {
    let mut i = 0;
    while i < entries.len() {
        if entries[i].is_step {
            let mut j = i + 1;
            while j < entries.len() && entries[j].is_step && entries[j].content == entries[i].content {
                entries[i].repeat_count += 1;
                j += 1;
            }
            if entries[i].repeat_count > 1 {
                entries.drain((i + 1)..j);
            }
        }
        i += 1;
    }
}

/// Returns true if the sender is part of the human ↔ manager conversation.
/// Used to filter the chat tab to only show direct messages between the
/// human and the manager agent, keeping other agents in the activity feed.
fn is_chat_participant(sender: &str) -> bool {
    sender == "human" || sender.starts_with("manager-")
}

/// Derive a display label and CSS class from a raw sender ID.
/// "human" → ("You", true, "human"), "manager-01J..." → ("Manager", false, "manager"), etc.
fn sender_display(sender: &str) -> (String, bool, String) {
    if sender == "human" {
        return ("You".to_string(), true, "human".to_string());
    }
    // Agent IDs look like "manager-01JTEST..." or "brainstormer-01JTEST..."
    let role = sender.split('-').next().unwrap_or(sender);
    let label = match role {
        "manager" => "Orchestrator",
        "brainstormer" => "Researcher",
        "planner" => "Architect",
        "dot_generator" => "Dot Generator",
        "critic" => "Critic",
        _ => {
            let mut capitalized = String::new();
            for (i, ch) in role.chars().enumerate() {
                if i == 0 {
                    capitalized.extend(ch.to_uppercase());
                } else {
                    capitalized.push(ch);
                }
            }
            return (capitalized, false, normalize_css_class(role));
        }
    };
    let role_class = normalize_css_class(role);
    (label.to_string(), false, role_class)
}

/// Normalize a string into a valid CSS class name: lowercase, replacing
/// any character that is not `[a-z0-9_-]` with a hyphen.
fn normalize_css_class(raw: &str) -> String {
    raw.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' { c } else { '-' })
        .collect()
}

/// Question data for templates.
pub enum QuestionData {
    Boolean {
        question_id: String,
        question: String,
        default: Option<bool>,
    },
    MultipleChoice {
        question_id: String,
        question: String,
        choices: Vec<String>,
        allow_multi: bool,
    },
    Freeform {
        question_id: String,
        question: String,
        placeholder: String,
    },
}

/// Convert a core UserQuestion into the template-friendly QuestionData.
fn question_to_view_data(q: &barnstormer_core::UserQuestion) -> QuestionData {
    match q {
        barnstormer_core::UserQuestion::Boolean {
            question_id,
            question,
            default,
        } => QuestionData::Boolean {
            question_id: question_id.to_string(),
            question: render_markdown(question),
            default: *default,
        },
        barnstormer_core::UserQuestion::MultipleChoice {
            question_id,
            question,
            choices,
            allow_multi,
        } => QuestionData::MultipleChoice {
            question_id: question_id.to_string(),
            question: render_markdown(question),
            choices: choices.clone(),
            allow_multi: *allow_multi,
        },
        barnstormer_core::UserQuestion::Freeform {
            question_id,
            question,
            placeholder,
            ..
        } => QuestionData::Freeform {
            question_id: question_id.to_string(),
            question: render_markdown(question),
            placeholder: placeholder.clone().unwrap_or_default(),
        },
    }
}

/// Query parameters for the transcript endpoint, allowing callers to specify
/// which container the response should target (activity panel vs chat tab).
/// The optional `part` field selects a sub-section: "feed" for messages only,
/// "question" for the question card only, or omitted for the full transcript.
#[derive(Deserialize)]
pub struct TranscriptQuery {
    pub container_id: Option<String>,
    pub part: Option<String>,
}

/// Validate and sanitize a container_id value. Only known IDs are accepted;
/// anything else falls back to "chat-transcript" to prevent XSS via
/// user-controlled values rendered into script tags and HTMX attributes.
///
/// Allowed IDs and where they are used:
/// - "activity-transcript" -- activity panel transcript (default for activity handlers)
/// - "chat-transcript"     -- chat panel transcript in refining phase
/// - "brainstorm-chat"     -- chat panel transcript in brainstorming phase
/// - "mission-ticker"      -- compact ticker strip; also the hx-target for answer forms
fn sanitize_container_id(raw: &str) -> String {
    match raw {
        "activity-transcript" | "chat-transcript" | "mission-ticker" | "brainstorm-chat" => {
            raw.to_string()
        }
        _ => "chat-transcript".to_string(),
    }
}

/// Activity panel template.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/activity.html")]
pub struct ActivityTemplate {
    pub spec_id: String,
    pub container_id: String,
    pub transcript: Vec<TranscriptEntry>,
    pub pending_question: Option<QuestionData>,
}

/// Activity transcript partial template (transcript entries + question widget only).
/// Used by the SSE refresh target so that chat input is not wiped on updates.
/// The `container_id` field controls the DOM IDs so the same template can serve
/// both the mission ticker and the full-width chat tab.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/activity_transcript.html")]
pub struct ActivityTranscriptTemplate {
    pub spec_id: String,
    pub container_id: String,
    pub transcript: Vec<TranscriptEntry>,
    pub pending_question: Option<QuestionData>,
}

/// GET /web/specs/{id}/activity - Render the activity panel.
pub async fn activity(
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
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let spec_state = handle.read_state().await;

    let mut transcript: Vec<TranscriptEntry> = spec_state
        .transcript
        .iter()
        .map(to_transcript_entry)
        .collect();
    mark_continuations(&mut transcript);
    collapse_repeated_steps(&mut transcript);

    let pending_question = spec_state.pending_question.as_ref().map(question_to_view_data);

    ActivityTemplate {
        spec_id: id,
        container_id: "activity-transcript".to_string(),
        transcript,
        pending_question,
    }
    .into_response()
}

/// GET /web/specs/{id}/activity/transcript - Render only the transcript + question widget.
/// Used as the SSE refresh target so chat input is preserved during live updates.
pub async fn activity_transcript(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Query(query): Query<TranscriptQuery>,
) -> impl IntoResponse {
    let spec_id = match parse_spec_id(&id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let spec_state = handle.read_state().await;

    let pending_question = spec_state.pending_question.as_ref().map(question_to_view_data);

    let container_id = sanitize_container_id(
        query.container_id.as_deref().unwrap_or("activity-transcript"),
    );

    // Chat containers only show human + manager messages (filtered by
    // is_chat_participant) so the user sees a clean conversation thread.
    // The activity-transcript and mission-ticker containers show all senders.
    let is_chat = container_id == "chat-transcript" || container_id == "brainstorm-chat";

    let mut transcript: Vec<TranscriptEntry> = spec_state
        .transcript
        .iter()
        .filter(|m| !is_chat || is_chat_participant(&m.sender))
        .map(to_transcript_entry)
        .collect();
    mark_continuations(&mut transcript);
    collapse_repeated_steps(&mut transcript);

    let part = query.part.as_deref().unwrap_or("");

    if is_chat && part == "feed" {
        ChatFeedTemplate {
            spec_id: id,
            container_id,
            transcript,
        }
        .into_response()
    } else if is_chat && part == "question" {
        ChatQuestionTemplate {
            spec_id: id,
            container_id,
            pending_question,
        }
        .into_response()
    } else if is_chat {
        ChatTranscriptTemplate {
            spec_id: id,
            container_id,
            transcript,
            pending_question,
        }
        .into_response()
    } else {
        ActivityTranscriptTemplate {
            spec_id: id,
            container_id,
            transcript,
            pending_question,
        }
        .into_response()
    }
}

/// Chat-style transcript for SSE refresh in the Chat tab.
/// Uses distinct markup from ActivityTranscriptTemplate — avatars, larger bubbles.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/chat_transcript.html")]
pub struct ChatTranscriptTemplate {
    pub spec_id: String,
    pub container_id: String,
    pub transcript: Vec<TranscriptEntry>,
    pub pending_question: Option<QuestionData>,
}

/// Chat message feed partial — messages, throbber, streaming, empty state.
/// Rendered independently so transcript refreshes don't disturb the question card.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/chat_feed.html")]
pub struct ChatFeedTemplate {
    pub spec_id: String,
    pub container_id: String,
    pub transcript: Vec<TranscriptEntry>,
}

/// Chat question card partial — pending question with answer form.
/// Rendered independently so question refreshes don't disturb the message feed.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/chat_question.html")]
pub struct ChatQuestionTemplate {
    pub spec_id: String,
    pub container_id: String,
    pub pending_question: Option<QuestionData>,
}

/// Chat panel template for the full-width Chat tab.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/chat_panel.html")]
pub struct ChatPanelTemplate {
    pub spec_id: String,
    pub container_id: String,
    pub transcript: Vec<TranscriptEntry>,
    pub pending_question: Option<QuestionData>,
}

/// GET /web/specs/{id}/chat-panel - Render the Chat tab content.
pub async fn chat_panel(
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
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let spec_state = handle.read_state().await;

    let container_id = if spec_state.phase == SpecPhase::Brainstorming {
        "brainstorm-chat".to_string()
    } else {
        "chat-transcript".to_string()
    };

    let mut transcript: Vec<TranscriptEntry> = spec_state
        .transcript
        .iter()
        .filter(|m| is_chat_participant(&m.sender))
        .map(to_transcript_entry)
        .collect();
    mark_continuations(&mut transcript);
    collapse_repeated_steps(&mut transcript);

    let pending_question = spec_state.pending_question.as_ref().map(question_to_view_data);

    ChatPanelTemplate {
        spec_id: id,
        container_id,
        transcript,
        pending_question,
    }
    .into_response()
}

/// Artifacts tab template showing exported spec content in multiple formats.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/artifacts.html")]
pub struct ArtifactsTemplate {
    pub spec_id: String,
    pub markdown_content: String,
    pub yaml_content: String,
    pub dot_content: String,
}

/// GET /web/specs/{id}/artifacts - Render the Artifacts tab with all three export formats.
pub async fn artifacts(
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
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let spec_state = handle.read_state().await;

    let markdown_content = barnstormer_core::export::export_markdown(&spec_state);
    let yaml_content = barnstormer_core::export::export_yaml(&spec_state).unwrap_or_else(|e| {
        format!("# YAML export error: {}", e)
    });
    let dot_content = barnstormer_core::export::export_dot(&spec_state);

    ArtifactsTemplate {
        spec_id: id,
        markdown_content,
        yaml_content,
        dot_content,
    }
    .into_response()
}

/// Spec tab template showing a synthesized specification document.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/spec.html")]
pub struct SpecTabTemplate {
    pub spec_id: String,
    pub spec_html: String,
    pub spec_markdown: String,
}

/// GET /web/specs/{id}/spec - Render the synthesized Spec tab.
pub async fn spec(
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
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let spec_state = handle.read_state().await;
    let spec_markdown = barnstormer_core::export::export_spec(&spec_state);
    let spec_html = render_markdown(&spec_markdown);

    SpecTabTemplate {
        spec_id: id,
        spec_html,
        spec_markdown,
    }
    .into_response()
}

/// GET /web/specs/{id}/export/markdown - Download spec as Markdown file.
pub async fn export_markdown(
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
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let spec_state = handle.read_state().await;
    let content = barnstormer_core::export::export_markdown(&spec_state);

    Response::builder()
        .header("content-type", "text/markdown")
        .header("content-disposition", "attachment; filename=\"spec.md\"")
        .body(axum::body::Body::from(content))
        .unwrap()
        .into_response()
}

/// GET /web/specs/{id}/export/yaml - Download spec as YAML file.
pub async fn export_yaml(
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
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let spec_state = handle.read_state().await;
    match barnstormer_core::export::export_yaml(&spec_state) {
        Ok(content) => Response::builder()
            .header("content-type", "text/yaml")
            .header(
                "content-disposition",
                "attachment; filename=\"spec.yaml\"",
            )
            .body(axum::body::Body::from(content))
            .unwrap()
            .into_response(),
        Err(e) => {
            tracing::error!("YAML export failed for spec {}: {}", id, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html("<p class=\"error-msg\">Failed to export YAML.</p>".to_string()),
            )
                .into_response()
        }
    }
}

/// GET /web/specs/{id}/export/dot - Download spec as DOT graph file.
pub async fn export_dot(
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
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let spec_state = handle.read_state().await;
    let content = barnstormer_core::export::export_dot(&spec_state);

    Response::builder()
        .header("content-type", "text/plain")
        .header("content-disposition", "attachment; filename=\"spec.dot\"")
        .body(axum::body::Body::from(content))
        .unwrap()
        .into_response()
}

/// GET /web/specs/{id}/export/spec - Download synthesized spec as Markdown file.
pub async fn export_spec_download(
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
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let spec_state = handle.read_state().await;
    let content = barnstormer_core::export::export_spec(&spec_state);

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/markdown; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"spec.md\"",
            ),
        ],
        content,
    )
        .into_response()
}

/// POST /web/specs/{id}/regenerate - Regenerate exports and save to disk.
/// Writes markdown, YAML, and DOT files to $BARNSTORMER_HOME/<spec_id>/exports/.
/// Returns an HTML snippet confirming the export.
pub async fn regenerate(
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
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let spec_state = handle.read_state().await;

    // Export all formats
    let markdown_content = barnstormer_core::export::export_markdown(&spec_state);
    let yaml_content = barnstormer_core::export::export_yaml(&spec_state)
        .unwrap_or_else(|e| format!("# YAML export error: {}", e));
    let dot_content = barnstormer_core::export::export_dot(&spec_state);

    // Write to $BARNSTORMER_HOME/specs/<spec_id>/exports/
    let exports_dir = state
        .barnstormer_home
        .join("specs")
        .join(spec_id.to_string())
        .join("exports");
    if let Err(e) = std::fs::create_dir_all(&exports_dir) {
        tracing::error!("failed to create exports directory: {}", e);
    } else {
        let spec_title = spec_state
            .core
            .as_ref()
            .map(|c| c.title.clone())
            .unwrap_or_else(|| "spec".to_string());
        let slug: String = spec_title
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();

        if let Err(e) = std::fs::write(exports_dir.join(format!("{}.md", slug)), &markdown_content) {
            tracing::error!("failed to write markdown export: {}", e);
        }
        if let Err(e) = std::fs::write(exports_dir.join(format!("{}.yaml", slug)), &yaml_content) {
            tracing::error!("failed to write YAML export: {}", e);
        }
        if let Err(e) = std::fs::write(exports_dir.join(format!("{}.dot", slug)), &dot_content) {
            tracing::error!("failed to write DOT export: {}", e);
        }
        tracing::info!(
            "regenerated exports for spec {} at {}",
            spec_id,
            exports_dir.display()
        );
    }

    Html("<span class=\"regen-confirm\">Exports saved successfully.</span>".to_string())
        .into_response()
}

/// Form data for sending a chat message.
#[derive(Deserialize)]
pub struct ChatForm {
    pub message: String,
}

/// Form data for answering a question.
#[derive(Deserialize)]
pub struct AnswerForm {
    pub question_id: String,
    pub answer: String,
}

/// POST /web/specs/{id}/answer - Submit an answer to a pending question.
pub async fn answer_question(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Form(form): Form<AnswerForm>,
) -> impl IntoResponse {
    let spec_id = match parse_spec_id(&id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    let question_id = match form.question_id.parse::<Ulid>() {
        Ok(qid) => qid,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Html("<p class=\"error-msg\">Invalid question ID.</p>".to_string()),
            )
                .into_response();
        }
    };

    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let cmd = Command::AnswerQuestion {
        question_id,
        answer: form.answer,
    };

    let _events = match handle.send_command(cmd).await {
        Ok(events) => events,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Html(format!(
                    "<p class=\"error-msg\">Failed to answer: {}</p>",
                    e
                )),
            )
                .into_response();
        }
    };

    // Events are persisted by the background broadcast subscriber.
    // Drop actors lock before acquiring swarms to avoid deadlock.
    drop(actors);

    // Wake the agent loop so agents resume promptly after an answer.
    {
        let swarms = state.swarms.read().await;
        if let Some(swarm_handle) = swarms.get(&spec_id) {
            let swarm = swarm_handle.swarm.lock().await;
            swarm.notify_human_message();
        }
    }

    // Re-acquire actors to read transcript for response
    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    // Determine container_id from HX-Target header so the response replaces
    // the correct transcript container (activity panel vs chat tab).
    // If the target ends with "-question", we return only the question card
    // partial so the message feed is untouched.
    let raw_target = headers
        .get("HX-Target")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches('#'))
        .unwrap_or("activity-transcript");

    let is_question_target = raw_target.ends_with("-question");
    let base_target = if is_question_target {
        raw_target.trim_end_matches("-question")
    } else {
        raw_target
    };
    let container_id = sanitize_container_id(base_target);

    // Return refreshed transcript partial
    let spec_state = handle.read_state().await;

    // Chat containers only show human + manager messages; see sanitize_container_id docs.
    let is_chat = container_id == "chat-transcript" || container_id == "brainstorm-chat";
    let is_ticker = container_id == "mission-ticker";

    // Read actual pending question from state instead of assuming None
    let pending_question = spec_state.pending_question.as_ref().map(question_to_view_data);

    // If the answer form targeted the question card directly, return only
    // the question partial so the message feed and any user input are preserved.
    if is_question_target && is_chat {
        return ChatQuestionTemplate {
            spec_id: id,
            container_id,
            pending_question,
        }
        .into_response();
    }

    let mut transcript: Vec<TranscriptEntry> = spec_state
        .transcript
        .iter()
        .filter(|m| !is_chat || is_chat_participant(&m.sender))
        .map(to_transcript_entry)
        .collect();
    mark_continuations(&mut transcript);
    collapse_repeated_steps(&mut transcript);

    if is_ticker {
        // For mission ticker, show only last 10 entries
        let ticker_entries: Vec<TranscriptEntry> = spec_state
            .transcript
            .iter()
            .rev()
            .take(10)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(to_transcript_entry)
            .collect();
        MissionTickerTemplate {
            spec_id: id,
            ticker_entries,
            pending_question,
        }
        .into_response()
    } else if is_chat {
        ChatTranscriptTemplate {
            spec_id: id,
            container_id,
            transcript,
            pending_question,
        }
        .into_response()
    } else {
        ActivityTranscriptTemplate {
            spec_id: id,
            container_id,
            transcript,
            pending_question,
        }
        .into_response()
    }
}

/// Maximum allowed length for a chat message (in characters).
const CHAT_MAX_LENGTH: usize = 10_000;

/// POST /web/specs/{id}/chat - Send a free-text message as the human.
pub async fn chat(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Form(form): Form<ChatForm>,
) -> impl IntoResponse {
    let spec_id = match parse_spec_id(&id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    // Validate message: trim whitespace, reject empty, cap length
    let message = form.message.trim().to_string();
    if message.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Html("<p class=\"error-msg\">Message cannot be empty.</p>".to_string()),
        )
            .into_response();
    }
    if message.chars().count() > CHAT_MAX_LENGTH {
        return (
            StatusCode::BAD_REQUEST,
            Html(format!(
                "<p class=\"error-msg\">Message too long (max {} characters).</p>",
                CHAT_MAX_LENGTH
            )),
        )
            .into_response();
    }

    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let cmd = Command::AppendTranscript {
        sender: "human".to_string(),
        content: message,
    };

    let _events = match handle.send_command(cmd).await {
        Ok(events) => events,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Html(format!(
                    "<p class=\"error-msg\">Failed to send message: {}</p>",
                    e
                )),
            )
                .into_response();
        }
    };
    // Drop actors read lock before acquiring swarms
    drop(actors);

    // Wake the agent loop so the manager responds to the human message promptly
    // instead of waiting for the next idle-cycle poll (up to 5 seconds).
    {
        let swarms = state.swarms.read().await;
        if let Some(swarm_handle) = swarms.get(&spec_id) {
            let swarm = swarm_handle.swarm.lock().await;
            swarm.notify_human_message();
        }
    }

    // Re-acquire actors to read transcript for response
    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    // Events are persisted by the background broadcast subscriber.

    // Determine container_id from HX-Target header so the response replaces
    // the correct transcript container (activity panel vs chat tab).
    let container_id = sanitize_container_id(
        headers
            .get("HX-Target")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim_start_matches('#'))
            .unwrap_or("activity-transcript"),
    );

    // Return refreshed transcript partial
    let spec_state = handle.read_state().await;

    // Chat containers only show human + manager messages; see sanitize_container_id docs.
    let is_chat = container_id == "chat-transcript" || container_id == "brainstorm-chat";
    let is_ticker = container_id == "mission-ticker";

    let mut transcript: Vec<TranscriptEntry> = spec_state
        .transcript
        .iter()
        .filter(|m| !is_chat || is_chat_participant(&m.sender))
        .map(to_transcript_entry)
        .collect();
    mark_continuations(&mut transcript);
    collapse_repeated_steps(&mut transcript);

    let pending_question = spec_state.pending_question.as_ref().map(question_to_view_data);

    if is_ticker {
        // For mission ticker, show only last 10 entries
        let ticker_entries: Vec<TranscriptEntry> = spec_state
            .transcript
            .iter()
            .rev()
            .take(10)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(to_transcript_entry)
            .collect();
        MissionTickerTemplate {
            spec_id: id,
            ticker_entries,
            pending_question,
        }
        .into_response()
    } else if is_chat {
        ChatTranscriptTemplate {
            spec_id: id,
            container_id,
            transcript,
            pending_question,
        }
        .into_response()
    } else {
        ActivityTranscriptTemplate {
            spec_id: id,
            container_id,
            transcript,
            pending_question,
        }
        .into_response()
    }
}

/// POST /web/specs/{id}/undo - Undo last operation, return refreshed board.
pub async fn undo(State(state): State<SharedState>, Path(id): Path<String>) -> impl IntoResponse {
    let spec_id = match parse_spec_id(&id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let _events = match handle.send_command(Command::Undo).await {
        Ok(events) => events,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Html(format!("<p class=\"error-msg\">Undo failed: {}</p>", e)),
            )
                .into_response();
        }
    };

    // Events are persisted by the background broadcast subscriber.

    // Return refreshed board
    let spec_state = handle.read_state().await;
    let lanes = cards_by_lane(&spec_state);
    BoardTemplate { spec_id: id, lanes }.into_response()
}

#[derive(Deserialize)]
pub struct PhaseForm {
    target: String,
}

/// POST /web/specs/{id}/phase - Transition a spec between phases.
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
        "refining" => SpecPhase::Refining,
        "complete" => SpecPhase::Complete,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Html("<p class=\"error-msg\">Invalid phase target.</p>".to_string()),
            )
                .into_response();
        }
    };

    let actors = state.actors.read().await;
    let Some(handle) = actors.get(&spec_id) else {
        return (
            StatusCode::NOT_FOUND,
            Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
        )
            .into_response();
    };

    match handle
        .send_command(Command::TransitionPhase {
            target: target.clone(),
        })
        .await
    {
        Ok(_) => {
            // Phase transition triggers SSE phase_transitioned event,
            // which causes the client to reload the entire workspace.
            (StatusCode::OK, Html("<span>OK</span>".to_string())).into_response()
        }
        Err(ActorError::AlreadyInPhase) => (
            StatusCode::CONFLICT,
            Html("<p class=\"error-msg\">Already in target phase.</p>".to_string()),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html(format!("<p class=\"error-msg\">Error: {}</p>", e)),
        )
            .into_response(),
    }
}

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
        SpecPhase::Brainstorming => "brainstorming",
        SpecPhase::Refining => "refining",
        SpecPhase::Complete => "complete",
    };
    phase_str.into_response()
}

/// Provider status partial template.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/provider_status.html")]
pub struct ProviderStatusTemplate {
    pub default_provider: String,
    pub default_model: Option<String>,
    pub providers: Vec<ProviderInfoView>,
    pub any_available: bool,
}

/// Provider info view for template rendering.
pub struct ProviderInfoView {
    pub name: String,
    pub has_api_key: bool,
    pub model: String,
}

/// GET /web/provider-status - Provider status partial.
pub async fn provider_status(State(state): State<SharedState>) -> ProviderStatusTemplate {
    let ps = &state.provider_status;
    ProviderStatusTemplate {
        default_provider: ps.default_provider.clone(),
        default_model: ps.default_model.clone(),
        providers: ps
            .providers
            .iter()
            .map(|p| ProviderInfoView {
                name: p.name.clone(),
                has_api_key: p.has_api_key,
                model: p.model.clone(),
            })
            .collect(),
        any_available: ps.any_available,
    }
}

/// Mission ticker template — compact activity list for the mission strip.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/mission_ticker.html")]
pub struct MissionTickerTemplate {
    pub spec_id: String,
    pub ticker_entries: Vec<TranscriptEntry>,
    pub pending_question: Option<QuestionData>,
}

/// Agent LED indicators template for the command bar.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/agent_leds.html")]
pub struct AgentLedsTemplate {
    pub spec_id: String,
    pub running: bool,
    pub started: bool,
}

/// Agent status partial template.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/agent_status.html")]
pub struct AgentStatusTemplate {
    pub spec_id: String,
    pub running: bool,
    pub started: bool,
    pub agent_count: usize,
}

/// GET /web/specs/{id}/ticker - Render the mission strip ticker content.
pub async fn ticker(
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
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let spec_state = handle.read_state().await;

    // Show last 10 transcript entries
    let ticker_entries: Vec<TranscriptEntry> = spec_state
        .transcript
        .iter()
        .rev()
        .take(10)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(to_transcript_entry)
        .collect();

    let pending_question = spec_state.pending_question.as_ref().map(question_to_view_data);

    MissionTickerTemplate {
        spec_id: id,
        ticker_entries,
        pending_question,
    }
    .into_response()
}

/// GET /web/specs/{id}/agents/leds - Render agent LED indicators.
pub async fn agent_leds(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let spec_id = match parse_spec_id(&id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    let swarms = state.swarms.read().await;
    match swarms.get(&spec_id) {
        Some(swarm_handle) => {
            let swarm = swarm_handle.swarm.lock().await;
            AgentLedsTemplate {
                spec_id: id,
                running: !swarm.is_paused(),
                started: true,
            }
            .into_response()
        }
        None => AgentLedsTemplate {
            spec_id: id,
            running: false,
            started: false,
        }
        .into_response(),
    }
}

/// POST /web/specs/{id}/agents/start - Start agents for a spec.
pub async fn start_agents(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let spec_id = match parse_spec_id(&id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    // Get actor handle first (read lock), then drop before acquiring swarms write lock
    let actors = state.actors.read().await;
    let actor_handle = match actors.get(&spec_id) {
        Some(h) => h,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    // Clone the existing actor handle so the swarm uses the same actor,
    // ensuring events flow through the server's main event bus.
    let swarm_actor_handle = actor_handle.clone();
    drop(actors);

    // Atomic check-and-insert: hold write lock to prevent TOCTOU race
    // where two concurrent requests both pass the existence check and
    // create duplicate swarms.
    let mut swarms = state.swarms.write().await;
    if let Some(swarm_handle) = swarms.get(&spec_id) {
        let swarm = swarm_handle.swarm.lock().await;
        return AgentStatusTemplate {
            spec_id: id,
            running: !swarm.is_paused(),
            started: true,
            agent_count: swarm.agent_count(),
        }
        .into_response();
    }

    // Create swarm (sync operation, safe to hold write lock)
    let swarm = match SwarmOrchestrator::with_defaults(spec_id, swarm_actor_handle) {
        Ok(s) => Arc::new(tokio::sync::Mutex::new(s)),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(format!(
                    "<p class=\"error-msg\">Failed to start agents: {}</p>",
                    e
                )),
            )
                .into_response();
        }
    };

    let agent_count = {
        // This lock is uncontested since the swarm was just created
        let s = swarm.lock().await;
        s.agent_count()
    };

    // Spawn agent loop task and store the handle for cancellation.
    // The loop lives in the agent crate; each agent gets its own
    // broadcast receiver so events are never lost.
    let task = tokio::spawn(barnstormer_agent::run_loop(Arc::clone(&swarm)));

    // Insert into swarms map while still holding write lock
    swarms.insert(
        spec_id,
        crate::app_state::SwarmHandle { swarm, task },
    );
    drop(swarms);

    AgentStatusTemplate {
        spec_id: id,
        running: true,
        started: true,
        agent_count,
    }
    .into_response()
}

/// POST /web/specs/{id}/agents/pause - Pause agents.
pub async fn pause_agents(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let spec_id = match parse_spec_id(&id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    let swarms = state.swarms.read().await;
    match swarms.get(&spec_id) {
        Some(swarm_handle) => {
            let swarm = swarm_handle.swarm.lock().await;
            swarm.pause();
            AgentStatusTemplate {
                spec_id: id,
                running: false,
                started: true,
                agent_count: swarm.agent_count(),
            }
            .into_response()
        }
        None => AgentStatusTemplate {
            spec_id: id,
            running: false,
            started: false,
            agent_count: 0,
        }
        .into_response(),
    }
}

/// POST /web/specs/{id}/agents/resume - Resume agents.
pub async fn resume_agents(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let spec_id = match parse_spec_id(&id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    let swarms = state.swarms.read().await;
    match swarms.get(&spec_id) {
        Some(swarm_handle) => {
            let swarm = swarm_handle.swarm.lock().await;
            swarm.resume();
            AgentStatusTemplate {
                spec_id: id,
                running: true,
                started: true,
                agent_count: swarm.agent_count(),
            }
            .into_response()
        }
        None => AgentStatusTemplate {
            spec_id: id,
            running: false,
            started: false,
            agent_count: 0,
        }
        .into_response(),
    }
}

/// GET /web/specs/{id}/agents/status - Get current agent status.
pub async fn agent_status(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let spec_id = match parse_spec_id(&id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    let swarms = state.swarms.read().await;
    match swarms.get(&spec_id) {
        Some(swarm_handle) => {
            let swarm = swarm_handle.swarm.lock().await;
            AgentStatusTemplate {
                spec_id: id,
                running: !swarm.is_paused(),
                started: true,
                agent_count: swarm.agent_count(),
            }
            .into_response()
        }
        None => AgentStatusTemplate {
            spec_id: id,
            running: false,
            started: false,
            agent_count: 0,
        }
        .into_response(),
    }
}

/// Helper to start the agent swarm for a spec, if a provider is available.
/// Returns silently if no provider is configured, if the swarm already exists,
/// or if swarm creation fails. Used by both web and API create_spec handlers.
pub async fn try_start_agents(state: &SharedState, spec_id: Ulid, actor_handle: &barnstormer_core::SpecActorHandle) {
    if !state.provider_status.any_available {
        tracing::info!("no LLM provider configured, skipping agent start for spec {}", spec_id);
        return;
    }

    // Clone the existing actor handle so the swarm uses the same actor,
    // ensuring events flow through the server's main event bus.
    let swarm_actor_handle = actor_handle.clone();

    // Atomic check-and-insert: hold write lock to prevent TOCTOU race
    // where two concurrent requests both pass the existence check and
    // create duplicate swarms.
    let mut swarms = state.swarms.write().await;
    if swarms.contains_key(&spec_id) {
        return;
    }

    // Create swarm (sync operation, safe to hold write lock)
    let swarm = match SwarmOrchestrator::with_defaults(spec_id, swarm_actor_handle) {
        Ok(s) => Arc::new(tokio::sync::Mutex::new(s)),
        Err(e) => {
            tracing::warn!("failed to auto-start agents for spec {}: {}", spec_id, e);
            return;
        }
    };

    let agent_count = {
        // This lock is uncontested since the swarm was just created
        let s = swarm.lock().await;
        s.agent_count()
    };

    // Spawn background agent loop and store the handle for cancellation.
    // The loop lives in the agent crate; each agent gets its own
    // broadcast receiver so events are never lost.
    let task = tokio::spawn(barnstormer_agent::run_loop(Arc::clone(&swarm)));

    // Insert into swarms map while still holding write lock
    swarms.insert(
        spec_id,
        crate::app_state::SwarmHandle { swarm, task },
    );
    drop(swarms);
    tracing::info!("auto-started {} agents for spec {}", agent_count, spec_id);
}

/// Spawn a background task that subscribes to an actor's broadcast channel
/// and persists every event to JSONL. This catches ALL events including
/// those produced by agents, which bypass the inline `persist_events` path.
///
/// On broadcast lag (missed events), saves a state snapshot so crash recovery
/// can restore from the snapshot rather than relying on a gapped JSONL log.
///
/// Returns the JoinHandle so the caller can store it for cleanup.
pub fn spawn_event_persister(
    actor: &barnstormer_core::SpecActorHandle,
    spec_id: Ulid,
    barnstormer_home: &std::path::Path,
) -> tokio::task::JoinHandle<()> {
    let mut rx = actor.subscribe();
    let actor_handle = actor.clone();
    let log_path = barnstormer_home
        .join("specs")
        .join(spec_id.to_string())
        .join("events.jsonl");
    let snapshot_dir = barnstormer_home
        .join("specs")
        .join(spec_id.to_string())
        .join("snapshots");

    tokio::spawn(async move {
        // Retry opening the JSONL log a few times before giving up, in case
        // the directory or filesystem is temporarily unavailable at startup.
        const MAX_OPEN_RETRIES: u32 = 5;
        const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

        let mut log = None;
        for attempt in 1..=MAX_OPEN_RETRIES {
            match JsonlLog::open(&log_path) {
                Ok(l) => {
                    log = Some(l);
                    break;
                }
                Err(e) => {
                    tracing::error!(
                        "event persister failed to open log for spec {} at {} (attempt {}/{}): {}",
                        spec_id,
                        log_path.display(),
                        attempt,
                        MAX_OPEN_RETRIES,
                        e
                    );
                    if attempt < MAX_OPEN_RETRIES {
                        tokio::time::sleep(RETRY_DELAY).await;
                    }
                }
            }
        }
        let Some(mut log) = log else {
            tracing::error!(
                "event persister giving up on spec {} after {} retries",
                spec_id,
                MAX_OPEN_RETRIES,
            );
            return;
        };

        loop {
            match rx.recv().await {
                Ok(event) => {
                    if event.payload.is_ephemeral() {
                        continue;
                    }
                    if let Err(e) = log.append(&event) {
                        tracing::error!(
                            "event persister failed to write event for spec {}: {}",
                            spec_id,
                            e
                        );
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(
                        "event persister for spec {} lagged, missed {} events — saving snapshot",
                        spec_id,
                        n
                    );
                    // Save a snapshot so crash recovery can restore from it
                    // rather than relying on the gapped JSONL log.
                    let state = actor_handle.read_state().await.clone();
                    let snap = SnapshotData {
                        last_event_id: state.last_event_id,
                        state: state.clone(),
                        agent_contexts: std::collections::HashMap::new(),
                        saved_at: Utc::now(),
                    };
                    if let Err(e) = save_snapshot(&snapshot_dir, &snap) {
                        tracing::error!(
                            "event persister for spec {} failed to save recovery snapshot: {}",
                            spec_id,
                            e
                        );
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tracing::debug!("event persister for spec {} shutting down (channel closed)", spec_id);
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_state::AppState;
    use crate::providers::ProviderStatus;
    use crate::routes::create_router;
    use axum::body::Body;
    use http::Request;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_state() -> SharedState {
        let dir = tempfile::TempDir::new().unwrap();
        let provider_status = ProviderStatus {
            default_provider: "anthropic".to_string(),
            default_model: None,
            providers: vec![],
            any_available: false,
        };
        Arc::new(AppState::new(dir.keep(), provider_status))
    }

    #[test]
    fn index_template_renders() {
        let tmpl = IndexTemplate {};
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("barnstormer"));
        assert!(rendered.contains("Select a spec"));
    }

    #[test]
    fn spec_list_template_renders_empty() {
        let tmpl = SpecListTemplate { specs: vec![] };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("No specs yet"));
    }

    #[test]
    fn spec_list_template_renders_with_specs() {
        let tmpl = SpecListTemplate {
            specs: vec![SpecSummary {
                spec_id: "01HTEST".to_string(),
                title: "My Spec".to_string(),
                one_liner: "A test spec".to_string(),
                updated_at: "2025-01-01T00:00:00Z".to_string(),
            }],
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("My Spec"));
        assert!(rendered.contains("A test spec"));
    }

    #[test]
    fn create_spec_form_template_renders() {
        let tmpl = CreateSpecFormTemplate {};
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("description"));
        assert!(rendered.contains("What do you want to build?"));
        assert!(rendered.contains("Start Building"));
    }

    #[test]
    fn extract_placeholder_title_first_sentence() {
        assert_eq!(
            extract_placeholder_title("Build a todo app. With tags and filters."),
            "Build a todo app."
        );
    }

    #[test]
    fn extract_placeholder_title_truncates_long_text() {
        let long = "a".repeat(80);
        let result = extract_placeholder_title(&long);
        assert_eq!(result.chars().count(), 63); // 60 chars + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn extract_placeholder_title_short_text() {
        assert_eq!(extract_placeholder_title("Todo app"), "Todo app");
    }

    #[test]
    fn extract_placeholder_title_empty() {
        assert_eq!(extract_placeholder_title(""), "Untitled Spec");
        assert_eq!(extract_placeholder_title("   "), "Untitled Spec");
    }

    #[test]
    fn extract_placeholder_title_question_mark() {
        assert_eq!(
            extract_placeholder_title("Can we build this? I think so."),
            "Can we build this?"
        );
    }

    #[test]
    fn extract_placeholder_title_exclamation() {
        assert_eq!(
            extract_placeholder_title("Build this now! It's urgent."),
            "Build this now!"
        );
    }

    #[test]
    fn extract_placeholder_title_multibyte_utf8() {
        // Ensure we don't panic on multi-byte characters near the 60-byte boundary
        let emoji_text = format!("{}🚀🚀🚀🚀🚀 more text after emojis", "a".repeat(55));
        let result = extract_placeholder_title(&emoji_text);
        // Should truncate at a character boundary, not panic
        assert!(result.chars().count() <= 63); // max 60 + "..."
        assert!(result.ends_with("..."));

        // CJK characters (3 bytes each) — 40 chars fits within the 60-char limit
        let cjk_short = "你好世界你好世界你好世界你好世界你好世界你好世界你好世界你好世界你好世界你好世界";
        let result = extract_placeholder_title(cjk_short);
        assert_eq!(result, cjk_short); // 40 chars, no truncation needed

        // CJK characters exceeding 60-char limit (65 chars)
        let cjk_long: String = "你好世界你".repeat(13); // 65 chars
        let result = extract_placeholder_title(&cjk_long);
        assert!(result.ends_with("..."));
        assert!(result.chars().count() <= 63); // max 60 + "..."
    }

    #[test]
    fn board_template_renders_empty_lanes() {
        let tmpl = BoardTemplate {
            spec_id: "01HTEST".to_string(),
            lanes: vec![
                LaneData {
                    name: "Ideas".to_string(),
                    cards: vec![],
                },
                LaneData {
                    name: "Plan".to_string(),
                    cards: vec![],
                },
                LaneData {
                    name: "Spec".to_string(),
                    cards: vec![],
                },
            ],
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("Ideas"));
        assert!(rendered.contains("Plan"));
        assert!(rendered.contains("Spec"));
    }

    #[test]
    fn board_template_renders_with_cards() {
        let tmpl = BoardTemplate {
            spec_id: "01HTEST".to_string(),
            lanes: vec![LaneData {
                name: "Ideas".to_string(),
                cards: vec![CardData {
                    card_id: "01HCARD".to_string(),
                    card_type: "idea".to_string(),
                    title: "My Idea".to_string(),
                    body: Some("An interesting idea".to_string()),
                    body_html: Some("<p>An interesting idea</p>\n".to_string()),
                    lane: "Ideas".to_string(),
                    order: 1.0,
                    created_by: "human".to_string(),
                    updated_at: "12:00:00".to_string(),
                }],
            }],
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("My Idea"));
        assert!(rendered.contains("An interesting idea"));
    }

    #[test]
    fn card_form_template_renders_create() {
        let tmpl = CardFormTemplate {
            spec_id: "01HTEST".to_string(),
            card_id: None,
            title: String::new(),
            card_type: "idea".to_string(),
            body: String::new(),
            lane: "Ideas".to_string(),
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("Create Card"));
    }

    #[test]
    fn card_form_template_renders_edit() {
        let tmpl = CardFormTemplate {
            spec_id: "01HTEST".to_string(),
            card_id: Some("01HCARD".to_string()),
            title: "Existing Card".to_string(),
            card_type: "task".to_string(),
            body: "Some body".to_string(),
            lane: "Plan".to_string(),
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("Edit Card"));
        assert!(rendered.contains("Existing Card"));
    }

    #[test]
    fn document_template_renders() {
        let tmpl = DocumentTemplate {
            spec_id: "01HTEST".to_string(),
            title: "Test Doc".to_string(),
            one_liner: "A test document".to_string(),
            goal: "Verify rendering".to_string(),
            goal_html: "<p>Verify rendering</p>\n".to_string(),
            description: Some("A detailed description".to_string()),
            description_html: Some("<p>A detailed description</p>\n".to_string()),
            constraints: None,
            constraints_html: None,
            success_criteria: None,
            success_criteria_html: None,
            risks: None,
            risks_html: None,
            notes: None,
            notes_html: None,
            lanes: vec![],
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("Test Doc"));
        assert!(rendered.contains("A test document"));
        assert!(rendered.contains("Verify rendering"));
        assert!(rendered.contains("A detailed description"));
        assert!(
            rendered.contains("Auto-generated from spec data"),
            "document should contain auto-generated notice"
        );
    }

    #[test]
    fn activity_template_renders_empty() {
        let tmpl = ActivityTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "activity-transcript".to_string(),
            transcript: vec![],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("activity-transcript-feed"));
    }

    #[test]
    fn activity_template_renders_with_entries() {
        let tmpl = ActivityTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "activity-transcript".to_string(),
            transcript: vec![TranscriptEntry {
                sender: "agent-1".to_string(),
                sender_label: "Agent-1".to_string(),
                initial: "A".to_string(),
                is_human: false,
                is_step: false,
                is_continuation: false,
                role_class: "agent".to_string(),
                content: "Started analysis".to_string(),
                content_html: "<p>Started analysis</p>\n".to_string(),
                timestamp: "12:34:56".to_string(),
                repeat_count: 1,
            }],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("Agent-1"), "should contain sender_label");
        assert!(rendered.contains("Started analysis"));
    }

    #[test]
    fn activity_template_renders_boolean_question() {
        let tmpl = ActivityTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "activity-transcript".to_string(),
            transcript: vec![],
            pending_question: Some(QuestionData::Boolean {
                question_id: "01HQID".to_string(),
                question: "Proceed with this?".to_string(),
                default: Some(true),
            }),
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("Proceed with this?"));
        assert!(rendered.contains("Yes"));
        assert!(rendered.contains("No"));
    }

    #[test]
    fn activity_template_renders_freeform_question() {
        let tmpl = ActivityTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "activity-transcript".to_string(),
            transcript: vec![],
            pending_question: Some(QuestionData::Freeform {
                question_id: "01HQID".to_string(),
                question: "Describe the feature".to_string(),
                placeholder: "Type here...".to_string(),
            }),
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("Describe the feature"));
        assert!(rendered.contains("Type here..."));
    }

    #[test]
    fn activity_template_renders_multiple_choice_question() {
        let tmpl = ActivityTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "activity-transcript".to_string(),
            transcript: vec![],
            pending_question: Some(QuestionData::MultipleChoice {
                question_id: "01HQID".to_string(),
                question: "Pick a color".to_string(),
                choices: vec!["Red".to_string(), "Blue".to_string(), "Green".to_string()],
                allow_multi: false,
            }),
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("Pick a color"));
        assert!(rendered.contains("Red"));
        assert!(rendered.contains("Blue"));
        assert!(rendered.contains("Green"));
    }

    #[tokio::test]
    async fn get_index_returns_html() {
        let state = test_state();
        let app = create_router(state, None);

        let resp = app
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("barnstormer"));
    }

    #[tokio::test]
    async fn get_web_specs_returns_html() {
        let state = test_state();
        let app = create_router(state, None);

        let resp = app
            .oneshot(Request::get("/web/specs").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("No specs yet"));
    }

    #[tokio::test]
    async fn post_web_specs_creates_and_returns_spec_view() {
        let state = test_state();
        let app = create_router(Arc::clone(&state), None);

        let resp = app
            .oneshot(
                Request::post("/web/specs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("description=Build+a+test+spec+for+testing"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        // Verify HX-Push-Url header is set for auto-navigation
        let hx_push = resp.headers().get("hx-push-url");
        assert!(hx_push.is_some(), "response should include HX-Push-Url header");
        let url = hx_push.unwrap().to_str().unwrap();
        assert!(url.starts_with("/web/specs/"), "HX-Push-Url should point to spec view");
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Build a test spec for testing"));
    }

    #[test]
    fn activity_transcript_template_renders_empty() {
        let tmpl = ActivityTranscriptTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "activity-transcript".to_string(),
            transcript: vec![],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("activity-transcript"), "should contain activity-transcript id");
        assert!(rendered.contains("activity-transcript-feed"), "should contain activity-transcript-feed div");
    }

    #[test]
    fn activity_transcript_template_renders_with_entries() {
        let tmpl = ActivityTranscriptTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "activity-transcript".to_string(),
            transcript: vec![TranscriptEntry {
                sender: "agent-1".to_string(),
                sender_label: "Agent-1".to_string(),
                initial: "A".to_string(),
                is_human: false,
                is_step: false,
                is_continuation: false,
                role_class: "agent".to_string(),
                content: "Started analysis".to_string(),
                content_html: "<p>Started analysis</p>\n".to_string(),
                timestamp: "12:34:56".to_string(),
                repeat_count: 1,
            }],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("Agent-1"), "should contain sender_label");
        assert!(rendered.contains("Started analysis"));
        assert!(!rendered.contains("chat-input"), "transcript template should not contain chat input");
    }

    #[test]
    fn transcript_template_renders_with_custom_container_id() {
        let tmpl = ActivityTranscriptTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "chat-transcript".to_string(),
            transcript: vec![TranscriptEntry {
                sender: "human".to_string(),
                sender_label: "You".to_string(),
                initial: "Y".to_string(),
                is_human: true,
                is_step: false,
                is_continuation: false,
                role_class: "human".to_string(),
                content: "Hello chat".to_string(),
                content_html: "<p>Hello chat</p>\n".to_string(),
                timestamp: "12:00:00".to_string(),
                repeat_count: 1,
            }],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("id=\"chat-transcript\""), "should use chat-transcript as container id");
        assert!(rendered.contains("id=\"chat-transcript-feed\""), "should use chat-transcript-feed as feed id");
        assert!(rendered.contains("hx-target=\"#chat-transcript\""), "should target chat-transcript");
        assert!(rendered.contains("container_id=chat-transcript"), "hx-get should include container_id param");
        assert!(!rendered.contains("id=\"activity-transcript\""), "should not contain activity-transcript id");
        assert!(rendered.contains("Hello chat"), "should render transcript content");
    }

    #[test]
    fn activity_template_does_not_contain_chat_input() {
        let tmpl = ActivityTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "activity-transcript".to_string(),
            transcript: vec![],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(!rendered.contains("chat-input"), "activity should not contain chat input div");
        assert!(!rendered.contains("Send a message"), "activity should not contain chat placeholder");
    }

    #[test]
    fn activity_template_contains_agent_controls() {
        let tmpl = ActivityTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "activity-transcript".to_string(),
            transcript: vec![],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("agent-controls"), "activity should contain agent controls");
        assert!(rendered.contains("agent-status"), "activity should contain agent status");
        assert!(rendered.contains("Undo"), "activity should contain undo button");
    }

    #[test]
    fn spec_view_template_contains_mission_control_layout() {
        let tmpl = SpecViewTemplate {
            spec_id: "01HTEST".to_string(),
            title: "Test Spec".to_string(),
            one_liner: "A test spec".to_string(),
            goal: "Test goal".to_string(),
            phase: "refining".to_string(),
            lanes: vec![],
            canvas_content: None,
            has_pending_question: false,
        };
        let rendered = tmpl.render().unwrap();
        // Command bar with title and subtitle
        assert!(rendered.contains("command-bar"), "should contain command-bar");
        assert!(rendered.contains("Test Spec"), "should contain spec title");
        assert!(rendered.contains("A test spec"), "should contain one-liner");
        // Capsule view toggles for document, board, spec
        assert!(rendered.contains("view-toggles-capsule"), "should contain capsule view toggles");
        assert!(rendered.contains("data-view=\"document\""), "should contain document toggle");
        assert!(rendered.contains("data-view=\"board\""), "should contain board toggle");
        assert!(rendered.contains("data-view=\"spec\""), "should contain spec toggle");
        assert!(rendered.contains("view-toggle active"), "document toggle should be active");
        // Canvas and chat rail
        assert!(rendered.contains("id=\"canvas\""), "should contain canvas element");
        assert!(rendered.contains("spec-body"), "should contain spec-body row");
        assert!(rendered.contains("chat-rail"), "should contain chat-rail");
        assert!(rendered.contains("chat-panel"), "should load chat panel");
        // Agent controls in command bar
        assert!(rendered.contains("agent-controls"), "should contain agent-controls");
        // SSE on spec-compositor
        assert!(rendered.contains("sse-connect"), "should have SSE connection");
        // Old layout elements should NOT be present
        assert!(!rendered.contains("mission-strip"), "should not contain mission-strip");
        assert!(!rendered.contains("mission-ticker"), "should not contain mission-ticker");
        assert!(!rendered.contains("tab-bar"), "should not contain old tab-bar");
        assert!(!rendered.contains("right-rail"), "should not contain right-rail references");
    }

    #[test]
    fn mission_ticker_template_renders_empty() {
        let tmpl = MissionTickerTemplate {
            spec_id: "01HTEST".to_string(),
            ticker_entries: vec![],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("Awaiting activity"), "should show empty state");
        assert!(rendered.contains("mission-ticker-feed"), "should contain ticker feed id");
    }

    #[test]
    fn mission_ticker_template_renders_with_entries() {
        let tmpl = MissionTickerTemplate {
            spec_id: "01HTEST".to_string(),
            ticker_entries: vec![TranscriptEntry {
                sender: "manager-01JTEST".to_string(),
                sender_label: "Manager".to_string(),
                initial: "M".to_string(),
                is_human: false,
                is_step: false,
                is_continuation: false,
                role_class: "manager".to_string(),
                content: "Analyzing requirements".to_string(),
                content_html: "<p>Analyzing requirements</p>\n".to_string(),
                timestamp: "12:34:56".to_string(),
                repeat_count: 1,
            }],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("Manager"), "should contain sender label");
        assert!(rendered.contains("Analyzing requirements"), "should contain message content");
        assert!(rendered.contains("ticker-entry"), "should contain ticker entry class");
    }

    #[test]
    fn mission_ticker_template_renders_with_question() {
        let tmpl = MissionTickerTemplate {
            spec_id: "01HTEST".to_string(),
            ticker_entries: vec![],
            pending_question: Some(QuestionData::Boolean {
                question_id: "01HQID".to_string(),
                question: "Should we proceed?".to_string(),
                default: None,
            }),
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("Should we proceed?"), "should contain question text");
        assert!(rendered.contains("Yes"), "should contain Yes button");
        assert!(rendered.contains("No"), "should contain No button");
    }

    #[test]
    fn agent_leds_template_renders_running() {
        let tmpl = AgentLedsTemplate {
            spec_id: "01HTEST".to_string(),
            running: true,
            started: true,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("led-active"), "should contain active LED class");
        assert!(rendered.contains("led-manager"), "should contain manager LED");
        assert!(rendered.contains("led-brainstormer"), "should contain brainstormer LED");
        assert!(rendered.contains("led-planner"), "should contain planner LED");
    }

    #[test]
    fn agent_leds_template_renders_paused() {
        let tmpl = AgentLedsTemplate {
            spec_id: "01HTEST".to_string(),
            running: false,
            started: true,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("led-paused"), "should contain paused LED class");
        assert!(!rendered.contains("led-active"), "should not contain active LED class");
    }

    #[test]
    fn agent_leds_template_renders_stopped() {
        let tmpl = AgentLedsTemplate {
            spec_id: "01HTEST".to_string(),
            running: false,
            started: false,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("led-off"), "should contain off LED class");
        assert!(!rendered.contains("led-active"), "should not contain active LED class");
        assert!(!rendered.contains("led-paused"), "should not contain paused LED class");
    }

    #[tokio::test]
    async fn post_chat_sends_message() {
        let state = test_state();

        // First create a spec
        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::post("/web/specs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("description=Build+a+chat+testing+system"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        // Get the spec_id from the actors
        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().expect("should have a spec")
        };

        // Post a chat message
        let app2 = create_router(Arc::clone(&state), None);
        let resp = app2
            .oneshot(
                Request::post(format!("/web/specs/{}/chat", spec_id))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("message=Hello+from+chat"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("Hello from chat"),
            "chat message should appear in activity: {}",
            html
        );
        assert!(
            html.contains("human"),
            "sender should be 'human' in activity: {}",
            html
        );
    }

    #[tokio::test]
    async fn post_chat_to_nonexistent_spec_returns_404() {
        let state = test_state();
        let app = create_router(state, None);
        let fake_id = ulid::Ulid::new();
        let resp = app
            .oneshot(
                Request::post(format!("/web/specs/{}/chat", fake_id))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("message=Hello"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
    }

    #[test]
    fn agent_status_template_renders_stopped() {
        let tmpl = AgentStatusTemplate {
            spec_id: "01HTEST".to_string(),
            running: false,
            started: false,
            agent_count: 0,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("agent-status"), "should contain agent-status id");
        assert!(rendered.contains("agent-pill-stopped"), "should have stopped pill class");
        assert!(rendered.contains("Start agents"), "should show start agents text");
        assert!(rendered.contains("/agents/start"), "should have start action URL");
    }

    #[test]
    fn agent_status_template_renders_running() {
        let tmpl = AgentStatusTemplate {
            spec_id: "01HTEST".to_string(),
            running: true,
            started: true,
            agent_count: 4,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("agent-pill-running"), "should have running pill class");
        assert!(rendered.contains("Agents active"), "should show active state");
        assert!(rendered.contains("/agents/pause"), "should have pause action URL");
    }

    #[test]
    fn agent_status_template_renders_paused() {
        let tmpl = AgentStatusTemplate {
            spec_id: "01HTEST".to_string(),
            running: false,
            started: true,
            agent_count: 4,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("agent-pill-paused"), "should have paused pill class");
        assert!(rendered.contains("Agents paused"), "should show paused state");
        assert!(rendered.contains("/agents/resume"), "should have resume action URL");
    }

    #[tokio::test]
    async fn get_agent_status_returns_stopped_when_no_swarm() {
        let state = test_state();

        // Create a spec first
        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::post("/web/specs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("description=Build+an+agent+testing+system"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().expect("should have a spec")
        };

        // Get agent status (no swarm started)
        let app2 = create_router(Arc::clone(&state), None);
        let resp = app2
            .oneshot(
                Request::get(format!("/web/specs/{}/agents/status", spec_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Start agents"), "should show stopped pill when no swarm: {}", html);
    }

    #[tokio::test]
    async fn pause_agents_returns_stopped_when_no_swarm() {
        let state = test_state();

        // Create a spec
        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::post("/web/specs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("description=Build+a+pause+testing+system"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().expect("should have a spec")
        };

        // Pause without starting returns stopped state
        let app2 = create_router(Arc::clone(&state), None);
        let resp = app2
            .oneshot(
                Request::post(format!("/web/specs/{}/agents/pause", spec_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Start agents"), "pause with no swarm should show stopped pill: {}", html);
    }

    #[tokio::test]
    async fn resume_agents_returns_stopped_when_no_swarm() {
        let state = test_state();

        // Create a spec
        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::post("/web/specs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("description=Build+a+resume+testing+system"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().expect("should have a spec")
        };

        // Resume without starting returns stopped state
        let app2 = create_router(Arc::clone(&state), None);
        let resp = app2
            .oneshot(
                Request::post(format!("/web/specs/{}/agents/resume", spec_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Start agents"), "resume with no swarm should show stopped pill: {}", html);
    }

    #[tokio::test]
    async fn agent_status_for_nonexistent_spec_returns_stopped() {
        let state = test_state();
        let app = create_router(state, None);
        let fake_id = ulid::Ulid::new();

        let resp = app
            .oneshot(
                Request::get(format!("/web/specs/{}/agents/status", fake_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Start agents"), "nonexistent spec should show stopped pill: {}", html);
    }

    #[test]
    fn provider_status_template_renders_no_providers() {
        let tmpl = ProviderStatusTemplate {
            default_provider: "anthropic".to_string(),
            default_model: None,
            providers: vec![
                ProviderInfoView { name: "anthropic".to_string(), has_api_key: false, model: "claude-sonnet-4-5-20250929".to_string() },
                ProviderInfoView { name: "openai".to_string(), has_api_key: false, model: "gpt-4o".to_string() },
                ProviderInfoView { name: "gemini".to_string(), has_api_key: false, model: "gemini-2.0-flash".to_string() },
            ],
            any_available: false,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("No provider configured"));
        assert!(rendered.contains("disconnected"));
    }

    #[test]
    fn provider_status_template_renders_with_provider() {
        let tmpl = ProviderStatusTemplate {
            default_provider: "anthropic".to_string(),
            default_model: Some("claude-sonnet-4-5-20250929".to_string()),
            providers: vec![
                ProviderInfoView { name: "anthropic".to_string(), has_api_key: true, model: "claude-sonnet-4-5-20250929".to_string() },
                ProviderInfoView { name: "openai".to_string(), has_api_key: false, model: "gpt-4o".to_string() },
            ],
            any_available: true,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("anthropic"));
        assert!(rendered.contains("connected"));
        assert!(rendered.contains("claude-sonnet-4-5-20250929"));
    }

    #[tokio::test]
    async fn get_provider_status_returns_html() {
        let state = test_state();
        let app = create_router(state, None);
        let resp = app
            .oneshot(Request::get("/web/provider-status").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("provider-status"));
    }

    /// Create a test state that explicitly has no LLM providers configured,
    /// regardless of the actual environment variables on the machine.
    fn test_state_no_provider() -> SharedState {
        let dir = tempfile::TempDir::new().unwrap();
        let provider_status = ProviderStatus {
            default_provider: "anthropic".to_string(),
            default_model: None,
            providers: vec![],
            any_available: false,
        };
        Arc::new(AppState::new(dir.keep(), provider_status))
    }

    #[tokio::test]
    async fn create_spec_with_no_provider_does_not_start_agents() {
        let state = test_state_no_provider();
        let app = create_router(Arc::clone(&state), None);

        let resp = app
            .oneshot(
                Request::post("/web/specs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("description=Build+a+system+without+agents"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        // Since provider_status.any_available is false, try_start_agents should
        // return early and no swarm should be created.
        let swarms = state.swarms.read().await;
        assert!(swarms.is_empty(), "no swarm should be created without provider");
    }

    #[tokio::test]
    async fn start_agents_for_nonexistent_spec_returns_404() {
        let state = test_state();
        let app = create_router(state, None);
        let fake_id = ulid::Ulid::new();

        let resp = app
            .oneshot(
                Request::post(format!("/web/specs/{}/agents/start", fake_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), 404);
    }

    // ---- Chat panel tests ----

    #[test]
    fn chat_panel_template_renders() {
        let tmpl = ChatPanelTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "chat-transcript".to_string(),

            transcript: vec![],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("chat-panel"), "should contain chat-panel div");
    }

    #[test]
    fn chat_panel_renders_with_transcript_entries() {
        let tmpl = ChatPanelTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "chat-transcript".to_string(),

            transcript: vec![
                TranscriptEntry {
                    sender: "human".to_string(),
                    sender_label: "You".to_string(),
                    initial: "Y".to_string(),
                    is_human: true,
                    is_step: false,
                    is_continuation: false,
                    role_class: "human".to_string(),
                    content: "Hello from human".to_string(),
                    content_html: "<p>Hello from human</p>\n".to_string(),
                    timestamp: "12:34:56".to_string(),
                    repeat_count: 1,
                },
                TranscriptEntry {
                    sender: "manager-01HAGENT".to_string(),
                    sender_label: "Manager".to_string(),
                    initial: "M".to_string(),
                    is_human: false,
                    is_step: false,
                    is_continuation: false,
                    role_class: "manager".to_string(),
                    content: "Agent response here".to_string(),
                    content_html: "<p>Agent response here</p>\n".to_string(),
                    timestamp: "12:35:00".to_string(),
                    repeat_count: 1,
                },
            ],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("Hello from human"), "should contain human message content");
        assert!(rendered.contains("Agent response here"), "should contain agent message content");
        assert!(rendered.contains("chat-message"), "should have chat-message class");
        assert!(rendered.contains("chat-avatar"), "should have avatar element");
        assert!(rendered.contains("chat-sender"), "should have sender label element");
        assert!(!rendered.contains("No messages yet"), "should not show empty state when entries exist");
    }

    #[test]
    fn chat_panel_renders_empty_transcript() {
        let tmpl = ChatPanelTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "chat-transcript".to_string(),

            transcript: vec![],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("No messages yet"), "should show empty state message");
        assert!(rendered.contains("chat-empty"), "should have chat-empty class");
        assert!(rendered.contains("Type below to start a conversation"), "should show hint text");
    }

    #[test]
    fn chat_panel_contains_chat_input_form() {
        let tmpl = ChatPanelTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "chat-transcript".to_string(),

            transcript: vec![],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("chat-input-area"), "should contain chat-input-area div");
        assert!(rendered.contains("chat-input-row"), "should contain chat-input-row div");
        assert!(rendered.contains(r#"hx-post="/web/specs/01HTEST/chat""#), "should post to chat endpoint");
        assert!(rendered.contains(r##"hx-target="#chat-transcript""##), "chat form should target chat-transcript");
        assert!(rendered.contains("Ask the agents anything"), "should have placeholder text");
    }

    #[test]
    fn chat_panel_contains_header_and_transcript() {
        let tmpl = ChatPanelTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "chat-transcript".to_string(),

            transcript: vec![],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("chat-panel-header"), "should contain chat panel header");
        assert!(rendered.contains("Chat"), "should contain Chat title");
        assert!(rendered.contains("sse:transcript_appended"), "should listen for transcript_appended event");
    }

    #[tokio::test]
    async fn chat_panel_handler_returns_200() {
        let state = test_state();

        // Create a spec first
        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::post("/web/specs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("description=Build+a+chat+panel+testing+system"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().expect("should have a spec")
        };

        let app2 = create_router(Arc::clone(&state), None);
        let resp = app2
            .oneshot(
                Request::get(format!("/web/specs/{}/chat-panel", spec_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("chat-panel"),
            "chat panel response should contain chat-panel: {}",
            html
        );
    }

    #[tokio::test]
    async fn chat_panel_for_nonexistent_spec_returns_404() {
        let state = test_state();
        let app = create_router(state, None);
        let fake_id = ulid::Ulid::new();
        let resp = app
            .oneshot(
                Request::get(format!("/web/specs/{}/chat-panel", fake_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
    }

    #[tokio::test]
    async fn chat_panel_brainstorming_targets_brainstorm_chat() {
        let state = test_state();
        let app = create_router(Arc::clone(&state), None);
        app.oneshot(
            Request::post("/web/specs")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("description=Chat+brainstorm+test"))
                .unwrap(),
        )
        .await
        .unwrap();

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
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("brainstorm-chat"),
            "should target brainstorm-chat container in brainstorming"
        );
        assert!(
            !html.contains("chat-fullwidth"),
            "should not have chat-fullwidth class (removed)"
        );
    }

    #[tokio::test]
    async fn chat_panel_refining_targets_chat_transcript() {
        let state = test_state();
        let app = create_router(Arc::clone(&state), None);
        app.oneshot(
            Request::post("/web/specs")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("description=Chat+refining+test"))
                .unwrap(),
        )
        .await
        .unwrap();

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().unwrap()
        };

        // Transition to Refining
        let actors = state.actors.read().await;
        let handle = actors.get(&spec_id).unwrap();
        handle
            .send_command(Command::TransitionPhase {
                target: SpecPhase::Refining,
            })
            .await
            .unwrap();
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
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("chat-transcript"),
            "should target chat-transcript container in refining"
        );
        assert!(
            !html.contains("chat-fullwidth"),
            "should not have chat-fullwidth class (removed)"
        );
    }

    // ---- Artifacts tests ----

    #[test]
    fn artifacts_template_renders() {
        let tmpl = ArtifactsTemplate {
            spec_id: "01HTEST".to_string(),
            markdown_content: "# My Spec".to_string(),
            yaml_content: "title: My Spec".to_string(),
            dot_content: "digraph {}".to_string(),
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("artifacts-panel"), "should contain artifacts-panel div");
    }

    #[test]
    fn artifacts_template_contains_all_content_sections() {
        let tmpl = ArtifactsTemplate {
            spec_id: "01HTEST".to_string(),
            markdown_content: "# My Spec".to_string(),
            yaml_content: "title: My Spec".to_string(),
            dot_content: "digraph {}".to_string(),
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("id=\"markdown-source\""), "should contain markdown-source section");
        assert!(rendered.contains("id=\"yaml-source\""), "should contain yaml-source section");
        assert!(rendered.contains("id=\"dot-source\""), "should contain dot-source section");
        assert!(rendered.contains("# My Spec"), "should render markdown content");
        assert!(rendered.contains("title: My Spec"), "should render yaml content");
        assert!(rendered.contains("digraph {}"), "should render dot content");
    }

    #[test]
    fn artifacts_template_contains_download_links() {
        let tmpl = ArtifactsTemplate {
            spec_id: "01HTEST".to_string(),
            markdown_content: "# Test".to_string(),
            yaml_content: "title: Test".to_string(),
            dot_content: "digraph {}".to_string(),
        };
        let rendered = tmpl.render().unwrap();
        assert!(
            rendered.contains("/web/specs/01HTEST/export/markdown"),
            "should contain markdown download link"
        );
        assert!(
            rendered.contains("/web/specs/01HTEST/export/yaml"),
            "should contain yaml download link"
        );
        assert!(
            rendered.contains("/web/specs/01HTEST/export/dot"),
            "should contain dot download link"
        );
        assert!(rendered.contains("download=\"spec.md\""), "should have spec.md download attribute");
        assert!(rendered.contains("download=\"spec.yaml\""), "should have spec.yaml download attribute");
        assert!(rendered.contains("download=\"spec.dot\""), "should have spec.dot download attribute");
    }

    #[test]
    fn artifacts_template_contains_copy_buttons() {
        let tmpl = ArtifactsTemplate {
            spec_id: "01HTEST".to_string(),
            markdown_content: "# Test".to_string(),
            yaml_content: "title: Test".to_string(),
            dot_content: "digraph {}".to_string(),
        };
        let rendered = tmpl.render().unwrap();
        // Count actual copy button elements by matching the class attribute on button tags,
        // not bare "btn-copy" which also matches JS selector references.
        let copy_count = rendered.matches("class=\"btn btn-sm btn-copy\"").count();
        assert_eq!(copy_count, 3, "should have exactly 3 copy buttons, found {}", copy_count);
    }

    #[tokio::test]
    async fn artifacts_handler_returns_200() {
        let state = test_state();

        // Create a spec first
        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::post("/web/specs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("description=Build+an+artifacts+testing+system"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().expect("should have a spec")
        };

        let app2 = create_router(Arc::clone(&state), None);
        let resp = app2
            .oneshot(
                Request::get(format!("/web/specs/{}/artifacts", spec_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("artifacts-panel"),
            "artifacts response should contain artifacts-panel: {}",
            html
        );
    }

    #[tokio::test]
    async fn artifacts_for_nonexistent_spec_returns_404() {
        let state = test_state();
        let app = create_router(state, None);
        let fake_id = ulid::Ulid::new();
        let resp = app
            .oneshot(
                Request::get(format!("/web/specs/{}/artifacts", fake_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
    }

    // ---- Spec tab tests ----

    #[tokio::test]
    async fn spec_handler_returns_200() {
        let state = test_state();
        let spec_id = create_test_spec(&state).await;

        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::get(format!("/web/specs/{}/spec", spec_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("spec-document"),
            "spec response should contain spec-document class: {}",
            html
        );
    }

    #[tokio::test]
    async fn spec_for_nonexistent_spec_returns_404() {
        let state = test_state();
        let app = create_router(state, None);
        let fake_id = ulid::Ulid::new();
        let resp = app
            .oneshot(
                Request::get(format!("/web/specs/{}/spec", fake_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
    }

    #[test]
    fn spec_template_renders_with_content() {
        let tmpl = SpecTabTemplate {
            spec_id: "01HTEST".to_string(),
            spec_html: "<h1>Test</h1>".to_string(),
            spec_markdown: "# Test".to_string(),
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("spec-document"), "should contain spec-document class");
        assert!(rendered.contains("spec-copy-md"), "should contain copy markdown button");
    }

    #[test]
    fn spec_template_renders_empty_state() {
        let tmpl = SpecTabTemplate {
            spec_id: "01HTEST".to_string(),
            spec_html: String::new(),
            spec_markdown: String::new(),
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("spec-document"));
    }

    // ---- Export download tests ----

    /// Helper to create a spec and return its ULID.
    async fn create_test_spec(state: &SharedState) -> ulid::Ulid {
        let app = create_router(Arc::clone(state), None);
        let resp = app
            .oneshot(
                Request::post("/web/specs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("description=Build+an+export+testing+system"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let actors = state.actors.read().await;
        *actors.keys().next().expect("should have a spec")
    }

    #[tokio::test]
    async fn export_markdown_returns_200_with_correct_headers() {
        let state = test_state();
        let spec_id = create_test_spec(&state).await;

        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::get(format!("/web/specs/{}/export/markdown", spec_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "text/markdown"
        );
        assert_eq!(
            resp.headers().get("content-disposition").unwrap(),
            "attachment; filename=\"spec.md\""
        );
    }

    #[tokio::test]
    async fn export_yaml_returns_200_with_correct_headers() {
        let state = test_state();
        let spec_id = create_test_spec(&state).await;

        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::get(format!("/web/specs/{}/export/yaml", spec_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "text/yaml"
        );
        assert_eq!(
            resp.headers().get("content-disposition").unwrap(),
            "attachment; filename=\"spec.yaml\""
        );
    }

    #[tokio::test]
    async fn export_dot_returns_200_with_correct_headers() {
        let state = test_state();
        let spec_id = create_test_spec(&state).await;

        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::get(format!("/web/specs/{}/export/dot", spec_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "text/plain"
        );
        assert_eq!(
            resp.headers().get("content-disposition").unwrap(),
            "attachment; filename=\"spec.dot\""
        );
    }

    #[tokio::test]
    async fn export_markdown_for_nonexistent_spec_returns_404() {
        let state = test_state();
        let app = create_router(state, None);
        let fake_id = ulid::Ulid::new();
        let resp = app
            .oneshot(
                Request::get(format!("/web/specs/{}/export/markdown", fake_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
    }

    #[tokio::test]
    async fn export_yaml_for_nonexistent_spec_returns_404() {
        let state = test_state();
        let app = create_router(state, None);
        let fake_id = ulid::Ulid::new();
        let resp = app
            .oneshot(
                Request::get(format!("/web/specs/{}/export/yaml", fake_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
    }

    #[tokio::test]
    async fn export_dot_for_nonexistent_spec_returns_404() {
        let state = test_state();
        let app = create_router(state, None);
        let fake_id = ulid::Ulid::new();
        let resp = app
            .oneshot(
                Request::get(format!("/web/specs/{}/export/dot", fake_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
    }

    #[tokio::test]
    async fn export_spec_returns_200_with_correct_headers() {
        let state = test_state();
        let spec_id = create_test_spec(&state).await;

        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::get(format!("/web/specs/{}/export/spec", spec_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let content_type = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            content_type.contains("text/markdown"),
            "content-type should be text/markdown, got: {}",
            content_type
        );

        let disposition = resp
            .headers()
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            disposition.contains("spec.md"),
            "should offer spec.md download, got: {}",
            disposition
        );
    }

    #[tokio::test]
    async fn export_spec_for_nonexistent_spec_returns_404() {
        let state = test_state();
        let app = create_router(state, None);
        let fake_id = ulid::Ulid::new();
        let resp = app
            .oneshot(
                Request::get(format!("/web/specs/{}/export/spec", fake_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
    }

    #[tokio::test]
    async fn activity_transcript_handler_defaults_container_id() {
        let state = test_state();

        // Create a spec
        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::post("/web/specs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("description=Build+a+container+testing+system"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().expect("should have a spec")
        };

        // GET transcript without container_id param
        let app2 = create_router(Arc::clone(&state), None);
        let resp = app2
            .oneshot(
                Request::get(format!("/web/specs/{}/activity/transcript", spec_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains(r#"id="activity-transcript""#),
            "should default to activity-transcript container id"
        );
    }

    #[tokio::test]
    async fn activity_transcript_handler_accepts_container_id_param() {
        let state = test_state();

        // Create a spec
        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::post("/web/specs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("description=Build+a+container+testing+system"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().expect("should have a spec")
        };

        // GET transcript with container_id=chat-transcript
        let app2 = create_router(Arc::clone(&state), None);
        let resp = app2
            .oneshot(
                Request::get(format!(
                    "/web/specs/{}/activity/transcript?container_id=chat-transcript",
                    spec_id
                ))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains(r#"id="chat-transcript""#),
            "should use chat-transcript container id from query param"
        );
        assert!(
            !html.contains(r#"id="activity-transcript""#),
            "should not contain activity-transcript when chat-transcript requested"
        );
    }

    #[test]
    fn sanitize_container_id_rejects_unknown_values() {
        assert_eq!(sanitize_container_id("activity-transcript"), "activity-transcript");
        assert_eq!(sanitize_container_id("chat-transcript"), "chat-transcript");
        assert_eq!(sanitize_container_id("mission-ticker"), "mission-ticker");
        assert_eq!(sanitize_container_id("brainstorm-chat"), "brainstorm-chat");
        // IDs that are DOM element IDs but not transcript container_ids should be rejected.
        assert_eq!(sanitize_container_id("canvas"), "chat-transcript");
        assert_eq!(sanitize_container_id("chat-rail"), "chat-transcript");
        assert_eq!(sanitize_container_id("'); alert('xss'); //"), "chat-transcript");
        assert_eq!(sanitize_container_id("malicious-id"), "chat-transcript");
        assert_eq!(sanitize_container_id(""), "chat-transcript");
    }

    // ---- sender_display tests ----

    #[test]
    fn sender_display_human() {
        let (label, is_human, role_class) = sender_display("human");
        assert_eq!(label, "You");
        assert!(is_human, "human should be flagged as is_human");
        assert_eq!(role_class, "human");
    }

    #[test]
    fn sender_display_manager_role() {
        let (label, is_human, role_class) = sender_display("manager-01JTESTID123");
        assert_eq!(label, "Orchestrator");
        assert!(!is_human, "agent should not be flagged as human");
        assert_eq!(role_class, "manager");
    }

    #[test]
    fn sender_display_brainstormer_role() {
        let (label, is_human, role_class) = sender_display("brainstormer-01JTESTID456");
        assert_eq!(label, "Researcher");
        assert!(!is_human);
        assert_eq!(role_class, "brainstormer");
    }

    #[test]
    fn sender_display_dot_generator_role() {
        let (label, is_human, role_class) = sender_display("dot_generator-01JTESTID789");
        assert_eq!(label, "Dot Generator");
        assert!(!is_human);
        assert_eq!(role_class, "dot_generator");
    }

    #[test]
    fn sender_display_unknown_sender() {
        let (label, is_human, role_class) = sender_display("CustomRole-01JTESTID");
        // The capitalization loop uppercases only the first character and keeps
        // the rest as-is, so "CustomRole" becomes "CustomRole" (already capitalized).
        assert_eq!(label, "CustomRole", "unknown role should keep original casing except first char");
        assert!(!is_human);
        assert_eq!(role_class, "customrole", "role_class should be normalized to lowercase");
    }

    #[test]
    fn sender_display_unusual_characters() {
        let (_label, is_human, role_class) = sender_display("My Agent!@#");
        assert!(!is_human);
        // No '-' separator, so the entire string is the role. Normalization:
        // lowercase + replace space/!/@ /# with hyphens → "my-agent---"
        assert_eq!(role_class, "my-agent---", "special chars should be replaced with hyphens");
    }

    // ---- is_chat_participant tests ----

    #[test]
    fn chat_participant_human() {
        assert!(is_chat_participant("human"));
    }

    #[test]
    fn chat_participant_manager() {
        assert!(is_chat_participant("manager-01JTESTID123"));
    }

    #[test]
    fn chat_participant_rejects_other_agents() {
        assert!(!is_chat_participant("brainstormer-01JTESTID"));
        assert!(!is_chat_participant("planner-01JTESTID"));
        assert!(!is_chat_participant("dot_generator-01JTESTID"));
        assert!(!is_chat_participant("critic-01JTESTID"));
    }

    // ---- normalize_css_class tests ----

    #[test]
    fn normalize_css_class_lowercases() {
        assert_eq!(normalize_css_class("Manager"), "manager");
    }

    #[test]
    fn normalize_css_class_replaces_special_chars() {
        assert_eq!(normalize_css_class("dot generator"), "dot-generator");
        assert_eq!(normalize_css_class("foo@bar"), "foo-bar");
    }

    #[test]
    fn normalize_css_class_preserves_valid_chars() {
        assert_eq!(normalize_css_class("my_role-1"), "my_role-1");
    }

    // ---- Handler tests for chat and answer with HX-Target ----

    #[tokio::test]
    async fn post_chat_with_hx_target_returns_chat_transcript() {
        let state = test_state();

        // Create a spec
        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::post("/web/specs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("description=Build+an+HX+chat+testing+system"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().expect("should have a spec")
        };

        // POST to /chat with HX-Target: #chat-transcript
        let app2 = create_router(Arc::clone(&state), None);
        let resp = app2
            .oneshot(
                Request::post(format!("/web/specs/{}/chat", spec_id))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Target", "#chat-transcript")
                    .body(Body::from("message=Hello+from+chat+tab"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains(r#"id="chat-transcript""#),
            "response should target chat-transcript container: {}",
            html
        );
        assert!(
            html.contains("Hello from chat tab"),
            "response should contain the posted message: {}",
            html
        );
    }

    #[tokio::test]
    async fn post_answer_with_hx_target_returns_chat_transcript() {
        let state = test_state();

        // Create a spec
        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::post("/web/specs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("description=Build+an+answer+testing+system"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().expect("should have a spec")
        };

        // Ask a question via the actor so we have a pending question to answer
        let question_id = ulid::Ulid::new();
        {
            let actors = state.actors.read().await;
            let handle = actors.get(&spec_id).expect("actor should exist");
            handle
                .send_command(Command::AskQuestion {
                    question: barnstormer_core::UserQuestion::Freeform {
                        question_id,
                        question: "What color?".to_string(),
                        placeholder: None,
                        validation_hint: None,
                    },
                })
                .await
                .unwrap();
        }

        // POST to /answer with HX-Target: #chat-transcript
        let app2 = create_router(Arc::clone(&state), None);
        let resp = app2
            .oneshot(
                Request::post(format!("/web/specs/{}/answer", spec_id))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Target", "#chat-transcript")
                    .body(Body::from(format!(
                        "question_id={}&answer=Blue",
                        question_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains(r#"id="chat-transcript""#),
            "response should target chat-transcript container: {}",
            html
        );
    }

    // ---- Chat feed / question split template tests ----

    #[test]
    fn chat_feed_template_renders_empty() {
        let tmpl = ChatFeedTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "chat-transcript".to_string(),
            transcript: vec![],
        };
        let rendered = tmpl.render().unwrap();
        assert!(
            rendered.contains(r#"id="chat-transcript-feed""#),
            "should contain feed container id"
        );
        assert!(
            rendered.contains("No messages yet"),
            "empty feed should show empty state"
        );
        assert!(
            rendered.contains("sse:transcript_appended"),
            "feed should trigger on transcript_appended"
        );
        assert!(
            !rendered.contains("chat-question-card"),
            "feed should not contain question card"
        );
    }

    #[test]
    fn chat_feed_template_renders_with_entries() {
        let tmpl = ChatFeedTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "chat-transcript".to_string(),
            transcript: vec![TranscriptEntry {
                sender: "human".to_string(),
                sender_label: "You".to_string(),
                initial: "Y".to_string(),
                is_human: true,
                is_step: false,
                is_continuation: false,
                role_class: "human".to_string(),
                content: "Hello world".to_string(),
                content_html: "<p>Hello world</p>\n".to_string(),
                timestamp: "12:00:00".to_string(),
                repeat_count: 1,
            }],
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("Hello world"));
        assert!(rendered.contains("You"), "should contain sender label");
        assert!(
            !rendered.contains("No messages yet"),
            "should not show empty state when entries exist"
        );
    }

    #[test]
    fn chat_feed_template_contains_part_feed_in_hx_get() {
        let tmpl = ChatFeedTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "brainstorm-chat".to_string(),
            transcript: vec![],
        };
        let rendered = tmpl.render().unwrap();
        assert!(
            rendered.contains("part=feed"),
            "feed hx-get should include part=feed param"
        );
        assert!(
            rendered.contains("container_id=brainstorm-chat"),
            "feed hx-get should include container_id param"
        );
    }

    #[test]
    fn chat_question_template_renders_no_question() {
        let tmpl = ChatQuestionTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "chat-transcript".to_string(),
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(
            rendered.contains(r#"id="chat-transcript-question""#),
            "should contain question container id"
        );
        assert!(
            !rendered.contains("chat-question-card"),
            "should not render question card when no question pending"
        );
        assert!(
            rendered.contains("sse:question_asked"),
            "question container should trigger on question_asked"
        );
    }

    #[test]
    fn chat_question_template_renders_boolean_question() {
        let tmpl = ChatQuestionTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "chat-transcript".to_string(),
            pending_question: Some(QuestionData::Boolean {
                question_id: "01HQID".to_string(),
                question: "Continue?".to_string(),
                default: Some(true),
            }),
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("Continue?"));
        assert!(rendered.contains("Yes"));
        assert!(rendered.contains("No"));
        assert!(
            rendered.contains("Something else"),
            "boolean question should have 'Something else' option"
        );
    }

    #[test]
    fn chat_question_template_renders_freeform_question() {
        let tmpl = ChatQuestionTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "chat-transcript".to_string(),
            pending_question: Some(QuestionData::Freeform {
                question_id: "01HQID".to_string(),
                question: "Describe the goal".to_string(),
                placeholder: "Enter goal...".to_string(),
            }),
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("Describe the goal"));
        assert!(rendered.contains("Enter goal..."));
    }

    #[test]
    fn chat_question_template_renders_multiple_choice() {
        let tmpl = ChatQuestionTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "chat-transcript".to_string(),
            pending_question: Some(QuestionData::MultipleChoice {
                question_id: "01HQID".to_string(),
                question: "Pick a language".to_string(),
                choices: vec!["Rust".to_string(), "Python".to_string()],
                allow_multi: false,
            }),
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("Pick a language"));
        assert!(rendered.contains("Rust"));
        assert!(rendered.contains("Python"));
    }

    #[test]
    fn chat_question_template_targets_question_container() {
        let tmpl = ChatQuestionTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "chat-transcript".to_string(),
            pending_question: Some(QuestionData::Boolean {
                question_id: "01HQID".to_string(),
                question: "Proceed?".to_string(),
                default: None,
            }),
        };
        let rendered = tmpl.render().unwrap();
        assert!(
            rendered.contains(r##"hx-target="#chat-transcript-question""##),
            "answer form should target question container, not full transcript: {}",
            rendered
        );
    }

    #[test]
    fn chat_question_template_contains_part_question_in_hx_get() {
        let tmpl = ChatQuestionTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "chat-transcript".to_string(),
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(
            rendered.contains("part=question"),
            "question hx-get should include part=question param"
        );
    }

    #[test]
    fn chat_question_boolean_has_options_set_wrapper() {
        let tmpl = ChatQuestionTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "chat-transcript".to_string(),
            pending_question: Some(QuestionData::Boolean {
                question_id: "01HQID".to_string(),
                question: "Continue?".to_string(),
                default: None,
            }),
        };
        let rendered = tmpl.render().unwrap();
        assert!(
            rendered.contains("chat-options-set"),
            "boolean question should wrap options in chat-options-set: {}",
            rendered
        );
        assert!(
            rendered.contains("chat-else-back"),
            "boolean question should have a back button: {}",
            rendered
        );
    }

    #[test]
    fn chat_question_multiple_choice_has_options_set_wrapper() {
        let tmpl = ChatQuestionTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "chat-transcript".to_string(),
            pending_question: Some(QuestionData::MultipleChoice {
                question_id: "01HQID".to_string(),
                question: "Pick one".to_string(),
                choices: vec!["A".to_string(), "B".to_string()],
                allow_multi: false,
            }),
        };
        let rendered = tmpl.render().unwrap();
        assert!(
            rendered.contains("chat-options-set"),
            "multiple choice question should wrap options in chat-options-set: {}",
            rendered
        );
        assert!(
            rendered.contains("chat-else-back"),
            "multiple choice question should have a back button: {}",
            rendered
        );
    }

    #[test]
    fn chat_transcript_wrapper_defines_toggle_else_helper() {
        let tmpl = ChatTranscriptTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "chat-transcript".to_string(),
            transcript: vec![],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(
            rendered.contains("function toggleElse"),
            "wrapper script should define toggleElse helper function: {}",
            rendered
        );
    }

    #[test]
    fn chat_transcript_template_includes_feed_and_question() {
        let tmpl = ChatTranscriptTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "chat-transcript".to_string(),
            transcript: vec![TranscriptEntry {
                sender: "human".to_string(),
                sender_label: "You".to_string(),
                initial: "Y".to_string(),
                is_human: true,
                is_step: false,
                is_continuation: false,
                role_class: "human".to_string(),
                content: "Test message".to_string(),
                content_html: "<p>Test message</p>\n".to_string(),
                timestamp: "12:00:00".to_string(),
                repeat_count: 1,
            }],
            pending_question: Some(QuestionData::Boolean {
                question_id: "01HQID".to_string(),
                question: "Ready?".to_string(),
                default: None,
            }),
        };
        let rendered = tmpl.render().unwrap();
        // Wrapper container
        assert!(
            rendered.contains(r#"id="chat-transcript""#),
            "should have wrapper container id"
        );
        // Feed sub-container
        assert!(
            rendered.contains(r#"id="chat-transcript-feed""#),
            "should include feed sub-container"
        );
        // Question sub-container
        assert!(
            rendered.contains(r#"id="chat-transcript-question""#),
            "should include question sub-container"
        );
        // Content from both
        assert!(rendered.contains("Test message"), "should contain transcript entry");
        assert!(rendered.contains("Ready?"), "should contain question");
    }

    #[test]
    fn chat_transcript_template_feed_and_question_have_independent_triggers() {
        let tmpl = ChatTranscriptTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "chat-transcript".to_string(),
            transcript: vec![],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        // The wrapper div itself should NOT have hx-trigger (only children do)
        // Feed triggers on transcript_appended only
        assert!(
            rendered.contains("sse:transcript_appended"),
            "feed should have transcript_appended trigger"
        );
        // Question triggers on question_asked and question_answered
        assert!(
            rendered.contains("sse:question_asked"),
            "question should have question_asked trigger"
        );
        assert!(
            rendered.contains("sse:question_answered"),
            "question should have question_answered trigger"
        );
    }

    // ---- Handler tests for part=feed and part=question ----

    #[tokio::test]
    async fn activity_transcript_with_part_feed_returns_feed_only() {
        let state = test_state();

        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::post("/web/specs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("description=Feed+part+test"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().expect("should have a spec")
        };

        let app2 = create_router(Arc::clone(&state), None);
        let resp = app2
            .oneshot(
                Request::get(format!(
                    "/web/specs/{}/activity/transcript?container_id=chat-transcript&part=feed",
                    spec_id
                ))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains(r#"id="chat-transcript-feed""#),
            "should return feed container: {}",
            html
        );
        assert!(
            !html.contains(r#"id="chat-transcript-question""#),
            "should not include question container in feed-only response"
        );
    }

    #[tokio::test]
    async fn activity_transcript_with_part_question_returns_question_only() {
        let state = test_state();

        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::post("/web/specs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("description=Question+part+test"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().expect("should have a spec")
        };

        let app2 = create_router(Arc::clone(&state), None);
        let resp = app2
            .oneshot(
                Request::get(format!(
                    "/web/specs/{}/activity/transcript?container_id=chat-transcript&part=question",
                    spec_id
                ))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains(r#"id="chat-transcript-question""#),
            "should return question container: {}",
            html
        );
        assert!(
            !html.contains(r#"id="chat-transcript-feed""#),
            "should not include feed container in question-only response"
        );
    }

    #[tokio::test]
    async fn post_answer_targeting_question_container_returns_question_only() {
        let state = test_state();

        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::post("/web/specs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("description=Answer+question+target+test"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().expect("should have a spec")
        };

        // Ask a question so we can answer it
        let question_id = ulid::Ulid::new();
        {
            let actors = state.actors.read().await;
            let handle = actors.get(&spec_id).expect("actor should exist");
            handle
                .send_command(Command::AskQuestion {
                    question: barnstormer_core::UserQuestion::Freeform {
                        question_id,
                        question: "What color?".to_string(),
                        placeholder: None,
                        validation_hint: None,
                    },
                })
                .await
                .unwrap();
        }

        // POST to /answer with HX-Target: #chat-transcript-question
        let app2 = create_router(Arc::clone(&state), None);
        let resp = app2
            .oneshot(
                Request::post(format!("/web/specs/{}/answer", spec_id))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Target", "#chat-transcript-question")
                    .body(Body::from(format!(
                        "question_id={}&answer=Blue",
                        question_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains(r#"id="chat-transcript-question""#),
            "response should be the question container only: {}",
            html
        );
        assert!(
            !html.contains(r#"id="chat-transcript-feed""#),
            "response should not include the feed container"
        );
    }

    // ---- render_markdown tests ----

    #[test]
    fn render_markdown_paragraphs() {
        let result = render_markdown("Hello world");
        assert_eq!(result, "<p>Hello world</p>\n");
    }

    #[test]
    fn render_markdown_bold_and_italic() {
        let result = render_markdown("This is **bold** and *italic*");
        assert!(result.contains("<strong>bold</strong>"));
        assert!(result.contains("<em>italic</em>"));
    }

    #[test]
    fn render_markdown_multiline_paragraphs() {
        let result = render_markdown("First paragraph\n\nSecond paragraph");
        assert!(result.contains("<p>First paragraph</p>"));
        assert!(result.contains("<p>Second paragraph</p>"));
    }

    #[test]
    fn render_markdown_list() {
        let result = render_markdown("- item one\n- item two\n- item three");
        assert!(result.contains("<ul>"));
        assert!(result.contains("<li>item one</li>"));
        assert!(result.contains("<li>item three</li>"));
    }

    #[test]
    fn render_markdown_strips_raw_html() {
        let result = render_markdown("Hello <script>alert('xss')</script> world");
        assert!(!result.contains("<script>"), "raw HTML should be stripped");
        assert!(result.contains("Hello"));
        assert!(result.contains("world"));
    }

    #[test]
    fn render_markdown_code_block() {
        let result = render_markdown("```\nlet x = 1;\n```");
        assert!(result.contains("<code>"));
        assert!(result.contains("let x = 1;"));
    }

    #[tokio::test]
    async fn phase_transition_to_refining_returns_200() {
        let state = test_state();
        let app = create_router(Arc::clone(&state), None);
        app.oneshot(
            Request::post("/web/specs")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("description=Phase+test+spec"))
                .unwrap(),
        )
        .await
        .unwrap();

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().unwrap()
        };

        let app2 = create_router(Arc::clone(&state), None);
        let resp = app2
            .oneshot(
                Request::post(&format!("/web/specs/{}/phase", spec_id))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("target=refining"))
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
        )
        .await
        .unwrap();

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().unwrap()
        };

        // First transition to Refining
        let app2 = create_router(Arc::clone(&state), None);
        app2.oneshot(
            Request::post(&format!("/web/specs/{}/phase", spec_id))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("target=refining"))
                .unwrap(),
        )
        .await
        .unwrap();

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
        )
        .await
        .unwrap();

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
        )
        .await
        .unwrap();

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().unwrap()
        };

        // Transition to refining first
        let app2 = create_router(Arc::clone(&state), None);
        app2.oneshot(
            Request::post(&format!("/web/specs/{}/phase", spec_id))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("target=refining"))
                .unwrap(),
        )
        .await
        .unwrap();

        // Try refining again — 409
        let app3 = create_router(Arc::clone(&state), None);
        let resp = app3
            .oneshot(
                Request::post(&format!("/web/specs/{}/phase", spec_id))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("target=refining"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn phase_transition_nonexistent_spec_returns_404() {
        let state = test_state();
        let fake_id = ulid::Ulid::new();
        let app = create_router(state, None);
        let resp = app
            .oneshot(
                Request::post(&format!("/web/specs/{}/phase", fake_id))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("target=refining"))
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
        )
        .await
        .unwrap();

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
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.get("phase").is_some());
    }

    #[tokio::test]
    async fn spec_view_brainstorming_contains_phase_marker() {
        let state = test_state();
        let app = create_router(Arc::clone(&state), None);
        app.oneshot(
            Request::post("/web/specs")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("description=Brainstorming+UI+test"))
                .unwrap(),
        )
        .await
        .unwrap();

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().unwrap()
        };

        let app2 = create_router(Arc::clone(&state), None);
        let resp = app2
            .oneshot(
                Request::get(&format!("/web/specs/{}", spec_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("data-view=\"brainstorming\""),
            "should have brainstorming marker"
        );
        assert!(
            html.contains("phase-stepper"),
            "should have phase stepper"
        );
        assert!(
            html.contains("step-active"),
            "should have active stepper step"
        );
        assert!(
            html.contains("agent-canvas"),
            "should have agent-canvas container"
        );
        assert!(
            !html.contains("view-toggles-row"),
            "brainstorming should not have view toggles row"
        );
    }

    #[tokio::test]
    async fn spec_view_refining_contains_tab_toggles() {
        let state = test_state();
        let app = create_router(Arc::clone(&state), None);
        app.oneshot(
            Request::post("/web/specs")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("description=Active+UI+test"))
                .unwrap(),
        )
        .await
        .unwrap();

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().unwrap()
        };

        // Transition to Refining
        let actors = state.actors.read().await;
        let handle = actors.get(&spec_id).unwrap();
        handle
            .send_command(Command::TransitionPhase {
                target: SpecPhase::Refining,
            })
            .await
            .unwrap();
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
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("data-view=\"document\""),
            "should have document tab toggle"
        );
        assert!(
            html.contains("view-toggles-row"),
            "refining should have view toggles row"
        );
        assert!(
            html.contains("phase-stepper"),
            "should have phase stepper"
        );
        assert!(
            html.contains("step-completed"),
            "brainstorming step should be completed in refining phase"
        );
        assert!(
            !html.contains("data-view=\"brainstorming\""),
            "should not have brainstorming marker"
        );
    }

    #[tokio::test]
    async fn board_returns_200_during_brainstorming() {
        let state = test_state();
        let app = create_router(Arc::clone(&state), None);
        app.oneshot(
            Request::post("/web/specs")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("description=Board+peek+test"))
                .unwrap(),
        )
        .await
        .unwrap();

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

    #[tokio::test]
    async fn spec_view_brainstorming_contains_canvas_listener() {
        let state = test_state();
        let app = create_router(Arc::clone(&state), None);
        app.oneshot(
            Request::post("/web/specs")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("description=Canvas+listener+test"))
                .unwrap(),
        )
        .await
        .unwrap();

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().unwrap()
        };

        let app2 = create_router(Arc::clone(&state), None);
        let resp = app2
            .oneshot(
                Request::get(&format!("/web/specs/{}", spec_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("sse:canvas_updated"),
            "should contain canvas_updated SSE listener"
        );
        assert!(
            html.contains("agent-canvas"),
            "should contain agent-canvas element"
        );
    }

    #[tokio::test]
    async fn spec_view_prepopulates_canvas_content() {
        let state = test_state();
        let app = create_router(Arc::clone(&state), None);
        app.oneshot(
            Request::post("/web/specs")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("description=Canvas+prepopulate+test"))
                .unwrap(),
        )
        .await
        .unwrap();

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().unwrap()
        };

        // Send UpdateCanvas command to set canvas content and a question so canvas is visible
        {
            let actors = state.actors.read().await;
            let handle = actors.get(&spec_id).unwrap();
            handle
                .send_command(Command::UpdateCanvas {
                    content: "<p>Canvas pre-populated content</p>".to_string(),
                })
                .await
                .unwrap();
            handle
                .send_command(Command::AskQuestion {
                    question: barnstormer_core::UserQuestion::Boolean {
                        question_id: ulid::Ulid::new(),
                        question: "Test?".to_string(),
                        default: Some(true),
                    },
                })
                .await
                .unwrap();
        }

        let app2 = create_router(Arc::clone(&state), None);
        let resp = app2
            .oneshot(
                Request::get(&format!("/web/specs/{}", spec_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("<p>Canvas pre-populated content</p>"),
            "should contain pre-populated canvas content"
        );
        assert!(
            html.contains("display:block"),
            "canvas should be visible when content is present"
        );
    }

    #[tokio::test]
    async fn state_api_includes_canvas_content() {
        let state = test_state();
        let app = create_router(Arc::clone(&state), None);
        app.oneshot(
            Request::post("/web/specs")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("description=Canvas+state+test"))
                .unwrap(),
        )
        .await
        .unwrap();

        let spec_id = {
            let actors = state.actors.read().await;
            *actors.keys().next().unwrap()
        };

        {
            let actors = state.actors.read().await;
            let handle = actors.get(&spec_id).unwrap();
            handle
                .send_command(Command::UpdateCanvas {
                    content: "<p>Check</p>".to_string(),
                })
                .await
                .unwrap();
        }

        let app2 = create_router(Arc::clone(&state), None);
        let resp = app2
            .oneshot(
                Request::get(&format!("/api/specs/{}/state", spec_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json.get("canvas_content").and_then(|v| v.as_str()),
            Some("<p>Check</p>")
        );
    }
}
