// ABOUTME: Runtime configuration shared by CLI and desktop entrypoints.
// ABOUTME: Resolves startup options into concrete server configuration.

use std::net::SocketAddr;
use std::path::PathBuf;

/// Startup options provided by a frontend before defaults are resolved.
#[derive(Debug, Clone, Default)]
pub struct RuntimeOptions {
    pub home: Option<PathBuf>,
    pub bind: Option<SocketAddr>,
    pub auth_token: Option<String>,
    pub static_dir: Option<PathBuf>,
    pub open_browser: bool,
}

/// Concrete runtime configuration after resolving defaults.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub home: PathBuf,
    pub bind: SocketAddr,
    pub auth_token: Option<String>,
    pub static_dir: PathBuf,
    pub open_browser: bool,
}

impl RuntimeConfig {
    pub fn from_parts(options: RuntimeOptions) -> anyhow::Result<Self> {
        let home = options.home.unwrap_or_else(default_home);
        let bind = options
            .bind
            .unwrap_or_else(|| "127.0.0.1:7331".parse().expect("valid default bind"));
        let auth_token = options
            .auth_token
            .or_else(|| std::env::var("BARNSTORMER_AUTH_TOKEN").ok())
            .filter(|token| !token.is_empty());
        let static_dir = options.static_dir.unwrap_or_else(|| PathBuf::from("static"));

        Ok(Self {
            home,
            bind,
            auth_token,
            static_dir,
            open_browser: options.open_browser,
        })
    }
}

fn default_home() -> PathBuf {
    std::env::var("BARNSTORMER_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/tmp"))
                .join(".barnstormer")
        })
}

#[cfg(test)]
mod tests {
    use super::{RuntimeConfig, RuntimeOptions};
    use std::path::PathBuf;

    #[test]
    fn config_uses_explicit_home_when_provided() {
        let config = RuntimeConfig::from_parts(RuntimeOptions {
            home: Some(PathBuf::from("/tmp/barnstormer-test")),
            bind: None,
            auth_token: None,
            static_dir: None,
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
            static_dir: None,
            open_browser: false,
        })
        .unwrap();

        assert_eq!(config.bind.ip().to_string(), "127.0.0.1");
        assert_eq!(config.bind.port(), 0);
    }
}
