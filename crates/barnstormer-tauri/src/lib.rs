// ABOUTME: Tauri desktop shell for Barnstormer.
// ABOUTME: Starts the embedded Barnstormer server and opens the main application window.

mod commands;
mod settings;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Mutex;

use barnstormer_runtime::{RuntimeOptions, ServerHandle, launch};
use barnstormer_server::ProviderStatus;
use tauri::{Manager, Runtime};

use settings::DesktopSettings;

struct DesktopRuntimeState {
    server: Mutex<Option<ServerHandle>>,
}

pub(crate) struct DesktopAppState {
    app_home: PathBuf,
    settings_path: PathBuf,
    static_dir: PathBuf,
    runtime: DesktopRuntimeState,
}

pub fn desktop_launch_options(app_home: PathBuf, static_dir: PathBuf) -> RuntimeOptions {
    RuntimeOptions {
        home: Some(app_home),
        bind: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
        auth_token: None,
        static_dir: Some(static_dir),
        open_browser: false,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::load_settings,
            commands::save_settings
        ])
        .setup(|app| {
            let app_home = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_home)?;
            let static_dir = resolve_desktop_static_dir(&app.handle())?;
            let settings_path = DesktopSettings::settings_path(&app_home);
            let saved_settings = DesktopSettings::load(&settings_path)?.unwrap_or_default();
            if saved_settings.has_any_provider_key() {
                saved_settings.apply_to_env()?;
            }

            app.manage(DesktopAppState {
                app_home,
                settings_path,
                static_dir,
                runtime: DesktopRuntimeState {
                    server: Mutex::new(None),
                },
            });

            if ProviderStatus::detect().any_available {
                let local_url = start_server_if_needed(&app.handle())?;
                open_main_window(&app.handle(), &local_url)?;
            } else {
                open_settings_window(&app.handle())?;
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build Barnstormer desktop app")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event
                && let Some(handle) = app
                    .state::<DesktopAppState>()
                    .runtime
                    .server
                    .lock()
                    .unwrap()
                    .take()
            {
                tauri::async_runtime::block_on(handle.shutdown())
                    .expect("failed to shut down Barnstormer server");
            }
        });
}

pub(crate) fn start_server_if_needed<R: Runtime>(app: &tauri::AppHandle<R>) -> anyhow::Result<String> {
    let state = app.state::<DesktopAppState>();
    let mut server = state.runtime.server.lock().unwrap();

    if let Some(existing) = server.as_ref() {
        return Ok(existing.local_url().to_string());
    }

    let launched = tauri::async_runtime::block_on(launch(desktop_launch_options(
        state.app_home.clone(),
        state.static_dir.clone(),
    )))?;
    let local_url = launched.local_url().to_string();
    *server = Some(launched);
    Ok(local_url)
}

pub(crate) fn open_main_window<R: Runtime>(
    app: &tauri::AppHandle<R>,
    local_url: &str,
) -> anyhow::Result<()> {
    if app.get_webview_window("main").is_some() {
        return Ok(());
    }

    tauri::WebviewWindowBuilder::new(
        app,
        "main",
        tauri::WebviewUrl::External(local_url.parse()?),
    )
    .title("Barnstormer")
    .build()?;

    Ok(())
}

fn open_settings_window<R: Runtime>(app: &tauri::AppHandle<R>) -> anyhow::Result<()> {
    if app.get_webview_window("settings").is_some() {
        return Ok(());
    }

    tauri::WebviewWindowBuilder::new(
        app,
        "settings",
        tauri::WebviewUrl::App("settings.html".into()),
    )
    .title("Barnstormer Setup")
    .inner_size(540.0, 720.0)
    .resizable(true)
    .build()?;

    Ok(())
}

fn resolve_desktop_static_dir<R: Runtime>(app: &tauri::AppHandle<R>) -> anyhow::Result<PathBuf> {
    let bundled_static_dir = app.path().resource_dir()?.join("static");
    if bundled_static_dir.exists() {
        return Ok(bundled_static_dir);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_static_dir = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("tauri crate should live under crates/")
        .join("static");
    Ok(repo_static_dir)
}

#[cfg(test)]
mod tests {
    use super::desktop_launch_options;
    use std::path::PathBuf;

    #[test]
    fn desktop_launch_uses_embedded_server_defaults() {
        let launch = desktop_launch_options(
            PathBuf::from("/tmp/barnstormer-ui"),
            PathBuf::from("/tmp/barnstormer-static"),
        );

        assert!(!launch.open_browser);
        assert_eq!(launch.bind.unwrap().ip().to_string(), "127.0.0.1");
        assert_eq!(
            launch.static_dir.unwrap(),
            PathBuf::from("/tmp/barnstormer-static")
        );
    }
}
