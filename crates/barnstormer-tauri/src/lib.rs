// ABOUTME: Tauri desktop shell for Barnstormer.
// ABOUTME: Starts the embedded Barnstormer server and opens the main application window.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Mutex;

use barnstormer_runtime::{RuntimeOptions, ServerHandle, launch};
use tauri::Manager;

struct DesktopRuntimeState {
    server: Mutex<Option<ServerHandle>>,
}

pub fn desktop_launch_options(app_home: PathBuf) -> RuntimeOptions {
    RuntimeOptions {
        home: Some(app_home),
        bind: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
        auth_token: None,
        open_browser: false,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let app_home = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_home)?;

            let server = tauri::async_runtime::block_on(launch(desktop_launch_options(app_home)))?;
            let url = server.local_url().parse()?;

            app.manage(DesktopRuntimeState {
                server: Mutex::new(Some(server)),
            });

            tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::External(url))
                .title("Barnstormer")
                .build()?;

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build Barnstormer desktop app")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event
                && let Some(handle) = app
                    .state::<DesktopRuntimeState>()
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

#[cfg(test)]
mod tests {
    use super::desktop_launch_options;
    use std::path::PathBuf;

    #[test]
    fn desktop_launch_uses_embedded_server_defaults() {
        let launch = desktop_launch_options(PathBuf::from("/tmp/barnstormer-ui"));

        assert!(!launch.open_browser);
        assert_eq!(launch.bind.unwrap().ip().to_string(), "127.0.0.1");
    }
}
