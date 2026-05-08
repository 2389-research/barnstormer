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
