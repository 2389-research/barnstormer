// ABOUTME: SwarmOrchestrator manages multiple agents per spec, using mux SubAgent for LLM execution.
// ABOUTME: Each agent runs as a mux SubAgent with domain tools, coordinated by pause/resume flags and event subscriptions.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::broadcast;
use tracing;
use ulid::Ulid;

use mux::agent::{AgentDefinition, SubAgent};
use mux::llm::LlmClient;

use crate::client;
use crate::context::{AgentContext, AgentRole};
use crate::mux_tools;
use crate::runtime::AgentError;
use specd_core::actor::SpecActorHandle;
use specd_core::command::Command;
use specd_core::event::Event;

/// System prompt for the Manager agent role.
const MANAGER_SYSTEM_PROMPT: &str = "You are the manager agent for a product specification. \
    You coordinate the spec refinement process, ensure all aspects are covered, and ask the user \
    questions when clarification is needed. You have access to tools for reading state, writing \
    commands, asking questions, and narrating your reasoning.";

/// System prompt for the Brainstormer agent role.
const BRAINSTORMER_SYSTEM_PROMPT: &str = "You are the brainstormer agent. Your job is to generate \
    creative ideas, explore possibilities, and create idea cards. Focus on breadth over depth.";

/// System prompt for the Planner agent role.
const PLANNER_SYSTEM_PROMPT: &str = "You are the planner agent. Your job is to organize ideas into \
    structured plans, move cards between lanes, and ensure the spec has clear goals and constraints.";

/// System prompt for the DotGenerator agent role.
const DOT_GENERATOR_SYSTEM_PROMPT: &str = "You are the DOT diagram generator. Your job is to read \
    the current spec state and generate Graphviz DOT notation representing the spec's structure \
    and relationships.";

/// System prompt for the Critic agent role.
const CRITIC_SYSTEM_PROMPT: &str = "You are the critic agent. Your job is to review the spec for \
    gaps, inconsistencies, and potential issues. Provide constructive feedback and suggestions.";

/// Return the system prompt for a given agent role.
pub fn system_prompt_for_role(role: &AgentRole) -> &'static str {
    match role {
        AgentRole::Manager => MANAGER_SYSTEM_PROMPT,
        AgentRole::Brainstormer => BRAINSTORMER_SYSTEM_PROMPT,
        AgentRole::Planner => PLANNER_SYSTEM_PROMPT,
        AgentRole::DotGenerator => DOT_GENERATOR_SYSTEM_PROMPT,
        AgentRole::Critic => CRITIC_SYSTEM_PROMPT,
    }
}

/// Wraps a single agent's role and mutable context.
///
/// The LLM runtime is handled by creating a mux SubAgent per step,
/// using the shared LLM client from SwarmOrchestrator.
pub struct AgentRunner {
    pub role: AgentRole,
    pub context: AgentContext,
    pub agent_id: String,
}

impl AgentRunner {
    /// Create a new runner for the given role.
    pub fn new(spec_id: Ulid, role: AgentRole) -> Self {
        let agent_id = format!("{}-{}", role.label(), Ulid::new());
        let context = AgentContext::new(spec_id, agent_id.clone(), role);
        Self {
            role,
            context,
            agent_id,
        }
    }
}

/// Orchestrates a swarm of agents working on a single spec.
/// Manages the agent loop, action routing, pause/resume, and question queue.
pub struct SwarmOrchestrator {
    pub spec_id: Ulid,
    pub actor: Arc<SpecActorHandle>,
    /// Each slot holds an Option so the run_loop can temporarily take ownership
    /// of a runner without needing a placeholder value (fixes Ulid::nil() hack).
    pub agents: Vec<Option<AgentRunner>>,
    /// Per-agent broadcast receivers so each agent sees all events independently.
    /// One receiver per agent, created at swarm construction time.
    event_receivers: Vec<broadcast::Receiver<Event>>,
    pub paused: Arc<AtomicBool>,
    pub question_pending: Arc<AtomicBool>,
    pub client: Arc<dyn LlmClient>,
    pub model: String,
}

