// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Secret management for Wassette components
//!
//! This module provides functionality to manage per-component secrets that are:
//! - Stored in OS-appropriate directories with proper permissions
//! - Persisted across runs without requiring server restart
//! - Easy to edit and audit via CLI
//! - Integrated with component environment variable system

use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{anyhow, Context, Result};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Cache entry for component secrets
#[derive(Debug, Clone)]
pub struct SecretCache {
    /// Environment variables from secrets
    pub env: HashMap<String, String>,
    /// Last modification time of the secrets file
    pub last_mtime: SystemTime,
}

/// Secrets manager for components
#[derive(Debug)]
pub struct SecretsManager {
    /// Directory where secrets are stored
    secrets_dir: PathBuf,
    /// Cache of component secrets
    cache: RwLock<HashMap<String, SecretCache>>,
}

impl SecretsManager {
    /// Create a new secrets manager
    pub fn new(secrets_dir: PathBuf) -> Self {
        Self {
            secrets_dir,
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// Get the secrets directory path
    pub fn secrets_dir(&self) -> &Path {
        &self.secrets_dir
    }

    /// Get the path to a component's secrets file
    pub fn get_component_secrets_path(&self, component_id: &str) -> PathBuf {
        let sanitized_id = sanitize_component_id(component_id);
        self.secrets_dir.join(format!("{}.yaml", sanitized_id))
    }

    /// Ensure the secrets directory exists with proper permissions
    pub async fn ensure_secrets_dir(&self) -> Result<()> {
        if !self.secrets_dir.exists() {
            info!("Creating secrets directory: {}", self.secrets_dir.display());
            tokio::fs::create_dir_all(&self.secrets_dir)
                .await
                .with_context(|| {
                    format!(
                        "Failed to create secrets directory: {}",
                        self.secrets_dir.display()
                    )
                })?;
        }

        // Set directory permissions to 0700 (user only)
        #[cfg(unix)]
        {
            let metadata = tokio::fs::metadata(&self.secrets_dir)
                .await
                .with_context(|| {
                    format!(
                        "Failed to get metadata for secrets directory: {}",
                        self.secrets_dir.display()
                    )
                })?;
            let mut perms = metadata.permissions();
            perms.set_mode(0o700);
            tokio::fs::set_permissions(&self.secrets_dir, perms)
                .await
                .with_context(|| {
                    format!(
                        "Failed to set permissions for secrets directory: {}",
                        self.secrets_dir.display()
                    )
                })?;
        }

        Ok(())
    }

    /// Load secrets for a component, using cache if file hasn't changed
    pub async fn load_component_secrets(
        &self,
        component_id: &str,
    ) -> Result<HashMap<String, String>> {
        let secrets_path = self.get_component_secrets_path(component_id);

        // Check if file exists
        if !secrets_path.exists() {
            debug!("No secrets file found for component: {}", component_id);
            return Ok(HashMap::new());
        }

        // Get file modification time
        let metadata = tokio::fs::metadata(&secrets_path).await.with_context(|| {
            format!(
                "Failed to get metadata for secrets file: {}",
                secrets_path.display()
            )
        })?;
        let mtime = metadata.modified().with_context(|| {
            format!(
                "Failed to get modification time for secrets file: {}",
                secrets_path.display()
            )
        })?;

        // Check cache
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(component_id) {
                if cached.last_mtime == mtime {
                    debug!("Using cached secrets for component: {}", component_id);
                    return Ok(cached.env.clone());
                }
            }
        }

        // Load from file
        debug!("Loading secrets from file for component: {}", component_id);
        let content = tokio::fs::read_to_string(&secrets_path)
            .await
            .with_context(|| format!("Failed to read secrets file: {}", secrets_path.display()))?;

        let secrets: HashMap<String, String> = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse secrets file: {}", secrets_path.display()))?;

        // Update cache
        let cache_entry = SecretCache {
            env: secrets.clone(),
            last_mtime: mtime,
        };

        {
            let mut cache = self.cache.write().await;
            cache.insert(component_id.to_string(), cache_entry);
        }

        Ok(secrets)
    }

    /// List secrets for a component (keys only by default)
    pub async fn list_component_secrets(
        &self,
        component_id: &str,
        show_values: bool,
    ) -> Result<HashMap<String, Option<String>>> {
        let secrets = self.load_component_secrets(component_id).await?;

        let result = if show_values {
            secrets.into_iter().map(|(k, v)| (k, Some(v))).collect()
        } else {
            secrets.into_keys().map(|k| (k, None)).collect()
        };

        Ok(result)
    }

    /// Set secrets for a component
    pub async fn set_component_secrets(
        &self,
        component_id: &str,
        secrets: &[(String, String)],
    ) -> Result<()> {
        self.ensure_secrets_dir().await?;

        let secrets_path = self.get_component_secrets_path(component_id);

        // Load existing secrets
        let mut existing_secrets = if secrets_path.exists() {
            let content = tokio::fs::read_to_string(&secrets_path)
                .await
                .with_context(|| {
                    format!(
                        "Failed to read existing secrets file: {}",
                        secrets_path.display()
                    )
                })?;
            serde_yaml::from_str::<HashMap<String, String>>(&content).with_context(|| {
                format!(
                    "Failed to parse existing secrets file: {}",
                    secrets_path.display()
                )
            })?
        } else {
            HashMap::new()
        };

        // Merge new secrets
        for (key, value) in secrets {
            existing_secrets.insert(key.clone(), value.clone());
        }

        // Write atomically
        self.write_secrets_file(&secrets_path, &existing_secrets)
            .await?;

        // Invalidate cache
        {
            let mut cache = self.cache.write().await;
            cache.remove(component_id);
        }

        info!("Updated secrets for component: {}", component_id);
        Ok(())
    }

    /// Delete secrets for a component
    pub async fn delete_component_secrets(
        &self,
        component_id: &str,
        keys: &[String],
    ) -> Result<()> {
        let secrets_path = self.get_component_secrets_path(component_id);

        if !secrets_path.exists() {
            return Err(anyhow!(
                "No secrets file found for component: {}",
                component_id
            ));
        }

        // Load existing secrets
        let content = tokio::fs::read_to_string(&secrets_path)
            .await
            .with_context(|| format!("Failed to read secrets file: {}", secrets_path.display()))?;
        let mut secrets: HashMap<String, String> = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse secrets file: {}", secrets_path.display()))?;

        // Remove specified keys
        for key in keys {
            if secrets.remove(key).is_none() {
                warn!(
                    "Secret key '{}' not found for component: {}",
                    key, component_id
                );
            }
        }

        if secrets.is_empty() {
            // Remove the file if no secrets remain
            tokio::fs::remove_file(&secrets_path)
                .await
                .with_context(|| {
                    format!(
                        "Failed to remove empty secrets file: {}",
                        secrets_path.display()
                    )
                })?;
            info!("Removed empty secrets file for component: {}", component_id);
        } else {
            // Write updated secrets
            self.write_secrets_file(&secrets_path, &secrets).await?;
            info!(
                "Deleted {} secret(s) for component: {}",
                keys.len(),
                component_id
            );
        }

        // Invalidate cache
        {
            let mut cache = self.cache.write().await;
            cache.remove(component_id);
        }

        Ok(())
    }

    /// Write secrets to file atomically with proper permissions
    async fn write_secrets_file(
        &self,
        secrets_path: &Path,
        secrets: &HashMap<String, String>,
    ) -> Result<()> {
        let content =
            serde_yaml::to_string(secrets).context("Failed to serialize secrets to YAML")?;

        // Write to temporary file first
        let temp_path = secrets_path.with_extension("tmp");
        tokio::fs::write(&temp_path, &content)
            .await
            .with_context(|| {
                format!(
                    "Failed to write temporary secrets file: {}",
                    temp_path.display()
                )
            })?;

        // Set file permissions to 0600 (user read/write only)
        #[cfg(unix)]
        {
            let metadata = tokio::fs::metadata(&temp_path).await.with_context(|| {
                format!(
                    "Failed to get metadata for temporary secrets file: {}",
                    temp_path.display()
                )
            })?;
            let mut perms = metadata.permissions();
            perms.set_mode(0o600);
            tokio::fs::set_permissions(&temp_path, perms)
                .await
                .with_context(|| {
                    format!(
                        "Failed to set permissions for temporary secrets file: {}",
                        temp_path.display()
                    )
                })?;
        }

        // Atomic rename
        tokio::fs::rename(&temp_path, secrets_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to rename temporary secrets file to: {}",
                    secrets_path.display()
                )
            })?;

