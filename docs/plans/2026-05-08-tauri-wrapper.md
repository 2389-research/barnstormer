# Barnstormer Tauri Wrapper Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a macOS-only Tauri desktop app that launches Barnstormer's existing Axum web UI from an embedded runtime and installs as a native `.app`.

**Architecture:** Extract the current CLI `start` path into a shared Rust runtime crate that can start Barnstormer's Axum server on loopback, report the resolved local URL, and shut down cleanly. Add a new Tauri workspace crate that uses that runtime in-process, opens the main app window to the embedded local URL, and persists provider settings locally so the installed app does not depend on terminal environment variables.

**Tech Stack:** Rust workspace crates, Axum, Tokio, Tauri v2, Askama/HTMX existing web UI, JSON settings file in macOS Application Support, `cargo test`, `cargo check`, `cargo tauri build`

---

Implementation notes:

- Follow `@superpowers:test-driven-development` for each code task.
- Before claiming success, run `@superpowers:verification-before-completion`.
- Keep commits small and task-scoped.

## Task 1: Extract a shared Barnstormer runtime crate

**Files:**
- Create: `crates/barnstormer-runtime/Cargo.toml`
- Create: `crates/barnstormer-runtime/src/lib.rs`
- Create: `crates/barnstormer-runtime/src/config.rs`
- Create: `crates/barnstormer-runtime/src/server.rs`
- Modify: `Cargo.toml`
- Modify: `src/main.rs`
- Test: `crates/barnstormer-runtime/src/config.rs`

**Step 1: Write the failing config tests**

Add unit tests to `crates/barnstormer-runtime/src/config.rs` that prove explicit runtime options can override CLI defaults without mutating current behavior.

```rust
#[test]
fn config_uses_explicit_home_when_provided() {
    let config = RuntimeConfig::from_parts(RuntimeOptions {
        home: Some(PathBuf::from("/tmp/barnstormer-test")),
        bind: None,
        auth_token: None,
        open_browser: false,
    })
    .unwrap();

    assert_eq!(config.home, PathBuf::from("/tmp/barnstormer-test"));
}

#[test]
fn config_allows_ephemeral_loopback_bind() {
    let config = RuntimeConfig::from_parts(RuntimeOptions {
        home: Some(PathBuf::from("/tmp/barnstormer-test")),
        bind: Some("127.0.0.1:0".parse().unwrap()),
        auth_token: None,
        open_browser: false,
    })
    .unwrap();

    assert_eq!(config.bind.ip().to_string(), "127.0.0.1");
    assert_eq!(config.bind.port(), 0);
}
```

**Step 2: Run the test to verify it fails**

Run: `cargo test -p barnstormer-runtime config_uses_explicit_home_when_provided -- --nocapture`

Expected: FAIL because the crate and config types do not exist yet.

**Step 3: Write the minimal shared runtime config implementation**

Create the new crate and move startup-only configuration logic out of `src/main.rs`.

```rust
#[derive(Debug, Clone, Default)]
pub struct RuntimeOptions {
    pub home: Option<PathBuf>,
    pub bind: Option<SocketAddr>,
    pub auth_token: Option<String>,
    pub open_browser: bool,
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub home: PathBuf,
    pub bind: SocketAddr,
    pub auth_token: Option<String>,
    pub open_browser: bool,
}
```

Export the new config types from `crates/barnstormer-runtime/src/lib.rs`, add the crate to workspace dependencies in `Cargo.toml`, and update `src/main.rs` to construct `RuntimeOptions` instead of doing all startup work inline.

**Step 4: Run the tests to verify they pass**

Run: `cargo test -p barnstormer-runtime --lib`

Expected: PASS for the new config tests.

**Step 5: Commit**

```bash
git add Cargo.toml src/main.rs crates/barnstormer-runtime
git commit -m "refactor: extract shared barnstormer runtime config"
```

## Task 2: Add embedded server lifecycle management with integration coverage

**Files:**
- Modify: `crates/barnstormer-runtime/src/lib.rs`
- Modify: `crates/barnstormer-runtime/src/server.rs`
- Modify: `src/main.rs`
- Test: `crates/barnstormer-runtime/tests/server_lifecycle.rs`

**Step 1: Write the failing lifecycle integration test**

Create `crates/barnstormer-runtime/tests/server_lifecycle.rs` proving the runtime can bind on an ephemeral port, answer `/health`, and shut down cleanly.

```rust
#[tokio::test]
async fn embedded_server_reports_url_and_shuts_down() {
    let temp = tempfile::tempdir().unwrap();
    let handle = barnstormer_runtime::launch(RuntimeOptions {
        home: Some(temp.path().to_path_buf()),
        bind: Some("127.0.0.1:0".parse().unwrap()),
        auth_token: None,
        open_browser: false,
    })
    .await
    .unwrap();

    let health: serde_json::Value = reqwest::get(format!("{}/health", handle.local_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(health["ok"], true);

    handle.shutdown().await.unwrap();
}
```

**Step 2: Run the test to verify it fails**

Run: `cargo test -p barnstormer-runtime embedded_server_reports_url_and_shuts_down -- --nocapture`

