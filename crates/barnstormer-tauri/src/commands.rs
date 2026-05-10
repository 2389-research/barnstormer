// ABOUTME: Tauri commands used by the Barnstormer desktop shell settings UI.
// ABOUTME: Loads and saves desktop provider settings, then launches the embedded app window.

use tauri::{AppHandle, Manager, Runtime, State};

use crate::DesktopAppState;
use crate::settings::DesktopSettings;

#[tauri::command]
pub fn load_settings(state: State<'_, DesktopAppState>) -> Result<DesktopSettings, String> {
    DesktopSettings::load(&state.settings_path)
        .map(|settings| settings.unwrap_or_default())
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub fn save_settings<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, DesktopAppState>,
    settings: DesktopSettings,
) -> Result<(), String> {
    // `apply_to_env` mutates process env vars that the embedded server reads
    // from other threads. Refuse to run while the server is alive — v1 only
    // surfaces the settings window before launch, so this should only fire if
    // a future entrypoint reaches the command after the server has booted.
    if state.runtime.server.lock().unwrap().is_some() {
        return Err(
            "Settings cannot be changed while Barnstormer is running. Quit and reopen the app to update credentials."
                .to_string(),
        );
    }

    settings
        .save(&state.settings_path)
        .map_err(|err| err.to_string())?;
    settings.apply_to_env().map_err(|err| err.to_string())?;

    let local_url = crate::start_server_if_needed(&app).map_err(|err| err.to_string())?;
    crate::open_main_window(&app, &local_url).map_err(|err| err.to_string())?;

    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.close();
    }

    Ok(())
}
