// ABOUTME: Desktop settings persistence for the Barnstormer Tauri shell.
// ABOUTME: Stores provider credentials and launch preferences in the app data directory.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            default_provider: "anthropic".to_string(),
            default_model: None,
            anthropic_api_key: None,
            anthropic_base_url: None,
            openai_api_key: None,
            openai_base_url: None,
            gemini_api_key: None,
            gemini_base_url: None,
        }
    }
}

impl DesktopSettings {
    pub fn load(path: &Path) -> anyhow::Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }

        let contents = std::fs::read_to_string(path)?;
        let settings = serde_json::from_str(&contents)?;
        Ok(Some(settings))
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let contents = serde_json::to_string_pretty(self)?;
        std::fs::write(path, contents)?;
        Ok(())
    }

    pub fn has_any_provider_key(&self) -> bool {
        [
            self.anthropic_api_key.as_deref(),
            self.openai_api_key.as_deref(),
            self.gemini_api_key.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|value| !value.trim().is_empty())
    }

    pub fn settings_path(app_home: &Path) -> PathBuf {
        app_home.join("desktop-settings.json")
    }

    pub fn apply_to_env(&self) -> anyhow::Result<()> {
        set_or_clear("BARNSTORMER_DEFAULT_PROVIDER", Some(&self.default_provider));
        set_or_clear("BARNSTORMER_DEFAULT_MODEL", self.default_model.as_deref());
        set_or_clear("ANTHROPIC_API_KEY", self.anthropic_api_key.as_deref());
        set_or_clear("ANTHROPIC_BASE_URL", self.anthropic_base_url.as_deref());
        set_or_clear("OPENAI_API_KEY", self.openai_api_key.as_deref());
        set_or_clear("OPENAI_BASE_URL", self.openai_base_url.as_deref());
        set_or_clear("GEMINI_API_KEY", self.gemini_api_key.as_deref());
        set_or_clear("GEMINI_BASE_URL", self.gemini_base_url.as_deref());
        Ok(())
    }

}

fn set_or_clear(key: &str, value: Option<&str>) {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => {
            // SAFETY: desktop startup and settings writes intentionally mutate
            // process env so existing provider detection code can be reused.
            unsafe { std::env::set_var(key, value) };
        }
        None => {
            // SAFETY: desktop startup and settings writes intentionally mutate
            // process env so existing provider detection code can be reused.
            unsafe { std::env::remove_var(key) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DesktopSettings;

    #[test]
    fn settings_round_trip_preserves_provider_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desktop-settings.json");

        let expected = DesktopSettings {
            default_provider: "anthropic".into(),
            default_model: None,
            anthropic_api_key: Some("test-key".into()),
            anthropic_base_url: None,
            openai_api_key: None,
            openai_base_url: None,
            gemini_api_key: None,
            gemini_base_url: None,
        };

        expected.save(&path).unwrap();
        let actual = DesktopSettings::load(&path).unwrap().unwrap();

        assert_eq!(actual.default_provider, "anthropic");
        assert_eq!(actual.anthropic_api_key.as_deref(), Some("test-key"));
        assert_eq!(
            DesktopSettings::settings_path(&dir.path().to_path_buf()),
            path
        );
    }
}
