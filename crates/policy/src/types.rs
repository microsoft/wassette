// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Type definitions

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;

use anyhow::bail;
use serde::{Deserialize, Serialize};

use crate::PolicyResult;

/// read: read access
/// write: write access
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessType {
    Read,
    Write,
}

/// uri: URI pattern for the resource (e.g. fs://work/agent/**)
/// access: Access types allowed (read, write)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoragePermission {
    /// URI pattern for the resource
    pub uri: String,
    /// Access types allowed
    pub access: Vec<AccessType>,
}

/// Network host permission
///
/// host: Hostname or pattern (supports wildcards like *.domain.com)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkHostPermission {
    /// Hostname or pattern (supports wildcards like *.domain.com)
    pub host: String,
}

/// Network CIDR permission
///
/// cidr: CIDR notation for network range (e.g. 10.0.0.0/8)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkCidrPermission {
    /// CIDR notation for network range
    pub cidr: String,
}

/// Network permission entry - can be either host or CIDR
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NetworkPermission {
    Host(NetworkHostPermission),
    Cidr(NetworkCidrPermission),
}

/// Environment variable permission
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentPermission {
    pub key: String,
}

/// Docker capability action
///
/// TODO: Add more capabilities
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum CapabilityAction {
    All,
    #[serde(rename = "NET_BIND_SERVICE")]
    NetBindService,
    #[serde(rename = "SYS_ADMIN")]
    SysAdmin,
    #[serde(rename = "SYS_TIME")]
    SysTime,
}

impl Display for CapabilityAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapabilityAction::All => write!(f, "ALL"),
            CapabilityAction::NetBindService => write!(f, "NET_BIND_SERVICE"),
            CapabilityAction::SysAdmin => write!(f, "SYS_ADMIN"),
            CapabilityAction::SysTime => write!(f, "SYS_TIME"),
        }
    }
}

/// Docker security capabilities configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DockerCapabilities {
    pub drop: Option<Vec<CapabilityAction>>,
    pub add: Option<Vec<CapabilityAction>>,
}

/// Docker security configuration
///
/// TODO: review this
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DockerSecurity {
    pub privileged: Option<bool>,
    pub no_new_privileges: Option<bool>,
    pub capabilities: Option<DockerCapabilities>,
}

/// Docker runtime configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DockerRuntime {
    pub security: Option<DockerSecurity>,
}

/// Hyperlight runtime configuration (future/TODO)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HyperlightRuntime {
    // TODO: Define hyperlight-specific configurations
    #[serde(flatten)]
    pub config: HashMap<String, serde_yaml::Value>,
}

/// CPU resource limit that supports k8s-style values
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CpuLimit {
    /// String format supporting millicores ("500m") or cores ("1", "2")
    String(String),
    /// Numeric format for backward compatibility
    Number(f64),
}

/// Memory resource limit that supports k8s-style values
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MemoryLimit {
    /// String format supporting Ki, Mi, Gi suffixes ("512Mi", "1Gi")
    String(String),
    /// Numeric format for backward compatibility (assumed to be in MB)
    Number(u64),
}

/// Resource limit values under the limits section
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResourceLimitValues {
    /// CPU limit in k8s format (millicores "500m" or cores "1")
    pub cpu: Option<CpuLimit>,
    /// Memory limit in k8s format ("512Mi", "1Gi", "256Ki")
    pub memory: Option<MemoryLimit>,
    /// Cached parsed CPU value in cores (not serialized)
    #[serde(skip)]
    cpu_cores_cache: OnceLock<f64>,
    /// Cached parsed memory value in bytes (not serialized)
    #[serde(skip)]
    memory_bytes_cache: OnceLock<u64>,
}

/// Resource limits configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResourceLimits {
    /// Resource limits in k8s-style format
    pub limits: Option<ResourceLimitValues>,
    /// Legacy numeric fields for backward compatibility
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub io: Option<u64>,
}

/// IPC permission configuration (future/TODO)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpcPermission {
    pub uri: String,
}

