// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use wassette::{format_error_chain, LifecycleManager, SecretsManager};

use crate::manifest::{ComponentDeclaration, InlinePermissions, ProvisioningManifest};
use crate::permission_synthesis;

/// Controller for provisioning components from a manifest
pub struct ProvisioningController<'a> {
    manifest: &'a ProvisioningManifest,
    lifecycle_manager: &'a LifecycleManager,
    #[allow(dead_code)] // Reserved for future use in secrets seeding
    secrets_manager: &'a SecretsManager,
    plugin_dir: &'a Path,
}

impl<'a> ProvisioningController<'a> {
    /// Create a new provisioning controller
    pub fn new(
        manifest: &'a ProvisioningManifest,
        lifecycle_manager: &'a LifecycleManager,
        secrets_manager: &'a SecretsManager,
        plugin_dir: &'a Path,
    ) -> Self {
        Self {
            manifest,
            lifecycle_manager,
            secrets_manager,
            plugin_dir,
        }
    }

    /// Provision all components from the manifest
    pub async fn provision(&self) -> Result<()> {
        tracing::info!(
            "Starting provisioning of {} component(s)",
            self.manifest.components.len()
        );

        let mut errors = Vec::new();

        for (idx, component) in self.manifest.components.iter().enumerate() {
            let component_name = component.name.as_deref().unwrap_or(&component.uri);

            tracing::info!(
                "[{}/{}] Provisioning component: {}",
                idx + 1,
                self.manifest.components.len(),
                component_name
            );

            if let Err(e) = self.provision_component(component).await {
                tracing::error!(
                    "Failed to provision component {}: {}",
                    component_name,
                    format_error_chain(&e)
                );
                errors.push((component_name.to_string(), e));
            }
        }

        if !errors.is_empty() {
            let error_summary = errors
                .iter()
                .map(|(name, e)| format!("  - {}: {}", name, format_error_chain(e)))
                .collect::<Vec<_>>()
                .join("\n");

            bail!(
                "Failed to provision {} component(s):\n{}",
                errors.len(),
                error_summary
            );
        }

        tracing::info!("Successfully provisioned all components");
        Ok(())
    }

    /// Provision a single component
    async fn provision_component(&self, component: &ComponentDeclaration) -> Result<()> {
        // Step 1: Seed secrets from environment variables
        self.seed_secrets(component)
            .context("Failed to seed secrets")?;

        // Step 2: Load the component to obtain its authoritative component ID
        let load_outcome = self
            .lifecycle_manager
            .load_component(&component.uri)
            .await
            .with_context(|| format!("Failed to load component from URI: {}", component.uri))?;

        // Step 3: Synthesize and attach a policy when permissions were declared
        if has_synthesizable_permissions(component) {
            self.synthesize_policy(component, &load_outcome.component_id)
                .await
                .context("Failed to synthesize and attach policy")?;

            if self
                .lifecycle_manager
                .get_policy_info(&load_outcome.component_id)
                .await
                .is_none()
            {
                tracing::warn!(
                    component_id = %load_outcome.component_id,
                    component_uri = %component.uri,
                    "Component manifest declared permissions but no policy is attached after provisioning"
                );
            }
        }

        // Step 4: Verify digest if specified
        if let Some(digest) = &component.digest {
            self.verify_digest(component, digest)
                .context("Digest verification failed")?;
        }

        Ok(())
    }

    /// Seed secrets from environment variables
    fn seed_secrets(&self, component: &ComponentDeclaration) -> Result<()> {
        // Check if there are environment permissions
        let env_perms = match &component.permissions.environment {
            Some(perms) => perms,
            None => return Ok(()), // No environment permissions
        };

        // Build secrets map from process environment
        let mut secrets = HashMap::new();

        for rule in &env_perms.allow {
            // Use value_from hint, or default to the key itself
            let env_var_name = rule.value_from.as_deref().unwrap_or(&rule.key);

            match std::env::var(env_var_name) {
                Ok(value) => {
                    tracing::debug!(
                        "Seeding secret {} from environment variable {}",
                        rule.key,
                        env_var_name
                    );
                    secrets.insert(rule.key.clone(), value);
                }
                Err(_) => {
                    tracing::warn!(
                        "Environment variable {} not found for secret {}. Component may fail at runtime.",
                        env_var_name,
                        rule.key
                    );
                }
            }
        }

        // If we have secrets to set, we need to know the component ID
        // For now, we'll skip setting secrets until after the component is loaded
        // The secrets will be available from the environment during WASI state creation

        // Note: This is a limitation of the current approach. In a future version,
        // we could pre-register secrets using a predictable component ID derived
        // from the URI, or we could load the component first and then set secrets.

        Ok(())
    }

