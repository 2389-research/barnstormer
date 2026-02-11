// ABOUTME: LLM provider status detection for the barnstormer UI.
// ABOUTME: Reads environment variables to determine which providers are configured.

use serde::Serialize;

/// Status of a single LLM provider.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    pub name: String,
    pub has_api_key: bool,
    pub model: String,
    pub base_url: Option<String>,
}

/// Overall provider status for the UI.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderStatus {
    pub default_provider: String,
    pub default_model: Option<String>,
    pub providers: Vec<ProviderInfo>,
    pub any_available: bool,
}

impl ProviderStatus {
    /// Detect available LLM providers from environment variables.
    ///
    /// Checks for:
    /// - ANTHROPIC_API_KEY / ANTHROPIC_MODEL / ANTHROPIC_BASE_URL
    /// - OPENAI_API_KEY / OPENAI_MODEL / OPENAI_BASE_URL
    /// - GEMINI_API_KEY / GEMINI_MODEL / GEMINI_BASE_URL
    /// - BARNSTORMER_DEFAULT_PROVIDER / BARNSTORMER_DEFAULT_MODEL
    ///
    /// Never exposes actual API key values.
    pub fn detect() -> Self {
        let default_provider = std::env::var("BARNSTORMER_DEFAULT_PROVIDER")
            .ok()
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "anthropic".to_string());
        let default_model = std::env::var("BARNSTORMER_DEFAULT_MODEL")
            .ok()
            .filter(|m| !m.is_empty());

        let providers = vec![
            Self::check_provider(
                "anthropic",
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_MODEL",
                "ANTHROPIC_BASE_URL",
                "claude-sonnet-4-5-20250929",
            ),
            Self::check_provider(
                "openai",
                "OPENAI_API_KEY",
                "OPENAI_MODEL",
                "OPENAI_BASE_URL",
                "gpt-4o",
            ),
            Self::check_provider(
                "gemini",
                "GEMINI_API_KEY",
                "GEMINI_MODEL",
                "GEMINI_BASE_URL",
                "gemini-2.0-flash",
            ),
        ];

        let any_available = providers.iter().any(|p| p.has_api_key);

