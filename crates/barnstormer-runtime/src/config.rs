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
    /// Per-session countable-token tranche for each spec swarm. `None` is unlimited.
    pub token_budget: Option<u64>,
}

impl RuntimeConfig {
    pub fn from_parts(options: RuntimeOptions) -> anyhow::Result<Self> {
        let home = options.home.unwrap_or_else(default_home);
        let bind = match options.bind {
            Some(addr) => addr,
            None => match std::env::var("BARNSTORMER_BIND") {
                Ok(value) => value.parse::<SocketAddr>().map_err(|err| {
                    anyhow::anyhow!(
                        "BARNSTORMER_BIND is not a valid socket address ({value}): {err}"
                    )
                })?,
                Err(_) => "127.0.0.1:7331".parse().expect("valid default bind"),
            },
        };
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
        let token_budget = match std::env::var("BARNSTORMER_TOKEN_BUDGET") {
            Ok(value) => match value.parse::<u64>() {
                Ok(0) => None,
                Ok(value) => Some(value),
                Err(err) => {
                    return Err(anyhow::anyhow!(
                        "BARNSTORMER_TOKEN_BUDGET is not a valid non-negative integer ({value}): {err}"
                    ));
                }
            },
            Err(std::env::VarError::NotPresent) => Some(2_000_000),
            Err(err) => return Err(err.into()),
        };

        Ok(Self {
            home,
            bind,
            auth_token,
            static_dir,
            open_browser: options.open_browser,
            token_budget,
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

    // Serializes any test that touches process env — both writers
    // (`disable_auth_fallback_skips_env_var`, the `BARNSTORMER_BIND` tests
    // below) and readers (any `from_parts` call with `bind: None` or
    // `disable_auth_fallback: false`, which consult env). cargo test
    // parallelizes by default, so a reader that skips the lock will see
    // whatever junk a concurrent writer set and panic.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn config_uses_explicit_home_when_provided() {
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());

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
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());

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

    #[test]
    fn config_reads_bind_from_env_var_when_options_bind_is_none() {
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());

        // SAFETY: ENV_LOCK serializes against any other env-mutating test in
        // this crate; this test restores the prior value before returning.
        let prior = std::env::var("BARNSTORMER_BIND").ok();
        unsafe { std::env::set_var("BARNSTORMER_BIND", "0.0.0.0:7331") };

        let config = RuntimeConfig::from_parts(RuntimeOptions {
            home: Some(PathBuf::from("/tmp/barnstormer-test")),
            bind: None,
            auth_token: None,
            static_dir: None,
            open_browser: false,
            disable_auth_fallback: true,
        });

        match prior {
            Some(value) => unsafe { std::env::set_var("BARNSTORMER_BIND", value) },
            None => unsafe { std::env::remove_var("BARNSTORMER_BIND") },
        }

        let config = config.expect("BARNSTORMER_BIND=0.0.0.0:7331 should parse");
        assert_eq!(config.bind.ip().to_string(), "0.0.0.0");
        assert_eq!(config.bind.port(), 7331);
    }

    #[test]
    fn config_returns_error_when_bind_env_var_is_invalid() {
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());

        // SAFETY: ENV_LOCK serializes against any other env-mutating test in
        // this crate; this test restores the prior value before returning.
        let prior = std::env::var("BARNSTORMER_BIND").ok();
        unsafe { std::env::set_var("BARNSTORMER_BIND", "not-a-socket") };

        let result = RuntimeConfig::from_parts(RuntimeOptions {
            home: Some(PathBuf::from("/tmp/barnstormer-test")),
            bind: None,
            auth_token: None,
            static_dir: None,
            open_browser: false,
            disable_auth_fallback: true,
        });

        match prior {
            Some(value) => unsafe { std::env::set_var("BARNSTORMER_BIND", value) },
            None => unsafe { std::env::remove_var("BARNSTORMER_BIND") },
        }

        let err = result.expect_err("expected parse error for invalid bind value");
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("BARNSTORMER_BIND is not a valid socket address"),
            "expected clear error message, got: {err_msg}"
        );
    }

    fn with_token_budget_env(value: Option<&str>, test: impl FnOnce()) {
        let prior = std::env::var("BARNSTORMER_TOKEN_BUDGET").ok();
        match value {
            Some(value) => unsafe { std::env::set_var("BARNSTORMER_TOKEN_BUDGET", value) },
            None => unsafe { std::env::remove_var("BARNSTORMER_TOKEN_BUDGET") },
        }
        test();
        match prior {
            Some(value) => unsafe { std::env::set_var("BARNSTORMER_TOKEN_BUDGET", value) },
            None => unsafe { std::env::remove_var("BARNSTORMER_TOKEN_BUDGET") },
        }
    }

    fn token_budget_config() -> RuntimeOptions {
        RuntimeOptions {
            home: Some(PathBuf::from("/tmp/barnstormer-test")),
            bind: Some("127.0.0.1:0".parse().unwrap()),
            auth_token: None,
            static_dir: None,
            open_browser: false,
            disable_auth_fallback: true,
        }
    }

    #[test]
    fn token_budget_defaults_to_two_million() {
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        with_token_budget_env(None, || {
            let config = RuntimeConfig::from_parts(token_budget_config()).unwrap();
            assert_eq!(config.token_budget, Some(2_000_000));
        });
    }

    #[test]
    fn token_budget_honors_numeric_value() {
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        with_token_budget_env(Some("123456"), || {
            let config = RuntimeConfig::from_parts(token_budget_config()).unwrap();
            assert_eq!(config.token_budget, Some(123_456));
        });
    }

    #[test]
    fn zero_token_budget_is_unlimited() {
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        with_token_budget_env(Some("0"), || {
            let config = RuntimeConfig::from_parts(token_budget_config()).unwrap();
            assert_eq!(config.token_budget, None);
        });
    }

    #[test]
    fn invalid_token_budget_returns_startup_error() {
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        for value in ["not-a-number", "-1"] {
            with_token_budget_env(Some(value), || {
                let error = RuntimeConfig::from_parts(token_budget_config()).unwrap_err();
                assert!(
                    error.to_string().contains("BARNSTORMER_TOKEN_BUDGET"),
                    "expected token budget error, got: {error}"
                );
            });
        }
    }
}
