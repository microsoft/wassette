// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use etcetera::BaseStrategy;
use figment::providers::{Env, Format, Serialized, Toml};
use serde::{Deserialize, Serialize};

use crate::commands::Serve;

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

fn default_bind_address() -> String {
    "127.0.0.1:9001".to_string()
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

    /// Bind address for HTTP-based transports (SSE and StreamableHttp)
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
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
        figment::Figment::new()
            .admerge(Toml::file(config_file_path))
            .admerge(Env::prefixed("WASSETTE_"))
            .admerge(Serialized::defaults(cli_config))
            .extract()
            .context("Unable to merge configs")
    }

    /// Creates a new config from a Serve struct that includes environment variable handling
    pub fn from_serve(serve_config: &Serve) -> Result<Self, anyhow::Error> {
        // Start with the base config using existing logic
        let mut config = Self::new(serve_config)?;

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

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn create_test_cli_config() -> Serve {
        Serve {
            component_dir: Some(PathBuf::from("/test/component/dir")),
            transport: Default::default(),
            env_vars: vec![],
            env_file: None,
            disable_builtin_tools: false,
            bind_address: None,
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
        }
    }

    struct SetEnv<'a> {
        old: Option<OsString>,
        key: &'a str,
    }

    impl Drop for SetEnv<'_> {
        fn drop(&mut self) {
            if let Some(old_value) = &self.old {
                std::env::set_var(self.key, old_value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    impl<'a> SetEnv<'a> {
        fn new(key: &'a str, value: &'a str) -> Self {
            let old_value = std::env::var_os(key);
            std::env::set_var(key, value);
            SetEnv {
                old: old_value,
                key,
            }
        }
    }

    #[test]
    fn test_config_file_not_exists_succeeds_with_defaults() {
        let temp_dir = TempDir::new().unwrap();
        let non_existent_config = temp_dir.path().join("non_existent_config.toml");

        let serve_config = create_test_cli_config();
        let config = Config::new_from_path(&serve_config, &non_existent_config)
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
        let config =
            Config::new_from_path(&serve_config, &config_file).expect("Failed to create config");

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

        let config = Config::new_from_path(&empty_test_cli_config(), &config_file)
            .expect("Failed to create config");

        assert_eq!(config.component_dir, PathBuf::from("/config/component/dir"));
    }

    #[test]
    fn test_cli_config_provides_defaults() {
        let temp_dir = TempDir::new().unwrap();
        let non_existent_config = temp_dir.path().join("non_existent_config.toml");

        let serve_config = create_test_cli_config();
        let config = Config::new_from_path(&serve_config, &non_existent_config)
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

        let config = Config::new_from_path(&empty_test_cli_config(), &config_file)
            .expect("Failed to create config");

        // component_dir should come from config file
        assert_eq!(config.component_dir, PathBuf::from("/config/component/dir"));
    }

    #[test]
    fn test_new_method_without_wassette_config_file_env() {
        // This test verifies that new() works when WASSETTE_CONFIG_FILE is not set
        // It should try to use the default config location, which likely won't exist
        // but should still succeed with defaults

        // Ensure WASSETTE_CONFIG_FILE is not set
        std::env::remove_var("WASSETTE_CONFIG_FILE");

        let serve_config = create_test_cli_config();
        let config = Config::new(&serve_config).expect("Failed to create config");

        // Should use CLI defaults since no config file exists
        assert_eq!(config.component_dir, PathBuf::from("/test/component/dir"));
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
        let result = Config::new_from_path(&serve_config, &config_file);

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

        // Use SetEnv helper to manage WASSETTE_CONFIG_FILE environment variable
        let _env = SetEnv::new("WASSETTE_CONFIG_FILE", config_file.to_str().unwrap());

        let config = Config::new(&empty_test_cli_config()).expect("Failed to create config");

        assert_eq!(config.component_dir, PathBuf::from("/custom/component/dir"));
    }

    #[test]
    fn test_bind_address_default() {
        temp_env::with_vars_unset(vec!["WASSETTE_BIND_ADDRESS"], || {
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
        temp_env::with_vars_unset(vec!["WASSETTE_BIND_ADDRESS"], || {
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
        };

        let config =
            Config::new_from_path(&serve_config, &config_file).expect("Failed to create config");

        // CLI value should take precedence
        assert_eq!(config.bind_address, "192.168.1.100:9090");
    }

    #[test]
    fn test_bind_address_env_var() {
        temp_env::with_var("WASSETTE_BIND_ADDRESS", Some("10.0.0.1:3000"), || {
            let temp_dir = TempDir::new().unwrap();
            let non_existent_config = temp_dir.path().join("non_existent_config.toml");

            let config = Config::new_from_path(&empty_test_cli_config(), &non_existent_config)
                .expect("Failed to create config");

            // Environment variable should be used
            assert_eq!(config.bind_address, "10.0.0.1:3000");
        });
    }

    #[test]
    fn test_bind_address_precedence() {
        temp_env::with_var("WASSETTE_BIND_ADDRESS", Some("10.0.0.1:3000"), || {
            let temp_dir = TempDir::new().unwrap();
            let config_file = temp_dir.path().join("config.toml");

            // Config file sets bind address
            let toml_content = r#"
bind_address = "0.0.0.0:8080"
"#;
            fs::write(&config_file, toml_content).unwrap();

            // CLI provides bind address
            let serve_config = Serve {
                component_dir: None,
                transport: Default::default(),
                env_vars: vec![],
                env_file: None,
                disable_builtin_tools: false,
                bind_address: Some("192.168.1.100:9090".to_string()),
            };

            let config = Config::new_from_path(&serve_config, &config_file)
                .expect("Failed to create config");

            // CLI value should take highest precedence
            assert_eq!(config.bind_address, "192.168.1.100:9090");
        });
    }
}
