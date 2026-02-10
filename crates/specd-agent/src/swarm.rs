// ABOUTME: SwarmOrchestrator manages multiple agents per spec, using mux SubAgent for LLM execution.
// ABOUTME: Each agent runs as a mux SubAgent with domain tools, coordinated by pause/resume flags and event subscriptions.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{broadcast, Notify};
use tracing;
use ulid::Ulid;

use mux::agent::{AgentDefinition, SubAgent};
use mux::llm::LlmClient;

use std::collections::HashMap;

use crate::client;
use crate::context::{AgentContext, AgentRole};
use crate::mux_tools;
use specd_core::actor::SpecActorHandle;
use specd_core::command::Command;
use specd_core::event::Event;

/// System prompt for the Manager agent role.
const MANAGER_SYSTEM_PROMPT: &str = "You are the manager agent for a product specification. \
    You coordinate the spec refinement process: identify gaps, ensure all aspects are covered, \
    and ask the user questions when clarification is needed. Start by reading the current state, \
    then decide what needs attention. If cards exist, review them and suggest \
    improvements or ask clarifying questions.\n\n\
    STARTUP PROTOCOL: When you first read the state and see a new spec with an empty one_liner \
    and goal, check the transcript for the human's initial description. Parse it into structured \
    fields using UpdateSpecCore:\n\
    - title: A concise, descriptive title (3-8 words)\n\
    - one_liner: A single sentence summarizing the product\n\
    - goal: The primary objective or outcome\n\
    - description: Expanded details from the user's input\n\
    Then create initial idea cards for the key features, components, or requirements you identify. \
    Narrate what you're doing so the user can follow along. After structuring the spec, \
    ask the user a clarifying question about the most important ambiguity.\n\n\
    IMPORTANT: You are the primary point of contact for the human user. When you see messages from \
    'human' in the recent transcript, treat them as top priority — acknowledge them with narration, \
    take action based on their input, and route their requests to the appropriate workflow. \
    The human is actively engaged, so always respond to their messages before doing other work.";

/// System prompt for the Brainstormer agent role.
const BRAINSTORMER_SYSTEM_PROMPT: &str = "You are the brainstormer agent. Your job is to generate \
    creative ideas, explore possibilities, and create idea cards. Focus on breadth over depth. \
    Read the current state first, then create cards with card_type 'idea' for each new idea. \
    Add a body with a brief explanation. Narrate your thought process so the user can follow along.";

/// System prompt for the Planner agent role.
const PLANNER_SYSTEM_PROMPT: &str = "You are the planner agent. Your job is to organize ideas into \
    structured plans. Read the current state, then: move promising idea cards to the 'Plan' lane, \
    create task cards that break down ideas into actionable steps, and update the spec core with \
    constraints and success criteria. Narrate your reasoning.";

/// System prompt for the DotGenerator agent role.
const DOT_GENERATOR_SYSTEM_PROMPT: &str = "You are the DOT diagram generator. Your job is to read \
    the current spec state and create a card containing Graphviz DOT notation that represents \
    the spec's structure and relationships between cards. Create a card with card_type 'note' \
    and title 'Spec Diagram' containing DOT source in the body. Update it if one already exists.";

/// System prompt for the Critic agent role.
const CRITIC_SYSTEM_PROMPT: &str = "You are the critic agent. Your job is to review the spec for \
    gaps, inconsistencies, and potential issues. Read the current state, then create cards with \
    card_type 'risk' or 'constraint' for issues you find. Narrate your analysis and provide \
    constructive feedback. Ask the user questions when you identify ambiguities that need human input.";