/// Runtime configuration
///
/// TODO: add more sandboxing runtimes
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Runtime {
    pub docker: Option<DockerRuntime>,
    pub hyperlight: Option<HyperlightRuntime>,
}

/// Permission list with allow/deny rules
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionList<T> {
    pub allow: Option<Vec<T>>,
    pub deny: Option<Vec<T>>,
}

impl<T> Default for PermissionList<T> {
    fn default() -> Self {
        Self {
            allow: None,
            deny: None,
        }
    }
}

/// Environment permissions (allow-only for security)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EnvironmentPermissions {
    pub allow: Option<Vec<EnvironmentPermission>>,
}

/// Complete permissions structure
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Permissions {
    pub storage: Option<PermissionList<StoragePermission>>,
    pub network: Option<PermissionList<NetworkPermission>>,
    pub environment: Option<EnvironmentPermissions>,
    pub runtime: Option<Runtime>,
    pub resources: Option<ResourceLimits>,
    pub ipc: Option<PermissionList<IpcPermission>>,
}

impl CpuLimit {
    /// Validate and convert CPU limit to numeric value (in cores)
    pub fn to_cores(&self) -> PolicyResult<f64> {
        match self {
            CpuLimit::String(s) => {
                if s.is_empty() {
                    bail!("CPU limit string cannot be empty");
                }

                if s.ends_with('m') {
                    // Millicores format like "500m"
                    let millicores_str = &s[..s.len() - 1];
                    let millicores: f64 = millicores_str
                        .parse()
                        .map_err(|_| anyhow::anyhow!("Invalid millicores value: {}", s))?;

                    if millicores < 0.0 {
                        bail!("CPU millicores cannot be negative: {}", s);
                    }

                    Ok(millicores / 1000.0)
                } else {
                    // Cores format like "1", "2", "0.5"
                    let cores: f64 = s
                        .parse()
                        .map_err(|_| anyhow::anyhow!("Invalid cores value: {}", s))?;

                    if cores < 0.0 {
                        bail!("CPU cores cannot be negative: {}", s);
                    }

                    Ok(cores)
                }
            }
            CpuLimit::Number(n) => {
                if *n < 0.0 {
                    bail!("CPU cores cannot be negative: {}", n);
                }
                Ok(*n)
            }
        }
    }
}

impl MemoryLimit {
    /// Validate and convert memory limit to bytes
    pub fn to_bytes(&self) -> PolicyResult<u64> {
        let bytes = match self {
            MemoryLimit::String(s) => {
                if s.is_empty() {
                    bail!("Memory limit string cannot be empty");
                }

                let (value_str, multiplier) = if s.ends_with("Ki") {
                    (&s[..s.len() - 2], 1024u64)
                } else if s.ends_with("Mi") {
                    (&s[..s.len() - 2], 1024u64 * 1024)
                } else if s.ends_with("Gi") {
                    (&s[..s.len() - 2], 1024u64 * 1024 * 1024)
                } else if s.ends_with("Ti") {
                    (&s[..s.len() - 2], 1024u64 * 1024 * 1024 * 1024)
                } else {
                    // No suffix, assume bytes
                    (s.as_str(), 1u64)
                };

                let value: u64 = value_str
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid memory value: {}", s))?;

                if value == 0 {
                    bail!("Memory limit cannot be zero: {}", s);
                }

                value
                    .checked_mul(multiplier)
                    .ok_or_else(|| anyhow::anyhow!("Memory value too large: {}", s))?
            }
            MemoryLimit::Number(n) => {
                if *n == 0 {
                    bail!("Memory limit cannot be zero");
                }
                // Assume legacy numeric values are in MB
                n.checked_mul(1024 * 1024)
                    .ok_or_else(|| anyhow::anyhow!("Memory value too large: {}", n))?
            }
        };

        Ok(bytes)
    }
}

