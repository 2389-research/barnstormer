// ABOUTME: Integration test for the shared barnstormer-runtime server lifecycle.
// ABOUTME: Boots the embedded server, probes /health, and exercises graceful shutdown.

use barnstormer_runtime::RuntimeOptions;

#[tokio::test]
async fn embedded_server_reports_url_and_shuts_down() {
    let temp = tempfile::tempdir().unwrap();
    let handle = barnstormer_runtime::launch(RuntimeOptions {
        home: Some(temp.path().to_path_buf()),
        bind: Some("127.0.0.1:0".parse().unwrap()),
        auth_token: None,
        static_dir: None,
        open_browser: false,
        disable_auth_fallback: true,
    })
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