Expected: FAIL because `launch`, `local_url`, and graceful shutdown do not exist yet.

**Step 3: Implement the shared runtime launcher**

Add a lifecycle API that both CLI and Tauri can call.

```rust
pub struct ServerHandle {
    pub local_url: String,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
    join_handle: tokio::task::JoinHandle<anyhow::Result<()>>,
}

pub async fn launch(options: RuntimeOptions) -> anyhow::Result<ServerHandle> {
    let config = RuntimeConfig::from_parts(options)?;
    let state = build_state(&config.home).await?;
    let app = create_router(state, config.auth_token.clone());
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    let local_addr = listener.local_addr()?;
    let local_url = format!("http://{}", local_addr);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let join_handle = tokio::spawn(run_server(listener, app, shutdown_rx));

    Ok(ServerHandle {
        local_url,
        shutdown_tx,
        join_handle,
    })
}
```

Update `src/main.rs` so `Cli::Start` calls `barnstormer_runtime::launch(...)`, optionally opens the browser to `handle.local_url`, and awaits the returned join handle instead of duplicating startup logic.

**Step 4: Run the tests to verify they pass**

Run: `cargo test -p barnstormer-runtime --tests`

Expected: PASS, including the new lifecycle test.

**Step 5: Commit**

```bash
git add src/main.rs crates/barnstormer-runtime
git commit -m "refactor: add embedded server lifecycle runtime"
```

## Task 3: Scaffold the Tauri desktop crate and open the main Barnstormer window

**Files:**
- Create: `crates/barnstormer-tauri/Cargo.toml`
- Create: `crates/barnstormer-tauri/build.rs`
- Create: `crates/barnstormer-tauri/src/main.rs`
- Create: `crates/barnstormer-tauri/src/lib.rs`
- Create: `crates/barnstormer-tauri/tauri.conf.json`
- Create: `crates/barnstormer-tauri/capabilities/default.json`
- Modify: `Cargo.toml`
- Test: `crates/barnstormer-tauri/src/lib.rs`

**Step 1: Write the failing desktop bootstrap test**

Add a small unit test in `crates/barnstormer-tauri/src/lib.rs` that proves the app resolves a desktop runtime mode with no browser auto-open and loopback-only bind.

```rust
#[test]
fn desktop_launch_uses_embedded_server_defaults() {
    let launch = desktop_launch_options(PathBuf::from("/tmp/barnstormer-ui"));

    assert!(!launch.open_browser);
    assert_eq!(launch.bind.unwrap().ip().to_string(), "127.0.0.1");
}
```

**Step 2: Run the test to verify it fails**

Run: `cargo test -p barnstormer-tauri desktop_launch_uses_embedded_server_defaults -- --nocapture`

Expected: FAIL because the Tauri crate does not exist yet.

**Step 3: Implement the desktop shell**

Create a Tauri v2 crate that starts the shared runtime in `setup`, stores the returned `ServerHandle` in managed state, and opens the main webview window to the embedded server URL.

```rust
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let app_home = app.path().app_data_dir().expect("app data dir");
            let runtime = tauri::async_runtime::block_on(barnstormer_runtime::launch(
                desktop_launch_options(app_home),
            ))?;

            let url = runtime.local_url.parse()?;
            app.manage(Mutex::new(Some(runtime)));

            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::External(url),
            )
            .title("Barnstormer")
            .build()?;

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build Barnstormer desktop app")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                if let Some(handle) = app.state::<Mutex<Option<ServerHandle>>>().lock().unwrap().take() {
                    tauri::async_runtime::spawn(async move {
                        let _ = handle.shutdown().await;
                    });
                }
            }
        });
}
```

Use the official Tauri project shape: `build.rs` with `tauri_build::build()`, `tauri.conf.json` at the crate root, and a default capability file.

**Step 4: Run the checks to verify it passes**

Run: `cargo check -p barnstormer-tauri`

Expected: PASS with the new desktop crate compiling.

**Step 5: Commit**

```bash
git add Cargo.toml crates/barnstormer-tauri
git commit -m "feat: scaffold tauri desktop shell"
```

## Task 4: Persist provider settings locally and bootstrap the server from saved desktop config

**Files:**
- Create: `crates/barnstormer-tauri/src/settings.rs`
- Create: `crates/barnstormer-tauri/src/commands.rs`
- Create: `crates/barnstormer-tauri/ui/settings.html`
- Create: `crates/barnstormer-tauri/ui/settings.js`
- Modify: `crates/barnstormer-tauri/src/lib.rs`
- Modify: `crates/barnstormer-tauri/tauri.conf.json`
- Modify: `crates/barnstormer-tauri/capabilities/default.json`
- Test: `crates/barnstormer-tauri/src/settings.rs`

**Step 1: Write the failing settings persistence tests**

Add unit tests for a JSON-backed desktop settings file.

