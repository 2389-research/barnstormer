// ABOUTME: Entry point for the barnstormer binary.
// ABOUTME: Parses CLI arguments with clap, recovers specs, spawns actors, and starts the Axum HTTP server.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use barnstormer_agent::client::create_llm_client;
use barnstormer_agent::import::{parse_with_llm, to_commands};
use barnstormer_server::{AppState, ProviderStatus, create_router};
use barnstormer_store::{JsonlLog, StorageManager};

#[derive(Parser)]
#[command(name = "barnstormer", about = "Agentic spec builder")]
enum Cli {
    /// Start the barnstormer server
    Start {
        /// Do not open the browser on startup
        #[arg(long, default_value = "false")]
        no_open: bool,
    },
    /// Check if barnstormer is running
    Status,
    /// Import a spec from any file or text (uses LLM to extract structure)
    Import {
        /// Path to file to import, or "-" for stdin
        #[arg(value_name = "FILE")]
        file: Option<String>,

        /// Import inline text instead of a file
        #[arg(long)]
        text: Option<String>,

        /// Format hint for the LLM (e.g. "dot", "yaml", "markdown")
        #[arg(long, short)]
        format: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    // Load .env if present (ignoring errors if missing)
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "barnstormer=debug,tower_http=debug".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();

    match cli {
        Cli::Start { no_open } => {
            let barnstormer_home = std::env::var("BARNSTORMER_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| dirs_or_default().join(".barnstormer"));

            tracing::info!("BARNSTORMER_HOME: {}", barnstormer_home.display());

            // Initialize StorageManager
            let storage = StorageManager::new(barnstormer_home.clone())
                .expect("failed to initialize storage manager");

            // Recover all existing specs
            let recovered_specs = storage
                .recover_all_specs()
                .expect("failed to recover specs");

            tracing::info!("recovered {} specs", recovered_specs.len());

            // Create AppState and spawn actors for recovered specs
            let state = Arc::new(AppState::new(barnstormer_home.clone(), ProviderStatus::detect()));
            {
                let mut actors = state.actors.write().await;
                let mut persisters = state.event_persisters.write().await;
                for (spec_id, spec_state) in recovered_specs {
                    let handle = barnstormer_core::spawn(spec_id, spec_state);
                    // Subscribe a background persister so all future events
                    // (including agent-produced ones) are written to JSONL.
                    let persister = barnstormer_server::web::spawn_event_persister(
                        &handle, spec_id, &barnstormer_home,
                    );
                    persisters.insert(spec_id, persister);
                    actors.insert(spec_id, handle);
                    tracing::info!("spawned actor for spec {}", spec_id);
                }
            }

            // Agents start paused until the user explicitly enables them per-spec
            // via the web UI "Start agents" button.
            tracing::info!("agents paused on startup — enable per-spec via the web UI");

            let auth_token = std::env::var("BARNSTORMER_AUTH_TOKEN")
                .ok()
                .filter(|t| !t.is_empty());

            let app = create_router(state, auth_token);

            let bind_addr: SocketAddr = std::env::var("BARNSTORMER_BIND")
                .unwrap_or_else(|_| "127.0.0.1:7331".to_string())
                .parse()
                .expect("BARNSTORMER_BIND must be a valid socket address");

            let url = format!("http://{}", bind_addr);
            tracing::info!("barnstormer listening on {}", url);

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
                std::env::var("BARNSTORMER_BIND").unwrap_or_else(|_| "127.0.0.1:7331".to_string());

            println!("barnstormer status: checking {}...", bind_addr);

            match std::net::TcpStream::connect(&bind_addr) {
                Ok(_) => println!("barnstormer is running on {}", bind_addr),
                Err(_) => println!("barnstormer is not running on {}", bind_addr),
            }
        }
        Cli::Import { file, text, format } => {
            if let Err(e) = run_import(file, text, format).await {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
    }
}

/// Execute the import subcommand: read input, call LLM, persist spec.
async fn run_import(
    file: Option<String>,
    text: Option<String>,
    format: Option<String>,
) -> Result<(), anyhow::Error> {
    // Read input content
    let content = match (file.as_deref(), text) {
        (_, Some(inline)) => inline,
        (Some("-"), None) => {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf
        }
        (Some(path), None) => std::fs::read_to_string(path)?,
        (None, None) => {
            return Err(anyhow::anyhow!(
                "provide a file path, \"-\" for stdin, or --text"
            ));
        }
    };

    if content.trim().is_empty() {
        return Err(anyhow::anyhow!("input content is empty"));
    }

    // Detect source format from file extension if not explicitly provided
    let source_hint = format.as_deref().or_else(|| {
        file.as_deref().and_then(|f| {
            std::path::Path::new(f)
                .extension()
                .and_then(|ext| ext.to_str())
        })
    });

    // Resolve LLM provider
    let provider_status = ProviderStatus::detect();
    let (client, model) = create_llm_client(
        &provider_status.default_provider,
        provider_status.default_model.as_deref(),
    )?;

    println!("Importing via {} ({})...", provider_status.default_provider, model);

    // Parse content via LLM
    let import_result = parse_with_llm(&content, source_hint, &client, &model).await?;

    let title = import_result.spec.title.clone();
    let card_count = import_result.cards.len();
    let commands = to_commands(&import_result);

    // Set up storage
    let barnstormer_home = std::env::var("BARNSTORMER_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs_or_default().join(".barnstormer"));

    let storage = StorageManager::new(barnstormer_home.clone())?;

    let spec_id = ulid::Ulid::new();
    let spec_dir = storage.create_spec_dir(&spec_id)?;

    let log_path = spec_dir.join("events.jsonl");
    let mut log = JsonlLog::open(&log_path)?;

    // Spawn actor and send all commands
    let handle = barnstormer_core::spawn(spec_id, barnstormer_core::SpecState::new());
    for cmd in commands {
        let events = handle.send_command(cmd).await?;
        for event in &events {
            log.append(event)?;
        }
    }

    println!("Imported spec: {}", title);
    println!("  spec_id: {}", spec_id);
    println!("  cards: {}", card_count);
    println!("  stored: {}", log_path.display());

    Ok(())
}

/// Get the user's home directory, falling back to /tmp if unavailable.
fn dirs_or_default() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}
