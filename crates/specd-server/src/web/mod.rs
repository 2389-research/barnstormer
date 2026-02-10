// ABOUTME: Web UI route handlers serving HTML via Askama templates and HTMX.
// ABOUTME: Provides browser-friendly views for spec management, board, documents, and activity.

use std::sync::Arc;

use axum::extract::{Form, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;
use specd_agent::SwarmOrchestrator;
use specd_core::{Command, SpecState, spawn};
use specd_store::{JsonlLog, SnapshotData, save_snapshot};
use chrono::Utc;
use ulid::Ulid;

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

/// Form data for creating a new spec.
#[derive(Deserialize)]
pub struct CreateSpecForm {
    pub title: String,
    pub one_liner: String,
    pub goal: String,
}

/// POST /web/specs - Create a spec from form data, return updated spec list.
pub async fn create_spec(
    State(state): State<SharedState>,
    Form(form): Form<CreateSpecForm>,
) -> impl IntoResponse {
    let spec_id = Ulid::new();
    let spec_dir = state.specd_home.join("specs").join(spec_id.to_string());
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
            title: form.title,
            one_liner: form.one_liner,
            goal: form.goal,
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

    // Subscribe the event persister BEFORE inserting the actor and starting
    // agents so it catches all subsequent events (agent-produced, etc.).
    // The CreateSpec events above were already persisted inline.
    let persister_handle = spawn_event_persister(&handle, spec_id, &state.specd_home);
    state.event_persisters.write().await.insert(spec_id, persister_handle);

    state.actors.write().await.insert(spec_id, handle);

    // Auto-start agents if a provider is available
    {
        let actors = state.actors.read().await;
        if let Some(handle_ref) = actors.get(&spec_id) {
            try_start_agents(&state, spec_id, handle_ref).await;
        }
    }

    // Return the updated spec list
    let actors = state.actors.read().await;
    let mut specs = Vec::new();
    for (sid, h) in actors.iter() {
        let ss = h.read_state().await;
        if let Some(ref core) = ss.core {
            specs.push(SpecSummary {
                spec_id: sid.to_string(),
                title: core.title.clone(),
                one_liner: core.one_liner.clone(),
                updated_at: core.updated_at.to_rfc3339(),
            });
        }
    }

    SpecListTemplate { specs }.into_response()
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
    let default_lanes = ["Ideas", "Plan", "Done"];
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
    pub lane: String,
    pub order: f64,
    pub created_by: String,
    pub updated_at: String,
}

impl CardData {
    fn from_card(card: &specd_core::Card) -> Self {
        Self {
            card_id: card.card_id.to_string(),
            card_type: card.card_type.clone(),
            title: card.title.clone(),
            body: card.body.clone(),
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
    pub lanes: Vec<LaneData>,
}

/// GET /web/specs/{id} - Render the full spec view (board + right rail).
pub async fn spec_view(
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

    SpecViewTemplate {
        spec_id: id,
        title: core.title.clone(),
        one_liner: core.one_liner.clone(),
        goal: core.goal.clone(),
        lanes,
    }
    .into_response()
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
    pub description: Option<String>,
    pub constraints: Option<String>,
    pub success_criteria: Option<String>,
    pub risks: Option<String>,
    pub notes: Option<String>,
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
        description: core.description.clone(),
        constraints: core.constraints.clone(),
        success_criteria: core.success_criteria.clone(),
        risks: core.risks.clone(),
        notes: core.notes.clone(),
        lanes,
    }
    .into_response()
}

/// Activity transcript data for templates.
pub struct TranscriptEntry {
    pub sender: String,
    pub sender_label: String,
    pub is_human: bool,
    pub role_class: String,
    pub content: String,
    pub timestamp: String,
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
        "manager" => "Manager",
        "brainstormer" => "Brainstormer",
        "planner" => "Planner",
        "dot_generator" => "Dot Generator",
        "critic" => "Critic",
        _ => role,
    };
    let mut capitalized = String::new();
    for (i, ch) in label.chars().enumerate() {
        if i == 0 {
            capitalized.extend(ch.to_uppercase());
        } else {
            capitalized.push(ch);
        }
    }
    let role_class = normalize_css_class(role);
    (capitalized, false, role_class)
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
fn question_to_view_data(q: &specd_core::UserQuestion) -> QuestionData {
    match q {
        specd_core::UserQuestion::Boolean {
            question_id,
            question,
            default,
        } => QuestionData::Boolean {
            question_id: question_id.to_string(),
            question: question.clone(),
            default: *default,
        },
        specd_core::UserQuestion::MultipleChoice {
            question_id,
            question,
            choices,
            allow_multi,
        } => QuestionData::MultipleChoice {
            question_id: question_id.to_string(),
            question: question.clone(),
            choices: choices.clone(),
            allow_multi: *allow_multi,
        },
        specd_core::UserQuestion::Freeform {
            question_id,
            question,
            placeholder,
            ..
        } => QuestionData::Freeform {
            question_id: question_id.to_string(),
            question: question.clone(),
            placeholder: placeholder.clone().unwrap_or_default(),
        },
    }
}

/// Query parameters for the transcript endpoint, allowing callers to specify
/// which container the response should target (activity panel vs chat tab).
#[derive(Deserialize)]
pub struct TranscriptQuery {
    pub container_id: Option<String>,
}

/// Validate and sanitize a container_id value. Only known IDs are accepted;
/// anything else falls back to "activity-transcript" to prevent XSS via
/// user-controlled values rendered into script tags and HTMX attributes.
fn sanitize_container_id(raw: &str) -> String {
    match raw {
        "activity-transcript" | "chat-transcript" => raw.to_string(),
        _ => "activity-transcript".to_string(),
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
/// both the right-rail activity panel and the full-width chat tab.
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

    let transcript: Vec<TranscriptEntry> = spec_state
        .transcript
        .iter()
        .map(|m| {
            let (sender_label, is_human, role_class) = sender_display(&m.sender);
            TranscriptEntry {
                sender: m.sender.clone(),
                sender_label,
                is_human,
                role_class,
                content: m.content.clone(),
                timestamp: m.timestamp.format("%H:%M:%S").to_string(),
            }
        })
        .collect();

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

    let transcript: Vec<TranscriptEntry> = spec_state
        .transcript
        .iter()
        .map(|m| {
            let (sender_label, is_human, role_class) = sender_display(&m.sender);
            TranscriptEntry {
                sender: m.sender.clone(),
                sender_label,
                is_human,
                role_class,
                content: m.content.clone(),
                timestamp: m.timestamp.format("%H:%M:%S").to_string(),
            }
        })
        .collect();

    let pending_question = spec_state.pending_question.as_ref().map(question_to_view_data);

    let container_id = sanitize_container_id(
        query.container_id.as_deref().unwrap_or("activity-transcript"),
    );

    if container_id == "chat-transcript" {
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

    let transcript: Vec<TranscriptEntry> = spec_state
        .transcript
        .iter()
        .map(|m| {
            let (sender_label, is_human, role_class) = sender_display(&m.sender);
            TranscriptEntry {
                sender: m.sender.clone(),
                sender_label,
                is_human,
                role_class,
                content: m.content.clone(),
                timestamp: m.timestamp.format("%H:%M:%S").to_string(),
            }
        })
        .collect();

    let pending_question = spec_state.pending_question.as_ref().map(question_to_view_data);

    ChatPanelTemplate {
        spec_id: id,
        container_id: "chat-transcript".to_string(),
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

    let markdown_content = specd_core::export::export_markdown(&spec_state);
    let yaml_content = specd_core::export::export_yaml(&spec_state).unwrap_or_else(|e| {
        format!("# YAML export error: {}", e)
    });
    let dot_content = specd_core::export::export_dot(&spec_state);

    ArtifactsTemplate {
        spec_id: id,
        markdown_content,
        yaml_content,
        dot_content,
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
    let content = specd_core::export::export_markdown(&spec_state);

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
    match specd_core::export::export_yaml(&spec_state) {
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
    let content = specd_core::export::export_dot(&spec_state);

    Response::builder()
        .header("content-type", "text/plain")
        .header("content-disposition", "attachment; filename=\"spec.dot\"")
        .body(axum::body::Body::from(content))
        .unwrap()
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
    let transcript: Vec<TranscriptEntry> = spec_state
        .transcript
        .iter()
        .map(|m| {
            let (sender_label, is_human, role_class) = sender_display(&m.sender);
            TranscriptEntry {
                sender: m.sender.clone(),
                sender_label,
                is_human,
                role_class,
                content: m.content.clone(),
                timestamp: m.timestamp.format("%H:%M:%S").to_string(),
            }
        })
        .collect();

    if container_id == "chat-transcript" {
        ChatTranscriptTemplate {
            spec_id: id,
            container_id,
            transcript,
            pending_question: None,
        }
        .into_response()
    } else {
        ActivityTranscriptTemplate {
            spec_id: id,
            container_id,
            transcript,
            pending_question: None,
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
    let transcript: Vec<TranscriptEntry> = spec_state
        .transcript
        .iter()
        .map(|m| {
            let (sender_label, is_human, role_class) = sender_display(&m.sender);
            TranscriptEntry {
                sender: m.sender.clone(),
                sender_label,
                is_human,
                role_class,
                content: m.content.clone(),
                timestamp: m.timestamp.format("%H:%M:%S").to_string(),
            }
        })
        .collect();

    let pending_question = spec_state.pending_question.as_ref().map(question_to_view_data);

    if container_id == "chat-transcript" {
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

/// Agent status partial template.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/agent_status.html")]
pub struct AgentStatusTemplate {
    pub spec_id: String,
    pub running: bool,
    pub started: bool,
    pub agent_count: usize,
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
    let task = tokio::spawn(specd_agent::run_loop(Arc::clone(&swarm)));

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
pub async fn try_start_agents(state: &SharedState, spec_id: Ulid, actor_handle: &specd_core::SpecActorHandle) {
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
    let task = tokio::spawn(specd_agent::run_loop(Arc::clone(&swarm)));

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
    actor: &specd_core::SpecActorHandle,
    spec_id: Ulid,
    specd_home: &std::path::Path,
) -> tokio::task::JoinHandle<()> {
    let mut rx = actor.subscribe();
    let actor_handle = actor.clone();
    let log_path = specd_home
        .join("specs")
        .join(spec_id.to_string())
        .join("events.jsonl");
    let snapshot_dir = specd_home
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
        assert!(rendered.contains("specd"));
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
        assert!(rendered.contains("title"));
        assert!(rendered.contains("one_liner"));
        assert!(rendered.contains("goal"));
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
                    name: "Done".to_string(),
                    cards: vec![],
                },
            ],
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("Ideas"));
        assert!(rendered.contains("Plan"));
        assert!(rendered.contains("Done"));
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
            description: Some("A detailed description".to_string()),
            constraints: None,
            success_criteria: None,
            risks: None,
            notes: None,
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
                is_human: false,
                role_class: "agent".to_string(),
                content: "Started analysis".to_string(),
                timestamp: "12:34:56".to_string(),
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
        assert!(html.contains("specd"));
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
    async fn post_web_specs_creates_and_returns_list() {
        let state = test_state();
        let app = create_router(Arc::clone(&state), None);

        let resp = app
            .oneshot(
                Request::post("/web/specs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("title=Test+Spec&one_liner=A+test&goal=Build+it"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Test Spec"));
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
                is_human: false,
                role_class: "agent".to_string(),
                content: "Started analysis".to_string(),
                timestamp: "12:34:56".to_string(),
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
                is_human: true,
                role_class: "human".to_string(),
                content: "Hello chat".to_string(),
                timestamp: "12:00:00".to_string(),
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
    fn spec_view_template_contains_all_four_tabs() {
        let tmpl = SpecViewTemplate {
            spec_id: "01HTEST".to_string(),
            title: "Test Spec".to_string(),
            one_liner: "A test spec".to_string(),
            goal: "Test goal".to_string(),
            lanes: vec![],
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("Board"), "should contain Board tab");
        assert!(rendered.contains("Document"), "should contain Document tab");
        assert!(rendered.contains("Chat"), "should contain Chat tab");
        assert!(rendered.contains("Artifacts"), "should contain Artifacts tab");
        assert!(rendered.contains("data-tab=\"board\""), "Board tab should have data-tab attribute");
        assert!(rendered.contains("data-tab=\"document\""), "Document tab should have data-tab attribute");
        assert!(rendered.contains("data-tab=\"chat\""), "Chat tab should have data-tab attribute");
        assert!(rendered.contains("data-tab=\"artifacts\""), "Artifacts tab should have data-tab attribute");
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
                    .body(Body::from("title=Chat+Test&one_liner=Testing+chat&goal=Verify+chat"))
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
        assert!(rendered.contains("Agents stopped"), "should show stopped state");
        assert!(rendered.contains("Start Agents"), "should show start button");
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
        assert!(rendered.contains("Agents running (4)"), "should show running state with count");
        assert!(rendered.contains("Pause"), "should show pause button");
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
        assert!(rendered.contains("Agents paused"), "should show paused state");
        assert!(rendered.contains("Resume"), "should show resume button");
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
                    .body(Body::from("title=Agent+Test&one_liner=Testing+agents&goal=Verify+agents"))
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
        assert!(html.contains("Agents stopped"), "should show stopped when no swarm: {}", html);
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
                    .body(Body::from("title=Pause+Test&one_liner=Test&goal=Test"))
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
        assert!(html.contains("Agents stopped"), "pause with no swarm should show stopped: {}", html);
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
                    .body(Body::from("title=Resume+Test&one_liner=Test&goal=Test"))
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
        assert!(html.contains("Agents stopped"), "resume with no swarm should show stopped: {}", html);
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
        assert!(html.contains("Agents stopped"), "nonexistent spec should show stopped: {}", html);
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
                    .body(Body::from("title=No+Agent+Test&one_liner=No+agents&goal=Verify+no+agents"))
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
                    is_human: true,
                    role_class: "human".to_string(),
                    content: "Hello from human".to_string(),
                    timestamp: "12:34:56".to_string(),
                },
                TranscriptEntry {
                    sender: "manager-01HAGENT".to_string(),
                    sender_label: "Manager".to_string(),
                    is_human: false,
                    role_class: "manager".to_string(),
                    content: "Agent response here".to_string(),
                    timestamp: "12:35:00".to_string(),
                },
            ],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("Hello from human"), "should contain human message content");
        assert!(rendered.contains("Agent response here"), "should contain agent message content");
        assert!(rendered.contains("chat-row-human"), "should have human chat row class");
        assert!(rendered.contains("chat-row-agent"), "should have agent chat row class");
        assert!(rendered.contains("Manager"), "should show agent sender label");
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
        assert!(rendered.contains("Send a message..."), "should have placeholder text");
    }

    #[test]
    fn chat_panel_contains_sse_connection() {
        let tmpl = ChatPanelTemplate {
            spec_id: "01HTEST".to_string(),
            container_id: "chat-transcript".to_string(),
            transcript: vec![],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("sse-connect"), "should contain sse-connect attribute");
        assert!(rendered.contains(r#"sse-connect="/api/specs/01HTEST/events/stream""#), "should connect to correct SSE endpoint");
        assert!(rendered.contains(r#"hx-ext="sse""#), "should have hx-ext sse");
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
                    .body(Body::from("title=Chat+Panel+Test&one_liner=Test&goal=Test"))
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
                    .body(Body::from("title=Artifacts+Test&one_liner=Test&goal=Test"))
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

    // ---- Export download tests ----

    /// Helper to create a spec and return its ULID.
    async fn create_test_spec(state: &SharedState) -> ulid::Ulid {
        let app = create_router(Arc::clone(state), None);
        let resp = app
            .oneshot(
                Request::post("/web/specs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("title=Export+Test&one_liner=Test&goal=Test"))
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
    async fn activity_transcript_handler_defaults_container_id() {
        let state = test_state();

        // Create a spec
        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::post("/web/specs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("title=Container+Test&one_liner=Testing&goal=Verify"))
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
                    .body(Body::from("title=Container+Test&one_liner=Testing&goal=Verify"))
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
        assert_eq!(sanitize_container_id("'); alert('xss'); //"), "activity-transcript");
        assert_eq!(sanitize_container_id("malicious-id"), "activity-transcript");
        assert_eq!(sanitize_container_id(""), "activity-transcript");
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
        assert_eq!(label, "Manager");
        assert!(!is_human, "agent should not be flagged as human");
        assert_eq!(role_class, "manager");
    }

    #[test]
    fn sender_display_brainstormer_role() {
        let (label, is_human, role_class) = sender_display("brainstormer-01JTESTID456");
        assert_eq!(label, "Brainstormer");
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
                    .body(Body::from("title=HX+Chat+Test&one_liner=Test&goal=Test"))
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
                    .body(Body::from("title=Answer+Test&one_liner=Test&goal=Test"))
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
                    question: specd_core::UserQuestion::Freeform {
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
}
