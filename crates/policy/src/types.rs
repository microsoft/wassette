//! Type definitions

use std::collections::HashMap;
use std::fmt::Display;

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

/// Resource limits configuration (future/TODO)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub cpu: Option<f64>,
    pub memory: Option<u64>,
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

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_permission_validation() {
        let mut permissions = Permissions::default();
        permissions.storage = Some(PermissionList {
            allow: Some(vec![StoragePermission {
                uri: "".to_string(),
                access: vec![AccessType::Read],
            }]),
            deny: None,
        });

        assert!(permissions.validate().is_err());
    }

    #[test]
    fn test_network_cidr_validation() {
        let mut permissions = Permissions::default();
        permissions.network = Some(PermissionList {
            allow: Some(vec![NetworkPermission::Cidr(NetworkCidrPermission {
                cidr: "invalid-cidr".to_string(), // Invalid CIDR format
            })]),
            deny: None,
        });

        assert!(permissions.validate().is_err());
    }

    #[test]
    fn test_valid_permissions() {
        let mut permissions = Permissions::default();
        permissions.storage = Some(PermissionList {
            allow: Some(vec![StoragePermission {
                uri: "fs://work/agent/**".to_string(),
                access: vec![AccessType::Read, AccessType::Write],
            }]),
            deny: None,
        });

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
        let mut permissions = Permissions::default();

        permissions.storage = Some(PermissionList {
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
        });

        permissions.network = Some(PermissionList {
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
        });

        // Test environment with valid keys (no wildcards allowed)
        permissions.environment = Some(EnvironmentPermissions {
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
        });

        assert!(permissions.validate().is_ok());
    }

    #[test]
    fn test_invalid_wildcard_combinations() {
        let mut permissions = Permissions::default();

        permissions.storage = Some(PermissionList {
            allow: Some(vec![StoragePermission {
                uri: "fs://work/agent/**file".to_string(),
                access: vec![AccessType::Read],
            }]),
            deny: None,
        });
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