impl ResourceLimitValues {
    /// Create a new ResourceLimitValues instance
    pub fn new(cpu: Option<CpuLimit>, memory: Option<MemoryLimit>) -> Self {
        Self {
            cpu,
            memory,
            cpu_cores_cache: OnceLock::new(),
            memory_bytes_cache: OnceLock::new(),
        }
    }

    /// Get CPU limit value in cores (cached)
    pub fn cpu_cores(&self) -> PolicyResult<Option<f64>> {
        if let Some(cpu) = &self.cpu {
            // Check if already cached
            if let Some(cached_value) = self.cpu_cores_cache.get() {
                return Ok(Some(*cached_value));
            }

            // Parse and cache the value
            let parsed_value = cpu.to_cores()?;
            let _ = self.cpu_cores_cache.set(parsed_value); // Ignore if already set by another thread
            Ok(Some(parsed_value))
        } else {
            Ok(None)
        }
    }

    /// Get memory limit value in bytes (cached)
    pub fn memory_bytes(&self) -> PolicyResult<Option<u64>> {
        if let Some(memory) = &self.memory {
            // Check if already cached
            if let Some(cached_value) = self.memory_bytes_cache.get() {
                return Ok(Some(*cached_value));
            }

            // Parse and cache the value
            let parsed_value = memory.to_bytes()?;
            let _ = self.memory_bytes_cache.set(parsed_value); // Ignore if already set by another thread
            Ok(Some(parsed_value))
        } else {
            Ok(None)
        }
    }

    /// Validate resource limit values
    pub fn validate(&self) -> PolicyResult<()> {
        // Validation now uses the cached getters, which will parse and cache the values
        self.cpu_cores()?;
        self.memory_bytes()?;
        Ok(())
    }
}

impl ResourceLimits {
    /// Validate resource limits
    pub fn validate(&self) -> PolicyResult<()> {
        if let Some(limits) = &self.limits {
            limits.validate()?;
        }

        // Validate legacy fields
        if let Some(cpu) = self.cpu {
            if cpu < 0.0 {
                bail!("Legacy CPU value cannot be negative: {}", cpu);
            }
        }

        if let Some(_memory) = self.memory {
            // Legacy memory values are fine as u64 is naturally non-negative
        }

        if let Some(_io) = self.io {
            // IO values are fine as u64 is naturally non-negative
        }

        Ok(())
    }
}

impl Permissions {
    fn validate_storage_uri(uri: &str) -> PolicyResult<()> {
        if uri.is_empty() {
            bail!("Storage URI can't be empty");
        }

        if uri.contains("***") {
            bail!("Too many wildcards in: {}", uri);
        }

        // Make sure ** is used properly (learned this the hard way)
        let parts: Vec<&str> = uri.split('/').collect();
        for part in parts.iter() {
            if part.contains("**") && *part != "**" {
                bail!("Wildcard ** needs to be its own path segment in: {}", uri);
            }

            // println!("DEBUG: checking part: {}", part); // TODO: remove this
            if part.contains('*') && *part != "*" && *part != "**" {
                let star_count = part.matches('*').count();
                if star_count > 1 && !part.contains("**") {
                    bail!("Multiple * in path segment '{}' - that's weird", part);
                }
            }
        }

        Ok(())
    }

    fn validate_network_host(host: &str) -> PolicyResult<()> {
        if host.is_empty() {
            bail!("Host can't be empty");
        }

        if host.matches('*').count() > 1 {
            bail!("Too many wildcards in host: {}", host);
        }

        if host.contains('*') && !host.starts_with("*.") && host != "*" {
            bail!("Wildcard should be at start like *.domain.com in: {}", host);
        }

        if let Some(domain_part) = host.strip_prefix("*.") {
            if domain_part.is_empty() || domain_part.ends_with('.') {
                bail!("Domain part looks wrong in: {}", host);
            }
        }

        Ok(())
    }

