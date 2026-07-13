// ABOUTME: Embedded Barnstormer server lifecycle shared by CLI and desktop app frontends.
// ABOUTME: Launches the Axum app on loopback, reports the local URL, and supports graceful shutdown.

use std::sync::Arc;

use barnstormer_server::{AppState, ProviderStatus, create_router_with_static_dir};
use barnstormer_store::StorageManager;
use tokio::sync::oneshot;

use crate::{RuntimeConfig, RuntimeOptions};

pub struct ServerHandle {
    local_url: String,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl ServerHandle {
    pub fn local_url(&self) -> &str {
        &self.local_url
    }

    pub async fn wait(self) -> anyhow::Result<()> {
        match self.join_handle.await {
            Ok(result) => result,
            Err(err) => Err(err.into()),
        }
    }

    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        self.wait().await
    }
}

pub async fn launch(options: RuntimeOptions) -> anyhow::Result<ServerHandle> {
    let runtime_config = RuntimeConfig::from_parts(options)?;
    tracing::info!("BARNSTORMER_HOME: {}", runtime_config.home.display());

    let state = build_state(&runtime_config).await?;
    let app = create_router_with_static_dir(
        state,
        runtime_config.auth_token.clone(),
        runtime_config.static_dir.clone(),
    );
    let listener = tokio::net::TcpListener::bind(runtime_config.bind).await?;
    let local_addr = listener.local_addr()?;
    let local_url = format!("http://{}", local_addr);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    tracing::info!("barnstormer listening on {}", local_url);

    let join_handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
            .map_err(anyhow::Error::from)
    });

    Ok(ServerHandle {
        local_url,
        shutdown_tx: Some(shutdown_tx),
        join_handle,
    })
}

async fn build_state(runtime_config: &RuntimeConfig) -> anyhow::Result<Arc<AppState>> {
    let storage = StorageManager::new(runtime_config.home.clone())?;
    let recovered_specs = storage.recover_all_specs()?;

    tracing::info!("recovered {} specs", recovered_specs.len());

    let state = Arc::new(AppState::new(
        runtime_config.home.clone(),
        ProviderStatus::detect(),
        runtime_config.token_budget,
    ));

    {
        let mut actors = state.actors.write().await;
        let mut persisters = state.event_persisters.write().await;
        for (spec_id, spec_state) in recovered_specs {
            let handle = barnstormer_core::spawn(spec_id, spec_state);
            let persister = barnstormer_server::web::spawn_event_persister(
                &handle,
                spec_id,
                &runtime_config.home,
            );
            persisters.insert(spec_id, persister);
            actors.insert(spec_id, handle);
            tracing::info!("spawned actor for spec {}", spec_id);
        }
    }

    tracing::info!("agents paused on startup — enable per-spec via the web UI");

    Ok(state)
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use barnstormer_agent::testing::StubLlmClient;
    use barnstormer_agent::{AgentRole, AgentRunner, AttachmentSummarizer, SwarmOrchestrator};
    use barnstormer_core::command::Command;
    use barnstormer_core::state::{ContextAttachment, SpecPhase, SpecState};
    use barnstormer_core::transcript::UserQuestion;
    use mux::llm::Usage;
    use ulid::Ulid;

    use super::*;
    use crate::config::test_env::{ENV_LOCK, EnvGuard};

    #[derive(Debug)]
    struct LocalSummarizer;

    #[async_trait]
    impl AttachmentSummarizer for LocalSummarizer {
        async fn answer_question(
            &self,
            _spec_id: Ulid,
            _attachment: &ContextAttachment,
            _question: &str,
        ) -> Result<String, String> {
            Ok("unused".to_string())
        }
    }

    async fn wait_until(description: &str, mut condition: impl AsyncFnMut() -> bool) {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while !condition().await {
                tokio::task::yield_now().await;
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {description}"));
    }

    fn pending_question_id(state: &SpecState) -> Ulid {
        match state.pending_question.as_ref() {
            Some(UserQuestion::Boolean {
                question_id,
                default,
                ..
            }) => {
                assert_eq!(*default, Some(false));
                *question_id
            }
            other => panic!("expected token budget question, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn runtime_budget_reaches_real_swarm_consent_lifecycle() {
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _budget_env = EnvGuard::set("BARNSTORMER_TOKEN_BUDGET", Some("100"));
        let temp = tempfile::tempdir().unwrap();
        let runtime_config = RuntimeConfig::from_parts(RuntimeOptions {
            home: Some(temp.path().to_path_buf()),
            bind: Some("127.0.0.1:0".parse().unwrap()),
            auth_token: None,
            static_dir: None,
            open_browser: false,
            disable_auth_fallback: true,
        })
        .unwrap();
        let app_state = build_state(&runtime_config).await.unwrap();
        assert_eq!(app_state.token_budget, Some(100));

        let spec_id = Ulid::new();
        let actor = barnstormer_core::spawn(spec_id, SpecState::new());
        actor
            .send_command(Command::CreateSpec {
                title: "Runtime budget wiring".to_string(),
                one_liner: "Exercise local swarm".to_string(),
                goal: "Verify runtime-to-swarm propagation".to_string(),
            })
            .await
            .unwrap();
        actor
            .send_command(Command::TransitionPhase {
                target: SpecPhase::Refining,
            })
            .await
            .unwrap();
        let actor = Arc::new(actor);
        let client = Arc::new(StubLlmClient::with_usage(
            "Done.",
            Usage {
                input_tokens: 80,
                output_tokens: 30,
                cache_read_tokens: 500,
                cache_write_tokens: 10,
            },
        ));
        let swarm = SwarmOrchestrator::with_agents(
            spec_id,
            actor.as_ref().clone(),
            vec![AgentRunner::new(spec_id, AgentRole::Brainstormer)],
            client,
            "local-stub".to_string(),
            temp.path().to_path_buf(),
            Arc::new(LocalSummarizer),
            app_state.token_budget,
        );
        assert_eq!(swarm.token_budget, Some(100));
        let swarm = Arc::new(tokio::sync::Mutex::new(swarm));
        let task = tokio::spawn(barnstormer_agent::run_loop(Arc::clone(&swarm)));

        wait_until("runtime-wired budget question", || async {
            actor.read_state().await.pending_question.is_some()
        })
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(swarm.lock().await.session_tokens_used, 120);
        let first_question = {
            let state = actor.read_state().await;
            pending_question_id(&state)
        };
        actor
            .send_command(Command::AnswerQuestion {
                question_id: first_question,
                answer: "Yes".to_string(),
            })
            .await
            .unwrap();
        swarm.lock().await.notify_human_message();

        wait_until("second runtime-wired budget question", || async {
            let state = actor.read_state().await;
            state.pending_question.as_ref().is_some_and(|question| {
                matches!(question, UserQuestion::Boolean { question_id, .. } if *question_id != first_question)
            })
        })
        .await;
        let second_question = {
            let state = actor.read_state().await;
            pending_question_id(&state)
        };
        actor
            .send_command(Command::AnswerQuestion {
                question_id: second_question,
                answer: "No".to_string(),
            })
            .await
            .unwrap();
        swarm.lock().await.notify_human_message();
        wait_until("pause after runtime-wired No", || async {
            swarm.lock().await.is_paused()
        })
        .await;

        task.abort();
        let _ = task.await;
    }
}
