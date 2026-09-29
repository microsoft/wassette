// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Prepare receipt-bound runtime policy before committing its authoritative bytes.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use super::{PolicyInfo, PolicyManager};
use crate::loader::{self, PolicyResource};
use crate::store::{
    ArtifactSnapshot, CommitOutcome, PolicyMetadata, PolicyProvenance, PreparedPolicy,
};
use crate::store_support::store_operation;
use crate::{SecretBinding, WasiStateTemplate};

pub(crate) struct PolicyCommit {
    pub(crate) outcome: CommitOutcome,
    pub(crate) template: Arc<WasiStateTemplate>,
    pub(crate) effective_policy: Option<Vec<u8>>,
}

impl PolicyManager {
    pub(crate) async fn policy_snapshot(&self, id: &str) -> Result<ArtifactSnapshot> {
        let id = id.to_owned();
        store_operation(&self.store, move |store| {
            store
                .read(&id)
                .with_context(|| format!("Component not found or unreadable: {id}"))
        })
        .await
    }

    pub(crate) async fn prepare_bound_template(
        &self,
        binding: &SecretBinding,
        bytes: Option<&[u8]>,
    ) -> Result<Arc<WasiStateTemplate>> {
        let secrets = self.secrets.load_bound_component_secrets(binding).await?;
        let template = match bytes {
            Some(bytes) => {
                let policy = policy::PolicyParser::parse_bytes(bytes)?;
                crate::create_wasi_state_template_from_policy(
                    &policy,
                    self.storage.root(),
                    self.environment_vars.as_ref(),
                    Some(&secrets),
                )?
            }
            None => {
                let mut config_vars = self.environment_vars.as_ref().clone();
                config_vars.extend(secrets);
                WasiStateTemplate {
                    config_vars,
                    ..WasiStateTemplate::default()
                }
            }
        };
        Ok(Arc::new(template))
    }

    pub(crate) async fn attach_transactional(&self, id: &str, uri: &str) -> Result<PolicyCommit> {
        let snapshot = self.policy_snapshot(id).await?;
        let resource =
            loader::load_resource::<PolicyResource>(uri, &self.oci_client, &self.http_client)
                .await?;
        let bytes = tokio::fs::read(resource.as_ref())
            .await
            .context("capturing attached policy")?;
        let policy = explicit_policy(bytes, uri)?;
        self.commit_policy(snapshot, policy).await
    }

    pub(crate) async fn clear_transactional(&self, id: &str) -> Result<PolicyCommit> {
        let snapshot = self.policy_snapshot(id).await?;
        // An operator clear protects absence against later bundled defaults.
        self.commit_policy(
            snapshot,
            PreparedPolicy::absent(PolicyProvenance::ExplicitAttachment),
        )
        .await
    }

    pub(crate) async fn edit_permission_transactional(
        &self,
        id: &str,
        permission_type: &str,
        details: &serde_json::Value,
        grant: bool,
    ) -> Result<PolicyCommit> {
        let snapshot = self.policy_snapshot(id).await?;
        let rule = self.parse_permission_rule(permission_type, details)?;
        self.validate_permission_rule(&rule)?;
        let mut policy = policy_document(&snapshot)?;
        if grant {
            self.add_permission_rule_to_policy(&mut policy, rule)?;
        } else {
            self.remove_permission_rule_from_policy(&mut policy, rule)?;
        }
        self.commit_permission_edit(snapshot, policy).await
    }

    pub(crate) async fn revoke_storage_transactional(
        &self,
        id: &str,
        uri: &str,
    ) -> Result<PolicyCommit> {
        anyhow::ensure!(!uri.is_empty(), "Storage URI cannot be empty");
        let snapshot = self.policy_snapshot(id).await?;
        let mut policy = policy_document(&snapshot)?;
        self.remove_storage_permission_by_uri_from_policy(&mut policy, uri)?;
        self.commit_permission_edit(snapshot, policy).await
    }

    async fn commit_permission_edit(
        &self,
        snapshot: ArtifactSnapshot,
        document: policy::PolicyDocument,
    ) -> Result<PolicyCommit> {
        // Editing permissions does not rewrite the attachment's source or time.
        let policy = PreparedPolicy::parse(
            serde_yaml::to_string(&document)?.into_bytes(),
            PolicyProvenance::PermissionEdit,
        )?
        .with_metadata(snapshot.receipt.policy.metadata.clone())?;
        self.commit_policy(snapshot, policy).await
    }

    async fn commit_policy(
        &self,
        snapshot: ArtifactSnapshot,
        policy: PreparedPolicy,
    ) -> Result<PolicyCommit> {
        let template = self
            .prepare_bound_template(&snapshot.receipt.secret_binding()?, policy.bytes())
            .await?;
        let id = snapshot.receipt.component_id.as_str().to_owned();
        let revision = snapshot.receipt.revision;
        let effective_policy = policy.bytes().map(<[u8]>::to_vec);
        let outcome = store_operation(&self.store, move |store| {
            Ok(store.update_policy(&id, &revision, policy)?)
        })
        .await?;
        Ok(PolicyCommit {
            outcome,
            template,
            effective_policy,
        })
    }

    pub(crate) async fn policy_info_transactional(&self, id: &str) -> Result<Option<PolicyInfo>> {
        let snapshot = self.policy_snapshot(id).await?;
        if snapshot.policy.is_none() {
            return Ok(None);
        }
        let receipt = snapshot.receipt;
        let path = self.storage.policy_path(&receipt.storage_key);
        let metadata = receipt.policy.metadata;
        let source_uri = metadata
            .as_ref()
            .map(|metadata| metadata.source_uri.clone())
            .unwrap_or_else(|| format!("file://{}", path.display()));
        let created_at = metadata
            .and_then(|metadata| metadata.attached_at)
            .and_then(|seconds| UNIX_EPOCH.checked_add(Duration::from_secs(seconds)))
            .unwrap_or(UNIX_EPOCH);
        Ok(Some(PolicyInfo {
            policy_id: format!("{id}-policy"),
            source_uri,
            local_path: path,
            component_id: id.to_owned(),
            created_at,
        }))
    }
}

pub(crate) fn explicit_policy(bytes: Vec<u8>, source: &str) -> Result<PreparedPolicy> {
    let source_uri = if source.starts_with("https://") {
        let mut url = url::Url::parse(source).context("invalid policy source URL")?;
        anyhow::ensure!(
            url.username().is_empty() && url.password().is_none(),
            "policy URLs must not include user information"
        );
        url.set_query(None);
        url.set_fragment(None);
        url.to_string()
    } else {
        source.to_owned()
    };
    let attached_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock precedes the Unix epoch")?
        .as_secs();
    Ok(
        PreparedPolicy::parse(bytes, PolicyProvenance::ExplicitAttachment)?.with_metadata(Some(
            PolicyMetadata {
                source_uri,
                attached_at: Some(attached_at),
            },
        ))?,
    )
}

fn policy_document(snapshot: &ArtifactSnapshot) -> Result<policy::PolicyDocument> {
    match snapshot.policy.as_deref() {
        Some(bytes) => Ok(policy::PolicyParser::parse_bytes(bytes)?),
        None => Ok(policy::PolicyDocument {
            version: "1.0".to_owned(),
            description: Some(format!(
                "Auto-generated policy for component: {}",
                snapshot.receipt.component_id.as_str()
            )),
            permissions: Default::default(),
        }),
    }
}