```rust
#[test]
fn settings_round_trip_preserves_provider_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop-settings.json");

    let expected = DesktopSettings {
        default_provider: "anthropic".into(),
        anthropic_api_key: Some("test-key".into()),
        openai_api_key: None,
        gemini_api_key: None,
    };

    expected.save(&path).unwrap();
    let actual = DesktopSettings::load(&path).unwrap().unwrap();

    assert_eq!(actual.default_provider, "anthropic");
    assert_eq!(actual.anthropic_api_key.as_deref(), Some("test-key"));
}
```

**Step 2: Run the test to verify it fails**

Run: `cargo test -p barnstormer-tauri settings_round_trip_preserves_provider_keys -- --nocapture`

Expected: FAIL because `DesktopSettings` does not exist yet.

**Step 3: Implement persisted desktop settings and bootstrap flow**

Store settings in the app data directory and project them into process env vars before launching the embedded server. This keeps the existing `ProviderStatus::detect()` and `create_llm_client()` code working in the first pass.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct DesktopSettings {
    pub default_provider: String,
    pub default_model: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub anthropic_base_url: Option<String>,
    pub openai_api_key: Option<String>,
    pub openai_base_url: Option<String>,
    pub gemini_api_key: Option<String>,
    pub gemini_base_url: Option<String>,
}

impl DesktopSettings {
    pub fn apply_to_env(&self) {
        set_or_clear("BARNSTORMER_DEFAULT_PROVIDER", Some(&self.default_provider));
        set_or_clear("BARNSTORMER_DEFAULT_MODEL", self.default_model.as_deref());
        set_or_clear("ANTHROPIC_API_KEY", self.anthropic_api_key.as_deref());
        set_or_clear("ANTHROPIC_BASE_URL", self.anthropic_base_url.as_deref());
        set_or_clear("OPENAI_API_KEY", self.openai_api_key.as_deref());
        set_or_clear("OPENAI_BASE_URL", self.openai_base_url.as_deref());
        set_or_clear("GEMINI_API_KEY", self.gemini_api_key.as_deref());
        set_or_clear("GEMINI_BASE_URL", self.gemini_base_url.as_deref());
    }
}
```

In `src/lib.rs`:

- If saved settings or process env already provide at least one provider key, start the embedded server and open the main Barnstormer window.
- If not, open a small bundled `settings.html` window first.
- Expose Tauri commands to load/save settings and, after save, restart the runtime then open the main window.

**Step 4: Run the tests and checks to verify it passes**

Run: `cargo test -p barnstormer-tauri --lib`

Expected: PASS for the settings tests.

Run: `cargo check -p barnstormer-tauri`

Expected: PASS with the settings bootstrap flow compiling.

**Step 5: Commit**

```bash
git add crates/barnstormer-tauri
git commit -m "feat: add desktop provider settings bootstrap"
```

## Task 5: Finish macOS bundle configuration, docs, and release verification

**Files:**
- Modify: `crates/barnstormer-tauri/tauri.conf.json`
- Modify: `README.md`
- Create: `docs/desktop-app.md`
- Test: `crates/barnstormer-runtime/tests/server_lifecycle.rs`

**Step 1: Add the failing docs/build verification target**

Write down the exact verification matrix in `docs/desktop-app.md` before changing the build config:

```markdown
- `cargo test -p barnstormer-runtime --tests`
- `cargo test -p barnstormer-tauri --lib`
- `cargo check -p barnstormer-tauri`
- `cargo tauri build --bundles app --config crates/barnstormer-tauri/tauri.conf.json`
```

**Step 2: Run the non-bundle verification first**

Run: `cargo test -p barnstormer-runtime --tests && cargo test -p barnstormer-tauri --lib && cargo check -p barnstormer-tauri`

Expected: PASS before attempting a macOS bundle.

**Step 3: Implement macOS packaging details and docs**

Update `crates/barnstormer-tauri/tauri.conf.json` with the desktop app identifier, product name, and macOS bundle settings.

```json
{
  "productName": "Barnstormer",
  "identifier": "ai.2389.barnstormer",
  "build": {
    "frontendDist": "ui"
  },
  "bundle": {
    "active": true,
    "targets": ["app"],
    "macOS": {
      "minimumSystemVersion": "12.0"
    }
  }
}
```

Document:

- how the desktop app stores data under macOS Application Support
- how provider settings bootstrap works
- how to build the `.app`
- which distribution steps are still deferred: signing, notarization, DMG, auto-update

**Step 4: Run the full verification suite**

Run: `cargo test -p barnstormer-runtime --tests`

Expected: PASS.

Run: `cargo test -p barnstormer-tauri --lib`

Expected: PASS.

Run: `cargo check -p barnstormer-tauri`

Expected: PASS.

Run: `cargo tauri build --bundles app --config crates/barnstormer-tauri/tauri.conf.json`

Expected: PASS and produce a macOS `.app` bundle under the Tauri target output.

Manual verification:

- Launch the built app with no saved provider settings and confirm the settings window opens first.
- Save one provider key and confirm the main Barnstormer window opens.
- Quit and reopen the app and confirm the saved settings are reused.
- Create or reopen a spec and confirm data persists under the app data directory.

**Step 5: Commit**

```bash
git add README.md docs/desktop-app.md crates/barnstormer-tauri/tauri.conf.json
git commit -m "docs: add macos desktop app build instructions"
```
