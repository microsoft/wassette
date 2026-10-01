// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::hash::{Hash, Hasher};

use super::*;
use crate::acquisition::AcquiredComponent;
use crate::policy_internal::PolicyCommit;
use crate::store::{
    ArtifactSnapshot, CommitOutcome, ExpectedEntry, InstallIntent, InstallOptions, InstallOwner,
    PolicyProvenance, PreparedCache, PreparedInstall, PreparedPolicy, StoredArtifactKind,
    StoredEntry, ValidationEvidence,
};
use crate::store_support::{source_binding_key, store_operation};
use crate::wasm_directory::{self, PackageSelector, ResolvedPackage, WasmDirectoryClient};

pub(crate) const CACHE_SCHEMA: &str = "wassette-tools-v1";

struct PreparedAcquiredInstall {
    expected: ExpectedEntry,
    guard: tokio::sync::OwnedMutexGuard<()>,
    prepared: PreparedComponentLoad,
    install: PreparedInstall,
}

pub(crate) enum PolicyMutation {
    Attach(String),
    Clear,
    Permission {
        permission_type: String,
        details: Value,
        grant: bool,
    },
    RevokeStorage(String),
}

impl LifecycleManager {
    pub(crate) async fn mutate_policy(&self, id: &str, mutation: PolicyMutation) -> Result<()> {
        let manager = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let _guard = manager.load_guard(&id).await.lock_owned().await;
            let cached = manager.policy_cache(&id).await?;
            let commit = match mutation {
                PolicyMutation::Attach(uri) => {
                    manager
                        .policy_manager
                        .attach_transactional(&id, &uri)
                        .await?
                }
                PolicyMutation::Clear => manager.policy_manager.clear_transactional(&id).await?,
                PolicyMutation::Permission {
                    permission_type,
                    details,
                    grant,
                } => {
                    manager
                        .policy_manager
                        .edit_permission_transactional(&id, &permission_type, &details, grant)
                        .await?
                }
                PolicyMutation::RevokeStorage(uri) => {
                    manager
                        .policy_manager
                        .revoke_storage_transactional(&id, &uri)
                        .await?
                }
            };
            if let Some(cache) = cached {
                if let StoredEntry::Installed(receipt) = &commit.outcome.entry {
                    if receipt.artifact_sha256 == cache.artifact_sha256 {
                        let revision = receipt.revision.clone();
                        let cache_id = id.clone();
                        if let Err(error) = store_operation(&manager.store, move |store| {
                            Ok(store.publish_cache(&cache_id, &revision, cache)?)
                        })
                        .await
                        {
                            warn!(component_id = %id, error = %format_error_chain(&error),
                                "Policy committed, but derived cache rebinding failed");
                        }
                    }
                }
            }
            let publication = manager.publish_policy_commit(commit).await;
            drop(_guard);
            let refresh = manager
                .refresh_from_store()
                .await
                .context("Policy committed, but catalog reconciliation failed");
            publication?;
            refresh?;
            Ok(())
        })
        .await
        .context("Policy mutation worker failed")?
    }

    async fn policy_cache(&self, id: &str) -> Result<Option<PreparedCache>> {
        let snapshot = self.store_snapshot(id).await?;
        if snapshot.receipt.kind != StoredArtifactKind::Tool {
            return Ok(None);
        }
        let id = id.to_owned();
        let engine = self.cache_engine();
        store_operation(&self.store, move |store| {
            Ok(store
                .read_cache(&id, &snapshot.receipt.revision, &engine, CACHE_SCHEMA)?
                .map(|cache| PreparedCache {
                    artifact_sha256: snapshot.receipt.artifact_sha256,
                    engine,
                    schema: CACHE_SCHEMA.to_owned(),
                    metadata: cache.metadata,
                    native: cache.native,
                }))
        })
        .await
    }

    pub(crate) async fn publish_removal(&self, outcome: CommitOutcome) -> Result<()> {
        let id = outcome.entry.component_id().as_str().to_owned();
        self.unregister_at_cursor(&id, &outcome.cursor)
            .await
            .context("Uninstall committed to disk, but runtime publication requires a retry")
    }

    pub(crate) async fn unregister_at_cursor(
        &self,
        id: &str,
        cursor: &store::StoreCursor,
    ) -> Result<()> {
        for _ in 0..crate::tool_catalog::PUBLICATION_ATTEMPTS {
            let registry = self.registry.clone();
            let cursor = cursor.clone();
            let id = id.to_owned();
            let published = store_operation(&self.store, move |store| {
                let _scope = store.checked_read(&cursor, None)?;
                let Ok(mut state) = registry.state.try_write() else {
                    return Ok(false);
                };
                state.unregister_component(&id);
                Ok(true)
            })
            .await
            .context("Store state changed before runtime removal; retry")?;
            if published {
                return Ok(());
            }
            tokio::task::yield_now().await;
        }
        bail!("Runtime registry remained busy while publishing removal; retry")
    }

    /// The shared persistent store, independent of this manager's runtime registry.
    pub fn component_store(&self) -> &store::ComponentStore {
        &self.store
    }

    /// Validate and install an artifact and explicitly selected policy together.
    ///
    /// Neither the artifact nor its tools are published if policy preparation
    /// fails. `source` records the non-secret policy origin, such as a manifest.
    pub async fn load_component_with_policy(
        &self,
        uri: &str,
        yaml: &str,
        source: &str,
    ) -> Result<ComponentLoadOutcome> {
        let policy = policy_internal::explicit_policy(yaml.as_bytes().to_vec(), source)?;
        let acquired = acquisition::acquire_component(uri, &self.config, false).await?;
        self.install_acquired(acquired, Some(policy)).await
    }

    pub(crate) async fn install_acquired(
        &self,
        acquired: AcquiredComponent,
        explicit_policy: Option<PreparedPolicy>,
    ) -> Result<ComponentLoadOutcome> {
        let PreparedAcquiredInstall {
            expected,
            guard,
            prepared,
            install,
        } = self
            .prepare_acquired_install(acquired, explicit_policy, InstallIntent::ExposeTools)
            .await?;
        let manager = self.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let outcome = store_operation(&manager.store, move |store| {
                Ok(store.commit_install(install, expected)?)
            })
            .await?;
            let publication = manager.publish_prepared(prepared, outcome).await;
            drop(_guard);
            let refresh = manager
                .refresh_from_store()
                .await
                .context("Component installed, but catalog reconciliation failed");
            let outcome = publication?;
            refresh?;
            Ok(outcome)
        })
        .await
        .context("Component installation worker failed")?
    }

    /// Resolve and install a wasm.directory package without exposing its tools.
    ///
    /// Package search metadata is not trusted for identity or artifact
    /// classification. The selected version is pulled by its manifest digest
    /// through this lifecycle's configured OCI client, then inspected,
    /// runtime-validated, and committed with `InstallOnly` intent.
    pub async fn install_package(
        &self,
        directory: &WasmDirectoryClient,
        selector: &PackageSelector,
        requested_version: Option<&str>,
    ) -> Result<(ResolvedPackage, CommitOutcome)> {
        let (resolved, acquired) =
            wasm_directory::acquire_package(directory, selector, requested_version, &self.config)
                .await?;
        let PreparedAcquiredInstall {
            expected,
            guard,
            install,
            ..
        } = self
            .prepare_acquired_install(acquired, None, InstallIntent::InstallOnly)
            .await?;
        let manager = self.clone();
        let outcome = tokio::spawn(async move {
            let _guard = guard;
            let outcome = store_operation(&manager.store, move |store| {
                Ok(store.commit_install(install, expected)?)
            })
            .await?;
            drop(_guard);
            manager
                .refresh_from_store()
                .await
                .context("Package installed, but catalog reconciliation failed")?;
            Ok::<_, anyhow::Error>(outcome)
        })
        .await
        .context("Package installation worker failed")??;
        Ok((resolved, outcome))
    }

    /// Resolve and load a wasm.directory package after an explicit load request.
    ///
    /// Unlike [`Self::install_package`], this operation deliberately requests
    /// tool exposure. Installation alone never registers package tools.
    pub async fn load_package(
        &self,
        directory: &WasmDirectoryClient,
        selector: &PackageSelector,
        requested_version: Option<&str>,
    ) -> Result<(ResolvedPackage, ComponentLoadOutcome)> {
        let (resolved, acquired) =
            wasm_directory::acquire_package(directory, selector, requested_version, &self.config)
                .await?;
        let outcome = self.install_acquired(acquired, None).await?;
        Ok((resolved, outcome))
    }

    async fn prepare_acquired_install(
        &self,
        acquired: AcquiredComponent,
        explicit_policy: Option<PreparedPolicy>,
        intent: InstallIntent,
    ) -> Result<PreparedAcquiredInstall> {
        let inspection = inspect_artifact(&acquired.wasm)?;
        let id = inspection.identity?.as_str().to_owned();
        anyhow::ensure!(
            inspection.shape == ArtifactShape::ToolCandidate,
            "Cannot load ACP or unsupported artifacts as ordinary tool components"
        );
        let guard = self.load_guard(&id).await.lock_owned().await;
        let key = acquired.storage_key.clone();
        let source = acquired.source.clone();
        let observed_id = id.clone();
        let expected = store_operation(&self.store, move |store| {
            Ok(store.observe(&observed_id, &key, &source)?)
        })
        .await?;
        let selected = if let Some(explicit) = explicit_policy {
            explicit
        } else if let Some(StoredEntry::Installed(receipt)) = expected.entry() {
            let snapshot = self.store_snapshot(&id).await?;
            anyhow::ensure!(
                snapshot.receipt.revision == receipt.revision,
                "Component changed while selecting effective policy; retry the installation"
            );
            if matches!(
                receipt.policy.provenance,
                PolicyProvenance::ExplicitAttachment
                    | PolicyProvenance::PermissionEdit
                    | PolicyProvenance::Legacy
            ) || acquired.policy.is_none()
            {
                policy_from_snapshot(&snapshot)?
            } else {
                incoming_policy(acquired.policy.clone())?
            }
        } else if let Some(StoredEntry::Retired(retired)) = expected.entry() {
            let previous = &retired.previous.policy;
            if matches!(
                previous.provenance,
                PolicyProvenance::ExplicitAttachment
                    | PolicyProvenance::PermissionEdit
                    | PolicyProvenance::Legacy
            ) {
                let candidate = match acquired.policy.clone() {
                    Some(bytes) => PreparedPolicy::parse(bytes, previous.provenance.clone())?,
                    None => PreparedPolicy::absent(previous.provenance.clone()),
                }
                .with_metadata(previous.metadata.clone())?;
                anyhow::ensure!(
                    candidate.evidence() == previous,
                    "Reinstalling this retired component requires its previous explicit policy; \
                     supply an explicit policy rather than inheriting different grants"
                );
                candidate
            } else {
                incoming_policy(acquired.policy.clone())?
            }
        } else {
            incoming_policy(acquired.policy.clone())?
        };
        let binding = SecretBinding::new(
            &inspect_artifact(&acquired.wasm)?.identity?,
            &acquired.storage_key,
            source_binding_key(&acquired.source)?,
        )?;
        self.secrets_manager.check_binding(&binding).await?;
        let prepared = self
            .prepare_component_load(
                CapturedComponent {
                    storage_key: acquired.storage_key.clone(),
                    wasm: acquired.wasm,
                    bundled_policy: acquired.policy,
                },
                &binding,
                selected.bytes().map(<[u8]>::to_vec),
            )
            .await?;
        let runtime = self.cache_engine();
        let install = PreparedInstall::prepare(
            prepared.captured.wasm.clone(),
            InstallOptions {
                storage_key: acquired.storage_key,
                source: acquired.source,
                origin: acquired.origin,
                owner: InstallOwner::Explicit,
                intent,
                policy: selected,
                observation: None,
            },
            |bytes, _, policy| {
                anyhow::ensure!(
                    bytes == prepared.captured.wasm
                        && policy == prepared.effective_policy.as_deref(),
                    "Prepared runtime inputs differ from the proposed installation"
                );
                Ok(ValidationEvidence::OrdinaryPrepared { runtime })
            },
        )?;
        Ok(PreparedAcquiredInstall {
            expected,
            guard,
            prepared,
            install,
        })
    }

    pub(crate) async fn store_snapshot(&self, id: &str) -> Result<ArtifactSnapshot> {
        let id = id.to_owned();
        store_operation(&self.store, move |store| {
            store
                .read(&id)
                .with_context(|| format!("Component not found or unreadable: {id}"))
        })
        .await
    }

    pub(crate) async fn known_secret_binding(&self, id: &str) -> Result<SecretBinding> {
        let id = id.to_owned();
        store_operation(&self.store, move |store| {
            let snapshot = store
                .snapshot_if_changed(None)?
                .context("missing store snapshot")?;
            let entry = snapshot
                .entries
                .iter()
                .find(|entry| entry.component_id().as_str() == id)
                .with_context(|| format!("Component not found: {id}"))?;
            entry.binding().secret_binding()
        })
        .await
    }

    #[cfg(test)]
    pub(crate) async fn read_cached_metadata(&self, id: &str) -> Result<Option<ComponentMetadata>> {
        let snapshot = self.store_snapshot(id).await?;
        if !snapshot.receipt.requests_tool_exposure() {
            return Ok(None);
        }
        let id = id.to_owned();
        let expected_id = id.clone();
        let engine = self.cache_engine();
        let hash = snapshot.receipt.artifact_sha256.clone();
        let cache = store_operation(&self.store, move |store| {
            Ok(store.read_cache(&id, &snapshot.receipt.revision, &engine, CACHE_SCHEMA)?)
        })
        .await?;
        let Some(cache) = cache else {
            return Ok(None);
        };
        let metadata: ComponentMetadata = serde_json::from_value(cache.metadata)?;
        if metadata.component_id != expected_id
            || metadata.validation_stamp.content_hash.as_deref() != Some(hash.as_str())
            || metadata.function_identifiers.len() != metadata.tool_schemas.len()
            || metadata.function_identifiers.len() != metadata.tool_names.len()
            || metadata.function_identifiers.iter().any(is_acp_identifier)
        {
            bail!("Derived tool metadata does not match its component receipt");
        }
        Ok(Some(metadata))
    }

    pub(crate) async fn publish_prepared(
        &self,
        mut prepared: PreparedComponentLoad,
        outcome: CommitOutcome,
    ) -> Result<ComponentLoadOutcome> {
        let StoredEntry::Installed(receipt) = &outcome.entry else {
            bail!("Cannot publish a retired component");
        };
        let id = receipt.component_id.as_str().to_owned();
        prepared.instance.revision = Some(receipt.revision.clone());
        prepared.instance.artifact_sha256 = receipt.artifact_sha256.clone();
        prepared.instance.policy_template = prepared.policy_template.clone();
        let metadata = self.metadata_for(&id, &prepared.tools, &prepared.captured.wasm)?;
        match prepared.instance.component.serialize() {
            Ok(native) => {
                let cache = PreparedCache {
                    artifact_sha256: receipt.artifact_sha256.clone(),
                    engine: self.cache_engine(),
                    schema: CACHE_SCHEMA.to_owned(),
                    metadata: serde_json::to_value(metadata)?,
                    native,
                };
                let cache_id = id.clone();
                let revision = receipt.revision.clone();
                if let Err(error) = store_operation(&self.store, move |store| {
                    Ok(store.publish_cache(&cache_id, &revision, cache)?)
                })
                .await
                {
                    warn!(component_id = %id, error = %format_error_chain(&error), "Could not publish derived component cache");
                }
            }
            Err(error) => {
                warn!(component_id = %id, %error, "Could not serialize derived component cache")
            }
        }
        let tools = prepared.tools;
        let tool_names = tools
            .iter()
            .map(|tool| tool.normalized_name.clone())
            .collect();
        let mut instance = Some(prepared.instance);
        let mut tools = Some(tools);
        let revision = receipt.revision.clone();
        for _ in 0..crate::tool_catalog::PUBLICATION_ATTEMPTS {
            let registry = self.registry.clone();
            let cursor = outcome.cursor.clone();
            let revision = revision.clone();
            let selected_id = id.clone();
            let owned_instance = instance
                .take()
                .expect("prepared instance retained on contention");
            let owned_tools = tools.take().expect("prepared tools retained on contention");
            let published = store_operation(&self.store, move |store| {
                let _scope = store.checked_read(&cursor, Some((&selected_id, &revision)))?;
                let Ok(mut state) = registry.state.try_write() else {
                    return Ok(Err((owned_instance, owned_tools)));
                };
                Ok(Ok(state.upsert_component(
                    selected_id,
                    owned_instance,
                    owned_tools,
                )?))
            })
            .await
            .context("Component committed to disk, but runtime publication requires a retry")?;
            match published {
                Ok(status) => {
                    return Ok(ComponentLoadOutcome {
                        component_id: id,
                        status,
                        tool_names,
                        commit: outcome,
                    });
                }
                Err((retained_instance, retained_tools)) => {
                    instance = Some(retained_instance);
                    tools = Some(retained_tools);
                    tokio::task::yield_now().await;
                }
            }
        }
        bail!("Component committed, but runtime registry remained busy; retry")
    }

    pub(crate) async fn restore_component_locked(&self, id: &str) -> Result<ComponentLoadOutcome> {
        let snapshot = self.store_snapshot(id).await?;
        anyhow::ensure!(
            snapshot.receipt.kind == StoredArtifactKind::Tool,
            "Cannot load ACP or unsupported artifacts as ordinary tool components"
        );
        anyhow::ensure!(
            snapshot.receipt.requests_tool_exposure(),
            "Component '{id}' is not installed for ordinary tool exposure"
        );
        let binding = snapshot.receipt.secret_binding()?;
        let policy_template = self
            .policy_manager
            .prepare_bound_template(&binding, snapshot.policy.as_deref())
            .await?;
        let component = self.compile_snapshot(&snapshot).await?;
        let (mut instance, tools) = self.prepare_component_instance(component, &snapshot.wasm)?;
        instance.secret_binding = Some(binding);
        instance.effective_policy = snapshot.policy.clone();
        let prepared = PreparedComponentLoad {
            captured: CapturedComponent {
                storage_key: snapshot.receipt.storage_key.clone(),
                wasm: snapshot.wasm,
                bundled_policy: None,
            },
            effective_policy: snapshot.policy,
            policy_template,
            instance,
            tools,
        };
        let outcome = self
            .publish_prepared(
                prepared,
                CommitOutcome {
                    entry: StoredEntry::Installed(snapshot.receipt),
                    cursor: snapshot.cursor,
                    change: None,
                },
            )
            .await?;
        info!(component_id = %id, "Restored component from store snapshot");
        Ok(outcome)
    }

    pub(crate) async fn compile_snapshot(&self, snapshot: &ArtifactSnapshot) -> Result<Component> {
        anyhow::ensure!(
            snapshot.receipt.kind == StoredArtifactKind::Tool,
            "Cannot load ACP or unsupported artifacts as ordinary tool components"
        );
        let id = snapshot.receipt.component_id.as_str().to_owned();
        let revision = snapshot.receipt.revision.clone();
        let engine = self.cache_engine();
        match store_operation(&self.store, move |store| {
            Ok(store.read_cache(&id, &revision, &engine, CACHE_SCHEMA)?)
        })
        .await
        {
            Ok(Some(cache)) => {
                // Only store-bound output of our runtime serializer is eligible.
                match unsafe { Component::deserialize(self.runtime.as_ref(), &cache.native) } {
                    Ok(component)
                        if compiled_artifact_shape(&component, self.runtime.as_ref())
                            == ArtifactShape::ToolCandidate =>
                    {
                        return Ok(component)
                    }
                    Ok(_) => warn!("Ignoring a non-tool native cache"),
                    Err(error) => warn!(%error, "Could not deserialize the bound native cache"),
                }
            }
            Ok(None) => {}
            Err(error) => {
                warn!(error = %format_error_chain(&error), "Could not read the bound native cache")
            }
        }
        Component::new(self.runtime.as_ref(), &snapshot.wasm)
            .map_err(anyhow::Error::from)
            .context("Failed to compile captured component")
    }

    pub(crate) async fn publish_policy_commit(&self, commit: PolicyCommit) -> Result<()> {
        let StoredEntry::Installed(receipt) = &commit.outcome.entry else {
            bail!("Policy transaction did not produce an installed entry");
        };
        let id = receipt.component_id.as_str().to_owned();
        let expected = receipt.revision.clone();
        for _ in 0..crate::tool_catalog::PUBLICATION_ATTEMPTS {
            let registry = self.registry.clone();
            let template = commit.template.clone();
            let effective_policy = commit.effective_policy.clone();
            let cursor = commit.outcome.cursor.clone();
            let receipt = receipt.clone();
            let selected_id = id.clone();
            let expected = expected.clone();
            let published = store_operation(&self.store, move |store| {
                let _scope = store.checked_read(&cursor, Some((&selected_id, &expected)))?;
                let Ok(mut state) = registry.state.try_write() else {
                    return Ok(false);
                };
                if !receipt.requests_tool_exposure() {
                    state.unregister_component(&selected_id);
                } else if let Some(instance) = state.components.get_mut(&selected_id) {
                    if instance.artifact_sha256 == receipt.artifact_sha256 {
                        instance.policy_template = template;
                        instance.effective_policy = effective_policy;
                        instance.revision = Some(expected);
                    } else {
                        state.unregister_component(&selected_id);
                    }
                }
                Ok(true)
            })
            .await
            .context("Policy committed to disk, but runtime publication requires a retry")?;
            if published {
                return Ok(());
            }
            tokio::task::yield_now().await;
        }
        bail!("Policy committed, but runtime registry remained busy; retry")
    }

    pub(crate) async fn installed_tool_ids(&self) -> Result<Vec<String>> {
        let snapshot = store_operation(&self.store, |store| {
            store
                .snapshot_if_changed(None)?
                .context("missing requested store snapshot")
        })
        .await?;
        for legacy in snapshot.protected {
            warn!(storage_key = %legacy.physical_key, diagnostic = ?legacy.diagnostic, "Unreceipted legacy component is protected and not exposed");
        }
        let mut ids: Vec<_> = snapshot
            .entries
            .into_iter()
            .filter_map(|entry| match entry {
                StoredEntry::Installed(receipt) if receipt.requests_tool_exposure() => {
                    Some(receipt.component_id.as_str().to_owned())
                }
                _ => None,
            })
            .collect();
        ids.sort();
        Ok(ids)
    }

    pub(crate) fn cache_engine(&self) -> String {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        self.runtime.precompile_compatibility_hash().hash(&mut hash);
        format!("ordinary-{:016x}", hash.finish())
    }

    pub(crate) fn metadata_for(
        &self,
        id: &str,
        tools: &[ToolMetadata],
        wasm: &[u8],
    ) -> Result<ComponentMetadata> {
        use sha2::{Digest, Sha256};
        Ok(ComponentMetadata {
            component_id: id.to_owned(),
            tool_schemas: tools.iter().map(|tool| tool.schema.clone()).collect(),
            function_identifiers: tools.iter().map(|tool| tool.identifier.clone()).collect(),
            tool_names: tools
                .iter()
                .map(|tool| tool.normalized_name.clone())
                .collect(),
            validation_stamp: ValidationStamp {
                file_size: wasm.len() as u64,
                mtime: 0,
                content_hash: Some(hex::encode(Sha256::digest(wasm))),
            },
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
        })
    }
}

fn incoming_policy(bytes: Option<Vec<u8>>) -> Result<PreparedPolicy> {
    Ok(match bytes {
        Some(bytes) => PreparedPolicy::parse(bytes, PolicyProvenance::Bundled)?,
        None => PreparedPolicy::absent(PolicyProvenance::Default),
    })
}

pub(crate) fn policy_from_snapshot(snapshot: &ArtifactSnapshot) -> Result<PreparedPolicy> {
    let policy = match snapshot.policy.clone() {
        Some(bytes) => PreparedPolicy::parse(bytes, snapshot.receipt.policy.provenance.clone())?,
        None => PreparedPolicy::absent(snapshot.receipt.policy.provenance.clone()),
    };
    Ok(policy.with_metadata(snapshot.receipt.policy.metadata.clone())?)
}
