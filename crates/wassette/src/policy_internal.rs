// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Policy management structures and types

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use policy::{
    AccessType, EnvironmentPermission, NetworkHostPermission, NetworkPermission, PolicyDocument,
    PolicyParser, StoragePermission,
};
use serde::{Deserialize, Serialize};
use tracing::{info, instrument};

use crate::WasiStateTemplate;

/// Granular permission rule types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PermissionRule {
    /// Network access permission
    #[serde(rename = "network")]
    Network(NetworkPermission),
    /// File system storage permission
    #[serde(rename = "storage")]
    Storage(StoragePermission),
    /// Environment variable access permission
    #[serde(rename = "environment")]
    Environment(EnvironmentPermission),
    /// Custom permission with arbitrary data
    #[serde(rename = "custom")]
    Custom(String, serde_json::Value),
}

/// Permission grant request structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionGrantRequest {
    /// The ID of the component requesting permission
    pub component_id: String,
    /// The type of permission being requested
    pub permission_type: String,
    /// Additional details specific to the permission type
    pub details: serde_json::Value,
}

/// Registry for storing policy templates associated with components
#[derive(Default)]
pub(crate) struct PolicyRegistry {
    /// Maps component IDs to their associated policy templates
    pub(crate) component_policies: HashMap<String, Arc<WasiStateTemplate>>,
}

/// Information about a policy attached to a component
#[derive(Debug, Clone)]
pub struct PolicyInfo {
    /// Unique identifier for the policy
    pub policy_id: String,
    /// The original URI where the policy was loaded from
    pub source_uri: String,
    /// Local filesystem path where the policy is stored
    pub local_path: PathBuf,
    /// ID of the component this policy is attached to
    pub component_id: String,
    /// Timestamp when the policy was created/attached
    pub created_at: std::time::SystemTime,
}

impl crate::LifecycleManager {
    /// Attaches a policy to a component. The policy can be a local file or a URL.
    /// This function will download the policy from the given URI and store it
    /// in the plugin directory specified by the `plugin_dir`, co-located with
    /// the component. The component_id must be the ID of a component that is
    /// already loaded.
    pub async fn attach_policy(&self, component_id: &str, policy_uri: &str) -> Result<()> {
        info!(component_id, policy_uri, "Attaching policy to component");

        if !self.components.read().await.contains_key(component_id) {
            return Err(anyhow!("Component not found: {}", component_id));
        }

        let downloaded_policy = crate::loader::load_resource::<crate::PolicyResource>(
            policy_uri,
            &self.oci_client,
            &self.http_client,
        )
        .await?;

        let policy = PolicyParser::parse_file(downloaded_policy.as_ref())?;

        let policy_path = self.get_component_policy_path(component_id);
        tokio::fs::copy(downloaded_policy.as_ref(), &policy_path).await?;

        // Store metadata about the policy source
        let metadata = serde_json::json!({
            "source_uri": policy_uri,
            "attached_at": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
        });
        let metadata_path = self.get_component_metadata_path(component_id);
        tokio::fs::write(&metadata_path, serde_json::to_string_pretty(&metadata)?).await?;

        let wasi_template =
            crate::create_wasi_state_template_from_policy(&policy, &self.plugin_dir)?;
        self.policy_registry
            .write()
            .await
            .component_policies
            .insert(component_id.to_string(), Arc::new(wasi_template));

        info!(component_id, policy_uri, "Policy attached successfully");
        Ok(())
    }

    /// Detaches a policy from a component. This will remove the policy from the
    /// component and remove the policy file from the plugin directory.
    pub async fn detach_policy(&self, component_id: &str) -> Result<()> {
        info!(component_id, "Detaching policy from component");

        // Remove files first, then clean up memory on success
        let policy_path = self.get_component_policy_path(component_id);
        self.remove_file_if_exists(&policy_path, "policy file", component_id)
            .await?;

        let metadata_path = self.get_component_metadata_path(component_id);
        self.remove_file_if_exists(&metadata_path, "policy metadata file", component_id)
            .await?;

        // Only cleanup memory after all files are successfully removed
        self.cleanup_policy_registry(component_id).await;

        info!(component_id, "Policy detached successfully");
        Ok(())
    }

