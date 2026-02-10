// ABOUTME: Entry point for the specd binary.
// ABOUTME: Parses CLI arguments, initializes tracing, and starts the HTTP server.

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "specd=debug,tower_http=debug".parse().unwrap()),
        )
        .init();

    tracing::info!("specd starting up");
}
