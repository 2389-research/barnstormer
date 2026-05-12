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
    /// When true, do not consult the `BARNSTORMER_AUTH_TOKEN` env var if
    /// `auth_token` is `None`. Frontends that embed a loopback-only server
    /// (e.g. the Tauri desktop shell) set this so a user's shell env cannot
    /// silently flip on bearer auth and break in-process API calls.
    pub disable_auth_fallback: bool,
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
            .or_else(|| {
                if options.disable_auth_fallback {
                    None
                } else {
                    std::env::var("BARNSTORMER_AUTH_TOKEN").ok()
                }
            })
            .filter(|token| !token.is_empty());
        let static_dir = options
            .static_dir
            .unwrap_or_else(|| PathBuf::from("static"));

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
    use std::sync::Mutex;

    // Tests that mutate process env must serialize against each other. cargo
    // test parallelizes by default and a second env-mutating test would race
    // with `disable_auth_fallback_skips_env_var`.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn config_uses_explicit_home_when_provided() {
        let config = RuntimeConfig::from_parts(RuntimeOptions {
            home: Some(PathBuf::from("/tmp/barnstormer-test")),
            bind: None,
            auth_token: None,
            static_dir: None,
            open_browser: false,
            disable_auth_fallback: false,
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
            disable_auth_fallback: false,
        })
        .unwrap();

        assert_eq!(config.bind.ip().to_string(), "127.0.0.1");
        assert_eq!(config.bind.port(), 0);
    }

    #[test]
    fn disable_auth_fallback_skips_env_var() {
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());

        // SAFETY: the ENV_LOCK above serializes against any other env-mutating
        // test in this crate; this test restores the prior value before
        // returning.
        let prior = std::env::var("BARNSTORMER_AUTH_TOKEN").ok();
        unsafe { std::env::set_var("BARNSTORMER_AUTH_TOKEN", "leaked-token") };

        let config = RuntimeConfig::from_parts(RuntimeOptions {
            home: Some(PathBuf::from("/tmp/barnstormer-test")),
            bind: None,
            auth_token: None,
            static_dir: None,
            open_browser: false,
            disable_auth_fallback: true,
        })
        .unwrap();

        match prior {
            Some(value) => unsafe { std::env::set_var("BARNSTORMER_AUTH_TOKEN", value) },
            None => unsafe { std::env::remove_var("BARNSTORMER_AUTH_TOKEN") },
        }

        assert!(
            config.auth_token.is_none(),
            "expected auth disabled when fallback is off"
        );
    }
}