        Self {
            default_provider,
            default_model,
            providers,
            any_available,
        }
    }

    fn check_provider(
        name: &str,
        key_var: &str,
        model_var: &str,
        base_url_var: &str,
        default_model: &str,
    ) -> ProviderInfo {
        let has_api_key = std::env::var(key_var)
            .ok()
            .filter(|k| !k.is_empty())
            .is_some();
        let model = std::env::var(model_var)
            .ok()
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| default_model.to_string());
        let base_url = std::env::var(base_url_var)
            .ok()
            .filter(|u| !u.is_empty());

        ProviderInfo {
            name: name.to_string(),
            has_api_key,
            model,
            base_url,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Mutex to serialize provider tests that manipulate process-wide env vars.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// Clear all provider-related env vars so tests start from a clean slate.
    ///
    /// SAFETY: Only call while holding ENV_MUTEX to prevent concurrent env var access.
    unsafe fn clear_provider_env() {
        // SAFETY: caller holds ENV_MUTEX, ensuring no concurrent env var access
        unsafe {
            std::env::remove_var("BARNSTORMER_DEFAULT_PROVIDER");
            std::env::remove_var("BARNSTORMER_DEFAULT_MODEL");
            std::env::remove_var("ANTHROPIC_API_KEY");
            std::env::remove_var("ANTHROPIC_MODEL");
            std::env::remove_var("ANTHROPIC_BASE_URL");
            std::env::remove_var("OPENAI_API_KEY");
            std::env::remove_var("OPENAI_MODEL");
            std::env::remove_var("OPENAI_BASE_URL");
            std::env::remove_var("GEMINI_API_KEY");
            std::env::remove_var("GEMINI_MODEL");
            std::env::remove_var("GEMINI_BASE_URL");
        }
    }

    #[test]
    fn detect_with_no_env_vars() {
        let _lock = ENV_MUTEX.lock().unwrap();

        // SAFETY: holding ENV_MUTEX, no concurrent env var access
        unsafe {
            clear_provider_env();
        }

        let status = ProviderStatus::detect();

        assert_eq!(status.default_provider, "anthropic");
        assert!(status.default_model.is_none());
        assert!(!status.any_available, "no providers should be available without API keys");
        assert_eq!(status.providers.len(), 3);

        // Verify default models are set even without env vars
        let anthropic = &status.providers[0];
        assert_eq!(anthropic.name, "anthropic");
        assert!(!anthropic.has_api_key);
        assert_eq!(anthropic.model, "claude-sonnet-4-5-20250929");
        assert!(anthropic.base_url.is_none());

        let openai = &status.providers[1];
        assert_eq!(openai.name, "openai");
        assert!(!openai.has_api_key);
        assert_eq!(openai.model, "gpt-4o");
        assert!(openai.base_url.is_none());

        let gemini = &status.providers[2];
        assert_eq!(gemini.name, "gemini");
        assert!(!gemini.has_api_key);
        assert_eq!(gemini.model, "gemini-2.0-flash");
        assert!(gemini.base_url.is_none());
    }

    #[test]
    fn detect_finds_default_provider() {
        let _lock = ENV_MUTEX.lock().unwrap();

        // SAFETY: holding ENV_MUTEX, no concurrent env var access
        unsafe {
            clear_provider_env();
        }

        let status = ProviderStatus::detect();
        assert_eq!(
            status.default_provider, "anthropic",
            "default provider should be 'anthropic' when BARNSTORMER_DEFAULT_PROVIDER is not set"
        );

        // Now set a custom default provider
        // SAFETY: holding ENV_MUTEX, no concurrent env var access
        unsafe {
            std::env::set_var("BARNSTORMER_DEFAULT_PROVIDER", "openai");
        }

        let status = ProviderStatus::detect();
        assert_eq!(status.default_provider, "openai");

        // Clean up
        // SAFETY: holding ENV_MUTEX, no concurrent env var access
        unsafe {
            std::env::remove_var("BARNSTORMER_DEFAULT_PROVIDER");
        }
    }

    #[test]
    fn detect_finds_configured_provider() {
        let _lock = ENV_MUTEX.lock().unwrap();

        // SAFETY: holding ENV_MUTEX, no concurrent env var access
        unsafe {
            clear_provider_env();
            std::env::set_var("OPENAI_API_KEY", "sk-test-key-not-real");
            std::env::set_var("OPENAI_MODEL", "gpt-4-turbo");
            std::env::set_var("OPENAI_BASE_URL", "https://custom.openai.example.com");
        }

        let status = ProviderStatus::detect();

        assert!(status.any_available, "should detect at least one available provider");

        let openai = &status.providers[1];
        assert_eq!(openai.name, "openai");
        assert!(openai.has_api_key);
        assert_eq!(openai.model, "gpt-4-turbo");
        assert_eq!(openai.base_url.as_deref(), Some("https://custom.openai.example.com"));

        // Anthropic should still be unavailable
        let anthropic = &status.providers[0];
        assert!(!anthropic.has_api_key);

        // Clean up
        // SAFETY: holding ENV_MUTEX, no concurrent env var access
        unsafe {
            std::env::remove_var("OPENAI_API_KEY");
            std::env::remove_var("OPENAI_MODEL");
            std::env::remove_var("OPENAI_BASE_URL");
        }
    }

    #[test]
    fn detect_ignores_empty_api_key() {
        let _lock = ENV_MUTEX.lock().unwrap();

        // SAFETY: holding ENV_MUTEX, no concurrent env var access
        unsafe {
            clear_provider_env();
            std::env::set_var("ANTHROPIC_API_KEY", "");
        }

        let status = ProviderStatus::detect();

        assert!(!status.any_available, "empty API key should not count as available");
        assert!(!status.providers[0].has_api_key);

        // Clean up
        // SAFETY: holding ENV_MUTEX, no concurrent env var access
        unsafe {
            std::env::remove_var("ANTHROPIC_API_KEY");
        }
    }

    #[test]
    fn detect_default_model_override() {
        let _lock = ENV_MUTEX.lock().unwrap();

        // SAFETY: holding ENV_MUTEX, no concurrent env var access
        unsafe {
            clear_provider_env();
            std::env::set_var("BARNSTORMER_DEFAULT_MODEL", "claude-opus-4-20250918");
        }

        let status = ProviderStatus::detect();
        assert_eq!(status.default_model.as_deref(), Some("claude-opus-4-20250918"));

        // Clean up
        // SAFETY: holding ENV_MUTEX, no concurrent env var access
        unsafe {
            std::env::remove_var("BARNSTORMER_DEFAULT_MODEL");
        }
    }

    #[test]
    fn detect_ignores_empty_default_provider() {
        let _lock = ENV_MUTEX.lock().unwrap();

        // SAFETY: holding ENV_MUTEX, no concurrent env var access
        unsafe {
            clear_provider_env();
            std::env::set_var("BARNSTORMER_DEFAULT_PROVIDER", "");
        }

        let status = ProviderStatus::detect();
        assert_eq!(
            status.default_provider, "anthropic",
            "empty BARNSTORMER_DEFAULT_PROVIDER should fall back to anthropic"
        );

        // Clean up
        // SAFETY: holding ENV_MUTEX, no concurrent env var access
        unsafe {
            std::env::remove_var("BARNSTORMER_DEFAULT_PROVIDER");
        }
    }
}
