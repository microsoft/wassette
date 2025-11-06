// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use anyhow::{Context, Result};
use policy::{
    AccessType as PolicyAccessType, EnvironmentPermission, EnvironmentPermissions,
    NetworkHostPermission, NetworkPermission, PermissionList, PolicyDocument, StoragePermission,
};

use crate::manifest::{AccessType, InlinePermissions};

/// Synthesize a PolicyDocument from inline permissions in the manifest
pub fn synthesize_policy_from_inline(
    inline: &InlinePermissions,
    component_name: Option<&str>,
) -> Result<PolicyDocument> {
    let mut policy = PolicyDocument::new(
        "1.0",
        Some(format!(
            "Auto-generated policy for {}",
            component_name.unwrap_or("component")
        )),
    );

    // Convert network permissions
    if let Some(network_perms) = &inline.network {
        let mut network_allow = Vec::new();
        for rule in &network_perms.allow {
            network_allow.push(NetworkPermission::Host(NetworkHostPermission {
                host: rule.host.clone(),
            }));
        }

        policy.permissions.network = Some(PermissionList {
            allow: Some(network_allow),
            deny: None,
        });
    }

    // Convert storage permissions
    if let Some(storage_perms) = &inline.storage {
        let mut storage_allow = Vec::new();
        for rule in &storage_perms.allow {
            let access = rule
                .access
                .iter()
                .map(|a| match a {
                    AccessType::Read => PolicyAccessType::Read,
                    AccessType::Write => PolicyAccessType::Write,
                })
                .collect();

            storage_allow.push(StoragePermission {
                uri: rule.uri.clone(),
                access,
            });
        }

        policy.permissions.storage = Some(PermissionList {
            allow: Some(storage_allow),
            deny: None,
        });
    }

    // Convert environment permissions
    if let Some(env_perms) = &inline.environment {
        let mut env_allow = Vec::new();
        for rule in &env_perms.allow {
            env_allow.push(EnvironmentPermission {
                key: rule.key.clone(),
            });
        }

        policy.permissions.environment = Some(EnvironmentPermissions {
            allow: Some(env_allow),
        });
    }

    // Validate the generated policy
    policy
        .validate()
        .context("Generated policy failed validation")?;

    Ok(policy)
}

/// Serialize a PolicyDocument to YAML string
pub fn serialize_policy_to_yaml(policy: &PolicyDocument) -> Result<String> {
    serde_yaml::to_string(policy).context("Failed to serialize policy to YAML")
}

/// Full synthesis: convert inline permissions to YAML string
pub fn synthesize_policy_yaml(
    inline: &InlinePermissions,
    component_name: Option<&str>,
) -> Result<String> {
    let policy = synthesize_policy_from_inline(inline, component_name)?;
    serialize_policy_to_yaml(&policy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{
        EnvironmentPermissions as ManifestEnvPerms, EnvironmentRule,
        NetworkPermissions as ManifestNetPerms, NetworkRule,
        StoragePermissions as ManifestStoragePerms, StorageRule,
    };

    #[test]
    fn test_synthesize_network_only() {
        let inline = InlinePermissions {
            network: Some(ManifestNetPerms {
                allow: vec![
                    NetworkRule {
                        host: "api.example.com".to_string(),
                    },
                    NetworkRule {
                        host: "*.google.com".to_string(),
                    },
                ],
            }),
            storage: None,
            environment: None,
            resources: None,
        };

        let policy = synthesize_policy_from_inline(&inline, Some("test-component")).unwrap();

        assert_eq!(policy.version, "1.0");
        assert!(policy.description.is_some());

        let network = policy.permissions.network.unwrap();
        let allow = network.allow.unwrap();
        assert_eq!(allow.len(), 2);

        match &allow[0] {
            NetworkPermission::Host(h) => assert_eq!(h.host, "api.example.com"),
            _ => panic!("Expected Host permission"),
        }
    }

    #[test]
    fn test_synthesize_storage_only() {
        let inline = InlinePermissions {
            network: None,
            storage: Some(ManifestStoragePerms {
                allow: vec![StorageRule {
                    uri: "fs:///tmp/data".to_string(),
                    access: vec![AccessType::Read, AccessType::Write],
                }],
            }),
            environment: None,
            resources: None,
        };

        let policy = synthesize_policy_from_inline(&inline, Some("test-component")).unwrap();

        let storage = policy.permissions.storage.unwrap();
        let allow = storage.allow.unwrap();
        assert_eq!(allow.len(), 1);
        assert_eq!(allow[0].uri, "fs:///tmp/data");
        assert_eq!(allow[0].access.len(), 2);
    }

    #[test]
    fn test_synthesize_environment_only() {
        let inline = InlinePermissions {
            network: None,
            storage: None,
            environment: Some(ManifestEnvPerms {
                allow: vec![
                    EnvironmentRule {
                        key: "API_KEY".to_string(),
                        value_from: None,
                    },
                    EnvironmentRule {
                        key: "DATABASE_URL".to_string(),
                        value_from: Some("DB_URL".to_string()),
                    },
                ],
            }),
            resources: None,
        };

        let policy = synthesize_policy_from_inline(&inline, Some("test-component")).unwrap();

        let env = policy.permissions.environment.unwrap();
        let allow = env.allow.unwrap();
        assert_eq!(allow.len(), 2);
        assert_eq!(allow[0].key, "API_KEY");
        assert_eq!(allow[1].key, "DATABASE_URL");
    }

    #[test]
    fn test_synthesize_all_permissions() {
        let inline = InlinePermissions {
            network: Some(ManifestNetPerms {
                allow: vec![NetworkRule {
                    host: "api.example.com".to_string(),
                }],
            }),
            storage: Some(ManifestStoragePerms {
                allow: vec![StorageRule {
                    uri: "fs:///tmp/data".to_string(),
                    access: vec![AccessType::Read],
                }],
            }),
            environment: Some(ManifestEnvPerms {
                allow: vec![EnvironmentRule {
                    key: "API_KEY".to_string(),
                    value_from: None,
                }],
            }),
            resources: None,
        };

        let policy = synthesize_policy_from_inline(&inline, Some("test-component")).unwrap();

        assert!(policy.permissions.network.is_some());
        assert!(policy.permissions.storage.is_some());
        assert!(policy.permissions.environment.is_some());
    }

    #[test]
    fn test_synthesize_to_yaml() {
        let inline = InlinePermissions {
            network: Some(ManifestNetPerms {
                allow: vec![NetworkRule {
                    host: "api.example.com".to_string(),
                }],
            }),
            storage: None,
            environment: None,
            resources: None,
        };

        let yaml = synthesize_policy_yaml(&inline, Some("test-component")).unwrap();

        // Check that YAML is valid and contains expected fields
        assert!(yaml.contains("version:"));
        assert!(yaml.contains("1.0"));
        assert!(yaml.contains("description:"));
        assert!(yaml.contains("permissions:"));
        assert!(yaml.contains("network:"));
        assert!(yaml.contains("api.example.com"));

        // Verify it can be parsed back
        let parsed: PolicyDocument = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.version, "1.0");
        parsed.validate().unwrap();
    }
}