/// Tool usage and workflow guidance appended to all agent system prompts at runtime.
/// Includes the agent's own ID so it can use it in commands.
fn tool_usage_guide(agent_id: &str) -> String {
    format!(
        "\n\nYour agent ID is: {agent_id}\n\n\
        You have the following tools:\n\
        - read_state: Read the current spec (title, goal, cards, transcript). Call this FIRST.\n\
        - write_commands: Submit commands to modify the spec. You MUST wrap commands in a {{\"commands\": [...]}} object. Example:\n\
          {{\"commands\": [{{\"type\": \"CreateCard\", \"card_type\": \"idea\", \"title\": \"My Idea\", \"body\": \"Details here\", \"lane\": null, \"created_by\": \"{agent_id}\"}}]}}\n\
          Individual command types:\n\
          * {{\"type\": \"CreateCard\", \"card_type\": \"idea\", \"title\": \"My Idea\", \"body\": \"Details here\", \"lane\": null, \"created_by\": \"{agent_id}\"}}\n\
          * {{\"type\": \"UpdateSpecCore\", \"description\": \"A detailed description\", \"constraints\": null, \"success_criteria\": null, \"risks\": null, \"notes\": null, \"title\": null, \"one_liner\": null, \"goal\": null}}\n\
          * {{\"type\": \"MoveCard\", \"card_id\": \"<ULID from read_state>\", \"lane\": \"Plan\", \"order\": 1.0, \"updated_by\": \"{agent_id}\"}}\n\
        - emit_narration: Post a message to the activity feed. Use this OFTEN to explain your reasoning.\n\
        - emit_diff_summary: Mark your step as finished with a change summary. Call this LAST.\n\
        - ask_user_boolean / ask_user_freeform / ask_user_multiple_choice: Ask the user questions.\n\n\
        Workflow: 1) read_state 2) emit_narration (explain plan) 3) write_commands (make changes) 4) emit_diff_summary (finish)"
    )
}

/// Return the base system prompt for a given agent role (without tool guide).
pub fn system_prompt_for_role(role: &AgentRole) -> &'static str {
    match role {
        AgentRole::Manager => MANAGER_SYSTEM_PROMPT,
        AgentRole::Brainstormer => BRAINSTORMER_SYSTEM_PROMPT,
        AgentRole::Planner => PLANNER_SYSTEM_PROMPT,
        AgentRole::DotGenerator => DOT_GENERATOR_SYSTEM_PROMPT,
        AgentRole::Critic => CRITIC_SYSTEM_PROMPT,
    }
}

/// Build the full system prompt for an agent, including the tool usage guide
/// with the agent's ID substituted in.
fn full_system_prompt(role: &AgentRole, agent_id: &str) -> String {
    format!("{}{}", system_prompt_for_role(role), tool_usage_guide(agent_id))
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
    /// Signal that a human message has arrived; wakes the run_loop from its
    /// idle sleep so the manager agent can respond promptly.
    pub human_message_notify: Arc<Notify>,
}

