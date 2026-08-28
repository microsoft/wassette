// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use etcetera::BaseStrategy;
use figment::providers::{Env, Format, Serialized, Toml};
use serde::{Deserialize, Serialize};

use crate::commands::{Run, Serve};

/// Get the default component directory path based on the OS
pub fn get_component_dir() -> Result<PathBuf, anyhow::Error> {
    let dir_strategy = etcetera::choose_base_strategy().context("Unable to get home directory")?;
    Ok(dir_strategy.data_dir().join("wassette").join("components"))
}

/// Get the default secrets directory path based on the OS
pub fn get_secrets_dir() -> Result<PathBuf, anyhow::Error> {
    let dir_strategy = etcetera::choose_base_strategy().context("Unable to get home directory")?;
    Ok(dir_strategy.config_dir().join("wassette").join("secrets"))
}

fn default_component_dir() -> PathBuf {
    get_component_dir().unwrap_or_else(|_| {
        eprintln!("WARN: Unable to determine default component directory, using `components` directory in the current working directory");
        PathBuf::from("./components")
    })
}

fn default_secrets_dir() -> PathBuf {
    get_secrets_dir().unwrap_or_else(|_| {
        eprintln!("WARN: Unable to determine default secrets directory, using `secrets` directory in the current working directory");
        PathBuf::from("./secrets")
    })
}

