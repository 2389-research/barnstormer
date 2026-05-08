// ABOUTME: Embedded Barnstormer server lifecycle shared by CLI and desktop app frontends.
// ABOUTME: Launches the Axum app on loopback, reports the local URL, and supports graceful shutdown.

use std::sync::Arc;

use barnstormer_server::{AppState, ProviderStatus, create_router};
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
    let app = create_router(state, runtime_config.auth_token.clone());
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
