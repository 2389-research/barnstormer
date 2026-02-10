// ABOUTME: Entry point for the specd binary.
// ABOUTME: Parses CLI arguments with clap, initializes tracing, and starts the Axum HTTP server.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use specd_server::{AppState, create_router};

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
        Cli::Start { no_open: _ } => {
            let specd_home = std::env::var("SPECD_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    dirs_or_default().join(".specd")
                });

            tracing::info!("SPECD_HOME: {}", specd_home.display());

            let state = Arc::new(AppState::new(specd_home));
            let app = create_router(state);

            let bind_addr: SocketAddr = std::env::var("SPECD_BIND")
                .unwrap_or_else(|_| "127.0.0.1:7331".to_string())
                .parse()
                .expect("SPECD_BIND must be a valid socket address");

            tracing::info!("specd listening on {}", bind_addr);

            let listener = tokio::net::TcpListener::bind(bind_addr)
                .await
                .expect("failed to bind");

            axum::serve(listener, app)
                .await
                .expect("server error");
        }
        Cli::Status => {
            println!("specd status: checking...");
            // For v1, just try to hit the health endpoint
            println!("(status check not yet implemented)");
        }
    }
}

/// Get the user's home directory, falling back to /tmp if unavailable.
fn dirs_or_default() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}