    /// Returns information about the policy attached to a component.
    /// Returns `None` if no policy is attached to the component.
    ///
    /// The information contains the policy ID, source URI, local path, component ID,
    /// and creation time.
    pub async fn get_policy_info(&self, component_id: &str) -> Option<PolicyInfo> {
        let policy_path = self.get_component_policy_path(component_id);
        if !tokio::fs::try_exists(&policy_path).await.unwrap_or(false) {
            return None;
        }

        let metadata_path = self.get_component_metadata_path(component_id);
        let source_uri =
            if let Ok(metadata_content) = tokio::fs::read_to_string(&metadata_path).await {
                if let Ok(metadata) = serde_json::from_str::<serde_json::Value>(&metadata_content) {
                    metadata
                        .get("source_uri")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string()
                } else {
                    format!("file://{}", policy_path.display())
                }
            } else {
                format!("file://{}", policy_path.display())
            };

        let metadata = tokio::fs::metadata(&policy_path).await.ok()?;
        let created_at = metadata
            .created()
            .unwrap_or_else(|_| std::time::SystemTime::now());

        Some(PolicyInfo {
            policy_id: format!("{component_id}-policy"),
            source_uri,
            local_path: policy_path,
            component_id: component_id.to_string(),
            created_at,
        })
    }

    pub(crate) fn get_component_policy_path(&self, component_id: &str) -> PathBuf {
        self.plugin_dir.join(format!("{component_id}.policy.yaml"))
    }

    pub(crate) fn get_component_metadata_path(&self, component_id: &str) -> PathBuf {
        self.plugin_dir
            .join(format!("{component_id}.policy.meta.json"))
    }

    pub(crate) fn create_default_policy_template() -> Arc<WasiStateTemplate> {
        Arc::new(WasiStateTemplate::default())
    }

    /// Helper function to clean up policy registry for a component
    pub(crate) async fn cleanup_policy_registry(&self, component_id: &str) {
        self.policy_registry
            .write()
            .await
            .component_policies
            .remove(component_id);
    }

    /// Grant a specific permission rule to a component
    #[instrument(skip(self))]
    pub async fn grant_permission(
        &self,
        component_id: &str,
        permission_type: &str,
        details: &serde_json::Value,
    ) -> Result<()> {
        info!(
            component_id,
            permission_type, "Granting permission to component"
        );
        if !self.components.read().await.contains_key(component_id) {
            return Err(anyhow!("Component not found: {}", component_id));
        }

        let permission_rule = self.parse_permission_rule(permission_type, details)?;
        self.validate_permission_rule(&permission_rule)?;
        let mut policy = self.load_or_create_component_policy(component_id).await?;
        self.add_permission_rule_to_policy(&mut policy, permission_rule)?;
        self.save_component_policy(component_id, &policy).await?;
        self.update_policy_registry(component_id, &policy).await?;

        info!(
            component_id,
            permission_type, "Permission granted successfully"
        );
        Ok(())
    }

    /// Parse a permission rule from the request details
    fn parse_permission_rule(
        &self,
        permission_type: &str,
        details: &serde_json::Value,
    ) -> Result<PermissionRule> {
        let permission_rule = match permission_type {
            "network" => {
                let host = details
                    .get("host")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("Missing 'host' field for network permission"))?;
                PermissionRule::Network(NetworkPermission::Host(NetworkHostPermission {
                    host: host.to_string(),
                }))
            }
            "storage" => {
                let uri = details
                    .get("uri")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("Missing 'uri' field for storage permission"))?;
                let access = details
                    .get("access")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| anyhow!("Missing 'access' field for storage permission"))?;

                let access_types: Result<Vec<AccessType>> = access
                    .iter()
                    .map(|v| v.as_str().ok_or_else(|| anyhow!("Invalid access type")))
                    .map(|s| match s? {
                        "read" => Ok(AccessType::Read),
                        "write" => Ok(AccessType::Write),
                        other => Err(anyhow!("Invalid access type: {}", other)),
                    })
                    .collect();