/// Split a comma-separated `Host` allowlist, dropping empty entries and surrounding
/// whitespace. Returns an empty vector when nothing usable is present.
fn parse_allowed_hosts(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|host| host.trim())
        .filter(|host| !host.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn default_bind_address() -> String {
    // Default bind address using PORT and BIND_HOST environment variables (twelve-factor app compliance).
    // This is only used when bind_address is not set via CLI, config file, or other higher-precedence sources.
    let host = std::env::var("BIND_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("PORT").unwrap_or_else(|_| "9001".to_string());
    format!("{}:{}", host, port)
}

fn default_legacy_sessions() -> bool {
    true
}

/// Configuration for the Wasette MCP server
#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    /// Directory where components are stored
    #[serde(default = "default_component_dir")]
    pub component_dir: PathBuf,

    /// Directory where secrets are stored
    #[serde(default = "default_secrets_dir")]
    pub secrets_dir: PathBuf,

    /// Environment variables to be made available to components
    #[serde(default)]
    pub environment_vars: HashMap<String, String>,

    /// Bind address for Streamable HTTP
    /// Configured via PORT and BIND_HOST environment variables or CLI/config file
    #[serde(default = "default_bind_address", rename = "bind_address")]
    pub bind_address: String,

    /// Hostnames or `host:port` authorities accepted in the inbound `Host` header for
    /// Streamable HTTP.
    ///
    /// `None` leaves the transport's own default in place, which accepts loopback only
    /// as protection against DNS rebinding. Set this when the server is addressed by a
    /// service name, container name or DNS name rather than by `localhost`.
    #[serde(default)]
    pub allowed_hosts: Option<Vec<String>>,

    /// Whether to keep serving the pre-2026-07-28 session lifecycle.
    ///
    /// Defaults to true so pre-2026 clients keep working. Requests negotiating
    /// 2026-07-28 or later are served statelessly either way.
    #[serde(default = "default_legacy_sessions")]
    pub legacy_sessions: bool,

    /// Whether to prefer `application/json` over `text/event-stream` for a
    /// simple stateless request that produces a single reply.
    #[serde(default)]
    pub json_response: bool,
}

impl Config {
    /// Returns a new [`Config`] instance by merging the configuration from the specified
    /// `cli_config` (any struct that is Serialize/Deserialize, but generally a Clap `Parser`) with
    /// the configuration file and environment variables. By default, the configuration file is
    /// located at `$XDG_CONFIG_HOME/wassette/config.toml`. This can be overridden by setting
    /// the `WASSETTE_CONFIG_FILE` environment variable.
    ///
    /// The order of precedence for configuration sources is as follows:
    /// 1. Values from `cli_config`
    /// 2. Environment variables prefixed with `WASSETTE_`
    /// 3. Configuration file specified by `WASSETTE_CONFIG_FILE` or default location
    pub fn new<T: Serialize>(cli_config: &T) -> Result<Self, anyhow::Error> {
        let config_file_path = match std::env::var_os("WASSETTE_CONFIG_FILE") {
            Some(path) => PathBuf::from(path),
            None => etcetera::choose_base_strategy()
                .context("Unable to get home directory")?
                .config_dir()
                .join("wassette")
                .join("config.toml"),
        };
        Self::new_from_path(cli_config, config_file_path)
    }

    /// Same as [`Config::new`], but allows specifying a custom path for the configuration file.
    pub fn new_from_path<T: Serialize>(
        cli_config: &T,
        config_file_path: impl AsRef<Path>,
    ) -> Result<Self, anyhow::Error> {
        // Build figment config, excluding bind_address from WASSETTE_ environment variables.
        // Instead, bind_address uses PORT and BIND_HOST env vars as defaults (via default_bind_address())
        // when not explicitly set via CLI or config file.
        //
        // allowed_hosts is excluded for a different reason: it is a list, and the generic
        // env provider has no separator configured, so `WASSETTE_ALLOWED_HOSTS=a,b` would
        // arrive as one string. It is parsed explicitly below instead of teaching the
        // provider to split every value.
        let env_provider = Env::prefixed("WASSETTE_")
            .filter(|key| key != "bind_address" && key != "allowed_hosts");

        let mut config: Self = figment::Figment::new()
            .admerge(Toml::file(config_file_path))
            .admerge(env_provider)
            .admerge(Serialized::defaults(cli_config))
            .extract()
            .context("Unable to merge configs")?;

        // Applied after extraction rather than as another figment layer because `admerge`
        // concatenates sequences instead of replacing them, so a file value and an env
        // value would combine into one longer allowlist rather than the env winning.
        //
        // An empty or whitespace-only value is ignored rather than treated as an empty
        // list: an empty list disables Host validation entirely in the transport, and
        // that must not be reachable by accident.
        if let Some(raw) = std::env::var_os("WASSETTE_ALLOWED_HOSTS") {
            let hosts = parse_allowed_hosts(&raw.to_string_lossy());
            if !hosts.is_empty() {
                config.allowed_hosts = Some(hosts);
            }
        }

        Ok(config)
    }

    /// Creates a new config from a Run struct for local stdio transport
    pub fn from_run(
        run_config: &Run,
        global_component_dir: Option<&Path>,
    ) -> Result<Self, anyhow::Error> {
        let mut run_config = run_config.clone();
        if run_config.component_dir.is_none() {
            run_config.component_dir = global_component_dir.map(Path::to_path_buf);
        }

        // Start with the base config using existing logic
        let mut config = Self::new(&run_config)?;

        // Load environment variables from file if specified
        if let Some(env_file) = &run_config.env_file {
            let file_env_vars = crate::utils::load_env_file(env_file).with_context(|| {
                format!("Failed to load environment file: {}", env_file.display())
            })?;

            // Merge file environment variables (they have lower precedence than CLI args)
            for (key, value) in file_env_vars {
                config.environment_vars.insert(key, value);
            }
        }

        // Apply CLI environment variables (highest precedence)
        for (key, value) in &run_config.env_vars {
            config.environment_vars.insert(key.clone(), value.clone());
        }

        // Also include system environment variables that aren't overridden
        // This maintains backward compatibility
        for (key, value) in std::env::vars() {
            config.environment_vars.entry(key).or_insert(value);
        }

        Ok(config)
    }

    /// Creates a new config from a Serve struct that includes environment variable handling
    pub fn from_serve(
        serve_config: &Serve,
        global_component_dir: Option<&Path>,
    ) -> Result<Self, anyhow::Error> {
        let mut serve_config = serve_config.clone();
        if serve_config.component_dir.is_none() {
            serve_config.component_dir = global_component_dir.map(Path::to_path_buf);
        }

        // Start with the base config using existing logic
        let mut config = Self::new(&serve_config)?;

        // Load environment variables from file if specified
        if let Some(env_file) = &serve_config.env_file {
            let file_env_vars = crate::utils::load_env_file(env_file).with_context(|| {
                format!("Failed to load environment file: {}", env_file.display())
            })?;

            // Merge file environment variables (they have lower precedence than CLI args)
            for (key, value) in file_env_vars {
                config.environment_vars.insert(key, value);
            }
        }

        // Apply CLI environment variables (highest precedence)
        for (key, value) in &serve_config.env_vars {
            config.environment_vars.insert(key.clone(), value.clone());
        }

        // Also include system environment variables that aren't overridden
        // This maintains backward compatibility
        for (key, value) in std::env::vars() {
            config.environment_vars.entry(key).or_insert(value);
        }

        // Highest precedence, so it lands after the config file and WASSETTE_ALLOWED_HOSTS.
        // Empty entries are dropped; if nothing usable remains the lower-precedence value
        // stands rather than becoming an empty list, which would disable Host validation.
        if let Some(cli_hosts) = &serve_config.allowed_hosts {
            let hosts = parse_allowed_hosts(&cli_hosts.join(","));
            if !hosts.is_empty() {
                config.allowed_hosts = Some(hosts);
            }
        }

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    /// Every environment variable `Config::new_from_path` reads.
    ///
    /// `WASSETTE_*` reaches it through `Env::prefixed`, and `PORT` and
    /// `BIND_HOST` through `default_bind_address`.
    const CONFIG_ENV_VARS: [&str; 7] = [
        "WASSETTE_CONFIG_FILE",
        "WASSETTE_COMPONENT_DIR",
        "WASSETTE_ALLOWED_HOSTS",
        "WASSETTE_LEGACY_SESSIONS",
        "WASSETTE_JSON_RESPONSE",
        "PORT",
        "BIND_HOST",
    ];

    /// Runs `f` with those variables unset.
    ///
    /// `temp_env` serialises only against other `temp_env` callers, so a test
    /// that reads the environment directly can run concurrently with one that
    /// has set a variable, and observe it. Reading through this helper puts
    /// every such test behind the same lock.
    fn with_isolated_env<R>(f: impl FnOnce() -> R) -> R {
        temp_env::with_vars_unset(CONFIG_ENV_VARS, f)
    }

    #[allow(dead_code)]
    fn create_test_run_config() -> Run {
        Run {
            component_dir: Some(PathBuf::from("/test/component/dir")),
            env_vars: vec![],
            env_file: None,
            disable_builtin_tools: false,
        }
    }

    #[allow(dead_code)]
    fn empty_test_run_config() -> Run {
        Run {
            component_dir: None,
            env_vars: vec![],
            env_file: None,
            disable_builtin_tools: false,
        }
    }

    fn create_test_cli_config() -> Serve {
        Serve {
            component_dir: Some(PathBuf::from("/test/component/dir")),
            transport: Default::default(),
            env_vars: vec![],
            env_file: None,
            disable_builtin_tools: false,
            bind_address: None,
            manifest: None,
            allowed_hosts: None,
            legacy_sessions: None,
            json_response: None,
        }
    }

    fn empty_test_cli_config() -> Serve {
        Serve {
            component_dir: None,
            transport: Default::default(),
            env_vars: vec![],
            env_file: None,
            disable_builtin_tools: false,
            bind_address: None,
            manifest: None,
            allowed_hosts: None,
            legacy_sessions: None,
            json_response: None,
        }
    }

    fn assert_run_and_serve_component_dir(
        run_config: &Run,
        serve_config: &Serve,
        global_component_dir: Option<&Path>,
        expected: &Path,
    ) {
        let run_config = Config::from_run(run_config, global_component_dir)
            .expect("Failed to create run config");
        assert_eq!(run_config.component_dir, expected);

        let serve_config = Config::from_serve(serve_config, global_component_dir)
            .expect("Failed to create serve config");
        assert_eq!(serve_config.component_dir, expected);
    }

    #[test]
    fn test_global_component_dir_used_for_run_and_serve() {
        let temp_dir = TempDir::new().unwrap();
        let config_file = temp_dir.path().join("non_existent_config.toml");
        let global_component_dir = temp_dir.path().join("global-components");

        temp_env::with_vars(
            vec![
                ("WASSETTE_CONFIG_FILE", Some(config_file.to_str().unwrap())),
                ("WASSETTE_COMPONENT_DIR", None),
            ],
            || {
                assert_run_and_serve_component_dir(
                    &empty_test_run_config(),
                    &empty_test_cli_config(),
                    Some(&global_component_dir),
                    &global_component_dir,
                );
            },
        );
    }

    #[test]
    fn test_subcommand_component_dir_overrides_global_for_run_and_serve() {
        let temp_dir = TempDir::new().unwrap();
        let config_file = temp_dir.path().join("non_existent_config.toml");
        let global_component_dir = temp_dir.path().join("global-components");
        let subcommand_component_dir = temp_dir.path().join("subcommand-components");
        let mut run_config = empty_test_run_config();
        run_config.component_dir = Some(subcommand_component_dir.clone());
        let mut serve_config = empty_test_cli_config();
        serve_config.component_dir = Some(subcommand_component_dir.clone());

        temp_env::with_vars(
            vec![
                ("WASSETTE_CONFIG_FILE", Some(config_file.to_str().unwrap())),
                ("WASSETTE_COMPONENT_DIR", None),
            ],
            || {
                assert_run_and_serve_component_dir(
                    &run_config,
                    &serve_config,
                    Some(&global_component_dir),
                    &subcommand_component_dir,
                );
            },
        );
    }

    #[test]
    fn test_global_component_dir_overrides_env_and_config_for_run_and_serve() {
        let temp_dir = TempDir::new().unwrap();
        let config_file = temp_dir.path().join("config.toml");
        let global_component_dir = temp_dir.path().join("global-components");
        let env_component_dir = temp_dir.path().join("env-components");
        let file_component_dir = temp_dir.path().join("file-components");
        fs::write(
            &config_file,
            format!("component_dir = {:?}\n", file_component_dir),
        )
        .unwrap();

        temp_env::with_vars(
            vec![
                ("WASSETTE_CONFIG_FILE", Some(config_file.to_str().unwrap())),
                (
                    "WASSETTE_COMPONENT_DIR",
                    Some(env_component_dir.to_str().unwrap()),
                ),
            ],
            || {
                assert_run_and_serve_component_dir(
                    &empty_test_run_config(),
                    &empty_test_cli_config(),
                    Some(&global_component_dir),
                    &global_component_dir,
                );
            },
        );
    }

    #[test]
    fn test_run_and_serve_without_cli_component_dir_use_lower_precedence_sources() {
        let temp_dir = TempDir::new().unwrap();
        let config_file = temp_dir.path().join("config.toml");
        let non_existent_config = temp_dir.path().join("non_existent_config.toml");
        let env_component_dir = temp_dir.path().join("env-components");
        let file_component_dir = temp_dir.path().join("file-components");
        fs::write(
            &config_file,
            format!("component_dir = {:?}\n", file_component_dir),
        )
        .unwrap();

        temp_env::with_vars(
            vec![
                ("WASSETTE_CONFIG_FILE", Some(config_file.to_str().unwrap())),
                (
                    "WASSETTE_COMPONENT_DIR",
                    Some(env_component_dir.to_str().unwrap()),
                ),
            ],
            || {
                assert_run_and_serve_component_dir(
                    &empty_test_run_config(),
                    &empty_test_cli_config(),
                    None,
                    &env_component_dir,
                );
            },
        );

        temp_env::with_vars(
            vec![
                ("WASSETTE_CONFIG_FILE", Some(config_file.to_str().unwrap())),
                ("WASSETTE_COMPONENT_DIR", None),
            ],
            || {
                assert_run_and_serve_component_dir(
                    &empty_test_run_config(),
                    &empty_test_cli_config(),
                    None,
                    &file_component_dir,
                );
            },
        );

        temp_env::with_vars(
            vec![
                (
                    "WASSETTE_CONFIG_FILE",
                    Some(non_existent_config.to_str().unwrap()),
                ),
                ("WASSETTE_COMPONENT_DIR", None),
            ],
            || {
                assert_run_and_serve_component_dir(
                    &empty_test_run_config(),
                    &empty_test_cli_config(),
                    None,
                    &get_component_dir().unwrap(),
                );
            },
        );
    }

    #[test]
    fn test_config_file_not_exists_succeeds_with_defaults() {
        let temp_dir = TempDir::new().unwrap();
        let non_existent_config = temp_dir.path().join("non_existent_config.toml");

        let serve_config = create_test_cli_config();
        let config =
            with_isolated_env(|| Config::new_from_path(&serve_config, &non_existent_config))
                .expect("Failed to create config");

        // Should use CLI config values since no config file exists
        assert_eq!(config.component_dir, PathBuf::from("/test/component/dir"));
    }

    #[test]
    fn test_config_file_exists_with_cli_override() {
        let temp_dir = TempDir::new().unwrap();
        let config_file = temp_dir.path().join("config.toml");

        let toml_content = r#"
component_dir = "/config/component/dir"
"#;
        fs::write(&config_file, toml_content).unwrap();

        let serve_config = create_test_cli_config();
        let config = with_isolated_env(|| Config::new_from_path(&serve_config, &config_file))
            .expect("Failed to create config");

        assert_eq!(config.component_dir, PathBuf::from("/test/component/dir"));
    }

    #[test]
    fn test_config_file_exists() {
        let temp_dir = TempDir::new().unwrap();
        let config_file = temp_dir.path().join("config.toml");

        let toml_content = r#"
component_dir = "/config/component/dir"
"#;
        fs::write(&config_file, toml_content).unwrap();

        let config =
            with_isolated_env(|| Config::new_from_path(&empty_test_cli_config(), &config_file))
                .expect("Failed to create config");

        assert_eq!(config.component_dir, PathBuf::from("/config/component/dir"));
    }

    #[test]
    fn test_cli_config_provides_defaults() {
        let temp_dir = TempDir::new().unwrap();
        let non_existent_config = temp_dir.path().join("non_existent_config.toml");

        let serve_config = create_test_cli_config();
        let config =
            with_isolated_env(|| Config::new_from_path(&serve_config, &non_existent_config))
                .expect("Failed to create config");

        // Should use CLI config values as defaults
        assert_eq!(config.component_dir, PathBuf::from("/test/component/dir"));
    }

    #[test]
    fn test_config_file_partial_values() {
        let temp_dir = TempDir::new().unwrap();
        let config_file = temp_dir.path().join("config.toml");

        // Config file only sets component_dir, not policy_file
        let toml_content = r#"
component_dir = "/config/component/dir"
"#;
        fs::write(&config_file, toml_content).unwrap();

        let config =
            with_isolated_env(|| Config::new_from_path(&empty_test_cli_config(), &config_file))
                .expect("Failed to create config");

        // component_dir should come from config file
        assert_eq!(config.component_dir, PathBuf::from("/config/component/dir"));
    }

    #[test]
    fn test_new_method_without_wassette_config_file_env() {
        // This test verifies that new() works when WASSETTE_CONFIG_FILE is not set
        // It should try to use the default config location, which likely won't exist
        // but should still succeed with defaults

        // Ensure WASSETTE_CONFIG_FILE is not set, using temp_env to serialize
        // access to the shared environment variable across tests.
        temp_env::with_var_unset("WASSETTE_CONFIG_FILE", || {
            let serve_config = create_test_cli_config();
            let config = Config::new(&serve_config).expect("Failed to create config");

            // Should use CLI defaults since no config file exists
            assert_eq!(config.component_dir, PathBuf::from("/test/component/dir"));
        });
    }

    #[test]
    fn test_invalid_toml_file_returns_error() {
        let temp_dir = TempDir::new().unwrap();
        let config_file = temp_dir.path().join("invalid_config.toml");

        // Write invalid TOML content
        let invalid_toml = r#"
component_dir = "/some/path"
policy_file = unclosed_string"
"#;
        fs::write(&config_file, invalid_toml).unwrap();

        let serve_config = create_test_cli_config();
        let result = with_isolated_env(|| Config::new_from_path(&serve_config, &config_file));

        // Should return an error due to invalid TOML
        assert!(result.is_err());
    }

    #[test]
    fn test_config_file_path_override_with_env_var() {
        let temp_dir = TempDir::new().unwrap();
        let config_file = temp_dir.path().join("custom_config.toml");

        let toml_content = r#"
component_dir = "/custom/component/dir"
policy_file = "custom_policy.yaml"
"#;
        fs::write(&config_file, toml_content).unwrap();

        // Serialize access to the shared environment. WASSETTE_COMPONENT_DIR has
        // higher precedence than the config file, so it has to be unset here or an
        // ambient value decides the assertion below.
        temp_env::with_vars(
            [
                ("WASSETTE_CONFIG_FILE", Some(config_file.to_str().unwrap())),
                ("WASSETTE_COMPONENT_DIR", None),
            ],
            || {
                let config =
                    Config::new(&empty_test_cli_config()).expect("Failed to create config");

                assert_eq!(config.component_dir, PathBuf::from("/custom/component/dir"));
            },
        );
    }

    #[test]
    fn test_bind_address_default() {
        temp_env::with_vars_unset(vec!["PORT", "BIND_HOST"], || {
            let temp_dir = TempDir::new().unwrap();
            let non_existent_config = temp_dir.path().join("non_existent_config.toml");

            let config = Config::new_from_path(&empty_test_cli_config(), &non_existent_config)
                .expect("Failed to create config");

            // Should use default bind address
            assert_eq!(config.bind_address, "127.0.0.1:9001");
        });
    }

    #[test]
    fn test_bind_address_from_config_file() {
        temp_env::with_vars_unset(vec!["PORT", "BIND_HOST"], || {
            let temp_dir = TempDir::new().unwrap();
            let config_file = temp_dir.path().join("config.toml");

            let toml_content = r#"
bind_address = "0.0.0.0:8080"
"#;
            fs::write(&config_file, toml_content).unwrap();

            let config = Config::new_from_path(&empty_test_cli_config(), &config_file)
                .expect("Failed to create config");

            assert_eq!(config.bind_address, "0.0.0.0:8080");
        });
    }

    #[test]
    fn test_bind_address_cli_override() {
        let temp_dir = TempDir::new().unwrap();
        let config_file = temp_dir.path().join("config.toml");

        // Config file sets one bind address
        let toml_content = r#"
bind_address = "0.0.0.0:8080"
"#;
        fs::write(&config_file, toml_content).unwrap();

        // CLI provides a different bind address
        let serve_config = Serve {
            component_dir: None,
            transport: Default::default(),
            env_vars: vec![],
            env_file: None,
            disable_builtin_tools: false,
            bind_address: Some("192.168.1.100:9090".to_string()),
            manifest: None,
            allowed_hosts: None,
            legacy_sessions: None,
            json_response: None,
        };

        let config = with_isolated_env(|| Config::new_from_path(&serve_config, &config_file))
            .expect("Failed to create config");

        // CLI value should take precedence
        assert_eq!(config.bind_address, "192.168.1.100:9090");
    }

    #[test]
    fn test_port_env_var() {
        temp_env::with_vars(vec![("PORT", Some("8080")), ("BIND_HOST", None)], || {
            let temp_dir = TempDir::new().unwrap();
            let non_existent_config = temp_dir.path().join("non_existent_config.toml");

            let config = Config::new_from_path(&empty_test_cli_config(), &non_existent_config)
                .expect("Failed to create config");

            // PORT environment variable should be used with default host
            assert_eq!(config.bind_address, "127.0.0.1:8080");
        });
    }

    #[test]
    fn test_bind_host_env_var() {
        temp_env::with_vars(vec![("BIND_HOST", Some("0.0.0.0")), ("PORT", None)], || {
            let temp_dir = TempDir::new().unwrap();
            let non_existent_config = temp_dir.path().join("non_existent_config.toml");

            let config = Config::new_from_path(&empty_test_cli_config(), &non_existent_config)
                .expect("Failed to create config");

            // BIND_HOST should be used with default port
            assert_eq!(config.bind_address, "0.0.0.0:9001");
        });
    }

    #[test]
    fn test_port_and_bind_host_env_vars() {
        temp_env::with_vars(
            vec![("PORT", Some("3000")), ("BIND_HOST", Some("0.0.0.0"))],
            || {
                let temp_dir = TempDir::new().unwrap();
                let non_existent_config = temp_dir.path().join("non_existent_config.toml");

                let config = Config::new_from_path(&empty_test_cli_config(), &non_existent_config)
                    .expect("Failed to create config");

                // Both PORT and BIND_HOST should be used together
                assert_eq!(config.bind_address, "0.0.0.0:3000");
            },
        );
    }

    #[test]
    fn test_allowed_hosts_defaults_to_none() {
        temp_env::with_var("WASSETTE_ALLOWED_HOSTS", None::<&str>, || {
            let temp_dir = TempDir::new().unwrap();
            let non_existent_config = temp_dir.path().join("non_existent_config.toml");

            let config = Config::new_from_path(&empty_test_cli_config(), &non_existent_config)
                .expect("Failed to create config");

            // None leaves the transport's loopback-only default untouched. An empty
            // Vec would disable Host validation entirely, so it must not be the default.
            assert_eq!(config.allowed_hosts, None);
        });
    }

    #[test]
    fn test_allowed_hosts_env_var_is_split_on_commas() {
        temp_env::with_var(
            "WASSETTE_ALLOWED_HOSTS",
            Some("wassette.internal, example.com:8080 ,"),
            || {
                let temp_dir = TempDir::new().unwrap();
                let non_existent_config = temp_dir.path().join("non_existent_config.toml");

                let config = Config::new_from_path(&empty_test_cli_config(), &non_existent_config)
                    .expect("Failed to create config");

                assert_eq!(
                    config.allowed_hosts,
                    Some(vec![
                        "wassette.internal".to_string(),
                        "example.com:8080".to_string()
                    ]),
                    "entries should be trimmed and empty ones dropped"
                );
            },
        );
    }

    #[test]
    fn test_allowed_hosts_empty_env_var_is_ignored() {
        temp_env::with_var("WASSETTE_ALLOWED_HOSTS", Some(" , "), || {
            let temp_dir = TempDir::new().unwrap();
            let non_existent_config = temp_dir.path().join("non_existent_config.toml");

            let config = Config::new_from_path(&empty_test_cli_config(), &non_existent_config)
                .expect("Failed to create config");

            // Falling through to None keeps the loopback default rather than
            // silently disabling Host validation.
            assert_eq!(config.allowed_hosts, None);
        });
    }

    #[test]
    fn test_allowed_hosts_cli_overrides_env_var() {
        let temp_dir = TempDir::new().unwrap();
        let non_existent_config = temp_dir.path().join("non_existent_config.toml");

        temp_env::with_vars(
            vec![
                ("WASSETTE_ALLOWED_HOSTS", Some("from-env")),
                (
                    "WASSETTE_CONFIG_FILE",
                    Some(non_existent_config.to_str().unwrap()),
                ),
            ],
            || {
                let cli_config = Serve {
                    allowed_hosts: Some(vec!["from-cli".to_string()]),
                    ..empty_test_cli_config()
                };

                // from_serve is the path that applies the CLI value, mirroring how
                // env_vars and env_file are handled.
                let config =
                    Config::from_serve(&cli_config, None).expect("Failed to create config");

                assert_eq!(config.allowed_hosts, Some(vec!["from-cli".to_string()]));
            },
        );
    }

    #[test]
    fn test_allowed_hosts_env_var_used_when_no_cli_value() {
        let temp_dir = TempDir::new().unwrap();
        let non_existent_config = temp_dir.path().join("non_existent_config.toml");

        temp_env::with_vars(
            vec![
                ("WASSETTE_ALLOWED_HOSTS", Some("from-env")),
                (
                    "WASSETTE_CONFIG_FILE",
                    Some(non_existent_config.to_str().unwrap()),
                ),
            ],
            || {
                let config = Config::from_serve(&empty_test_cli_config(), None)
                    .expect("Failed to create config");

                assert_eq!(config.allowed_hosts, Some(vec!["from-env".to_string()]));
            },
        );
    }

    #[test]
    fn test_allowed_hosts_env_var_replaces_config_file_list() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        fs::write(&config_path, "allowed_hosts = [\"from-file\"]\n").unwrap();

        temp_env::with_var("WASSETTE_ALLOWED_HOSTS", Some("from-env"), || {
            let config = Config::new_from_path(&empty_test_cli_config(), &config_path)
                .unwrap_or_else(|e| {
                    panic!("Failed to create config: {e}");
                });

            // Not ["from-file", "from-env"]. figment's admerge concatenates sequences,
            // which is why this field is resolved outside figment.
            assert_eq!(
                config.allowed_hosts,
                Some(vec!["from-env".to_string()]),
                "the environment value must replace the file list, not extend it"
            );
        });
    }

    #[test]
    fn test_allowed_hosts_cli_replaces_config_file_list() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        fs::write(&config_path, "allowed_hosts = [\"from-file\"]\n").unwrap();

        temp_env::with_vars(
            vec![
                ("WASSETTE_ALLOWED_HOSTS", None),
                ("WASSETTE_CONFIG_FILE", Some(config_path.to_str().unwrap())),
            ],
            || {
                let cli_config = Serve {
                    allowed_hosts: Some(vec!["from-cli".to_string()]),
                    ..empty_test_cli_config()
                };

                let config =
                    Config::from_serve(&cli_config, None).expect("Failed to create config");

                assert_eq!(
                    config.allowed_hosts,
                    Some(vec!["from-cli".to_string()]),
                    "the CLI value must replace the file list, not extend it"
                );
            },
        );
    }

    #[test]
    fn test_allowed_hosts_empty_toml_list_extracts_as_empty_vec() {
        temp_env::with_var("WASSETTE_ALLOWED_HOSTS", None::<&str>, || {
            let temp_dir = TempDir::new().unwrap();
            let config_path = temp_dir.path().join("config.toml");
            fs::write(&config_path, "allowed_hosts = []\n").unwrap();

            let config = Config::new_from_path(&empty_test_cli_config(), &config_path)
                .expect("Failed to create config");

            // Deliberately Some(vec![]) rather than None: the empty list survives
            // extraction, and the guard that stops it reaching the transport lives at
            // the call site in main.rs, because an empty list there would mean "allow
            // every Host". This pins the shape that guard depends on.
            assert_eq!(config.allowed_hosts, Some(vec![]));
        });
    }

    #[test]
    fn test_allowed_hosts_from_config_file() {
        temp_env::with_var("WASSETTE_ALLOWED_HOSTS", None::<&str>, || {
            let temp_dir = TempDir::new().unwrap();
            let config_path = temp_dir.path().join("config.toml");
            fs::write(&config_path, "allowed_hosts = [\"from-file\"]\n").unwrap();

            let config = Config::new_from_path(&empty_test_cli_config(), &config_path)
                .expect("Failed to create config");

            assert_eq!(config.allowed_hosts, Some(vec!["from-file".to_string()]));
        });
    }
    #[test]
    fn test_transport_settings_default_to_todays_behaviour() {
        let temp_dir = TempDir::new().unwrap();
        let non_existent_config = temp_dir.path().join("non_existent_config.toml");

        let config = with_isolated_env(|| {
            Config::new_from_path(&empty_test_cli_config(), &non_existent_config)
        })
        .expect("Failed to create config");

        // Legacy clients keep their session lifecycle unless an operator opts out.
        assert!(config.legacy_sessions);
        assert!(!config.json_response);
    }

    #[test]
    fn test_transport_settings_from_config_file() {
        let temp_dir = TempDir::new().unwrap();
        let config_file = temp_dir.path().join("config.toml");

        let toml_content = r#"
legacy_sessions = false
json_response = true
"#;
        fs::write(&config_file, toml_content).unwrap();

        let config =
            with_isolated_env(|| Config::new_from_path(&empty_test_cli_config(), &config_file))
                .expect("Failed to create config");

        assert!(!config.legacy_sessions);
        assert!(config.json_response);
    }

    #[test]
    fn test_transport_settings_from_env_vars() {
        temp_env::with_vars(
            vec![
                ("WASSETTE_LEGACY_SESSIONS", Some("false")),
                ("WASSETTE_JSON_RESPONSE", Some("true")),
            ],
            || {
                let temp_dir = TempDir::new().unwrap();
                let non_existent_config = temp_dir.path().join("non_existent_config.toml");

                let config = Config::new_from_path(&empty_test_cli_config(), &non_existent_config)
                    .expect("Failed to create config");

                assert!(!config.legacy_sessions);
                assert!(config.json_response);
            },
        );
    }

    #[test]
    fn test_transport_settings_cli_override() {
        let temp_dir = TempDir::new().unwrap();
        let config_file = temp_dir.path().join("config.toml");

        let toml_content = r#"
legacy_sessions = false
json_response = true
"#;
        fs::write(&config_file, toml_content).unwrap();

        let serve_config = Serve {
            component_dir: None,
            transport: Default::default(),
            env_vars: vec![],
            env_file: None,
            disable_builtin_tools: false,
            bind_address: None,
            manifest: None,
            allowed_hosts: None,
            legacy_sessions: Some(true),
            json_response: Some(false),
        };

        let config = with_isolated_env(|| Config::new_from_path(&serve_config, &config_file))
            .expect("Failed to create config");

        // CLI values should take precedence over the config file
        assert!(config.legacy_sessions);
        assert!(!config.json_response);
    }
}
