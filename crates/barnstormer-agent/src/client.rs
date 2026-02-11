// ABOUTME: Factory function for creating LLM clients using the mux library.
// ABOUTME: Resolves provider name + optional model into a configured (Arc<dyn LlmClient>, model) pair.

use std::env;
use std::sync::Arc;

use mux::llm::{AnthropicClient, GeminiClient, LlmClient, OpenAIClient};

/// Read an env var and return `Some(value)` only if it is non-empty after trimming.
/// Prevents empty or whitespace-only values from producing invalid URLs or model names.
fn non_empty_env(key: &str) -> Option<String> {
    env::var(key).ok().and_then(|v| {
        let trimmed = v.trim().to_string();
        if trimmed.is_empty() { None } else { Some(trimmed) }
    })
}

/// Create an LLM client for the given provider name.
///
/// Returns a tuple of (client, resolved_model). The model is resolved from:
/// 1. The explicit `model` parameter (if Some)
/// 2. A provider-specific environment variable (e.g. ANTHROPIC_MODEL)
/// 3. A sensible default for that provider
pub fn create_llm_client(
    provider: &str,
    model: Option<&str>,
) -> Result<(Arc<dyn LlmClient>, String), anyhow::Error> {
    match provider {
        "anthropic" => {
            let api_key = env::var("ANTHROPIC_API_KEY")
                .map_err(|_| anyhow::anyhow!("ANTHROPIC_API_KEY environment variable not set"))?;
            let resolved_model = model
                .map(String::from)
                .or_else(|| non_empty_env("ANTHROPIC_MODEL"))
                .unwrap_or_else(|| "claude-sonnet-4-5-20250929".to_string());
            let mut client = AnthropicClient::new(api_key);
            if let Some(base_url) = non_empty_env("ANTHROPIC_BASE_URL") {
                client = client.with_base_url(base_url);
            }
            Ok((Arc::new(client), resolved_model))
        }
        "openai" => {
            let api_key = env::var("OPENAI_API_KEY")
                .map_err(|_| anyhow::anyhow!("OPENAI_API_KEY environment variable not set"))?;
            let resolved_model = model
                .map(String::from)
                .or_else(|| non_empty_env("OPENAI_MODEL"))
                .unwrap_or_else(|| "gpt-4o".to_string());
            let mut client = OpenAIClient::new(api_key);
            if let Some(base_url) = non_empty_env("OPENAI_BASE_URL") {
                client = client.with_base_url(base_url);
            }
            Ok((Arc::new(client), resolved_model))
        }
        "gemini" => {
            let api_key = env::var("GEMINI_API_KEY")
                .map_err(|_| anyhow::anyhow!("GEMINI_API_KEY environment variable not set"))?;
            let resolved_model = model
                .map(String::from)
                .or_else(|| non_empty_env("GEMINI_MODEL"))
                .unwrap_or_else(|| "gemini-2.0-flash".to_string());
            let mut client = GeminiClient::new(api_key);
            if let Some(base_url) = non_empty_env("GEMINI_BASE_URL") {
                client = client.with_base_url(base_url);
            }
            Ok((Arc::new(client), resolved_model))
        }
        unknown => Err(anyhow::anyhow!("unsupported LLM provider: {}", unknown)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialize all tests that read/write env vars to prevent race conditions.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// All env var names that tests may read or mutate.
    const ENV_VARS: &[&str] = &[
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_MODEL",
        "ANTHROPIC_BASE_URL",
        "OPENAI_API_KEY",
        "OPENAI_MODEL",
        "OPENAI_BASE_URL",
        "GEMINI_API_KEY",
        "GEMINI_MODEL",
        "GEMINI_BASE_URL",
    ];

    /// Save the current values of all env vars we touch, returning a snapshot.
    fn save_env() -> Vec<(&'static str, Option<String>)> {
        ENV_VARS.iter().map(|&k| (k, env::var(k).ok())).collect()
    }

    /// Restore env vars to a previously captured snapshot.
    fn restore_env(snapshot: &[(&str, Option<String>)]) {
        for &(key, ref val) in snapshot {
            match val {
                Some(v) => unsafe { env::set_var(key, v) },
                None => unsafe { env::remove_var(key) },
            }
        }
    }

    /// Helper to extract the error string from a create_llm_client result.
    /// Uses match instead of unwrap_err() because Arc<dyn LlmClient> doesn't impl Debug.
    fn expect_err(result: Result<(Arc<dyn LlmClient>, String), anyhow::Error>) -> String {
        match result {
            Err(e) => e.to_string(),
            Ok((_client, model)) => panic!("expected error, got Ok with model: {}", model),
        }
    }

    #[test]
    fn unknown_provider_returns_error() {
        let err = expect_err(create_llm_client("unknown", None));
        assert!(
            err.contains("unsupported LLM provider"),
            "expected 'unsupported LLM provider' in error, got: {}",
            err
        );
    }

    #[test]
    fn anthropic_missing_api_key_returns_error() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let saved = save_env();
        unsafe { env::remove_var("ANTHROPIC_API_KEY") };
        let err = expect_err(create_llm_client("anthropic", None));
        restore_env(&saved);
        assert!(
            err.contains("ANTHROPIC_API_KEY"),
            "expected mention of ANTHROPIC_API_KEY in error, got: {}",
            err
        );
    }

    #[test]
    fn openai_missing_api_key_returns_error() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let saved = save_env();
        unsafe { env::remove_var("OPENAI_API_KEY") };
        let err = expect_err(create_llm_client("openai", None));
        restore_env(&saved);
        assert!(
            err.contains("OPENAI_API_KEY"),
            "expected mention of OPENAI_API_KEY in error, got: {}",
            err
        );
    }

    #[test]
    fn gemini_missing_api_key_returns_error() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let saved = save_env();
        unsafe { env::remove_var("GEMINI_API_KEY") };
        let err = expect_err(create_llm_client("gemini", None));
        restore_env(&saved);
        assert!(
            err.contains("GEMINI_API_KEY"),
            "expected mention of GEMINI_API_KEY in error, got: {}",
            err
        );
    }

    #[test]
    fn explicit_model_param_overrides_default() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let saved = save_env();
        unsafe { env::set_var("ANTHROPIC_API_KEY", "test-key-456") };

        let result = create_llm_client("anthropic", Some("claude-opus-4-20250514"));
        restore_env(&saved);

        let (_client, resolved_model) = match result {
            Ok(pair) => pair,
            Err(e) => panic!("expected Ok, got Err: {}", e),
        };
        assert_eq!(
            resolved_model, "claude-opus-4-20250514",
            "explicit model param should override default"
        );
    }

    #[test]
    fn anthropic_success_returns_default_model() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let saved = save_env();
        unsafe { env::set_var("ANTHROPIC_API_KEY", "test-key-123") };

        let result = create_llm_client("anthropic", None);
        restore_env(&saved);

        let (_client, resolved_model) = match result {
            Ok(pair) => pair,
            Err(e) => panic!("expected Ok, got Err: {}", e),
        };
        assert_eq!(
            resolved_model, "claude-sonnet-4-5-20250929",
            "expected default Anthropic model, got: {}",
            resolved_model
        );
    }
}
