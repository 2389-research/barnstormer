// ABOUTME: Integration test for the shared barnstormer-runtime server lifecycle.
// ABOUTME: Boots the embedded server, probes /health, and exercises graceful shutdown.

use barnstormer_runtime::RuntimeOptions;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn runtime_options(home: std::path::PathBuf) -> RuntimeOptions {
    RuntimeOptions {
        home: Some(home),
        bind: Some("127.0.0.1:0".parse().unwrap()),
        auth_token: None,
        static_dir: None,
        open_browser: false,
        disable_auth_fallback: true,
    }
}

#[tokio::test]
async fn embedded_server_reports_url_and_shuts_down() {
    let temp = tempfile::tempdir().unwrap();
    let handle = barnstormer_runtime::launch(runtime_options(temp.path().to_path_buf()))
    .await
    .unwrap();

    let health: serde_json::Value = reqwest::get(format!("{}/health", handle.local_url()))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(health["status"], "ok");

    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn embedded_server_boots_with_unlimited_token_budget() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let prior = std::env::var("BARNSTORMER_TOKEN_BUDGET").ok();
    unsafe { std::env::set_var("BARNSTORMER_TOKEN_BUDGET", "0") };
    let temp = tempfile::tempdir().unwrap();

    let handle = barnstormer_runtime::launch(runtime_options(temp.path().to_path_buf()))
        .await
        .unwrap();
    let health = reqwest::get(format!("{}/health", handle.local_url()))
        .await
        .unwrap();
    assert!(health.status().is_success());
    handle.shutdown().await.unwrap();

    match prior {
        Some(value) => unsafe { std::env::set_var("BARNSTORMER_TOKEN_BUDGET", value) },
        None => unsafe { std::env::remove_var("BARNSTORMER_TOKEN_BUDGET") },
    }
}

#[tokio::test]
async fn embedded_server_boots_with_tiny_token_budget() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let prior = std::env::var("BARNSTORMER_TOKEN_BUDGET").ok();
    unsafe { std::env::set_var("BARNSTORMER_TOKEN_BUDGET", "100") };
    let temp = tempfile::tempdir().unwrap();

    let handle = barnstormer_runtime::launch(runtime_options(temp.path().to_path_buf()))
        .await
        .unwrap();
    let health = reqwest::get(format!("{}/health", handle.local_url()))
        .await
        .unwrap();
    assert!(health.status().is_success());
    handle.shutdown().await.unwrap();

    match prior {
        Some(value) => unsafe { std::env::set_var("BARNSTORMER_TOKEN_BUDGET", value) },
        None => unsafe { std::env::remove_var("BARNSTORMER_TOKEN_BUDGET") },
    }
}

#[tokio::test]
async fn embedded_server_rejects_invalid_token_budget() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let prior = std::env::var("BARNSTORMER_TOKEN_BUDGET").ok();
    unsafe { std::env::set_var("BARNSTORMER_TOKEN_BUDGET", "invalid") };
    let temp = tempfile::tempdir().unwrap();

    let error = barnstormer_runtime::launch(runtime_options(temp.path().to_path_buf()))
        .await
        .err()
        .expect("invalid token budget should fail startup");
    assert!(error.to_string().contains("BARNSTORMER_TOKEN_BUDGET"));

    match prior {
        Some(value) => unsafe { std::env::set_var("BARNSTORMER_TOKEN_BUDGET", value) },
        None => unsafe { std::env::remove_var("BARNSTORMER_TOKEN_BUDGET") },
    }
}