impl SwarmOrchestrator {
    /// Create a new orchestrator with default agents for the given spec.
    /// Uses the default provider (from env or "anthropic") and model.
    pub fn with_defaults(spec_id: Ulid, actor: SpecActorHandle) -> Result<Self, AgentError> {
        let provider =
            std::env::var("SPECD_DEFAULT_PROVIDER").unwrap_or_else(|_| "anthropic".to_string());
        let model_override = std::env::var("SPECD_DEFAULT_MODEL").ok();

        let (llm_client, resolved_model) =
            client::create_llm_client(&provider, model_override.as_deref()).map_err(|e| {
                AgentError::ProviderError(e.to_string())
            })?;

        let actor = Arc::new(actor);

        let roles = [
            AgentRole::Manager,
            AgentRole::Brainstormer,
            AgentRole::Planner,
            AgentRole::DotGenerator,
        ];

        let agents: Vec<Option<AgentRunner>> = roles
            .iter()
            .map(|role| Some(AgentRunner::new(spec_id, *role)))
            .collect();

        // Each agent gets its own broadcast receiver so events are not
        // stolen by whichever agent drains the channel first.
        let event_receivers = agents.iter().map(|_| actor.subscribe()).collect();

        Ok(Self {
            spec_id,
            actor,
            agents,
            event_receivers,
            paused: Arc::new(AtomicBool::new(false)),
            question_pending: Arc::new(AtomicBool::new(false)),
            client: llm_client,
            model: resolved_model,
        })
    }

    /// Create an orchestrator with a specific set of agent runners and LLM client.
    pub fn with_agents(
        spec_id: Ulid,
        actor: SpecActorHandle,
        agents: Vec<AgentRunner>,
        client: Arc<dyn LlmClient>,
        model: String,
    ) -> Self {
        let actor = Arc::new(actor);
        let event_receivers = agents.iter().map(|_| actor.subscribe()).collect();
        let agents = agents.into_iter().map(Some).collect();
        Self {
            spec_id,
            actor,
            agents,
            event_receivers,
            paused: Arc::new(AtomicBool::new(false)),
            question_pending: Arc::new(AtomicBool::new(false)),
            client,
            model,
        }
    }

