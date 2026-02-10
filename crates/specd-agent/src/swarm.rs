// ABOUTME: SwarmOrchestrator manages multiple agents per spec, routing actions and enforcing question queue.
// ABOUTME: Each agent runs in its own tokio task, coordinated by pause/resume flags and event subscriptions.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::broadcast;
use tracing;
use ulid::Ulid;

use crate::context::{AgentContext, AgentRole};
use crate::providers::anthropic::AnthropicRuntime;
use crate::providers::gemini::GeminiRuntime;
use crate::providers::openai::OpenAIRuntime;
use crate::runtime::{AgentAction, AgentError, AgentRuntime};
use specd_core::actor::SpecActorHandle;
use specd_core::command::Command;
use specd_core::event::Event;

/// Wraps a single agent's runtime, role, and mutable context.
pub struct AgentRunner {
    pub role: AgentRole,
    pub runtime: Box<dyn AgentRuntime>,
    pub context: AgentContext,
}

impl AgentRunner {
    /// Create a new runner for the given role and runtime.
    pub fn new(spec_id: Ulid, role: AgentRole, runtime: Box<dyn AgentRuntime>) -> Self {
        let agent_id = format!("{}-{}", role.label(), Ulid::new());
        let context = AgentContext::new(spec_id, agent_id, role);
        Self {
            role,
            runtime,
            context,
        }
    }
}

/// Orchestrates a swarm of agents working on a single spec.
/// Manages the agent loop, action routing, pause/resume, and question queue.
pub struct SwarmOrchestrator {
    pub spec_id: Ulid,
    pub actor: Arc<SpecActorHandle>,
    pub agents: Vec<AgentRunner>,
    pub paused: Arc<AtomicBool>,
    pub question_pending: Arc<AtomicBool>,
}