                PermissionRule::Storage(StoragePermission {
                    uri: uri.to_string(),
                    access: access_types?,
                })
            }
            "environment" => {
                let key = details
                    .get("key")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("Missing 'key' field for environment permission"))?;
                PermissionRule::Environment(EnvironmentPermission {
                    key: key.to_string(),
                })
            }
            other => {
                // For custom permission types, store the type name and raw details
                PermissionRule::Custom(other.to_string(), details.clone())
            }
        };

        Ok(permission_rule)
    }

    /// Load or create component policy
    async fn load_or_create_component_policy(
        &self,
        component_id: &str,
    ) -> Result<policy::PolicyDocument> {
        let policy_path = self.get_component_policy_path(component_id);

        if policy_path.exists() {
            let policy_content = tokio::fs::read_to_string(&policy_path).await?;
            Ok(PolicyParser::parse_str(&policy_content)?)
        } else {
            // Create minimal policy document
            Ok(policy::PolicyDocument {
                version: "1.0".to_string(),
                description: Some(format!(
                    "Auto-generated policy for component: {component_id}"
                )),
                permissions: Default::default(),
            })
        }
    }

    /// Add permission rule to policy
    fn add_permission_rule_to_policy(
        &self,
        policy: &mut policy::PolicyDocument,
        rule: PermissionRule,
    ) -> Result<()> {
        match rule {
            PermissionRule::Network(network) => {
                self.add_network_permission_to_policy(policy, network)
            }
            PermissionRule::Storage(storage) => {
                self.add_storage_permission_to_policy(policy, storage)
            }
            PermissionRule::Environment(env) => {
                self.add_environment_permission_to_policy(policy, env)
            }
            PermissionRule::Custom(type_name, _details) => {
                todo!("Custom permission type '{}' not yet implemented", type_name);
            }
        }
    }

    /// Add network permission to policy
    fn add_network_permission_to_policy(
        &self,
        policy: &mut PolicyDocument,
        network: NetworkPermission,
    ) -> Result<()> {
        let allow_set = policy
            .permissions
            .network
            .get_or_insert_with(Default::default)
            .allow
            .get_or_insert_with(Vec::new);

        // Only add if not already present (prevent duplicates)
        if !allow_set.contains(&network) {
            allow_set.push(network);
        }

        Ok(())
    }

    /// Add storage permission to policy
    fn add_storage_permission_to_policy(
        &self,
        policy: &mut PolicyDocument,
        storage: StoragePermission,
    ) -> Result<()> {
        let allow_set = policy
            .permissions
            .storage
            .get_or_insert_with(Default::default)
            .allow
            .get_or_insert_with(Vec::new);

        // Check if we already have a permission for this URI
        if let Some(existing) = allow_set.iter_mut().find(|p| p.uri == storage.uri) {
            // Merge access types, ensuring no duplicates
            for access_type in storage.access {
                if !existing.access.contains(&access_type) {
                    existing.access.push(access_type);
                }
            }
        } else {
            // Add new storage permission (only if not already present)
            if !allow_set.contains(&storage) {
                allow_set.push(storage);
            }
        }

        Ok(())
    }

    /// Add environment permission to policy
    fn add_environment_permission_to_policy(
        &self,
        policy: &mut PolicyDocument,
        env: EnvironmentPermission,
    ) -> Result<()> {
        let allow_set = policy
            .permissions
            .environment
            .get_or_insert_with(Default::default)
            .allow
            .get_or_insert_with(Vec::new);

        // Only add if not already present (prevent duplicates)
        if !allow_set.contains(&env) {
            allow_set.push(env);
        }

        Ok(())
    }

    /// Save component policy to file
    async fn save_component_policy(
        &self,
        component_id: &str,
        policy: &PolicyDocument,
    ) -> Result<()> {
        let policy_path = self.get_component_policy_path(component_id);
        let policy_yaml = serde_yaml::to_string(policy)?;
        tokio::fs::write(&policy_path, policy_yaml).await?;
        Ok(())
    }

    /// Update policy registry with new policy
    async fn update_policy_registry(
        &self,
        component_id: &str,
        policy: &PolicyDocument,
    ) -> Result<()> {
        let wasi_template =
            crate::create_wasi_state_template_from_policy(policy, &self.plugin_dir)?;
        self.policy_registry
            .write()
            .await
            .component_policies
            .insert(component_id.to_string(), Arc::new(wasi_template));
        Ok(())
    }

    /// Validate permission rule
    fn validate_permission_rule(&self, rule: &PermissionRule) -> Result<()> {
        match rule {
            PermissionRule::Network(NetworkPermission::Host(NetworkHostPermission { host })) => {
                if host.is_empty() {
                    return Err(anyhow!("Network host cannot be empty"));
                }
            }
            PermissionRule::Storage(storage) => {
                // TODO: the validation should verify if the uri is actually valid or not
                if storage.uri.is_empty() {
                    return Err(anyhow!("Storage URI cannot be empty"));
                }
                if storage.access.is_empty() {
                    return Err(anyhow!("Storage access cannot be empty"));
                }
            }
            PermissionRule::Environment(env) => {
                if env.key.is_empty() {
                    return Err(anyhow!("Environment variable key cannot be empty"));
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::*;

    #[tokio::test]
    async fn test_policy_attachment_and_detachment() -> Result<()> {
        let manager = create_test_manager().await?;
        manager.load_test_component().await?;

        // Create a test policy file
        let policy_content = r#"
version: "1.0"
description: "Test policy"
permissions:
  network:
    allow:
      - host: "example.com"
  environment:
    allow:
      - key: "TEST_VAR"
"#;
        let policy_path = manager.plugin_dir.join("test-policy.yaml");
        tokio::fs::write(&policy_path, policy_content).await?;

        let policy_uri = format!("file://{}", policy_path.display());

        // Test policy attachment
        manager
            .attach_policy(TEST_COMPONENT_ID, &policy_uri)
            .await?;

        // Verify policy is attached
        let policy_info = manager.get_policy_info(TEST_COMPONENT_ID).await;
        assert!(policy_info.is_some());
        let info = policy_info.unwrap();
        assert_eq!(info.component_id, TEST_COMPONENT_ID);
        assert_eq!(info.source_uri, policy_uri);

        // Verify co-located policy file exists
        let co_located_path = manager.get_component_policy_path(TEST_COMPONENT_ID);
        assert!(co_located_path.exists());

        // Test policy detachment
        manager.detach_policy(TEST_COMPONENT_ID).await?;

        // Verify policy is detached
        let policy_info_after = manager.get_policy_info(TEST_COMPONENT_ID).await;
        assert!(policy_info_after.is_none());

        // Verify co-located policy file is removed
        assert!(!co_located_path.exists());

        Ok(())
    }

    #[tokio::test]
    async fn test_policy_attachment_component_not_found() -> Result<()> {
        let manager = create_test_manager().await?;

        let policy_content = r#"
version: "1.0"
description: "Test policy"
permissions: {}
"#;
        let policy_path = manager.plugin_dir.join("test-policy.yaml");
        tokio::fs::write(&policy_path, policy_content).await?;

        let policy_uri = format!("file://{}", policy_path.display());

        // Test attaching policy to non-existent component
        let result = manager.attach_policy("non-existent", &policy_uri).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Component not found"));

        Ok(())
    }

    #[tokio::test]
    async fn test_grant_permission_network() -> Result<()> {
        let manager = create_test_manager().await?;
        manager.load_test_component().await?;

        // Grant network permission
        let details = serde_json::json!({"host": "api.example.com"});
        manager
            .grant_permission(TEST_COMPONENT_ID, "network", &details)
            .await?;

        // Verify policy file was created and contains the permission
        let policy_path = manager.get_component_policy_path(TEST_COMPONENT_ID);
        assert!(policy_path.exists());

        let policy_content = tokio::fs::read_to_string(&policy_path).await?;
        assert!(policy_content.contains("api.example.com"));
        assert!(policy_content.contains("network"));

        Ok(())
    }

    #[tokio::test]
    async fn test_grant_permission_storage() -> Result<()> {
        let manager = create_test_manager().await?;
        manager.load_test_component().await?;

        // Grant storage permission
        let details = serde_json::json!({"uri": "fs:///tmp/test", "access": ["read", "write"]});
        manager
            .grant_permission(TEST_COMPONENT_ID, "storage", &details)
            .await?;

        // Verify policy file was created and contains the permission
        let policy_path = manager.get_component_policy_path(TEST_COMPONENT_ID);
        assert!(policy_path.exists());

        let policy_content = tokio::fs::read_to_string(&policy_path).await?;
        assert!(policy_content.contains("fs:///tmp/test"));
        assert!(policy_content.contains("storage"));
        assert!(policy_content.contains("read"));
        assert!(policy_content.contains("write"));

        Ok(())
    }

    #[tokio::test]
    async fn test_grant_permission_duplicate_prevention() -> Result<()> {
        let manager = create_test_manager().await?;
        manager.load_test_component().await?;

        let network_details = serde_json::json!({"host": "api.example.com"});
        manager
            .grant_permission(TEST_COMPONENT_ID, "network", &network_details)
            .await?;
        manager
            .grant_permission(TEST_COMPONENT_ID, "network", &network_details)
            .await?;

        let env_details = serde_json::json!({"key": "API_KEY"});
        manager
            .grant_permission(TEST_COMPONENT_ID, "environment", &env_details)
            .await?;
        manager
            .grant_permission(TEST_COMPONENT_ID, "environment", &env_details)
            .await?;

        let storage_details = serde_json::json!({"uri": "fs:///tmp/test", "access": ["read"]});
        manager
            .grant_permission(TEST_COMPONENT_ID, "storage", &storage_details)
            .await?;
        manager
            .grant_permission(TEST_COMPONENT_ID, "storage", &storage_details)
            .await?;

        let storage_write_details =
            serde_json::json!({"uri": "fs:///tmp/test", "access": ["write"]});
        manager
            .grant_permission(TEST_COMPONENT_ID, "storage", &storage_write_details)
            .await?;

        let storage_different_uri =
            serde_json::json!({"uri": "fs:///tmp/other", "access": ["read"]});
        manager
            .grant_permission(TEST_COMPONENT_ID, "storage", &storage_different_uri)
            .await?;
        manager
            .grant_permission(TEST_COMPONENT_ID, "storage", &storage_different_uri)
            .await?;

        let policy_path = manager.get_component_policy_path(TEST_COMPONENT_ID);
        let policy_content = tokio::fs::read_to_string(&policy_path).await?;

        let network_occurrences = policy_content.matches("api.example.com").count();
        assert_eq!(
            network_occurrences, 1,
            "Network host should appear only once"
        );

        let env_occurrences = policy_content.matches("API_KEY").count();
        assert_eq!(
            env_occurrences, 1,
            "Environment key should appear only once"
        );

        let storage_test_occurrences = policy_content.matches("fs:///tmp/test").count();
        assert_eq!(
            storage_test_occurrences, 1,
            "Storage URI fs:///tmp/test should appear only once"
        );

        let storage_other_occurrences = policy_content.matches("fs:///tmp/other").count();
        assert_eq!(
            storage_other_occurrences, 1,
            "Storage URI fs:///tmp/other should appear only once"
        );

        assert!(
            policy_content.contains("read"),
            "Should contain read access"
        );
        assert!(
            policy_content.contains("write"),
            "Should contain write access"
        );

        let policy: policy::PolicyDocument = serde_yaml::from_str(&policy_content)?;

        let network_perms = policy.permissions.network.as_ref().unwrap();
        let network_allow = network_perms.allow.as_ref().unwrap();
        assert_eq!(
            network_allow.len(),
            1,
            "Should have exactly one network permission"
        );

        let env_perms = policy.permissions.environment.as_ref().unwrap();
        let env_allow = env_perms.allow.as_ref().unwrap();
        assert_eq!(
            env_allow.len(),
            1,
            "Should have exactly one environment permission"
        );

        let storage_perms = policy.permissions.storage.as_ref().unwrap();
        let storage_allow = storage_perms.allow.as_ref().unwrap();
        assert_eq!(
            storage_allow.len(),
            2,
            "Should have exactly two storage permissions"
        );

        let test_storage = storage_allow
            .iter()
            .find(|p| p.uri == "fs:///tmp/test")
            .expect("Should have storage permission for fs:///tmp/test");
        assert_eq!(
            test_storage.access.len(),
            2,
            "Should have both read and write access"
        );
        assert!(test_storage.access.contains(&policy::AccessType::Read));
        assert!(test_storage.access.contains(&policy::AccessType::Write));

        let other_storage = storage_allow
            .iter()
            .find(|p| p.uri == "fs:///tmp/other")
            .expect("Should have storage permission for fs:///tmp/other");
        assert_eq!(
            other_storage.access.len(),
            1,
            "Should have only read access"
        );
        assert!(other_storage.access.contains(&policy::AccessType::Read));

        Ok(())
    }

    #[tokio::test]
    async fn test_grant_permission_storage_access_merging() -> Result<()> {
        let manager = create_test_manager().await?;
        manager.load_test_component().await?;

        // Grant read access first
        let read_details = serde_json::json!({"uri": "fs:///tmp/test", "access": ["read"]});
        manager
            .grant_permission(TEST_COMPONENT_ID, "storage", &read_details)
            .await?;

        // Grant write access to the same URI
        let write_details = serde_json::json!({"uri": "fs:///tmp/test", "access": ["write"]});
        manager
            .grant_permission(TEST_COMPONENT_ID, "storage", &write_details)
            .await?;

        // Verify policy file contains both access types for the same URI
        let policy_path = manager.get_component_policy_path(TEST_COMPONENT_ID);
        let policy_content = tokio::fs::read_to_string(&policy_path).await?;

        // Should have both read and write access
        assert!(policy_content.contains("read"));
        assert!(policy_content.contains("write"));

        // Should only have one URI entry
        let uri_occurrences = policy_content.matches("fs:///tmp/test").count();
        assert_eq!(uri_occurrences, 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_grant_permission_component_not_found() -> Result<()> {
        let manager = create_test_manager().await?;

        // Try to grant permission to non-existent component
        let details = serde_json::json!({"host": "api.example.com"});
        let result = manager
            .grant_permission("non-existent", "network", &details)
            .await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Component not found"));

        Ok(())
    }

    #[tokio::test]
    async fn test_grant_permission_missing_required_fields() -> Result<()> {
        let manager = create_test_manager().await?;
        manager.load_test_component().await?;

        // Try to grant network permission without host field
        let details = serde_json::json!({});
        let result = manager
            .grant_permission(TEST_COMPONENT_ID, "network", &details)
            .await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing 'host' field"));

        Ok(())
    }

    #[tokio::test]
    async fn test_grant_permission_validation_empty_host() -> Result<()> {
        let manager = create_test_manager().await?;
        manager.load_test_component().await?;

        // Try to grant network permission with empty host
        let details = serde_json::json!({"host": ""});
        let result = manager
            .grant_permission(TEST_COMPONENT_ID, "network", &details)
            .await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Network host cannot be empty"));

        Ok(())
    }

    #[tokio::test]
    async fn test_grant_permission_multiple_permissions() -> Result<()> {
        let manager = create_test_manager().await?;
        manager.load_test_component().await?;

        // Grant multiple different permissions
        let network_details = serde_json::json!({"host": "api.example.com"});
        manager
            .grant_permission(TEST_COMPONENT_ID, "network", &network_details)
            .await?;

        let storage_details = serde_json::json!({"uri": "fs:///tmp/test", "access": ["read"]});
        manager
            .grant_permission(TEST_COMPONENT_ID, "storage", &storage_details)
            .await?;

        // Verify policy file contains all permissions
        let policy_path = manager.get_component_policy_path(TEST_COMPONENT_ID);
        let policy_content = tokio::fs::read_to_string(&policy_path).await?;

        assert!(policy_content.contains("api.example.com"));
        assert!(policy_content.contains("fs:///tmp/test"));
        assert!(policy_content.contains("network"));
        assert!(policy_content.contains("storage"));

        Ok(())
    }

    #[tokio::test]
    async fn test_grant_permission_updates_policy_registry() -> Result<()> {
        let manager = create_test_manager().await?;
        manager.load_test_component().await?;

        // Grant permission
        let details = serde_json::json!({"host": "api.example.com"});
        manager
            .grant_permission(TEST_COMPONENT_ID, "network", &details)
            .await?;

        // Verify policy registry was updated by attempting to get WASI state
        let _wasi_state = manager
            .get_wasi_state_for_component(TEST_COMPONENT_ID)
            .await?;

        // If we get here without error, the policy registry was updated successfully
        Ok(())
    }

    #[tokio::test]
    async fn test_grant_permission_to_existing_policy() -> Result<()> {
        let manager = create_test_manager().await?;
        manager.load_test_component().await?;

        // First, attach a policy using the existing system
        let policy_content = r#"
version: "1.0"
description: "Initial policy"
permissions:
  network:
    allow:
      - host: "initial.example.com"
"#;
        let policy_path = manager.plugin_dir.join("initial-policy.yaml");
        tokio::fs::write(&policy_path, policy_content).await?;

        let policy_uri = format!("file://{}", policy_path.display());
        manager
            .attach_policy(TEST_COMPONENT_ID, &policy_uri)
            .await?;

        // Now grant additional permission using granular system
        let details = serde_json::json!({"host": "additional.example.com"});
        manager
            .grant_permission(TEST_COMPONENT_ID, "network", &details)
            .await?;

        // Verify both permissions exist in the policy file
        let co_located_path = manager.get_component_policy_path(TEST_COMPONENT_ID);
        let final_policy_content = tokio::fs::read_to_string(&co_located_path).await?;

        assert!(final_policy_content.contains("initial.example.com"));
        assert!(final_policy_content.contains("additional.example.com"));

        Ok(())
    }

    #[test]
    fn test_permission_rule_serialization() -> Result<()> {
        // Test serialization of PermissionRule
        let network_rule =
            PermissionRule::Network(NetworkPermission::Host(NetworkHostPermission {
                host: "example.com".to_string(),
            }));
        let serialized = serde_json::to_string(&network_rule)?;
        assert!(serialized.contains("example.com"));

        let storage_rule = PermissionRule::Storage(StoragePermission {
            uri: "fs:///tmp/test".to_string(),
            access: vec![AccessType::Read, AccessType::Write],
        });
        let serialized = serde_json::to_string(&storage_rule)?;
        assert!(serialized.contains("fs:///tmp/test"));
        assert!(serialized.contains("read"));
        assert!(serialized.contains("write"));

        Ok(())
    }

    #[test]
    fn test_permission_type_enum() -> Result<()> {
        // Test that PermissionRule properly wraps different permission types
        let network_perm =
            PermissionRule::Network(NetworkPermission::Host(NetworkHostPermission {
                host: "example.com".to_string(),
            }));
        let storage_perm = PermissionRule::Storage(StoragePermission {
            uri: "fs:///tmp".to_string(),
            access: vec![AccessType::Read, AccessType::Write],
        });
        let env_perm = PermissionRule::Environment(EnvironmentPermission {
            key: "API_KEY".to_string(),
        });
        let custom_perm = PermissionRule::Custom(
            "custom-type".to_string(),
            serde_json::json!({"custom": "data"}),
        );

        // Test serialization/deserialization
        let network_rule = network_perm;
        let serialized = serde_json::to_string(&network_rule)?;
        let _deserialized: PermissionRule = serde_json::from_str(&serialized)?;

        // Test that the variants can be created and used
        assert!(matches!(storage_perm, PermissionRule::Storage(_)));
        assert!(matches!(env_perm, PermissionRule::Environment(_)));
        assert!(matches!(custom_perm, PermissionRule::Custom(_, _)));

        // Test pattern matching works correctly
        let rule = PermissionRule::Network(NetworkPermission::Host(NetworkHostPermission {
            host: "test.com".to_string(),
        }));
        match rule {
            PermissionRule::Network(NetworkPermission::Host(NetworkHostPermission { host })) => {
                assert_eq!(host, "test.com");
            }
            _ => panic!("Expected network permission"),
        }

        Ok(())
    }

    #[test]
    fn test_access_type_serialization() -> Result<()> {
        // Test serialization of AccessType
        let read_access = AccessType::Read;
        let serialized = serde_json::to_string(&read_access)?;
        assert_eq!(serialized, "\"read\"");

        let write_access = AccessType::Write;
        let serialized = serde_json::to_string(&write_access)?;
        assert_eq!(serialized, "\"write\"");

        Ok(())
    }
}
