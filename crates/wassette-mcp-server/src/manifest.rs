// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Provisioning manifest for headless deployment mode
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvisioningManifest {
    /// Manifest schema version
    pub version: u32,

    /// List of components to provision
    pub components: Vec<ComponentDeclaration>,
}

/// Component declaration in manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentDeclaration {
    /// Component URI (file://, oci://, https://)
    pub uri: String,

    /// Optional name for logging/identification
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Optional SHA-256 digest for verification
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,

    /// Permissions configuration (inline only in MVP)
    pub permissions: InlinePermissions,

    /// Optional retry policy (deferred to post-MVP)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_policy: Option<RetryPolicy>,
}

/// Inline permission declarations (only mode supported in MVP)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InlinePermissions {
    /// Network permissions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkPermissions>,

    /// Storage (filesystem) permissions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage: Option<StoragePermissions>,

    /// Environment variable permissions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentPermissions>,

    /// Memory and resource limits (deferred to post-MVP)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceLimits>,
}

/// Network access permissions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPermissions {
    /// List of allowed hosts
    pub allow: Vec<NetworkRule>,
}

/// Network access rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkRule {
    /// Host to allow (e.g., "api.example.com")
    pub host: String,
}

/// Storage access permissions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoragePermissions {
    /// List of allowed filesystem paths
    pub allow: Vec<StorageRule>,
}

/// Storage access rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageRule {
    /// URI of storage resource (e.g., "fs:///tmp/workspace")
    pub uri: String,

    /// Access types (read, write)
    pub access: Vec<AccessType>,
}

/// Storage access type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AccessType {
    Read,
    Write,
}

/// Environment variable permissions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentPermissions {
    /// List of allowed environment variables
    pub allow: Vec<EnvironmentRule>,
}

/// Environment variable rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentRule {
    /// Environment variable key
    pub key: String,

    /// Optional source hint (e.g., for GitHub Actions secrets)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_from: Option<String>,
}

/// Resource limits (deferred to post-MVP)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Memory limit in bytes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,

    /// CPU time limit in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_time_ms: Option<u64>,
}

/// Retry policy (deferred to post-MVP)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Number of retry attempts
    pub attempts: u32,

    /// Backoff strategy
    pub backoff: BackoffStrategy,
}

/// Backoff strategy for retries
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum BackoffStrategy {
    Exponential { base_ms: u64 },
    Linear { increment_ms: u64 },
}

impl ProvisioningManifest {
    /// Parse manifest from a YAML file
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read manifest file: {}", path.display()))?;

        Self::from_yaml(&content)
            .with_context(|| format!("Failed to parse manifest file: {}", path.display()))
    }

    /// Parse manifest from YAML string
    pub fn from_yaml(content: &str) -> Result<Self> {
        serde_yaml::from_str(content).context("Failed to deserialize manifest YAML")
    }

    /// Validate the manifest
    pub fn validate(&self) -> Result<()> {
        // Check version
        if self.version != 1 {
            bail!(
                "Unsupported manifest version: {}. Only version 1 is supported.",
                self.version
            );
        }

        // Check for components
        if self.components.is_empty() {
            bail!("Manifest must declare at least one component");
        }

        // Check for duplicate URIs
        let mut seen_uris = HashSet::new();
        let mut duplicate_uris = Vec::new();

        for component in &self.components {
            if !seen_uris.insert(&component.uri) {
                duplicate_uris.push(&component.uri);
            }
        }

        if !duplicate_uris.is_empty() {
            bail!(
                "Duplicate component URIs found: {}",
                duplicate_uris
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        // Validate each component
        for (idx, component) in self.components.iter().enumerate() {
            component
                .validate()
                .with_context(|| format!("Invalid component at index {}", idx))?;
        }

        Ok(())
    }
}

impl ComponentDeclaration {
    /// Validate the component declaration
    pub fn validate(&self) -> Result<()> {
        // Validate URI
        if self.uri.is_empty() {
            bail!("Component URI cannot be empty");
        }

        // Validate URI scheme
        let valid_schemes = ["file://", "oci://", "https://", "http://"];
        if !valid_schemes
            .iter()
            .any(|scheme| self.uri.starts_with(scheme))
        {
            bail!(
                "Component URI must start with one of: {}. Got: {}",
                valid_schemes.join(", "),
                self.uri
            );
        }

        // Validate digest format if present
        if let Some(digest) = &self.digest {
            if !digest.starts_with("sha256:") {
                bail!("Digest must be in format 'sha256:<hex>'. Got: {}", digest);
            }

            let hex_part = &digest[7..]; // Skip "sha256:"
            if hex_part.len() != 64 {
                bail!(
                    "SHA-256 digest must be 64 hex characters. Got: {} characters",
                    hex_part.len()
                );
            }

            if !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
                bail!("SHA-256 digest must contain only hex characters");
            }
        }

        // Validate permissions
        self.permissions
            .validate()
            .context("Invalid permissions configuration")?;

        Ok(())
    }
}