        Ok(())
    }
}

/// Sanitize component ID for use as filename
/// Maps [^A-Za-z0-9._-] → _, collapses repeats, trims to 128 bytes
fn sanitize_component_id(component_id: &str) -> String {
    let mut result = String::new();
    let mut last_was_underscore = false;

    for ch in component_id.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' {
            result.push(ch);
            last_was_underscore = false;
        } else if !last_was_underscore {
            result.push('_');
            last_was_underscore = true;
        }
    }

    // Trim leading and trailing underscores
    while result.starts_with('_') {
        result.remove(0);
    }
    while result.ends_with('_') {
        result.pop();
    }

    // Trim to 128 bytes (being conservative with UTF-8)
    if result.len() > 128 {
        result.truncate(128);
        // Ensure we don't break in the middle of a character
        while !result.is_char_boundary(result.len()) {
            result.pop();
        }
    }

    // Ensure non-empty result
    if result.is_empty() {
        result = "unnamed".to_string();
    }

    result
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn test_sanitize_component_id() {
        assert_eq!(sanitize_component_id("simple"), "simple");
        assert_eq!(sanitize_component_id("with-dashes"), "with-dashes");
        assert_eq!(sanitize_component_id("with.dots"), "with.dots");
        assert_eq!(
            sanitize_component_id("with_underscores"),
            "with_underscores"
        );
        assert_eq!(sanitize_component_id("with/slashes"), "with_slashes");
        assert_eq!(sanitize_component_id("with spaces"), "with_spaces");
        assert_eq!(sanitize_component_id("with///multiple"), "with_multiple");
        assert_eq!(sanitize_component_id("trailing/"), "trailing");
        assert_eq!(sanitize_component_id("/leading"), "leading");
        assert_eq!(sanitize_component_id(""), "unnamed");

        // Test long string truncation
        let long_id = "a".repeat(200);
        let sanitized = sanitize_component_id(&long_id);
        assert!(sanitized.len() <= 128);
        assert!(sanitized.is_char_boundary(sanitized.len()));
    }

    #[tokio::test]
    async fn test_secrets_manager_basic() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let secrets_dir = temp_dir.path().join("secrets");
        let manager = SecretsManager::new(secrets_dir);

        // Test setting secrets
        let secrets = vec![
            ("API_KEY".to_string(), "secret123".to_string()),
            ("REGION".to_string(), "us-west-2".to_string()),
        ];
        manager
            .set_component_secrets("test-component", &secrets)
            .await?;

        // Test loading secrets
        let loaded = manager.load_component_secrets("test-component").await?;
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get("API_KEY"), Some(&"secret123".to_string()));
        assert_eq!(loaded.get("REGION"), Some(&"us-west-2".to_string()));

        // Test listing secrets
        let listed = manager
            .list_component_secrets("test-component", false)
            .await?;
        assert_eq!(listed.len(), 2);
        assert!(listed.contains_key("API_KEY"));
        assert!(listed.contains_key("REGION"));
        assert_eq!(listed.get("API_KEY"), Some(&None));

        let listed_with_values = manager
            .list_component_secrets("test-component", true)
            .await?;
        assert_eq!(
            listed_with_values.get("API_KEY"),
            Some(&Some("secret123".to_string()))
        );

        // Test deleting secrets
        manager
            .delete_component_secrets("test-component", &["API_KEY".to_string()])
            .await?;
        let after_delete = manager.load_component_secrets("test-component").await?;
        assert_eq!(after_delete.len(), 1);
        assert!(!after_delete.contains_key("API_KEY"));
        assert!(after_delete.contains_key("REGION"));

        Ok(())
    }

    #[tokio::test]
    async fn test_cache_invalidation() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let secrets_dir = temp_dir.path().join("secrets");
        let manager = SecretsManager::new(secrets_dir);

        // Set initial secrets
        let secrets = vec![("KEY1".to_string(), "value1".to_string())];
        manager.set_component_secrets("test", &secrets).await?;

        // Load secrets (should populate cache)
        let loaded1 = manager.load_component_secrets("test").await?;
        assert_eq!(loaded1.get("KEY1"), Some(&"value1".to_string()));

        // Modify secrets directly
        let secrets_path = manager.get_component_secrets_path("test");

        // Sleep to ensure mtime changes
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let new_content = "KEY1: value2\nKEY2: value3\n";
        tokio::fs::write(&secrets_path, new_content).await?;

        // Load again (should detect file change and reload)
        let loaded2 = manager.load_component_secrets("test").await?;
        assert_eq!(loaded2.get("KEY1"), Some(&"value2".to_string()));
        assert_eq!(loaded2.get("KEY2"), Some(&"value3".to_string()));

        Ok(())
    }

    #[tokio::test]
    async fn test_secrets_with_environment_precedence() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let secrets_dir = temp_dir.path().join("secrets");
        let manager = SecretsManager::new(secrets_dir);

        // Set secrets
        let secrets = vec![
            ("SECRET_KEY".to_string(), "from_secrets".to_string()),
            ("ONLY_IN_SECRETS".to_string(), "secret_value".to_string()),
        ];
        manager.set_component_secrets("test", &secrets).await?;

        // Test environment precedence using extract_env_vars function
        use policy::PolicyParser;

        use crate::wasistate::extract_env_vars;

        let yaml_content = r#"
version: "1.0"
description: "Test policy"
permissions:
  environment:
    allow:
      - key: "SECRET_KEY"
      - key: "ONLY_IN_SECRETS" 
      - key: "ONLY_IN_ENV"
"#;
        let policy = PolicyParser::parse_str(yaml_content)?;

        // Environment vars (highest precedence)
        let mut env_vars = std::collections::HashMap::new();
        env_vars.insert("SECRET_KEY".to_string(), "from_env".to_string());
        env_vars.insert("ONLY_IN_ENV".to_string(), "env_value".to_string());

        // Load secrets
        let loaded_secrets = manager.load_component_secrets("test").await?;

        // Test precedence
        let result = extract_env_vars(&policy, &env_vars, Some(&loaded_secrets))?;

        // SECRET_KEY should come from env (highest precedence)
        assert_eq!(result.get("SECRET_KEY"), Some(&"from_env".to_string()));

        // ONLY_IN_SECRETS should come from secrets
        assert_eq!(
            result.get("ONLY_IN_SECRETS"),
            Some(&"secret_value".to_string())
        );

        // ONLY_IN_ENV should come from env
        assert_eq!(result.get("ONLY_IN_ENV"), Some(&"env_value".to_string()));

        Ok(())
    }
}