    /// Returns the number of agent slots in this swarm.
    pub fn agent_count(&self) -> usize {
        self.agents.len()
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

    /// Run a single agent step using a mux SubAgent.
    ///
    /// Creates a fresh SubAgent with the domain tool registry, sends it the
    /// agent's context as a task prompt, and lets mux handle the think-act loop.
    /// Returns true if the agent produced useful work, false if idle/error.
    pub async fn run_agent_step(
        runner: &mut AgentRunner,
        actor: &Arc<SpecActorHandle>,
        question_pending: &Arc<AtomicBool>,
        client: &Arc<dyn LlmClient>,
        model: &str,
    ) -> bool {
        // Start agent step
        let start_cmd = Command::StartAgentStep {
            agent_id: runner.agent_id.clone(),
            description: format!("{} reasoning step", runner.role.label()),
        };
        if let Err(e) = actor.send_command(start_cmd).await {
            tracing::warn!(
                agent = %runner.agent_id,
                error = %e,
                "failed to start agent step"
            );
        }

        // Build tool registry for this agent
        let registry = mux_tools::build_registry(
            Arc::clone(actor),
            Arc::clone(question_pending),
            runner.agent_id.clone(),
        )
        .await;

        // Create agent definition with role-specific system prompt
        let definition = AgentDefinition::new(
            runner.role.label(),
            system_prompt_for_role(&runner.role),
        )
        .model(model)
        .max_iterations(10);

        // Create a fresh SubAgent
        let mut sub_agent = SubAgent::new(
            definition,
            Arc::clone(client),
            registry,
        );

        // Build task prompt from context
        let task_prompt = build_task_prompt(&runner.context);

        // Run the agent
        match sub_agent.run(&task_prompt).await {
            Ok(result) => {
                tracing::info!(
                    agent = %runner.agent_id,
                    iterations = result.iterations,
                    tool_calls = result.tool_use_count,
                    "agent step completed"
                );

                // FinishAgentStep is emitted by the emit_diff_summary tool,
                // so we do not send it here to avoid duplicate events.

                // Agent did work if it used any tools
                result.tool_use_count > 0
            }
            Err(e) => {
                tracing::error!(
                    agent = %runner.agent_id,
                    error = %e,
                    "agent step failed"
                );
                false
            }
        }
    }

    /// Update an agent's context from the current actor state.
    /// If `question_pending` is provided, syncs the atomic flag from actor state.
    pub async fn refresh_context(
        runner: &mut AgentRunner,
        actor: &SpecActorHandle,
        event_rx: &mut broadcast::Receiver<Event>,
    ) {
        Self::refresh_context_with_flag(runner, actor, event_rx, None).await;
    }

    /// Update an agent's context and optionally sync the question_pending flag.
    pub async fn refresh_context_with_flag(
        runner: &mut AgentRunner,
        actor: &SpecActorHandle,
        event_rx: &mut broadcast::Receiver<Event>,
        question_pending: Option<&AtomicBool>,
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

        // Sync question_pending flag from actor state
        if let Some(flag) = question_pending {
            flag.store(state.pending_question.is_some(), Ordering::SeqCst);
        }

        // Copy recent transcript
        let transcript_len = state.transcript.len();
        let start = transcript_len.saturating_sub(10);
        runner.context.recent_transcript = state.transcript[start..].to_vec();
    }
}

/// Run the agent loop. This drives all agents in the swarm through their
/// think-act cycles. Runs until the task is cancelled (via JoinHandle::abort).
///
/// Each agent has its own broadcast receiver, so events are never stolen
/// by whichever agent drains the channel first.
pub async fn run_loop(swarm: Arc<tokio::sync::Mutex<SwarmOrchestrator>>) {
    loop {
        // Check pause and get agent count in one lock acquisition
        let (is_paused, agent_count) = {
            let s = swarm.lock().await;
            (s.is_paused(), s.agents.len())
        };

        if is_paused {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            continue;
        }

        let mut any_work = false;
        for i in 0..agent_count {
            // Check pause before each agent
            {
                let s = swarm.lock().await;
                if s.is_paused() {
                    break;
                }
            }

            // Extract runner, its event receiver, and shared fields.
            // The Option::take() avoids needing a Ulid::nil() placeholder.
            let (mut runner, mut event_rx, actor_ref, question_pending, client, model) = {
                let mut s = swarm.lock().await;
                let actor_ref = Arc::clone(&s.actor);
                let question_pending = Arc::clone(&s.question_pending);
                let client = Arc::clone(&s.client);
                let model = s.model.clone();
                let runner = s.agents[i]
                    .take()
                    .expect("agent runner should not be None during loop");
                // Swap out the receiver with a fresh one; the old one keeps its
                // buffered events so we drain them below.
                let event_rx =
                    std::mem::replace(&mut s.event_receivers[i], actor_ref.subscribe());
                (runner, event_rx, actor_ref, question_pending, client, model)
            };

            SwarmOrchestrator::refresh_context_with_flag(
                &mut runner,
                &actor_ref,
                &mut event_rx,
                Some(&question_pending),
            )
            .await;

            let did_work = SwarmOrchestrator::run_agent_step(
                &mut runner,
                &actor_ref,
                &question_pending,
                &client,
                &model,
            )
            .await;

            // Put the runner and its (now-drained) receiver back
            {
                let mut s = swarm.lock().await;
                s.agents[i] = Some(runner);
                s.event_receivers[i] = event_rx;
            }

            if did_work {
                any_work = true;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }

        if any_work {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        } else {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }
}

/// Build a task prompt string from the agent's current context.
///
/// Combines the state summary, recent events, and rolling summary into
/// a single prompt that the mux SubAgent will work with.
fn build_task_prompt(ctx: &AgentContext) -> String {
    let mut parts = Vec::new();

    if !ctx.state_summary.is_empty() {
        parts.push(format!("Current state: {}", ctx.state_summary));
    }

    if !ctx.rolling_summary.is_empty() {
        parts.push(format!("Your accumulated context: {}", ctx.rolling_summary));
    }

    if !ctx.recent_events.is_empty() {
        let event_descriptions: Vec<String> = ctx
            .recent_events
            .iter()
            .map(|e| format!("  - {:?}", e.payload))
            .collect();
        parts.push(format!(
            "Recent events:\n{}",
            event_descriptions.join("\n")
        ));
    }

    if !ctx.recent_transcript.is_empty() {
        let transcript_lines: Vec<String> = ctx
            .recent_transcript
            .iter()
            .map(|msg| format!("  [{}]: {}", msg.sender, msg.content))
            .collect();
        parts.push(format!(
            "Recent transcript:\n{}",
            transcript_lines.join("\n")
        ));
    }

    if !ctx.key_decisions.is_empty() {
        let decisions: Vec<String> = ctx
            .key_decisions
            .iter()
            .map(|d| format!("  - {}", d))
            .collect();
        parts.push(format!(
            "Key decisions so far:\n{}",
            decisions.join("\n")
        ));
    }

    if parts.is_empty() {
        "The spec was just created. Begin your work by reading the current state and taking appropriate action for your role.".to_string()
    } else {
        parts.push("\nReview the above context and take the next appropriate action for your role. Use the available tools to read state, write commands, narrate your reasoning, or ask the user questions.".to_string());
        parts.join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::StubLlmClient;
    use specd_core::state::SpecState;
    use std::sync::atomic::Ordering;

    fn make_test_actor() -> (Ulid, SpecActorHandle) {
        let spec_id = Ulid::new();
        let handle = specd_core::actor::spawn(spec_id, SpecState::new());
        (spec_id, handle)
    }

    fn make_test_client() -> Arc<dyn LlmClient> {
        Arc::new(StubLlmClient::done())
    }

    #[tokio::test]
    async fn swarm_creates_default_agents() {
        let (spec_id, actor) = make_test_actor();

        let roles = [
            AgentRole::Manager,
            AgentRole::Brainstormer,
            AgentRole::Planner,
            AgentRole::DotGenerator,
        ];

        let agents: Vec<AgentRunner> = roles
            .iter()
            .map(|role| AgentRunner::new(spec_id, *role))
            .collect();

        let swarm = SwarmOrchestrator::with_agents(
            spec_id,
            actor,
            agents,
            make_test_client(),
            "stub-model".to_string(),
        );

        assert_eq!(swarm.agents.len(), 4);
        assert_eq!(swarm.agents[0].as_ref().unwrap().role, AgentRole::Manager);
        assert_eq!(swarm.agents[1].as_ref().unwrap().role, AgentRole::Brainstormer);
        assert_eq!(swarm.agents[2].as_ref().unwrap().role, AgentRole::Planner);
        assert_eq!(swarm.agents[3].as_ref().unwrap().role, AgentRole::DotGenerator);

        assert!(!swarm.is_paused());
        assert!(!swarm.has_pending_question());
    }

    #[tokio::test]
    async fn swarm_pause_resume() {
        let (spec_id, actor) = make_test_actor();
        let swarm = SwarmOrchestrator::with_agents(
            spec_id,
            actor,
            Vec::new(),
            make_test_client(),
            "stub-model".to_string(),
        );

        assert!(!swarm.is_paused());

        swarm.pause();
        assert!(swarm.is_paused());

        swarm.resume();
        assert!(!swarm.is_paused());
    }

    #[tokio::test]
    async fn run_agent_step_completes_with_stub() {
        let (spec_id, actor) = make_test_actor();
        let client = make_test_client();
        let actor_arc = Arc::new(actor);
        let question_pending = Arc::new(AtomicBool::new(false));

        let mut runner = AgentRunner::new(spec_id, AgentRole::Brainstormer);

        let did_work = SwarmOrchestrator::run_agent_step(
            &mut runner,
            &actor_arc,
            &question_pending,
            &client,
            "stub-model",
        )
        .await;

        // StubLlmClient returns text-only (no tool use), so agent does no tool work
        assert!(!did_work);
    }

    #[tokio::test]
    async fn refresh_context_updates_state() {
        let (spec_id, actor) = make_test_actor();
        let mut event_rx = actor.subscribe();

        let mut runner = AgentRunner::new(spec_id, AgentRole::Manager);

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

    #[test]
    fn system_prompt_for_role_returns_non_empty() {
        let roles = [
            AgentRole::Manager,
            AgentRole::Brainstormer,
            AgentRole::Planner,
            AgentRole::DotGenerator,
            AgentRole::Critic,
        ];

        for role in &roles {
            let prompt = system_prompt_for_role(role);
            assert!(
                !prompt.is_empty(),
                "system prompt for {:?} should not be empty",
                role
            );
        }
    }

    #[test]
    fn agent_runner_new_generates_unique_ids() {
        let spec_id = Ulid::new();
        let a = AgentRunner::new(spec_id, AgentRole::Manager);
        let b = AgentRunner::new(spec_id, AgentRole::Manager);

        assert_ne!(a.agent_id, b.agent_id, "each runner should get a unique agent_id");
        assert!(a.agent_id.starts_with("manager-"));
        assert!(b.agent_id.starts_with("manager-"));
    }

    #[test]
    fn build_task_prompt_empty_context() {
        let ctx = AgentContext::new(Ulid::new(), "test-agent".to_string(), AgentRole::Manager);
        let prompt = build_task_prompt(&ctx);
        assert!(prompt.contains("just created"), "empty context should produce intro prompt");
    }

    #[test]
    fn build_task_prompt_with_state_summary() {
        let mut ctx = AgentContext::new(Ulid::new(), "test-agent".to_string(), AgentRole::Manager);
        ctx.state_summary = "Title: Foo. Goal: Bar.".to_string();

        let prompt = build_task_prompt(&ctx);
        assert!(prompt.contains("Current state: Title: Foo"));
        assert!(prompt.contains("take the next appropriate action"));
    }

    #[tokio::test]
    async fn question_pending_cleared_after_answer() {
        let (spec_id, actor) = make_test_actor();
        let question_pending = AtomicBool::new(false);

        // Ask a question via actor command directly
        let question_id = Ulid::new();
        actor
            .send_command(Command::AskQuestion {
                question: specd_core::transcript::UserQuestion::Freeform {
                    question_id,
                    question: "What color?".to_string(),
                    placeholder: None,
                    validation_hint: None,
                },
            })
            .await
            .unwrap();

        // Manually set flag (simulating what the tool would do)
        question_pending.store(true, Ordering::SeqCst);
        assert!(question_pending.load(Ordering::SeqCst));

        // Answer the question
        actor
            .send_command(Command::AnswerQuestion {
                question_id,
                answer: "Blue".to_string(),
            })
            .await
            .unwrap();

        // refresh_context_with_flag should sync the flag from actor state
        let mut event_rx = actor.subscribe();
        let mut runner = AgentRunner::new(spec_id, AgentRole::Manager);

        SwarmOrchestrator::refresh_context_with_flag(
            &mut runner,
            &actor,
            &mut event_rx,
            Some(&question_pending),
        )
        .await;

        // After the answer, the flag should be cleared
        assert!(
            !question_pending.load(Ordering::SeqCst),
            "question_pending should be false after answer and refresh"
        );
    }

    #[tokio::test]
    async fn run_loop_can_be_cancelled() {
        let (spec_id, actor) = make_test_actor();
        let swarm = SwarmOrchestrator::with_agents(
            spec_id,
            actor,
            Vec::new(),
            make_test_client(),
            "stub-model".to_string(),
        );
        let swarm = Arc::new(tokio::sync::Mutex::new(swarm));

        let handle = tokio::spawn(run_loop(Arc::clone(&swarm)));

        // Let it run briefly, then abort
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        handle.abort();

        // Verify it was cancelled (abort causes JoinError)
        let result = handle.await;
        assert!(result.is_err(), "run_loop should be cancelled by abort");
        assert!(result.unwrap_err().is_cancelled());
    }

    #[tokio::test]
    async fn agent_count_returns_slot_count() {
        let (spec_id, actor) = make_test_actor();
        let agents = vec![
            AgentRunner::new(spec_id, AgentRole::Manager),
            AgentRunner::new(spec_id, AgentRole::Brainstormer),
        ];
        let swarm = SwarmOrchestrator::with_agents(
            spec_id,
            actor,
            agents,
            make_test_client(),
            "stub-model".to_string(),
        );
        assert_eq!(swarm.agent_count(), 2);
    }

    #[tokio::test]
    async fn each_agent_gets_own_event_receiver() {
        let (spec_id, actor) = make_test_actor();
        let agents = vec![
            AgentRunner::new(spec_id, AgentRole::Manager),
            AgentRunner::new(spec_id, AgentRole::Brainstormer),
        ];
        let swarm = SwarmOrchestrator::with_agents(
            spec_id,
            actor,
            agents,
            make_test_client(),
            "stub-model".to_string(),
        );
        // Each agent should have a dedicated event receiver
        assert_eq!(
            swarm.event_receivers.len(),
            2,
            "each agent should have its own event receiver"
        );
    }
}