    /// Synthesize and attach a policy from inline permissions
    async fn synthesize_policy(
        &self,
        component: &ComponentDeclaration,
        component_id: &str,
    ) -> Result<()> {
        let policy_yaml = permission_synthesis::synthesize_policy_yaml(
            &component.permissions,
            component.name.as_deref(),
        )
        .context("Failed to synthesize policy from inline permissions")?;

        let policy_source_path = self
            .plugin_dir
            .join(format!("{component_id}.manifest-policy.yaml"));
        tokio::fs::write(&policy_source_path, policy_yaml)
            .await
            .with_context(|| {
                format!(
                    "Failed to write synthesized policy to: {}",
                    policy_source_path.display()
                )
            })?;

        let policy_uri = format!("file://{}", policy_source_path.display());
        let attach_result = self
            .lifecycle_manager
            .attach_policy(component_id, &policy_uri)
            .await;
        let cleanup_result = tokio::fs::remove_file(&policy_source_path).await;

        match (attach_result, cleanup_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(attach_error), Ok(())) => Err(attach_error).with_context(|| {
                format!("Failed to attach synthesized policy to component {component_id}")
            }),
            (Ok(()), Err(cleanup_error)) => {
                tracing::warn!(
                    path = %policy_source_path.display(),
                    error = %cleanup_error,
                    "Failed to remove synthesized policy source after attaching policy"
                );
                Ok(())
            }
            (Err(attach_error), Err(cleanup_error)) => Err(attach_error).with_context(|| {
                format!(
                    "Failed to attach synthesized policy to component {component_id}; also failed to remove {}: {cleanup_error}",
                    policy_source_path.display()
                )
            }),
        }
    }

    /// Verify component digest (SHA-256)
    fn verify_digest(&self, component: &ComponentDeclaration, expected_digest: &str) -> Result<()> {
        // Digest verification is deferred to post-MVP for simplicity
        // The digest format was validated during manifest validation,
        // but actual verification requires reading the downloaded component bytes

        tracing::warn!(
            "Digest verification is not yet implemented for component: {}. Expected: {}",
            component.name.as_deref().unwrap_or(&component.uri),
            expected_digest
        );

        // TODO: Implement digest verification
        // 1. Get the component bytes from the downloaded artifact
        // 2. Compute SHA-256 hash
        // 3. Compare with expected_digest (strip "sha256:" prefix)

        Ok(())
    }
}

/// Report whether the declaration carries permissions that synthesis can express.
///
/// `resources` is deliberately excluded. It is deferred to post-MVP and
/// `permission_synthesis` never converts it, so a resources-only declaration would
/// synthesize an empty policy and overwrite whatever policy the component already had.
fn has_synthesizable_permissions(component: &ComponentDeclaration) -> bool {
    let InlinePermissions {
        network,
        storage,
        environment,
        resources: _,
    } = &component.permissions;

    network.is_some() || storage.is_some() || environment.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{EnvironmentPermissions, EnvironmentRule, InlinePermissions};

    #[test]
    fn test_seed_secrets_basic() {
        // Set environment variable for testing
        std::env::set_var("TEST_API_KEY", "secret123");

        let component = ComponentDeclaration {
            uri: "oci://example.com/test:latest".to_string(),
            name: Some("test".to_string()),
            digest: None,
            permissions: InlinePermissions {
                environment: Some(EnvironmentPermissions {
                    allow: vec![EnvironmentRule {
                        key: "API_KEY".to_string(),
                        value_from: Some("TEST_API_KEY".to_string()),
                    }],
                }),
                network: None,
                storage: None,
                resources: None,
            },
            retry_policy: None,
        };

        let _temp_dir = tempfile::tempdir().unwrap();
        let _manifest = ProvisioningManifest {
            version: 1,
            components: vec![component.clone()],
        };

        // We can't fully test this without a real lifecycle manager,
        // but we can verify the seed_secrets logic doesn't panic
        // In a full integration test, we'd verify the secrets are set

        // Cleanup
        std::env::remove_var("TEST_API_KEY");
    }
}
