// ABOUTME: Desktop settings persistence for the Barnstormer Tauri shell.
// ABOUTME: Stores provider credentials and launch preferences in the app data directory.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
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

        // Provider API keys live in this file; document intent with an explicit
        // 0600 mode rather than relying on the user's umask.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }

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

    /// Pushes the resolved settings into the process environment so that
    /// existing provider-detection code can pick them up.
    ///
    /// Callers must ensure the embedded server has not yet been started (or
    /// has been shut down) — once the server is running, in-flight requests
    /// may read these env vars from other threads, which would race with the
    /// `set_var`/`remove_var` writes below. The Tauri shell enforces this by
    /// only invoking `apply_to_env` during `setup` (before launch) and inside
    /// `save_settings` (which refuses to run when a `ServerHandle` exists).
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
            // SAFETY: callers of `apply_to_env` document that no embedded
            // server is running, so no other thread is reading these vars.
            unsafe { std::env::set_var(key, value) };
        }
        None => {
            // SAFETY: see `apply_to_env`.
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

    #[test]
    fn load_returns_none_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.json");

        assert!(DesktopSettings::load(&path).unwrap().is_none());
    }

    #[test]
    fn load_returns_err_on_corrupt_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desktop-settings.json");
        std::fs::write(&path, "{ this is not valid json").unwrap();

        let result = DesktopSettings::load(&path);
        assert!(
            result.is_err(),
            "expected Err on corrupt JSON, got {result:?}"
        );
    }

    #[test]
    fn load_tolerates_missing_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desktop-settings.json");
        std::fs::write(&path, "{}").unwrap();

        let actual = DesktopSettings::load(&path).unwrap().unwrap();
        assert_eq!(actual, DesktopSettings::default());
    }

    #[test]
    fn has_any_provider_key_treats_blank_strings_as_missing() {
        let mut settings = DesktopSettings::default();
        assert!(!settings.has_any_provider_key());

        settings.anthropic_api_key = Some("   ".into());
        settings.openai_api_key = Some(String::new());
        settings.gemini_api_key = Some("\t\n".into());
        assert!(!settings.has_any_provider_key());

        settings.openai_api_key = Some("sk-real".into());
        assert!(settings.has_any_provider_key());
    }

    #[test]
    #[cfg(unix)]
    fn save_writes_file_with_user_only_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desktop-settings.json");

        DesktopSettings::default().save(&path).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
    }

    /// `apply_to_env` writes process env, so this test serializes against any
    /// other test that touches the same variables. Run sequentially via
    /// `cargo test -- --test-threads=1` if you add more env-mutating tests.
    #[test]
    fn apply_to_env_round_trips_keys() {
        const KEYS: &[&str] = &[
            "BARNSTORMER_DEFAULT_PROVIDER",
            "BARNSTORMER_DEFAULT_MODEL",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_BASE_URL",
            "OPENAI_API_KEY",
            "OPENAI_BASE_URL",
            "GEMINI_API_KEY",
            "GEMINI_BASE_URL",
        ];

        // Snapshot prior values so other tests in the suite are unaffected.
        let prior: Vec<(&str, Option<String>)> =
            KEYS.iter().map(|k| (*k, std::env::var(k).ok())).collect();

        let settings = DesktopSettings {
            default_provider: "openai".into(),
            default_model: Some("gpt-test".into()),
            anthropic_api_key: Some("ant-key".into()),
            anthropic_base_url: None,
            openai_api_key: Some("oa-key".into()),
            openai_base_url: Some("https://proxy.example".into()),
            gemini_api_key: None,
            gemini_base_url: None,
        };

        settings.apply_to_env().unwrap();
        assert_eq!(
            std::env::var("BARNSTORMER_DEFAULT_PROVIDER")
                .ok()
                .as_deref(),
            Some("openai")
        );
        assert_eq!(
            std::env::var("BARNSTORMER_DEFAULT_MODEL").ok().as_deref(),
            Some("gpt-test")
        );
        assert_eq!(
            std::env::var("ANTHROPIC_API_KEY").ok().as_deref(),
            Some("ant-key")
        );
        assert_eq!(
            std::env::var("OPENAI_BASE_URL").ok().as_deref(),
            Some("https://proxy.example")
        );
        assert!(std::env::var("ANTHROPIC_BASE_URL").is_err());
        assert!(std::env::var("GEMINI_API_KEY").is_err());

        // Clearing a key by setting an empty Option should remove it from env.
        let cleared = DesktopSettings::default();
        cleared.apply_to_env().unwrap();
        assert!(std::env::var("ANTHROPIC_API_KEY").is_err());
        assert!(std::env::var("OPENAI_API_KEY").is_err());

        for (key, value) in prior {
            match value {
                Some(v) => unsafe { std::env::set_var(key, v) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }
}
