// ABOUTME: Entry point for the specd binary.
// ABOUTME: Parses CLI arguments with clap, recovers specs, spawns actors, and starts the Axum HTTP server.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use specd_server::{AppState, ProviderStatus, create_router};
use specd_store::StorageManager;

#[derive(Parser)]
#[command(name = "specd", about = "Agentic spec builder")]
enum Cli {
    /// Start the specd server
    Start {
        /// Do not open the browser on startup
        #[arg(long, default_value = "false")]
        no_open: bool,
    },
    /// Check if specd is running
    Status,
}

#[tokio::main]
async fn main() {
    // Load .env if present (ignoring errors if missing)
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "specd=debug,tower_http=debug".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();

    match cli {
        Cli::Start { no_open } => {
            let specd_home = std::env::var("SPECD_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| dirs_or_default().join(".specd"));

            tracing::info!("SPECD_HOME: {}", specd_home.display());

            // Initialize StorageManager
            let storage = StorageManager::new(specd_home.clone())
                .expect("failed to initialize storage manager");

            // Recover all existing specs
            let recovered_specs = storage
                .recover_all_specs()
                .expect("failed to recover specs");

            tracing::info!("recovered {} specs", recovered_specs.len());

            // Create AppState and spawn actors for recovered specs
            let state = Arc::new(AppState::new(specd_home, ProviderStatus::detect()));
            {
                let mut actors = state.actors.write().await;
                for (spec_id, spec_state) in recovered_specs {
                    let handle = specd_core::spawn(spec_id, spec_state);
                    actors.insert(spec_id, handle);
                    tracing::info!("spawned actor for spec {}", spec_id);
                }
            }

            let auth_token = std::env::var("SPECD_AUTH_TOKEN")
                .ok()
                .filter(|t| !t.is_empty());

            let app = create_router(state, auth_token);

            let bind_addr: SocketAddr = std::env::var("SPECD_BIND")
                .unwrap_or_else(|_| "127.0.0.1:7331".to_string())
                .parse()
                .expect("SPECD_BIND must be a valid socket address");

            let url = format!("http://{}", bind_addr);
            tracing::info!("specd listening on {}", url);

            // Open browser unless --no-open was specified
            if !no_open {
                #[cfg(target_os = "macos")]
                {
                    let _ = std::process::Command::new("open").arg(&url).spawn();
                }
                #[cfg(target_os = "linux")]
                {
                    let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
                }
            }

            let listener = tokio::net::TcpListener::bind(bind_addr)
                .await
                .expect("failed to bind");

            axum::serve(listener, app).await.expect("server error");
        }
        Cli::Status => {
            let bind_addr =
                std::env::var("SPECD_BIND").unwrap_or_else(|_| "127.0.0.1:7331".to_string());

            println!("specd status: checking {}...", bind_addr);

            match std::net::TcpStream::connect(&bind_addr) {
                Ok(_) => println!("specd is running on {}", bind_addr),
                Err(_) => println!("specd is not running on {}", bind_addr),
            }
        }
    }
}

/// Get the user's home directory, falling back to /tmp if unavailable.
fn dirs_or_default() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}