impl SwarmOrchestrator {
    /// Create a new orchestrator with default agents for the given spec.
    /// Uses the default provider (from env or "anthropic") and model.
    pub fn with_defaults(spec_id: Ulid, actor: SpecActorHandle) -> Result<Self, AgentError> {
        let provider =
            std::env::var("SPECD_DEFAULT_PROVIDER").unwrap_or_else(|_| "anthropic".to_string());
        let model = std::env::var("SPECD_DEFAULT_MODEL").ok();

        let actor = Arc::new(actor);

        let roles = [
            AgentRole::Manager,
            AgentRole::Brainstormer,
            AgentRole::Planner,
            AgentRole::DotGenerator,
        ];

        let mut agents = Vec::new();
        for role in &roles {
            let runtime = create_runtime(&provider, model.as_deref())?;
            agents.push(AgentRunner::new(spec_id, *role, runtime));
        }

        Ok(Self {
            spec_id,
            actor,
            agents,
            paused: Arc::new(AtomicBool::new(false)),
            question_pending: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Create an orchestrator with a specific set of agent runners.
    pub fn with_agents(spec_id: Ulid, actor: SpecActorHandle, agents: Vec<AgentRunner>) -> Self {
        Self {
            spec_id,
            actor: Arc::new(actor),
            agents,
            paused: Arc::new(AtomicBool::new(false)),
            question_pending: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Pause all agent loops. Agents will complete their current step
    /// but won't start new ones.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        tracing::info!(spec_id = %self.spec_id, "swarm paused");
    }

    /// Resume agent loops.
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        tracing::info!(spec_id = %self.spec_id, "swarm resumed");
    }

    /// Returns true if the swarm is currently paused.
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// Returns true if a question is currently pending for the user.
    pub fn has_pending_question(&self) -> bool {
        self.question_pending.load(Ordering::SeqCst)
    }

    /// Process a single AgentAction produced by an agent, submitting commands
    /// to the actor and managing the question queue. Returns true if the agent
    /// should continue working, false if it should idle.
    pub async fn process_action(
        actor: &SpecActorHandle,
        action: AgentAction,
        agent_id: &str,
        question_pending: &AtomicBool,
    ) -> bool {
        match action {
            AgentAction::EmitNarration(text) => {
                let cmd = Command::AppendTranscript {
                    sender: agent_id.to_string(),
                    content: text,
                };
                if let Err(e) = actor.send_command(cmd).await {
                    tracing::warn!(agent = agent_id, error = %e, "failed to emit narration");
                }
                true
            }

            AgentAction::WriteCommands(commands) => {
                for cmd in commands {
                    if let Err(e) = actor.send_command(cmd).await {
                        tracing::warn!(agent = agent_id, error = %e, "failed to write command");
                    }
                }
                true
            }

            AgentAction::AskUser(question) => {
                // Only one pending question at a time
                if question_pending.load(Ordering::SeqCst) {
                    tracing::debug!(agent = agent_id, "question already pending, skipping ask");
                    // Create an assumption card instead
                    let assumption_cmd = Command::CreateCard {
                        card_type: "assumption".to_string(),
                        title: format!("Assumed answer (question queued by {})", agent_id),
                        body: None,
                        lane: Some("Ideas".to_string()),
                        created_by: agent_id.to_string(),
                    };
                    if let Err(e) = actor.send_command(assumption_cmd).await {
                        tracing::warn!(
                            agent = agent_id,
                            error = %e,
                            "failed to create assumption card"
                        );
                    }
                    true
                } else {
                    question_pending.store(true, Ordering::SeqCst);
                    let cmd = Command::AskQuestion { question };
                    if let Err(e) = actor.send_command(cmd).await {
                        tracing::warn!(agent = agent_id, error = %e, "failed to ask question");
                        question_pending.store(false, Ordering::SeqCst);
                    }
                    true
                }
            }

            AgentAction::AskAgent {
                agent_id: target_agent_id,
                question,
            } => {
                // Route as a transcript message addressed to the target agent
                let cmd = Command::AppendTranscript {
                    sender: agent_id.to_string(),
                    content: format!("@{}: {}", target_agent_id, question),
                };
                if let Err(e) = actor.send_command(cmd).await {
                    tracing::warn!(agent = agent_id, error = %e, "failed to route inter-agent message");
                }
                true
            }

            AgentAction::EmitDiffSummary(summary) => {
                let cmd = Command::FinishAgentStep {
                    agent_id: agent_id.to_string(),
                    diff_summary: summary,
                };
                if let Err(e) = actor.send_command(cmd).await {
                    tracing::warn!(agent = agent_id, error = %e, "failed to emit diff summary");
                }
                true
            }

            AgentAction::Done => {
                tracing::debug!(agent = agent_id, "agent signaled done, going idle");
                false
            }
        }
    }

    /// Run a single agent step: call the runtime, process the action.
    /// Returns true if the agent should continue, false to idle.
    pub async fn run_single_step(
        runner: &mut AgentRunner,
        actor: &SpecActorHandle,
        question_pending: &AtomicBool,
    ) -> bool {
        // Start agent step
        let start_cmd = Command::StartAgentStep {
            agent_id: runner.context.agent_id.clone(),
            description: format!("{} reasoning step", runner.role.label()),
        };
        if let Err(e) = actor.send_command(start_cmd).await {
            tracing::warn!(
                agent = %runner.context.agent_id,
                error = %e,
                "failed to start agent step"
            );
        }

        match runner.runtime.run_step(&runner.context).await {
            Ok(action) => {
                Self::process_action(actor, action, &runner.context.agent_id, question_pending)
                    .await
            }
            Err(AgentError::RateLimited) => {
                tracing::warn!(
                    agent = %runner.context.agent_id,
                    "rate limited, will retry"
                );
                // Signal to continue (caller can add backoff)
                true
            }
            Err(e) => {
                tracing::error!(
                    agent = %runner.context.agent_id,
                    error = %e,
                    "agent step failed"
                );
                false
            }
        }
    }

    /// Update an agent's context from the current actor state.
    pub async fn refresh_context(
        runner: &mut AgentRunner,
        actor: &SpecActorHandle,
        event_rx: &mut broadcast::Receiver<Event>,
    ) {
        // Drain any buffered events
        let mut events = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            events.push(event);
        }
        runner.context.update_from_events(&events);
        runner.context.recent_events = events;

        // Read current state for the summary
        let state = actor.read_state().await;
        if let Some(ref core) = state.core {
            runner.context.state_summary = format!(
                "Title: {}. Goal: {}. Cards: {}. Pending question: {}",
                core.title,
                core.goal,
                state.cards.len(),
                state.pending_question.is_some()
            );
        }

        // Copy recent transcript
        let transcript_len = state.transcript.len();
        let start = transcript_len.saturating_sub(10);
        runner.context.recent_transcript = state.transcript[start..].to_vec();
    }
}

/// Create a runtime for the given provider name and optional model override.
pub fn create_runtime(
    provider: &str,
    model: Option<&str>,
) -> Result<Box<dyn AgentRuntime>, AgentError> {
    match provider {
        "anthropic" => {
            let api_key = std::env::var("ANTHROPIC_API_KEY")
                .map_err(|_| AgentError::ProviderError("ANTHROPIC_API_KEY not set".to_string()))?;
            let base_url = std::env::var("ANTHROPIC_BASE_URL")
                .unwrap_or_else(|_| "https://api.anthropic.com".to_string());
            let model_str = model
                .map(String::from)
                .or_else(|| std::env::var("ANTHROPIC_MODEL").ok())
                .unwrap_or_else(|| "claude-sonnet-4-5-20250929".to_string());
            Ok(Box::new(AnthropicRuntime::new(
                api_key, base_url, model_str,
            )))
        }

        "openai" => {
            let api_key = std::env::var("OPENAI_API_KEY")
                .map_err(|_| AgentError::ProviderError("OPENAI_API_KEY not set".to_string()))?;
            let base_url = std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".to_string());
            let model_str = model
                .map(String::from)
                .or_else(|| std::env::var("OPENAI_MODEL").ok())
                .unwrap_or_else(|| "gpt-4o".to_string());
            Ok(Box::new(OpenAIRuntime::new(api_key, base_url, model_str)))
        }

        "gemini" => {
            let api_key = std::env::var("GEMINI_API_KEY")
                .map_err(|_| AgentError::ProviderError("GEMINI_API_KEY not set".to_string()))?;
            let base_url = std::env::var("GEMINI_BASE_URL")
                .unwrap_or_else(|_| "https://generativelanguage.googleapis.com".to_string());
            let model_str = model
                .map(String::from)
                .or_else(|| std::env::var("GEMINI_MODEL").ok())
                .unwrap_or_else(|| "gemini-2.0-flash".to_string());
            Ok(Box::new(GeminiRuntime::new(api_key, base_url, model_str)))
        }

        other => Err(AgentError::ProviderError(format!(
            "unknown provider: {}",
            other
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use specd_core::state::SpecState;
    use std::sync::atomic::Ordering;

    fn make_test_actor() -> (Ulid, SpecActorHandle) {
        let spec_id = Ulid::new();
        let handle = specd_core::actor::spawn(spec_id, SpecState::new());
        (spec_id, handle)
    }

    /// A test runtime that always returns Done.
    struct StubRuntime;

    #[async_trait::async_trait]
    impl AgentRuntime for StubRuntime {
        async fn run_step(&self, _context: &AgentContext) -> Result<AgentAction, AgentError> {
            Ok(AgentAction::Done)
        }

        fn provider_name(&self) -> &str {
            "stub"
        }

        fn model_name(&self) -> &str {
            "stub-v1"
        }
    }

    #[tokio::test]
    async fn swarm_creates_default_agents() {
        // Use with_agents to test the default role configuration without env vars
        let (spec_id, actor) = make_test_actor();

        let roles = [
            AgentRole::Manager,
            AgentRole::Brainstormer,
            AgentRole::Planner,
            AgentRole::DotGenerator,
        ];

        let agents: Vec<AgentRunner> = roles
            .iter()
            .map(|role| AgentRunner::new(spec_id, *role, Box::new(StubRuntime)))
            .collect();

        let swarm = SwarmOrchestrator::with_agents(spec_id, actor, agents);

        assert_eq!(swarm.agents.len(), 4);
        assert_eq!(swarm.agents[0].role, AgentRole::Manager);
        assert_eq!(swarm.agents[1].role, AgentRole::Brainstormer);
        assert_eq!(swarm.agents[2].role, AgentRole::Planner);
        assert_eq!(swarm.agents[3].role, AgentRole::DotGenerator);

        assert!(!swarm.is_paused());
        assert!(!swarm.has_pending_question());
    }

    #[tokio::test]
    async fn swarm_pause_resume() {
        let (spec_id, actor) = make_test_actor();
        let swarm = SwarmOrchestrator::with_agents(spec_id, actor, Vec::new());

        assert!(!swarm.is_paused());

        swarm.pause();
        assert!(swarm.is_paused());

        swarm.resume();
        assert!(!swarm.is_paused());
    }

    #[test]
    fn create_runtime_selects_provider() {
        // SAFETY: These env var mutations are isolated to test code
        // and tests in this module run sequentially via test serialization.
        unsafe {
            // Test anthropic
            std::env::set_var("ANTHROPIC_API_KEY", "test-key");
            let runtime = create_runtime("anthropic", None).unwrap();
            assert_eq!(runtime.provider_name(), "anthropic");
            assert_eq!(runtime.model_name(), "claude-sonnet-4-5-20250929");
            std::env::remove_var("ANTHROPIC_API_KEY");

            // Test openai
            std::env::set_var("OPENAI_API_KEY", "test-key");
            let runtime = create_runtime("openai", None).unwrap();
            assert_eq!(runtime.provider_name(), "openai");
            assert_eq!(runtime.model_name(), "gpt-4o");
            std::env::remove_var("OPENAI_API_KEY");

            // Test gemini
            std::env::set_var("GEMINI_API_KEY", "test-key");
            let runtime = create_runtime("gemini", None).unwrap();
            assert_eq!(runtime.provider_name(), "gemini");
            assert_eq!(runtime.model_name(), "gemini-2.0-flash");
            std::env::remove_var("GEMINI_API_KEY");
        }

        // Test unknown provider (no env vars needed)
        let err = create_runtime("unknown_provider", None);
        assert!(err.is_err());
        match err {
            Err(e) => assert!(e.to_string().contains("unknown provider")),
            Ok(_) => panic!("expected error for unknown provider"),
        }
    }

    #[test]
    fn create_runtime_with_model_override() {
        // SAFETY: Isolated test env var mutation
        unsafe {
            std::env::set_var("ANTHROPIC_API_KEY", "test-key");
            let runtime = create_runtime("anthropic", Some("claude-opus-4-20250514")).unwrap();
            assert_eq!(runtime.model_name(), "claude-opus-4-20250514");
            std::env::remove_var("ANTHROPIC_API_KEY");
        }
    }

    #[tokio::test]
    async fn process_action_emit_narration() {
        let (_, actor) = make_test_actor();
        let question_pending = AtomicBool::new(false);

        let cont = SwarmOrchestrator::process_action(
            &actor,
            AgentAction::EmitNarration("Testing narration".to_string()),
            "test-agent",
            &question_pending,
        )
        .await;

        assert!(cont, "should continue after narration");

        // Check transcript was updated
        let state = actor.read_state().await;
        assert_eq!(state.transcript.len(), 1);
        assert_eq!(state.transcript[0].content, "Testing narration");
        assert_eq!(state.transcript[0].sender, "test-agent");
    }

    #[tokio::test]
    async fn process_action_done_returns_false() {
        let (_, actor) = make_test_actor();
        let question_pending = AtomicBool::new(false);

        let cont = SwarmOrchestrator::process_action(
            &actor,
            AgentAction::Done,
            "test-agent",
            &question_pending,
        )
        .await;

        assert!(!cont, "should stop after Done");
    }

    #[tokio::test]
    async fn process_action_ask_user_sets_pending() {
        let (_, actor) = make_test_actor();
        let question_pending = AtomicBool::new(false);

        let question = specd_core::transcript::UserQuestion::Boolean {
            question_id: Ulid::new(),
            question: "Continue?".to_string(),
            default: None,
        };

        let cont = SwarmOrchestrator::process_action(
            &actor,
            AgentAction::AskUser(question),
            "test-agent",
            &question_pending,
        )
        .await;

        assert!(cont);
        assert!(question_pending.load(Ordering::SeqCst));

        let state = actor.read_state().await;
        assert!(state.pending_question.is_some());
    }

    #[tokio::test]
    async fn process_action_ask_user_skips_when_pending() {
        let (_, actor) = make_test_actor();
        let question_pending = AtomicBool::new(true);

        let question = specd_core::transcript::UserQuestion::Boolean {
            question_id: Ulid::new(),
            question: "This should be skipped".to_string(),
            default: None,
        };

        let cont = SwarmOrchestrator::process_action(
            &actor,
            AgentAction::AskUser(question),
            "test-agent",
            &question_pending,
        )
        .await;

        assert!(cont);
        // Should have created an assumption card instead
        let state = actor.read_state().await;
        assert!(state.pending_question.is_none()); // No question was asked
        assert_eq!(state.cards.len(), 1);
        assert_eq!(state.cards.values().next().unwrap().card_type, "assumption");
    }

    #[tokio::test]
    async fn process_action_write_commands() {
        let (_, actor) = make_test_actor();
        let question_pending = AtomicBool::new(false);

        // Create spec first so we can create cards
        actor
            .send_command(Command::CreateSpec {
                title: "Test".to_string(),
                one_liner: "Test spec".to_string(),
                goal: "Testing".to_string(),
            })
            .await
            .unwrap();

        let commands = vec![Command::CreateCard {
            card_type: "idea".to_string(),
            title: "Test card".to_string(),
            body: None,
            lane: None,
            created_by: "test-agent".to_string(),
        }];

        let cont = SwarmOrchestrator::process_action(
            &actor,
            AgentAction::WriteCommands(commands),
            "test-agent",
            &question_pending,
        )
        .await;

        assert!(cont);
        let state = actor.read_state().await;
        assert_eq!(state.cards.len(), 1);
    }

    #[tokio::test]
    async fn run_single_step_with_stub_runtime() {
        let (spec_id, actor) = make_test_actor();
        let question_pending = AtomicBool::new(false);

        let mut runner = AgentRunner::new(spec_id, AgentRole::Brainstormer, Box::new(StubRuntime));

        let cont = SwarmOrchestrator::run_single_step(&mut runner, &actor, &question_pending).await;

        // StubRuntime returns Done, so agent should idle
        assert!(!cont);
    }

    #[tokio::test]
    async fn refresh_context_updates_state() {
        let (spec_id, actor) = make_test_actor();
        let mut event_rx = actor.subscribe();

        let mut runner = AgentRunner::new(spec_id, AgentRole::Manager, Box::new(StubRuntime));

        // Create a spec so there's state to read
        actor
            .send_command(Command::CreateSpec {
                title: "Context Test".to_string(),
                one_liner: "Testing context refresh".to_string(),
                goal: "Verify context update".to_string(),
            })
            .await
            .unwrap();

        SwarmOrchestrator::refresh_context(&mut runner, &actor, &mut event_rx).await;

        assert!(runner.context.state_summary.contains("Context Test"));
        assert!(runner.context.last_event_seen > 0);
    }
}