    fn validate_environment_key(key: &str) -> PolicyResult<()> {
        if key.is_empty() {
            bail!("Environment key can't be empty");
        }

        // No wildcards in env vars - too risky
        if key.contains('*') {
            bail!("No wildcards allowed in environment keys: {}", key);
        }

        Ok(())
    }

    /// Validate the permissions structure
    pub fn validate(&self) -> PolicyResult<()> {
        if let Some(storage) = &self.storage {
            if let Some(allow_list) = &storage.allow {
                for perm in allow_list {
                    Self::validate_storage_uri(&perm.uri)?;
                    if perm.access.is_empty() {
                        bail!("Storage needs some access permissions");
                    }
                }
            }
            if let Some(deny_list) = &storage.deny {
                for perm in deny_list {
                    Self::validate_storage_uri(&perm.uri)?;
                    if perm.access.is_empty() {
                        bail!("Storage needs some access permissions");
                    }
                }
            }
        }

        if let Some(network) = &self.network {
            if let Some(allow_list) = &network.allow {
                for perm in allow_list {
                    match perm {
                        NetworkPermission::Host(host_perm) => {
                            Self::validate_network_host(&host_perm.host)?;
                        }
                        NetworkPermission::Cidr(cidr_perm) => {
                            if cidr_perm.cidr.is_empty() {
                                bail!("CIDR can't be empty");
                            }
                            if !cidr_perm.cidr.contains('/') {
                                bail!("CIDR needs a slash: {}", cidr_perm.cidr);
                            }
                        }
                    }
                }
            }
            if let Some(deny_list) = &network.deny {
                for perm in deny_list {
                    match perm {
                        NetworkPermission::Host(host_perm) => {
                            Self::validate_network_host(&host_perm.host)?;
                        }
                        NetworkPermission::Cidr(cidr_perm) => {
                            if cidr_perm.cidr.is_empty() {
                                bail!("CIDR can't be empty");
                            }
                            if !cidr_perm.cidr.contains('/') {
                                bail!("CIDR needs a slash: {}", cidr_perm.cidr);
                            }
                        }
                    }
                }
            }
        }

        if let Some(env) = &self.environment {
            if let Some(allow_list) = &env.allow {
                for perm in allow_list {
                    Self::validate_environment_key(&perm.key)?;
                }
            }
        }

        if let Some(resources) = &self.resources {
            resources.validate()?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_permission_validation() {
        let permissions = Permissions {
            storage: Some(PermissionList {
                allow: Some(vec![StoragePermission {
                    uri: "".to_string(),
                    access: vec![AccessType::Read],
                }]),
                deny: None,
            }),
            ..Default::default()
        };

        assert!(permissions.validate().is_err());
    }

    #[test]
    fn test_network_cidr_validation() {
        let permissions = Permissions {
            network: Some(PermissionList {
                allow: Some(vec![NetworkPermission::Cidr(NetworkCidrPermission {
                    cidr: "invalid-cidr".to_string(), // Invalid CIDR format
                })]),
                deny: None,
            }),
            ..Default::default()
        };

        assert!(permissions.validate().is_err());
    }

    #[test]
    fn test_valid_permissions() {
        let permissions = Permissions {
            storage: Some(PermissionList {
                allow: Some(vec![StoragePermission {
                    uri: "fs://work/agent/**".to_string(),
                    access: vec![AccessType::Read, AccessType::Write],
                }]),
                deny: None,
            }),
            ..Default::default()
        };

        assert!(permissions.validate().is_ok());
    }

    #[test]
    fn test_storage_uri_wildcard_validation() {
        assert!(Permissions::validate_storage_uri("fs://work/agent/**").is_ok());
        assert!(Permissions::validate_storage_uri("fs://work/*/data").is_ok());
        assert!(Permissions::validate_storage_uri("fs://work/agent/*").is_ok());
        assert!(Permissions::validate_storage_uri("fs://work/agent/*/subdir/**").is_ok());

        assert!(Permissions::validate_storage_uri("").is_err());
        assert!(Permissions::validate_storage_uri("fs://work/agent/***").is_err());
        assert!(Permissions::validate_storage_uri("fs://work/agent/**file").is_err());
        assert!(Permissions::validate_storage_uri("fs://work/agent/file**.txt").is_err());
        assert!(Permissions::validate_storage_uri("fs://work/agent/**/**.txt").is_err());
    }

    #[test]
    fn test_network_host_wildcard_validation() {
        assert!(Permissions::validate_network_host("example.com").is_ok());
        assert!(Permissions::validate_network_host("*.example.com").is_ok());
        assert!(Permissions::validate_network_host("sub.example.com").is_ok());
        assert!(Permissions::validate_network_host("*").is_ok()); // only deny is allowed for *

        assert!(Permissions::validate_network_host("").is_err());
        assert!(Permissions::validate_network_host("*.*.example.com").is_err());
        assert!(Permissions::validate_network_host("example*.com").is_err());
        assert!(Permissions::validate_network_host("exam*ple.com").is_err());
        assert!(Permissions::validate_network_host("**example.com").is_err());
        assert!(Permissions::validate_network_host("*.").is_err());
        assert!(Permissions::validate_network_host("*.example.").is_err());
    }

    #[test]
    fn test_environment_key_validation() {
        assert!(Permissions::validate_environment_key("PATH").is_ok());
        assert!(Permissions::validate_environment_key("MY_VAR").is_ok());
        assert!(Permissions::validate_environment_key("HOME").is_ok());

        assert!(Permissions::validate_environment_key("").is_err());
        assert!(Permissions::validate_environment_key("PATH_*").is_err());
        assert!(Permissions::validate_environment_key("*_DEBUG").is_err());
        assert!(Permissions::validate_environment_key("*").is_err());
        assert!(Permissions::validate_environment_key("PA*TH").is_err());
        assert!(Permissions::validate_environment_key("*PATH*").is_err());
        assert!(Permissions::validate_environment_key("**PATH").is_err());
        assert!(Permissions::validate_environment_key("PATH**").is_err());
    }

    #[test]
    fn test_comprehensive_wildcard_validation() {
        let permissions = Permissions {
            storage: Some(PermissionList {
                allow: Some(vec![
                    StoragePermission {
                        uri: "fs://work/agent/**".to_string(),
                        access: vec![AccessType::Read, AccessType::Write],
                    },
                    StoragePermission {
                        uri: "fs://work/*/temp".to_string(),
                        access: vec![AccessType::Read],
                    },
                ]),
                deny: Some(vec![StoragePermission {
                    uri: "fs://work/agent/secret/*".to_string(),
                    access: vec![AccessType::Write],
                }]),
            }),
            network: Some(PermissionList {
                allow: Some(vec![
                    NetworkPermission::Host(NetworkHostPermission {
                        host: "*.example.com".to_string(),
                    }),
                    NetworkPermission::Host(NetworkHostPermission {
                        host: "api.service.com".to_string(),
                    }),
                ]),
                deny: Some(vec![NetworkPermission::Host(NetworkHostPermission {
                    host: "*.malicious.com".to_string(),
                })]),
            }),
            // Test environment with valid keys (no wildcards allowed)
            environment: Some(EnvironmentPermissions {
                allow: Some(vec![
                    EnvironmentPermission {
                        key: "PATH".to_string(),
                    },
                    EnvironmentPermission {
                        key: "HOME".to_string(),
                    },
                    EnvironmentPermission {
                        key: "MY_DEBUG_VAR".to_string(),
                    },
                ]),
            }),
            ..Default::default()
        };

        assert!(permissions.validate().is_ok());
    }

    #[test]
    fn test_cpu_limit_parsing() {
        // Test millicores format
        let cpu_millicores = CpuLimit::String("500m".to_string());
        assert_eq!(cpu_millicores.to_cores().unwrap(), 0.5);

        let cpu_millicores_large = CpuLimit::String("2000m".to_string());
        assert_eq!(cpu_millicores_large.to_cores().unwrap(), 2.0);

        // Test cores format
        let cpu_cores = CpuLimit::String("1".to_string());
        assert_eq!(cpu_cores.to_cores().unwrap(), 1.0);

        let cpu_cores_decimal = CpuLimit::String("1.5".to_string());
        assert_eq!(cpu_cores_decimal.to_cores().unwrap(), 1.5);

        // Test numeric format
        let cpu_numeric = CpuLimit::Number(2.5);
        assert_eq!(cpu_numeric.to_cores().unwrap(), 2.5);

        // Test invalid formats
        let invalid_empty = CpuLimit::String("".to_string());
        assert!(invalid_empty.to_cores().is_err());

        let invalid_millicores = CpuLimit::String("invalidm".to_string());
        assert!(invalid_millicores.to_cores().is_err());

        let invalid_cores = CpuLimit::String("invalid".to_string());
        assert!(invalid_cores.to_cores().is_err());

        let negative_numeric = CpuLimit::Number(-1.0);
        assert!(negative_numeric.to_cores().is_err());

        let negative_millicores = CpuLimit::String("-100m".to_string());
        assert!(negative_millicores.to_cores().is_err());
    }

    #[test]
    fn test_memory_limit_parsing() {
        // Test Ki format
        let memory_ki = MemoryLimit::String("512Ki".to_string());
        assert_eq!(memory_ki.to_bytes().unwrap(), 512 * 1024);

        // Test Mi format
        let memory_mi = MemoryLimit::String("256Mi".to_string());
        assert_eq!(memory_mi.to_bytes().unwrap(), 256 * 1024 * 1024);

        // Test Gi format
        let memory_gi = MemoryLimit::String("2Gi".to_string());
        assert_eq!(memory_gi.to_bytes().unwrap(), 2 * 1024 * 1024 * 1024);

        // Test Gi format (larger value)
        let memory_gi_large = MemoryLimit::String("32Gi".to_string());
        assert_eq!(
            memory_gi_large.to_bytes().unwrap(),
            32u64 * 1024 * 1024 * 1024
        );

        // Test plain bytes (above minimum)
        let memory_bytes = MemoryLimit::String("131072".to_string()); // 128KB
        assert_eq!(memory_bytes.to_bytes().unwrap(), 131072);

        // Test numeric format (legacy, assumes MB)
        let memory_numeric = MemoryLimit::Number(512);
        assert_eq!(memory_numeric.to_bytes().unwrap(), 512 * 1024 * 1024);

        // Test invalid formats
        let invalid_empty = MemoryLimit::String("".to_string());
        assert!(invalid_empty.to_bytes().is_err());

        let invalid_suffix = MemoryLimit::String("512Xi".to_string());
        assert!(invalid_suffix.to_bytes().is_err());

        let invalid_number = MemoryLimit::String("invalidMi".to_string());
        assert!(invalid_number.to_bytes().is_err());
    }

    #[test]
    fn test_resource_limit_values_validation() {
        // Valid resource limits
        let valid_limits = ResourceLimitValues::new(
            Some(CpuLimit::String("500m".to_string())),
            Some(MemoryLimit::String("512Mi".to_string())),
        );
        assert!(valid_limits.validate().is_ok());

        // Valid with numeric values
        let valid_numeric =
            ResourceLimitValues::new(Some(CpuLimit::Number(1.5)), Some(MemoryLimit::Number(256)));
        assert!(valid_numeric.validate().is_ok());

        // Invalid CPU
        let invalid_cpu =
            ResourceLimitValues::new(Some(CpuLimit::String("invalidm".to_string())), None);
        assert!(invalid_cpu.validate().is_err());

        // Invalid memory
        let invalid_memory =
            ResourceLimitValues::new(None, Some(MemoryLimit::String("invalidMi".to_string())));
        assert!(invalid_memory.validate().is_err());
    }

    #[test]
    fn test_resource_limit_values_caching() {
        // Test that parsing is cached for CPU
        let cpu_limits = ResourceLimitValues::new(
            Some(CpuLimit::String("500m".to_string())),
            Some(MemoryLimit::String("512Mi".to_string())),
        );

        // First call should parse and cache
        let cpu_result1 = cpu_limits.cpu_cores().unwrap();
        assert_eq!(cpu_result1, Some(0.5));

        // Second call should use cached value
        let cpu_result2 = cpu_limits.cpu_cores().unwrap();
        assert_eq!(cpu_result2, Some(0.5));

        // First call should parse and cache memory
        let memory_result1 = cpu_limits.memory_bytes().unwrap();
        assert_eq!(memory_result1, Some(512 * 1024 * 1024));

        // Second call should use cached value
        let memory_result2 = cpu_limits.memory_bytes().unwrap();
        assert_eq!(memory_result2, Some(512 * 1024 * 1024));

        // Test with None values
        let empty_limits = ResourceLimitValues::new(None, None);
        assert_eq!(empty_limits.cpu_cores().unwrap(), None);
        assert_eq!(empty_limits.memory_bytes().unwrap(), None);
    }

    #[test]
    fn test_resource_limits_validation() {
        // Valid new format
        let valid_new = ResourceLimits {
            limits: Some(ResourceLimitValues::new(
                Some(CpuLimit::String("500m".to_string())),
                Some(MemoryLimit::String("512Mi".to_string())),
            )),
            cpu: None,
            memory: None,
            io: None,
        };
        assert!(valid_new.validate().is_ok());

        // Valid legacy format
        let valid_legacy = ResourceLimits {
            limits: None,
            cpu: Some(1.5),
            memory: Some(512),
            io: Some(1000),
        };
        assert!(valid_legacy.validate().is_ok());

        // Invalid new format
        let invalid_new = ResourceLimits {
            limits: Some(ResourceLimitValues::new(
                Some(CpuLimit::String("invalidm".to_string())),
                None,
            )),
            cpu: None,
            memory: None,
            io: None,
        };
        assert!(invalid_new.validate().is_err());

        // Invalid legacy format
        let invalid_legacy = ResourceLimits {
            limits: None,
            cpu: Some(-1.0),
            memory: None,
            io: None,
        };
        assert!(invalid_legacy.validate().is_err());
    }

    #[test]
    fn test_k8s_style_permissions_validation() {
        let permissions = Permissions {
            storage: Some(PermissionList {
                allow: Some(vec![StoragePermission {
                    uri: "fs://workspace/**".to_string(),
                    access: vec![AccessType::Read, AccessType::Write],
                }]),
                deny: None,
            }),
            network: None,
            environment: None,
            runtime: None,
            resources: Some(ResourceLimits {
                limits: Some(ResourceLimitValues::new(
                    Some(CpuLimit::String("500m".to_string())),
                    Some(MemoryLimit::String("512Mi".to_string())),
                )),
                cpu: None,
                memory: None,
                io: None,
            }),
            ipc: None,
        };

        assert!(permissions.validate().is_ok());
    }

    #[test]
    fn test_invalid_wildcard_combinations() {
        let mut permissions = Permissions {
            storage: Some(PermissionList {
                allow: Some(vec![StoragePermission {
                    uri: "fs://work/agent/**file".to_string(),
                    access: vec![AccessType::Read],
                }]),
                deny: None,
            }),
            ..Default::default()
        };

        assert!(permissions.validate().is_err());

        permissions = Permissions::default();
        permissions.network = Some(PermissionList {
            allow: Some(vec![NetworkPermission::Host(NetworkHostPermission {
                host: "example*.com".to_string(), // Invalid: * in middle
            })]),
            deny: None,
        });
        assert!(permissions.validate().is_err());

        permissions = Permissions::default();
        permissions.environment = Some(EnvironmentPermissions {
            allow: Some(vec![EnvironmentPermission {
                key: "PATH_WITH_WILDCARD_*".to_string(),
            }]),
        });
        assert!(permissions.validate().is_err());
    }
}