impl InlinePermissions {
    /// Validate inline permissions
    pub fn validate(&self) -> Result<()> {
        // At least one permission type should be specified
        if self.network.is_none()
            && self.storage.is_none()
            && self.environment.is_none()
            && self.resources.is_none()
        {
            bail!("Inline permissions must specify at least one permission type (network, storage, environment, or resources)");
        }

        // Validate network permissions
        if let Some(network) = &self.network {
            if network.allow.is_empty() {
                bail!("Network permissions 'allow' list cannot be empty");
            }

            for rule in &network.allow {
                if rule.host.is_empty() {
                    bail!("Network rule host cannot be empty");
                }
            }
        }

        // Validate storage permissions
        if let Some(storage) = &self.storage {
            if storage.allow.is_empty() {
                bail!("Storage permissions 'allow' list cannot be empty");
            }

            for rule in &storage.allow {
                if rule.uri.is_empty() {
                    bail!("Storage rule URI cannot be empty");
                }

                if !rule.uri.starts_with("fs://") {
                    bail!("Storage URI must start with 'fs://'. Got: {}", rule.uri);
                }

                if rule.access.is_empty() {
                    bail!("Storage rule must specify at least one access type (read or write)");
                }
            }
        }

        // Validate environment permissions
        if let Some(env) = &self.environment {
            if env.allow.is_empty() {
                bail!("Environment permissions 'allow' list cannot be empty");
            }

            let mut seen_keys = HashSet::new();
            for rule in &env.allow {
                if rule.key.is_empty() {
                    bail!("Environment variable key cannot be empty");
                }

                if !seen_keys.insert(&rule.key) {
                    bail!("Duplicate environment variable key: {}", rule.key);
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_manifest() {
        let yaml = r#"
version: 1
components:
  - uri: oci://ghcr.io/microsoft/get-weather-js:1.2.3
    name: weather-service
    digest: sha256:1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef
    permissions:
      network:
        allow:
          - host: api.openweathermap.com
      environment:
        allow:
          - key: OPENWEATHER_API_KEY
"#;

        let manifest = ProvisioningManifest::from_yaml(yaml).unwrap();
        assert_eq!(manifest.version, 1);
        assert_eq!(manifest.components.len(), 1);
        assert_eq!(
            manifest.components[0].uri,
            "oci://ghcr.io/microsoft/get-weather-js:1.2.3"
        );

        // Validation should pass
        manifest.validate().unwrap();
    }

    #[test]
    fn test_parse_multi_component_manifest() {
        let yaml = r#"
version: 1
components:
  - uri: oci://example.com/component1:latest
    permissions:
      network:
        allow:
          - host: api.example.com
  - uri: file:///opt/components/component2.wasm
    permissions:
      storage:
        allow:
          - uri: fs:///tmp/data
            access: [read, write]
"#;

        let manifest = ProvisioningManifest::from_yaml(yaml).unwrap();
        assert_eq!(manifest.components.len(), 2);
        manifest.validate().unwrap();
    }

    #[test]
    fn test_invalid_version() {
        let manifest = ProvisioningManifest {
            version: 2,
            components: vec![],
        };

        assert!(manifest.validate().is_err());
    }

    #[test]
    fn test_empty_components() {
        let manifest = ProvisioningManifest {
            version: 1,
            components: vec![],
        };

        assert!(manifest.validate().is_err());
    }

    #[test]
    fn test_duplicate_uris() {
        let yaml = r#"
version: 1
components:
  - uri: oci://example.com/component:latest
    permissions:
      network:
        allow:
          - host: api.example.com
  - uri: oci://example.com/component:latest
    permissions:
      network:
        allow:
          - host: api.example.com
"#;

        let manifest = ProvisioningManifest::from_yaml(yaml).unwrap();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn test_invalid_uri_scheme() {
        let yaml = r#"
version: 1
components:
  - uri: invalid://example.com/component
    permissions:
      network:
        allow:
          - host: api.example.com
"#;

        let manifest = ProvisioningManifest::from_yaml(yaml).unwrap();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn test_invalid_digest_format() {
        let yaml = r#"
version: 1
components:
  - uri: oci://example.com/component:latest
    digest: invalid-digest
    permissions:
      network:
        allow:
          - host: api.example.com
"#;

        let manifest = ProvisioningManifest::from_yaml(yaml).unwrap();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn test_empty_inline_permissions() {
        let yaml = r#"
version: 1
components:
  - uri: oci://example.com/component:latest
    permissions: {}
"#;

        let manifest = ProvisioningManifest::from_yaml(yaml).unwrap();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn test_invalid_storage_uri() {
        let yaml = r#"
version: 1
components:
  - uri: oci://example.com/component:latest
    permissions:
      storage:
        allow:
          - uri: /tmp/data
            access: [read]
"#;

        let manifest = ProvisioningManifest::from_yaml(yaml).unwrap();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn test_duplicate_env_keys() {
        let yaml = r#"
version: 1
components:
  - uri: oci://example.com/component:latest
    permissions:
      environment:
        allow:
          - key: API_KEY
          - key: API_KEY
"#;

        let manifest = ProvisioningManifest::from_yaml(yaml).unwrap();
        assert!(manifest.validate().is_err());
    }
}