impl SwarmOrchestrator {
    /// Create a new orchestrator with default agents for the given spec.
    /// Uses the default provider (from env or "anthropic") and model.
    pub fn with_defaults(spec_id: Ulid, actor: SpecActorHandle) -> Result<Self, anyhow::Error> {
        let provider =
            std::env::var("SPECD_DEFAULT_PROVIDER").unwrap_or_else(|_| "anthropic".to_string());
        let model_override = std::env::var("SPECD_DEFAULT_MODEL").ok();

        let (llm_client, resolved_model) =
            client::create_llm_client(&provider, model_override.as_deref())?;

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
            human_message_notify: Arc::new(Notify::new()),
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
            human_message_notify: Arc::new(Notify::new()),
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

    /// Signal that a human message has arrived so the run_loop wakes
    /// from its idle sleep and prioritises the manager agent.
    pub fn notify_human_message(&self) {
        self.human_message_notify.notify_one();
    }

    /// Re-create any agent runner slots that are `None` (e.g. from a cancelled task).
    /// Each restored slot gets a fresh AgentRunner and event receiver.
    /// Only works for slots whose index maps to a known default role.
    pub fn recover_empty_slots(&mut self) {
        let default_roles = [
            AgentRole::Manager,
            AgentRole::Brainstormer,
            AgentRole::Planner,
            AgentRole::DotGenerator,
        ];
        for i in 0..self.agents.len() {
            if self.agents[i].is_none()
                && let Some(&role) = default_roles.get(i)
            {
                tracing::warn!(
                    agent_index = i,
                    role = %role,
                    "recovering empty agent slot after cancellation"
                );
                self.agents[i] = Some(AgentRunner::new(self.spec_id, role));
                self.event_receivers[i] = self.actor.subscribe();
            }
        }
    }

    /// Collect all agent contexts for inclusion in a snapshot.
    pub fn collect_agent_contexts(&self) -> HashMap<String, serde_json::Value> {
        let contexts: Vec<AgentContext> = self
            .agents
            .iter()
            .filter_map(|opt| opt.as_ref().map(|r| r.context.clone()))
            .collect();
        crate::context::contexts_to_snapshot_map(&contexts)
    }

    /// Restore agent contexts from a snapshot map.
    /// Matches by agent_role, since agent_ids may differ between sessions.
    /// Restores all agents whose role matches (not just the first),
    /// so duplicate-role swarms are handled correctly.
    pub fn restore_agent_contexts(&mut self, map: &HashMap<String, serde_json::Value>) {
        let restored = crate::context::contexts_from_snapshot_map(map);
        for ctx in restored {
            for agent_opt in &mut self.agents {
                if let Some(runner) = agent_opt.as_mut()
                    && runner.role == ctx.agent_role
                {
                    runner.context.rolling_summary = ctx.rolling_summary.clone();
                    runner.context.key_decisions = ctx.key_decisions.clone();
                    runner.context.last_event_seen = ctx.last_event_seen;
                }
            }
        }
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

        // Create agent definition with role-specific system prompt + tool guide
        let definition = AgentDefinition::new(
            runner.role.label(),
            full_system_prompt(&runner.role, &runner.agent_id),
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
                // Log the full error details for debugging
                tracing::error!(
                    agent = %runner.agent_id,
                    error = %e,
                    "agent step failed"
                );
                // Show a sanitized, user-friendly message in the transcript
                // without exposing internal error details.
                let user_msg = format!(
                    "[{}] encountered an issue and will retry on the next cycle.",
                    runner.role.label(),
                );
                let _ = actor
                    .send_command(Command::AppendTranscript {
                        sender: runner.agent_id.clone(),
                        content: user_msg,
                    })
                    .await;
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

/// Run a single agent step by index, extracting the runner from the swarm,
/// refreshing its context, running the step, and putting it back.
/// Returns true if the agent produced useful work.
async fn run_agent_by_index(
    swarm: &Arc<tokio::sync::Mutex<SwarmOrchestrator>>,
    index: usize,
) -> bool {
    let extracted = {
        let mut s = swarm.lock().await;
        let actor_ref = Arc::clone(&s.actor);
        let question_pending = Arc::clone(&s.question_pending);
        let client = Arc::clone(&s.client);
        let model = s.model.clone();
        match s.agents[index].take() {
            Some(runner) => {
                // Swap out the receiver with a fresh one; the old one keeps its
                // buffered events so we drain them below.
                let event_rx =
                    std::mem::replace(&mut s.event_receivers[index], actor_ref.subscribe());
                Some((runner, event_rx, actor_ref, question_pending, client, model))
            }
            None => {
                tracing::warn!(agent_index = index, "agent runner slot is empty, skipping");
                None
            }
        }
    };
    let Some((mut runner, mut event_rx, actor_ref, question_pending, client, model)) = extracted
    else {
        return false;
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
        s.agents[index] = Some(runner);
        s.event_receivers[index] = event_rx;
    }

    did_work
}

/// Find the index of the manager agent (first agent with AgentRole::Manager).
fn find_manager_index(swarm: &SwarmOrchestrator) -> Option<usize> {
    swarm.agents.iter().position(|opt| {
        opt.as_ref()
            .map(|r| r.role == AgentRole::Manager)
            .unwrap_or(false)
    })
}

/// Run the agent loop. This drives all agents in the swarm through their
/// think-act cycles. Runs until the task is cancelled (via JoinHandle::abort).
///
/// Each agent has its own broadcast receiver, so events are never stolen
/// by whichever agent drains the channel first.
///
/// When a human sends a chat message, `human_message_notify` wakes the loop
/// from its idle sleep so the manager agent responds promptly.
pub async fn run_loop(swarm: Arc<tokio::sync::Mutex<SwarmOrchestrator>>) {
    loop {
        // Recover any empty slots from prior cancellations, then check pause.
        let (is_paused, agent_count, notify) = {
            let mut s = swarm.lock().await;
            s.recover_empty_slots();
            (s.is_paused(), s.agents.len(), Arc::clone(&s.human_message_notify))
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

            let did_work = run_agent_by_index(&swarm, i).await;

            if did_work {
                any_work = true;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }

        // Wait between cycles. Use tokio::select! so a human message
        // notification can interrupt the idle sleep and wake us early.
        let sleep_duration = if any_work {
            std::time::Duration::from_secs(1)
        } else {
            std::time::Duration::from_secs(5)
        };

        tokio::select! {
            _ = tokio::time::sleep(sleep_duration) => {}
            _ = notify.notified() => {
                // Human message arrived — run the manager agent immediately
                // before starting the next full cycle.
                let manager_idx = {
                    let s = swarm.lock().await;
                    find_manager_index(&s)
                };
                if let Some(idx) = manager_idx {
                    tracing::info!("human message received, prioritising manager agent");
                    run_agent_by_index(&swarm, idx).await;
                }
            }
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

    #[tokio::test]
    async fn collect_agent_contexts_returns_all_agents() {
        let (spec_id, actor) = make_test_actor();

        let mut manager = AgentRunner::new(spec_id, AgentRole::Manager);
        manager.context.rolling_summary = "Manager saw events".to_string();
        manager.context.last_event_seen = 10;
        manager.context.add_decision("Use gRPC".to_string());

        let mut brainstormer = AgentRunner::new(spec_id, AgentRole::Brainstormer);
        brainstormer.context.rolling_summary = "Brainstormer explored ideas".to_string();
        brainstormer.context.last_event_seen = 7;

        let mut planner = AgentRunner::new(spec_id, AgentRole::Planner);
        planner.context.rolling_summary = "Planner organized tasks".to_string();
        planner.context.last_event_seen = 5;

        let manager_id = manager.agent_id.clone();
        let brainstormer_id = brainstormer.agent_id.clone();
        let planner_id = planner.agent_id.clone();

        let swarm = SwarmOrchestrator::with_agents(
            spec_id,
            actor,
            vec![manager, brainstormer, planner],
            make_test_client(),
            "stub-model".to_string(),
        );

        let map = swarm.collect_agent_contexts();

        assert_eq!(map.len(), 3, "should have one entry per agent");
        assert!(map.contains_key(&manager_id));
        assert!(map.contains_key(&brainstormer_id));
        assert!(map.contains_key(&planner_id));

        // Verify content is properly serialized
        let manager_val = &map[&manager_id];
        assert_eq!(
            manager_val["rolling_summary"],
            serde_json::json!("Manager saw events")
        );
        assert_eq!(manager_val["last_event_seen"], serde_json::json!(10));
    }

    #[tokio::test]
    async fn restore_agent_contexts_round_trip() {
        let (spec_id, actor) = make_test_actor();

        let mut manager = AgentRunner::new(spec_id, AgentRole::Manager);
        manager.context.rolling_summary = "Manager memory".to_string();
        manager.context.last_event_seen = 15;
        manager.context.add_decision("Ship it".to_string());

        let mut brainstormer = AgentRunner::new(spec_id, AgentRole::Brainstormer);
        brainstormer.context.rolling_summary = "Brainstormer memory".to_string();
        brainstormer.context.last_event_seen = 12;
        brainstormer.context.add_decision("Add caching layer".to_string());

        let agents = vec![manager, brainstormer];
        let mut swarm = SwarmOrchestrator::with_agents(
            spec_id,
            actor,
            agents,
            make_test_client(),
            "stub-model".to_string(),
        );

        // Collect contexts
        let map = swarm.collect_agent_contexts();
        assert_eq!(map.len(), 2);

        // Clear contexts on the agents to simulate a fresh session
        for agent_opt in &mut swarm.agents {
            if let Some(runner) = agent_opt.as_mut() {
                runner.context.rolling_summary.clear();
                runner.context.key_decisions.clear();
                runner.context.last_event_seen = 0;
            }
        }

        // Restore from the collected map
        swarm.restore_agent_contexts(&map);

        // Verify contexts were restored
        let mgr = swarm.agents[0].as_ref().unwrap();
        assert_eq!(mgr.role, AgentRole::Manager);
        assert_eq!(mgr.context.rolling_summary, "Manager memory");
        assert_eq!(mgr.context.last_event_seen, 15);
        assert_eq!(mgr.context.key_decisions, vec!["Ship it"]);

        let brain = swarm.agents[1].as_ref().unwrap();
        assert_eq!(brain.role, AgentRole::Brainstormer);
        assert_eq!(brain.context.rolling_summary, "Brainstormer memory");
        assert_eq!(brain.context.last_event_seen, 12);
        assert_eq!(brain.context.key_decisions, vec!["Add caching layer"]);
    }

    #[tokio::test]
    async fn restore_agent_contexts_matches_by_role() {
        let (spec_id, actor) = make_test_actor();

        // Create agents with known contexts
        let mut original_manager = AgentRunner::new(spec_id, AgentRole::Manager);
        original_manager.context.rolling_summary = "Original manager context".to_string();
        original_manager.context.last_event_seen = 20;
        original_manager.context.add_decision("Decision A".to_string());

        let mut original_planner = AgentRunner::new(spec_id, AgentRole::Planner);
        original_planner.context.rolling_summary = "Original planner context".to_string();
        original_planner.context.last_event_seen = 18;
        original_planner.context.add_decision("Decision B".to_string());

        // Collect the snapshot from the originals
        let contexts: Vec<AgentContext> = vec![
            original_manager.context.clone(),
            original_planner.context.clone(),
        ];
        let map = crate::context::contexts_to_snapshot_map(&contexts);

        // Create a fresh swarm with new agents (different agent_ids)
        let new_manager = AgentRunner::new(spec_id, AgentRole::Manager);
        let new_planner = AgentRunner::new(spec_id, AgentRole::Planner);

        // Verify the new agents have different IDs from the originals
        assert_ne!(new_manager.agent_id, original_manager.agent_id);
        assert_ne!(new_planner.agent_id, original_planner.agent_id);

        // Verify the new agents start with empty contexts
        assert!(new_manager.context.rolling_summary.is_empty());
        assert!(new_planner.context.rolling_summary.is_empty());

        let mut swarm = SwarmOrchestrator::with_agents(
            spec_id,
            actor,
            vec![new_manager, new_planner],
            make_test_client(),
            "stub-model".to_string(),
        );

        // Restore the old contexts onto the new agents
        swarm.restore_agent_contexts(&map);

        // Manager should have the original manager's context
        let mgr = swarm.agents[0].as_ref().unwrap();
        assert_eq!(mgr.role, AgentRole::Manager);
        assert_eq!(mgr.context.rolling_summary, "Original manager context");
        assert_eq!(mgr.context.last_event_seen, 20);
        assert_eq!(mgr.context.key_decisions, vec!["Decision A"]);
        // But the agent_id should NOT have changed
        assert_ne!(mgr.agent_id, original_manager.agent_id);

        // Planner should have the original planner's context
        let plnr = swarm.agents[1].as_ref().unwrap();
        assert_eq!(plnr.role, AgentRole::Planner);
        assert_eq!(plnr.context.rolling_summary, "Original planner context");
        assert_eq!(plnr.context.last_event_seen, 18);
        assert_eq!(plnr.context.key_decisions, vec!["Decision B"]);
        assert_ne!(plnr.agent_id, original_planner.agent_id);
    }

    #[tokio::test]
    async fn notify_human_message_wakes_run_loop() {
        let (spec_id, actor) = make_test_actor();
        let swarm = SwarmOrchestrator::with_agents(
            spec_id,
            actor,
            Vec::new(),
            make_test_client(),
            "stub-model".to_string(),
        );
        let notify = Arc::clone(&swarm.human_message_notify);
        let swarm = Arc::new(tokio::sync::Mutex::new(swarm));

        let handle = tokio::spawn(run_loop(Arc::clone(&swarm)));

        // Let it enter the idle sleep
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Send the notification — should wake the loop instead of sleeping 5s
        notify.notify_one();

        // Give it a moment to process the wake, then abort
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        handle.abort();

        let result = handle.await;
        assert!(result.unwrap_err().is_cancelled());
    }

    #[tokio::test]
    async fn find_manager_index_finds_manager() {
        let (spec_id, actor) = make_test_actor();
        let agents = vec![
            AgentRunner::new(spec_id, AgentRole::Brainstormer),
            AgentRunner::new(spec_id, AgentRole::Manager),
            AgentRunner::new(spec_id, AgentRole::Planner),
        ];
        let swarm = SwarmOrchestrator::with_agents(
            spec_id,
            actor,
            agents,
            make_test_client(),
            "stub-model".to_string(),
        );
        assert_eq!(find_manager_index(&swarm), Some(1));
    }

    #[tokio::test]
    async fn find_manager_index_returns_none_without_manager() {
        let (spec_id, actor) = make_test_actor();
        let agents = vec![
            AgentRunner::new(spec_id, AgentRole::Brainstormer),
            AgentRunner::new(spec_id, AgentRole::Planner),
        ];
        let swarm = SwarmOrchestrator::with_agents(
            spec_id,
            actor,
            agents,
            make_test_client(),
            "stub-model".to_string(),
        );
        assert_eq!(find_manager_index(&swarm), None);
    }

    #[test]
    fn manager_prompt_mentions_human_priority() {
        let prompt = system_prompt_for_role(&AgentRole::Manager);
        assert!(
            prompt.contains("human"),
            "manager prompt should mention human messages"
        );
        assert!(
            prompt.contains("top priority"),
            "manager prompt should prioritize human messages"
        );
    }
}
