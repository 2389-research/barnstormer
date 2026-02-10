// ABOUTME: Web UI route handlers serving HTML via Askama templates and HTMX.
// ABOUTME: Provides browser-friendly views for spec management, board, documents, and activity.

use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;
use specd_core::{Command, SpecState, spawn};
use specd_store::JsonlLog;
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

    state.actors.write().await.insert(spec_id, handle);

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
    let mut extra_lane_names: Vec<String> = spec_state
        .cards
        .values()
        .map(|c| c.lane.clone())
        .filter(|l| !default_lanes.contains(&l.as_str()))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    extra_lane_names.sort();

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

    let events = match handle.send_command(cmd).await {
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

    persist_events(&state, spec_id, &events);

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

    let events = match handle.send_command(cmd).await {
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

    persist_events(&state, spec_id, &events);

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

    let events = match handle.send_command(cmd).await {
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

    persist_events(&state, spec_id, &events);

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
    pub content: String,
    pub timestamp: String,
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

/// Activity panel template.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/activity.html")]
pub struct ActivityTemplate {
    pub spec_id: String,
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
        .map(|m| TranscriptEntry {
            sender: m.sender.clone(),
            content: m.content.clone(),
            timestamp: m.timestamp.format("%H:%M:%S").to_string(),
        })
        .collect();

    let pending_question = spec_state.pending_question.as_ref().map(|q| match q {
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
    });

    ActivityTemplate {
        spec_id: id,
        transcript,
        pending_question,
    }
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

    let events = match handle.send_command(cmd).await {
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

    persist_events(&state, spec_id, &events);

    // Return refreshed activity panel
    let spec_state = handle.read_state().await;
    let transcript: Vec<TranscriptEntry> = spec_state
        .transcript
        .iter()
        .map(|m| TranscriptEntry {
            sender: m.sender.clone(),
            content: m.content.clone(),
            timestamp: m.timestamp.format("%H:%M:%S").to_string(),
        })
        .collect();

    ActivityTemplate {
        spec_id: id,
        transcript,
        pending_question: None,
    }
    .into_response()
}

/// POST /web/specs/{id}/chat - Send a free-text message as the human.
pub async fn chat(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Form(form): Form<ChatForm>,
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

    let cmd = Command::AppendTranscript {
        sender: "human".to_string(),
        content: form.message,
    };

    let events = match handle.send_command(cmd).await {
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

    persist_events(&state, spec_id, &events);

    // Return refreshed activity panel
    let spec_state = handle.read_state().await;
    let transcript: Vec<TranscriptEntry> = spec_state
        .transcript
        .iter()
        .map(|m| TranscriptEntry {
            sender: m.sender.clone(),
            content: m.content.clone(),
            timestamp: m.timestamp.format("%H:%M:%S").to_string(),
        })
        .collect();

    let pending_question = spec_state.pending_question.as_ref().map(|q| match q {
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
    });

    ActivityTemplate {
        spec_id: id,
        transcript,
        pending_question,
    }
    .into_response()
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

    let events = match handle.send_command(Command::Undo).await {
        Ok(events) => events,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Html(format!("<p class=\"error-msg\">Undo failed: {}</p>", e)),
            )
                .into_response();
        }
    };

    persist_events(&state, spec_id, &events);

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

/// Helper to persist events to the JSONL log.
fn persist_events(state: &SharedState, spec_id: Ulid, events: &[specd_core::Event]) {
    let log_path = state
        .specd_home
        .join("specs")
        .join(spec_id.to_string())
        .join("events.jsonl");

    if let Ok(mut log) = JsonlLog::open(&log_path) {
        for event in events {
            if let Err(e) = log.append(event) {
                tracing::error!("failed to persist event: {}", e);
            }
        }
    }
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
        Arc::new(AppState::new(dir.keep(), ProviderStatus::detect()))
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
    }

    #[test]
    fn activity_template_renders_empty() {
        let tmpl = ActivityTemplate {
            spec_id: "01HTEST".to_string(),
            transcript: vec![],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("activity-feed"));
    }

    #[test]
    fn activity_template_renders_with_entries() {
        let tmpl = ActivityTemplate {
            spec_id: "01HTEST".to_string(),
            transcript: vec![TranscriptEntry {
                sender: "agent-1".to_string(),
                content: "Started analysis".to_string(),
                timestamp: "12:34:56".to_string(),
            }],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("agent-1"));
        assert!(rendered.contains("Started analysis"));
    }

    #[test]
    fn activity_template_renders_boolean_question() {
        let tmpl = ActivityTemplate {
            spec_id: "01HTEST".to_string(),
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
    fn activity_template_renders_chat_input() {
        let tmpl = ActivityTemplate {
            spec_id: "01HTEST".to_string(),
            transcript: vec![],
            pending_question: None,
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("chat-input"), "should contain chat input div");
        assert!(rendered.contains("Type a message"), "should contain placeholder text");
        assert!(rendered.contains("/chat"), "should contain chat form action");
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
                Request::post(&format!("/web/specs/{}/chat", spec_id))
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
                Request::post(&format!("/web/specs/{}/chat", fake_id))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("message=Hello"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
    }
}
