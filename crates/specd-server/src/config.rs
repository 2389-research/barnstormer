// ABOUTME: Configuration loading and validation for the specd server.
// ABOUTME: Reads environment variables per spec Section 11 and enforces security constraints.

use std::net::SocketAddr;
use std::path::PathBuf;

use thiserror::Error;

/// Errors that can occur during configuration loading.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("SPECD_BIND is not a valid socket address: {0}")]
    InvalidBind(String),

    #[error("SPECD_ALLOW_REMOTE is true but SPECD_AUTH_TOKEN is not set; refusing to start without authentication")]
    RemoteWithoutToken,
}

/// Server configuration loaded from environment variables.
#[derive(Debug, Clone)]
pub struct SpecdConfig {
    pub home: PathBuf,
    pub bind: SocketAddr,
    pub allow_remote: bool,
    pub auth_token: Option<String>,
    pub default_provider: String,
    pub default_model: Option<String>,
    pub public_base_url: String,
}

impl SpecdConfig {
    /// Load configuration from environment variables with sensible defaults.
    ///
    /// Environment variables:
    /// - SPECD_HOME: data directory (default: ~/.specd)
    /// - SPECD_BIND: socket address to bind (default: 127.0.0.1:7331)
    /// - SPECD_ALLOW_REMOTE: allow non-loopback connections (default: false)
    /// - SPECD_AUTH_TOKEN: bearer token for API auth (optional)
    /// - SPECD_DEFAULT_PROVIDER: LLM provider (default: anthropic)
    /// - SPECD_DEFAULT_MODEL: LLM model name (optional)
    /// - SPECD_PUBLIC_BASE_URL: public URL for the server (default: http://localhost:7331)
    pub fn from_env() -> Result<Self, ConfigError> {
        let home = std::env::var("SPECD_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::var("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from("/tmp"))
                    .join(".specd")
            });

        let bind_str = std::env::var("SPECD_BIND")
            .unwrap_or_else(|_| "127.0.0.1:7331".to_string());
        let bind: SocketAddr = bind_str
            .parse()
            .map_err(|_| ConfigError::InvalidBind(bind_str))?;

        let allow_remote = std::env::var("SPECD_ALLOW_REMOTE")
            .map(|v| v == "true" || v == "1" || v == "yes")
            .unwrap_or(false);

        let auth_token = std::env::var("SPECD_AUTH_TOKEN").ok().filter(|t| !t.is_empty());

        let default_provider = std::env::var("SPECD_DEFAULT_PROVIDER")
            .unwrap_or_else(|_| "anthropic".to_string());

        let default_model = std::env::var("SPECD_DEFAULT_MODEL").ok().filter(|m| !m.is_empty());

        let public_base_url = std::env::var("SPECD_PUBLIC_BASE_URL")
            .unwrap_or_else(|_| format!("http://{}", bind));

        // Security validation: if allowing remote access, require auth token
        if allow_remote && auth_token.is_none() {
            return Err(ConfigError::RemoteWithoutToken);
        }

        Ok(Self {
            home,
            bind,
            allow_remote,
            auth_token,
            default_provider,
            default_model,
            public_base_url,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_loads_defaults() {
        // Clear any env vars that might interfere
        // SAFETY: test-only code, single-threaded test execution
        unsafe {
            std::env::remove_var("SPECD_HOME");
            std::env::remove_var("SPECD_BIND");
            std::env::remove_var("SPECD_ALLOW_REMOTE");
            std::env::remove_var("SPECD_AUTH_TOKEN");
            std::env::remove_var("SPECD_DEFAULT_PROVIDER");
            std::env::remove_var("SPECD_DEFAULT_MODEL");
            std::env::remove_var("SPECD_PUBLIC_BASE_URL");
        }

        let config = SpecdConfig::from_env().unwrap();

        assert_eq!(config.bind, "127.0.0.1:7331".parse::<SocketAddr>().unwrap());
        assert!(!config.allow_remote);
        assert!(config.auth_token.is_none());
        assert_eq!(config.default_provider, "anthropic");
        assert!(config.default_model.is_none());
        assert!(config.home.to_string_lossy().contains(".specd"));
    }

    #[test]
    fn config_rejects_remote_without_token() {
        // SAFETY: test-only code, single-threaded test execution
        unsafe {
            std::env::remove_var("SPECD_AUTH_TOKEN");
            std::env::remove_var("SPECD_HOME");
            std::env::remove_var("SPECD_BIND");
            std::env::remove_var("SPECD_DEFAULT_PROVIDER");
            std::env::remove_var("SPECD_DEFAULT_MODEL");
            std::env::remove_var("SPECD_PUBLIC_BASE_URL");
            std::env::set_var("SPECD_ALLOW_REMOTE", "true");
        }

        let result = SpecdConfig::from_env();

        // Clean up before asserting
        // SAFETY: test-only code, single-threaded test execution
        unsafe {
            std::env::remove_var("SPECD_ALLOW_REMOTE");
        }

        assert!(result.is_err(), "should reject remote without token");
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("SPECD_AUTH_TOKEN"),
            "error should mention auth token: {}",
            err
        );
    }
}
