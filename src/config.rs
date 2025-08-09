// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::path::{Path, PathBuf};

use anyhow::Context;
use etcetera::BaseStrategy;
use figment::providers::{Env, Format, Serialized, Toml};
use serde::{Deserialize, Serialize};

/// Get the default component directory path based on the OS
pub fn get_component_dir() -> Result<PathBuf, anyhow::Error> {
    let dir_strategy = etcetera::choose_base_strategy().context("Unable to get home directory")?;
    Ok(dir_strategy.data_dir().join("wasette").join("components"))
}

fn default_plugin_dir() -> PathBuf {
    get_component_dir().unwrap_or_else(|_| {
        eprintln!("WARN: Unable to determine default component directory, using `components` directory in the current working directory");
        PathBuf::from("./components")
    })
}

/// Configuration for the Wasette MCP server
#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    /// Directory where plugins are stored
    #[serde(default = "default_plugin_dir")]
    pub plugin_dir: PathBuf,
}

impl Config {
    /// Returns a new [`Config`] instance by merging the configuration from the specified
    /// `cli_config` (any struct that is Serialize/Deserialize, but generally a Clap `Parser`) with
    /// the configuration file and environment variables. By default, the configuration file is
    /// located at `$XDG_CONFIG_HOME/wasette/config.toml`. This can be overridden by setting
    /// the `WASETTE_CONFIG_FILE` environment variable.
    ///
    /// The order of precedence for configuration sources is as follows:
    /// 1. Values from `cli_config`
    /// 2. Environment variables prefixed with `WASETTE_`
    /// 3. Configuration file specified by `WASETTE_CONFIG_FILE` or default location
    pub fn new<T: Serialize>(cli_config: &T) -> Result<Self, anyhow::Error> {
        let config_file_path = match std::env::var_os("WASETTE_CONFIG_FILE") {
            Some(path) => PathBuf::from(path),
            None => etcetera::choose_base_strategy()
                .context("Unable to get home directory")?
                .config_dir()
                .join("wasette")
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
            .admerge(Env::prefixed("WASETTE_"))
            .admerge(Serialized::defaults(cli_config))
            .extract()
            .context("Unable to merge configs")
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn create_test_cli_config() -> crate::Serve {
        crate::Serve {
            plugin_dir: Some(PathBuf::from("/test/plugin/dir")),
            stdio: true,
            sse: false,
            streamable_http: false,
        }
    }

    fn empty_test_cli_config() -> crate::Serve {
        crate::Serve {
            plugin_dir: None,
            stdio: false,
            sse: false,
            streamable_http: false,
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
        assert_eq!(config.plugin_dir, PathBuf::from("/test/plugin/dir"));
    }

    #[test]
    fn test_config_file_exists_with_cli_override() {
        let temp_dir = TempDir::new().unwrap();
        let config_file = temp_dir.path().join("config.toml");

        let toml_content = r#"
plugin_dir = "/config/plugin/dir"
"#;
        fs::write(&config_file, toml_content).unwrap();

        let serve_config = create_test_cli_config();
        let config =
            Config::new_from_path(&serve_config, &config_file).expect("Failed to create config");

        assert_eq!(config.plugin_dir, PathBuf::from("/test/plugin/dir"));
    }

    #[test]
    fn test_config_file_exists() {
        let temp_dir = TempDir::new().unwrap();
        let config_file = temp_dir.path().join("config.toml");

        let toml_content = r#"
plugin_dir = "/config/plugin/dir"
"#;
        fs::write(&config_file, toml_content).unwrap();

        let config = Config::new_from_path(&empty_test_cli_config(), &config_file)
            .expect("Failed to create config");

        assert_eq!(config.plugin_dir, PathBuf::from("/config/plugin/dir"));
    }

    #[test]
    fn test_cli_config_provides_defaults() {
        let temp_dir = TempDir::new().unwrap();
        let non_existent_config = temp_dir.path().join("non_existent_config.toml");

        let serve_config = create_test_cli_config();
        let config = Config::new_from_path(&serve_config, &non_existent_config)
            .expect("Failed to create config");

        // Should use CLI config values as defaults
        assert_eq!(config.plugin_dir, PathBuf::from("/test/plugin/dir"));
    }

    #[test]
    fn test_config_file_partial_values() {
        let temp_dir = TempDir::new().unwrap();
        let config_file = temp_dir.path().join("config.toml");

        // Config file only sets plugin_dir, not policy_file
        let toml_content = r#"
plugin_dir = "/config/plugin/dir"
"#;
        fs::write(&config_file, toml_content).unwrap();

        let config = Config::new_from_path(&empty_test_cli_config(), &config_file)
            .expect("Failed to create config");

        // plugin_dir should come from config file
        assert_eq!(config.plugin_dir, PathBuf::from("/config/plugin/dir"));
    }

    #[test]
    fn test_new_method_without_wasette_config_file_env() {
        // This test verifies that new() works when WASETTE_CONFIG_FILE is not set
        // It should try to use the default config location, which likely won't exist
        // but should still succeed with defaults

        // Ensure WASETTE_CONFIG_FILE is not set
        std::env::remove_var("WASETTE_CONFIG_FILE");

        let serve_config = create_test_cli_config();
        let config = Config::new(&serve_config).expect("Failed to create config");

        // Should use CLI defaults since no config file exists
        assert_eq!(config.plugin_dir, PathBuf::from("/test/plugin/dir"));
    }

    #[test]
    fn test_invalid_toml_file_returns_error() {
        let temp_dir = TempDir::new().unwrap();
        let config_file = temp_dir.path().join("invalid_config.toml");

        // Write invalid TOML content
        let invalid_toml = r#"
plugin_dir = "/some/path"
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
plugin_dir = "/custom/plugin/dir"
policy_file = "custom_policy.yaml"
"#;
        fs::write(&config_file, toml_content).unwrap();

        // Use SetEnv helper to manage WASETTE_CONFIG_FILE environment variable
        let _env = SetEnv::new("WASETTE_CONFIG_FILE", config_file.to_str().unwrap());

        let config = Config::new(&empty_test_cli_config()).expect("Failed to create config");

        assert_eq!(config.plugin_dir, PathBuf::from("/custom/plugin/dir"));
    }
}
