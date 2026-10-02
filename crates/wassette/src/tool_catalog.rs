// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Revision-bound tool catalogs and invocation admission, independent of a protocol.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Context, Result};
use component2json::{json_to_vals, ToolMetadata};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{watch, Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use wasmtime::component::types::ComponentItem;
use wasmtime::component::Val;

use crate::store::{
    ArtifactSnapshot, EntryRevision, InstallReceipt, PreparedCache, StoreCursor, StoreError,
    StoredEntry,
};
use crate::store_runtime::CACHE_SCHEMA;
use crate::store_support::store_operation;
use crate::tool::ToolSelector;
use crate::{
    inspect_artifact, schema, ArtifactShape, ComponentId, ComponentInstance, ComponentMetadata,
    ComponentRegistryState, CustomResourceLimiter, LifecycleManager, ScopedToolDescriptor,
    ScopedToolOutput, ToolKey, ToolLookupError, WasiState, WassetteWasiState,
};

pub(crate) const PUBLICATION_ATTEMPTS: usize = 8;
const DESCRIPTOR_FORMAT: &[u8] = b"wassette-tool-contract-v1";
static NEXT_RUNTIME: AtomicU64 = AtomicU64::new(1);

/// A host-issued exact export, installed revision and descriptor contract.
///
/// This is not a permission grant. It contains no physical path or secret data.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolRef {
    key: ToolKey,
    revision: EntryRevision,
    contract: String,
}

impl ToolRef {
    /// The embedded semantic identity and exact export.
    pub fn key(&self) -> &ToolKey {
        &self.key
    }

    /// The opaque anti-ABA revision, not an artifact digest.
    pub fn revision(&self) -> &EntryRevision {
        &self.revision
    }
}

/// An exact tool schema tied to a particular installed revision.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDescriptor {
    /// Reference to retain across a permission prompt.
    pub reference: ToolRef,
    /// Full canonical schema and exact semantic export key.
    pub tool: ScopedToolDescriptor,
}

/// A process-local catalog publication token, including its runtime identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogGeneration {
    runtime: u64,
    sequence: u64,
}

/// One atomically published, complete callable-tool view.
#[derive(Debug, Clone)]
pub struct CatalogSnapshot {
    /// In-memory publication generation, unrelated to the durable store cursor.
    pub generation: CatalogGeneration,
    /// Eligible tools, retaining all normalized-name collisions.
    pub tools: Vec<ToolDescriptor>,
}

/// An unavailable entry. Diagnostics never contain captured secret values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogDiagnostic {
    /// Semantic identity, when it could be established by the store.
    pub component_id: Option<ComponentId>,
    /// The failed operation, without embedding artifact, policy or secret contents.
    pub message: String,
}

/// The outcome of an attempted coherent publication.
#[derive(Debug, Clone)]
pub struct RefreshReport {
    /// Store observation reconciled (or attempted, when returned in an error).
    pub cursor: StoreCursor,
    /// Resulting in-memory publication generation.
    pub generation: CatalogGeneration,
    /// Whether observable references, schemas, membership or availability changed.
    pub changed: bool,
    /// Unavailable entries; a nonempty list makes refresh fail.
    pub diagnostics: Vec<CatalogDiagnostic>,
}

/// A failed hydration can publish unavailability, but never acknowledge its cursor.
#[derive(Debug, thiserror::Error)]
#[error("Catalog refresh has unavailable entries")]
pub struct CatalogRefreshError {
    /// Publication outcome and per-entry diagnostics.
    pub report: RefreshReport,
}

/// Typed host failures from preparation and execution; guest-returned `err` is data.
#[derive(Debug, thiserror::Error)]
pub enum ToolInvocationError {
    /// Initial lookup failed, before a revision-bound reference was issued.
    #[error("{0}")]
    NotFound(#[source] anyhow::Error),
    /// A previously issued reference or required name view is no longer current.
    #[error("Stale tool reference: {0}")]
    Stale(#[source] anyhow::Error),
    /// Arguments cannot be converted for the exact export.
    #[error("Invalid tool arguments: {0}")]
    InvalidArguments(#[source] anyhow::Error),
    /// A typed host capability check denied execution.
    #[error("{0}")]
    PolicyDenied(#[source] anyhow::Error),
    /// Guest trap or other host execution failure.
    #[error("{0}")]
    ExecutionFailed(#[source] anyhow::Error),
    /// Required runtime/store/policy/secret inputs could not be prepared.
    #[error("Tool unavailable: {0}")]
    Unavailable(#[source] anyhow::Error),
}

/// Output and the revision-bound descriptor actually admitted for the call.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    /// Admitted descriptor, never re-read after execution.
    pub descriptor: ToolDescriptor,
    /// Existing raw guest return representation.
    pub raw_result: String,
}

/// Owned execution inputs, not a spawned job or an authorization grant.
///
/// The caller owns scheduling and concurrency permits. Cancelling a waiter does
/// not interrupt non-yielding Wasm; supervisors must retain the actual job.
pub struct PreparedInvocation {
    manager: LifecycleManager,
    component: ComponentInstance,
    descriptor: ToolDescriptor,
    arguments: Vec<Val>,
    state: (WassetteWasiState<WasiState>, Option<CustomResourceLimiter>),
    name: Option<(Option<String>, String)>,
}

impl std::fmt::Debug for PreparedInvocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedInvocation")
            .field("descriptor", &self.descriptor)
            .finish_non_exhaustive()
    }
}

impl PreparedInvocation {
    /// Final-admit, then execute the immutable prepared bundle without held locks.
    ///
    /// Replacement/removal after admission does not revoke this running call.
    pub async fn run(self) -> std::result::Result<ToolOutput, ToolInvocationError> {
        self.admit().await?;
        self.execute_admitted().await
    }

    async fn execute_admitted(self) -> std::result::Result<ToolOutput, ToolInvocationError> {
        let output = self
            .manager
            .execute_tool_call(
                self.component,
                self.descriptor.tool.clone(),
                self.arguments,
                self.state,
            )
            .await
            .map_err(|error| match error.downcast::<ToolInvocationError>() {
                Ok(error) => error,
                Err(error) => ToolInvocationError::ExecutionFailed(error),
            })?;
        Ok(ToolOutput {
            descriptor: self.descriptor,
            raw_result: output.raw_result,
        })
    }

    fn admit(
        &self,
    ) -> impl std::future::Future<Output = std::result::Result<(), ToolInvocationError>> + Send + 'static
    {
        Self::admit_reference(
            self.manager.clone(),
            self.descriptor.reference.clone(),
            self.name.clone(),
        )
    }

    async fn admit_reference(
        manager: LifecycleManager,
        expected: ToolRef,
        required_name: Option<(Option<String>, String)>,
    ) -> std::result::Result<(), ToolInvocationError> {
        for _ in 0..PUBLICATION_ATTEMPTS {
            let cursor = if let Some((component, name)) = &required_name {
                let catalog = manager
                    .catalog()
                    .await
                    .map_err(ToolInvocationError::Unavailable)?;
                require_name_reference(&catalog.tools, component.as_deref(), name, &expected)?;
                manager
                    .catalog_runtime
                    .state
                    .read()
                    .await
                    .cursor
                    .clone()
                    .ok_or_else(|| ToolInvocationError::Unavailable(anyhow!("Catalog not ready")))?
            } else {
                let snapshot = store_operation(&manager.store, |store| {
                    store
                        .snapshot_if_changed(None)?
                        .context("Missing store snapshot")
                })
                .await
                .map_err(ToolInvocationError::Unavailable)?;
                let current = snapshot
                    .entries
                    .iter()
                    .find(|entry| entry.component_id() == &expected.key.component_id);
                if !matches!(current, Some(StoredEntry::Installed(receipt))
                    if receipt.requests_tool_exposure()
                        && receipt.revision == expected.revision)
                {
                    return Err(ToolInvocationError::Stale(anyhow!(
                        "Component was removed, replaced or made unavailable"
                    )));
                }
                snapshot.cursor
            };
            let reference = expected.clone();
            let catalog = manager.catalog_runtime.clone();
            let name = required_name.clone();
            let checked = store_operation(&manager.store, move |store| {
                let _scope = store.checked_read(
                    &cursor,
                    Some((reference.key.component_id.as_str(), &reference.revision)),
                )?;
                if let Some((component, name)) = name {
                    let Ok(state) = catalog.state.try_read() else {
                        return Ok(false);
                    };
                    if state.cursor.as_ref() != Some(&cursor) || !state.diagnostics.is_empty() {
                        return Ok(false);
                    }
                    require_name_reference(&state.tools, component.as_deref(), &name, &reference)?;
                }
                Ok(true)
            })
            .await;
            match checked {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(error) if is_store_conflict(&error) => {}
                Err(error) => return Err(invocation_error(error)),
            }
            tokio::task::yield_now().await;
        }
        Err(ToolInvocationError::Unavailable(anyhow!(
            "Store or catalog kept changing during admission; retry"
        )))
    }
}

#[derive(Clone)]
struct CatalogState {
    cursor: Option<StoreCursor>,
    generation: CatalogGeneration,
    tools: Vec<ToolDescriptor>,
    diagnostics: Vec<CatalogDiagnostic>,
}

pub(crate) struct CatalogRuntime {
    gate: Mutex<()>,
    state: RwLock<Arc<CatalogState>>,
    changed: watch::Sender<CatalogGeneration>,
    #[cfg(test)]
    before_publish: std::sync::Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
}

impl CatalogRuntime {
    pub(crate) fn new() -> Result<Self> {
        let runtime = NEXT_RUNTIME
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| anyhow!("Catalog runtime identity exhausted"))?;
        let generation = CatalogGeneration {
            runtime,
            sequence: 0,
        };
        let (changed, _) = watch::channel(generation.clone());
        Ok(Self {
            gate: Mutex::new(()),
            state: RwLock::new(Arc::new(CatalogState {
                cursor: None,
                generation,
                tools: Vec::new(),
                diagnostics: Vec::new(),
            })),
            changed,
            #[cfg(test)]
            before_publish: std::sync::Mutex::new(None),
        })
    }
}

impl LifecycleManager {
    /// Read a fresh, atomically published catalog. This grants no permissions.
    ///
    /// Reads validate captured bytes even when the durable cursor is unchanged;
    /// a cached kind/name cannot conceal damaged or externally replaced artifacts.
    pub async fn catalog(&self) -> Result<CatalogSnapshot> {
        self.resnapshot_from_store().await?;
        let state = self.catalog_runtime.state.read().await;
        ensure!(
            state.diagnostics.is_empty(),
            "Catalog contains unavailable entries"
        );
        Ok(CatalogSnapshot {
            generation: state.generation.clone(),
            tools: state.tools.clone(),
        })
    }

    /// Reconcile the durable cursor. Concurrent callers share one publication gate.
    pub async fn refresh_from_store(&self) -> Result<RefreshReport> {
        self.refresh_catalog(false).await
    }

    /// Force a full store observation, including after lost hints or an epoch change.
    pub async fn resnapshot_from_store(&self) -> Result<RefreshReport> {
        self.refresh_catalog(true).await
    }

    /// Wait for in-process publication, not external filesystem changes.
    ///
    /// A foreign generation returns immediately so the caller can resnapshot.
    /// Registering the receiver before checking its value prevents lost wakeups.
    pub async fn wait_changed(&self, after: &CatalogGeneration) -> Result<CatalogGeneration> {
        let mut changes = self.catalog_runtime.changed.subscribe();
        loop {
            let current = changes.borrow_and_update().clone();
            if current != *after {
                return Ok(current);
            }
            changes
                .changed()
                .await
                .context("Catalog publisher closed")?;
        }
    }

    /// Run an opt-in refresh loop in the caller's task until shutdown.
    ///
    /// The caller chooses the interval and owns/reaps this future; core does not
    /// spawn a driver or source watcher. Refresh failures stop the driver explicitly.
    pub async fn run_refresh_driver(
        &self,
        interval: Duration,
        shutdown: CancellationToken,
    ) -> Result<()> {
        ensure!(!interval.is_zero(), "Refresh interval must be nonzero");
        let mut ticks = tokio::time::interval(interval);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => return Ok(()),
                _ = ticks.tick() => { self.refresh_from_store().await?; }
            }
        }
    }

    async fn refresh_catalog(&self, force: bool) -> Result<RefreshReport> {
        let _gate = self.catalog_runtime.gate.lock().await;
        'publication: for _ in 0..PUBLICATION_ATTEMPTS {
            let previous = self.catalog_runtime.state.read().await.clone();
            let known = if force || !previous.diagnostics.is_empty() {
                None
            } else {
                previous.cursor.clone()
            };
            let snapshot = store_operation(&self.store, move |store| {
                Ok(store.snapshot_if_changed(known.as_ref())?)
            })
            .await?;
            let Some(snapshot) = snapshot else {
                return Ok(RefreshReport {
                    cursor: previous
                        .cursor
                        .clone()
                        .context("Uninitialized catalog cursor")?,
                    generation: previous.generation.clone(),
                    changed: false,
                    diagnostics: Vec::new(),
                });
            };
            let registry_observation = self.registry.state.read().await.modification.clone();
            let mut registry = ComponentRegistryState::default();
            let mut tools = Vec::new();
            let mut diagnostics = Vec::new();
            let mut failure = None;
            for protected in snapshot.protected {
                diagnostics.push(CatalogDiagnostic {
                    component_id: protected.component_id,
                    message: "Unreceipted component requires explicit store admission".to_owned(),
                });
            }
            for entry in snapshot.entries {
                let StoredEntry::Installed(receipt) = entry else {
                    continue;
                };
                if !receipt.requests_tool_exposure() {
                    continue;
                }
                let id = receipt.component_id.as_str().to_owned();
                match self.prepare_catalog_entry(&receipt).await {
                    Ok((descriptors, instance)) => {
                        let metadata = descriptor_metadata(&descriptors)?;
                        registry.register_tools_only(&id, metadata);
                        if let Some(instance) = instance {
                            registry.components.insert(id, instance);
                        }
                        tools.extend(descriptors);
                    }
                    Err(error) if is_store_conflict(&error) => {
                        tokio::task::yield_now().await;
                        continue 'publication;
                    }
                    Err(error) => {
                        diagnostics.push(CatalogDiagnostic {
                            component_id: Some(receipt.component_id),
                            message: "Could not prepare this component's catalog entry".to_owned(),
                        });
                        failure.get_or_insert(error);
                    }
                }
            }
            sort_tools(&mut tools);
            let changed = previous.tools != tools || previous.diagnostics != diagnostics;
            let mut generation = previous.generation.clone();
            if changed {
                generation.sequence = generation
                    .sequence
                    .checked_add(1)
                    .context("Catalog generation exhausted")?;
            }
            let report = RefreshReport {
                cursor: snapshot.cursor.clone(),
                generation: generation.clone(),
                changed,
                diagnostics: diagnostics.clone(),
            };
            let next = Arc::new(CatalogState {
                cursor: if diagnostics.is_empty() {
                    Some(snapshot.cursor.clone())
                } else {
                    previous.cursor.clone()
                },
                generation,
                tools,
                diagnostics,
            });
            let runtime = self.catalog_runtime.clone();
            let live_registry = self.registry.clone();
            let cursor = snapshot.cursor;
            let catalog_observation = previous.clone();
            #[cfg(test)]
            {
                let hook = self
                    .catalog_runtime
                    .before_publish
                    .lock()
                    .expect("publication hook lock poisoned")
                    .take();
                if let Some((ready, resume)) = hook {
                    ready
                        .send(())
                        .map_err(|_| anyhow!("Publication test listener closed"))?;
                    resume.await.context("Publication test resumer closed")?;
                }
            }
            let published = store_operation(&self.store, move |store| {
                let scope = store.checked_read(&cursor, None)?;
                let Ok(mut live) = live_registry.state.try_write() else {
                    return Ok(false);
                };
                if !Arc::ptr_eq(&live.modification, &registry_observation) {
                    return Ok(false);
                }
                let Ok(mut state) = runtime.state.try_write() else {
                    return Ok(false);
                };
                if !Arc::ptr_eq(&state, &catalog_observation) {
                    return Ok(false);
                }
                let generation = next.generation.clone();
                let retired_registry = std::mem::replace(&mut *live, registry);
                let retired_catalog = std::mem::replace(&mut *state, next);
                drop(state);
                drop(live);
                drop(scope);
                drop(retired_registry);
                drop(retired_catalog);
                if changed {
                    // The blocking publisher outlives a cancelled refresh waiter.
                    runtime.changed.send_if_modified(|current| {
                        if current.sequence < generation.sequence {
                            *current = generation;
                            true
                        } else {
                            false
                        }
                    });
                }
                Ok(true)
            })
            .await;
            match published {
                Ok(true) => {
                    if !report.diagnostics.is_empty() {
                        return Err(failure
                            .unwrap_or_else(|| anyhow!("Protected entries cannot be exposed"))
                            .context(CatalogRefreshError { report }));
                    }
                    return Ok(report);
                }
                Ok(false) => {}
                Err(error) if is_store_conflict(&error) => {}
                Err(error) => return Err(error),
            }
            tokio::task::yield_now().await;
        }
        bail!("Store or registry kept changing during catalog publication; retry")
    }

    async fn prepare_catalog_entry(
        &self,
        receipt: &InstallReceipt,
    ) -> Result<(Vec<ToolDescriptor>, Option<ComponentInstance>)> {
        let id = receipt.component_id.as_str();
        let _guard = self.load_guard(id).await.lock_owned().await;
        let snapshot = self.store_snapshot(id).await?;
        ensure!(
            snapshot.receipt == *receipt,
            "Entry changed during catalog preparation"
        );
        validate_snapshot(&snapshot)?;
        let binding = receipt.secret_binding()?;
        let policy = self
            .policy_manager
            .prepare_bound_template(&binding, snapshot.policy.as_deref())
            .await?;
        let reused = {
            let state = self.registry.state.read().await;
            state
                .components
                .get(id)
                .filter(|instance| instance.artifact_sha256 == receipt.artifact_sha256)
                .map(|instance| {
                    (
                        instance.clone(),
                        state.descriptors_for_component(&receipt.component_id),
                    )
                })
        };
        if let Some((mut instance, descriptors)) = reused {
            if instance.revision.as_ref() != Some(&receipt.revision) {
                instance.policy_template = policy;
            }
            instance.revision = Some(receipt.revision.clone());
            instance.effective_policy = snapshot.policy;
            instance.secret_binding = Some(binding);
            return Ok((version_descriptors(descriptors?, receipt)?, Some(instance)));
        }
        let cache_id = id.to_owned();
        let revision = receipt.revision.clone();
        let engine = self.cache_engine();
        let cache = store_operation(&self.store, move |store| {
            Ok(store.read_cache(&cache_id, &revision, &engine, CACHE_SCHEMA)?)
        })
        .await;
        match cache {
            Ok(Some(cache)) => match metadata_descriptors(cache.metadata, receipt) {
                Ok(descriptors) => return Ok((descriptors, None)),
                Err(_) => {
                    tracing::warn!(component_id = %id, "Rebuilding invalid derived tool metadata")
                }
            },
            Ok(None) => {}
            Err(_) => {
                tracing::warn!(component_id = %id, "Rebuilding unavailable derived tool cache")
            }
        }
        let component = self.compile_snapshot(&snapshot).await?;
        let (mut instance, metadata) =
            self.prepare_component_instance(component, &snapshot.wasm)?;
        instance.revision = Some(receipt.revision.clone());
        instance.artifact_sha256 = receipt.artifact_sha256.clone();
        instance.policy_template = policy;
        instance.effective_policy = snapshot.policy.clone();
        instance.secret_binding = Some(binding);
        let metadata = self.metadata_for(id, &metadata, &snapshot.wasm)?;
        let descriptors = metadata_descriptors(serde_json::to_value(&metadata)?, receipt)?;
        let cache = PreparedCache {
            artifact_sha256: receipt.artifact_sha256.clone(),
            engine: self.cache_engine(),
            schema: CACHE_SCHEMA.to_owned(),
            metadata: serde_json::to_value(metadata)?,
            native: instance.component.serialize()?,
        };
        let cache_id = id.to_owned();
        let revision = receipt.revision.clone();
        if let Err(error) = store_operation(&self.store, move |store| {
            Ok(store.publish_cache(&cache_id, &revision, cache)?)
        })
        .await
        {
            if is_store_conflict(&error) {
                return Err(error);
            }
            tracing::warn!(component_id = %id, error = %crate::format_error_chain(&error),
                "Catalog prepared, but derived cache publication failed");
        }
        Ok((descriptors, Some(instance)))
    }

    /// Describe a still-current reference, rejecting replacement rather than rebinding.
    pub async fn describe_tool(&self, reference: &ToolRef) -> Result<ToolDescriptor> {
        let catalog = self.catalog().await?;
        catalog
            .tools
            .into_iter()
            .find(|tool| tool.reference == *reference)
            .ok_or_else(|| {
                ToolInvocationError::Stale(anyhow!("Tool is no longer in the catalog")).into()
            })
    }

    /// Capture code, policy, arguments and private secrets before final admission.
    pub async fn prepare_invocation(
        &self,
        reference: &ToolRef,
        arguments: &Value,
    ) -> std::result::Result<PreparedInvocation, ToolInvocationError> {
        self.prepare_invocation_inner(reference, arguments)
            .await
            .map_err(invocation_error)
    }

    async fn prepare_invocation_inner(
        &self,
        reference: &ToolRef,
        arguments: &Value,
    ) -> Result<PreparedInvocation> {
        let id = reference.key.component_id.as_str();
        let snapshot = self.store_snapshot(id).await.map_err(|error| {
            if matches!(
                error.downcast_ref::<StoreError>(),
                Some(StoreError::NotFound(_))
            ) {
                anyhow::Error::new(ToolInvocationError::Stale(error))
            } else {
                error
            }
        })?;
        if snapshot.receipt.revision != reference.revision
            || !snapshot.receipt.requests_tool_exposure()
        {
            return Err(ToolInvocationError::Stale(anyhow!(
                "Component revision or eligibility changed"
            ))
            .into());
        }
        if let Err(error) = self.ensure_component_loaded(id).await {
            let current = self.store_snapshot(id).await;
            match current {
                Ok(current)
                    if current.receipt.revision == reference.revision
                        && current.receipt.requests_tool_exposure() =>
                {
                    return Err(error)
                }
                Err(current)
                    if !matches!(
                        current.downcast_ref::<StoreError>(),
                        Some(StoreError::NotFound(_))
                    ) =>
                {
                    return Err(current);
                }
                _ => return Err(ToolInvocationError::Stale(error).into()),
            }
        }
        let (mut component, tool) = self
            .select_loaded_tool(ToolSelector::Exact(&reference.key))
            .await
            .map_err(ToolInvocationError::Stale)?;
        let selected_revision = component
            .revision
            .as_ref()
            .context("Missing instance revision")?;
        let descriptor = version_descriptor(tool, selected_revision)?;
        if descriptor.reference != *reference {
            return Err(ToolInvocationError::Stale(anyhow!(
                "Export revision or descriptor changed"
            ))
            .into());
        }
        let function = invocation_function(self, &component, &descriptor.tool.key)?;
        let params = function
            .params()
            .map(|(name, ty)| (name.to_owned(), ty))
            .collect::<Vec<_>>();
        let arguments = json_to_vals(arguments, &params)
            .map_err(|error| ToolInvocationError::InvalidArguments(error.into()))?;
        let binding = component
            .secret_binding
            .as_ref()
            .context("Missing component secret binding")?;
        component.policy_template = self
            .policy_manager
            .prepare_bound_template(binding, component.effective_policy.as_deref())
            .await?;
        let state = Self::wasi_state_from_template(&component.policy_template)?;
        Ok(PreparedInvocation {
            manager: self.clone(),
            component,
            descriptor,
            arguments,
            state,
            name: None,
        })
    }

    pub(crate) async fn invoke_catalog_name(
        &self,
        component: Option<&str>,
        name: &str,
        arguments: &Value,
    ) -> Result<ScopedToolOutput> {
        let catalog = self.catalog().await?;
        let descriptor = resolve_name(&catalog.tools, component, name)?;
        let mut prepared = self
            .prepare_invocation(&descriptor.reference, arguments)
            .await?;
        prepared.name = Some((component.map(str::to_owned), name.to_owned()));
        let output = prepared.run().await?;
        Ok(ScopedToolOutput {
            descriptor: output.descriptor.tool,
            raw_result: output.raw_result,
        })
    }
}

fn validate_snapshot(snapshot: &ArtifactSnapshot) -> Result<()> {
    let inspection = inspect_artifact(&snapshot.wasm)?;
    ensure!(
        inspection.shape == ArtifactShape::ToolCandidate,
        "Artifact is not an ordinary tool"
    );
    Ok(())
}

fn metadata_descriptors(value: Value, receipt: &InstallReceipt) -> Result<Vec<ToolDescriptor>> {
    let metadata: ComponentMetadata = serde_json::from_value(value)?;
    ensure!(
        metadata.component_id == receipt.component_id.as_str()
            && metadata.validation_stamp.content_hash.as_deref() == Some(&receipt.artifact_sha256)
            && metadata.function_identifiers.len() == metadata.tool_schemas.len()
            && metadata.function_identifiers.len() == metadata.tool_names.len(),
        "Tool metadata does not match the receipt"
    );
    let mut descriptors = Vec::new();
    for ((export, schema), name) in metadata
        .function_identifiers
        .into_iter()
        .zip(metadata.tool_schemas)
        .zip(metadata.tool_names)
    {
        ensure!(
            !crate::is_acp_identifier(&export) && schema["name"].as_str() == Some(&name),
            "Invalid cached tool identity"
        );
        let tool = ScopedToolDescriptor {
            key: ToolKey {
                component_id: receipt.component_id.clone(),
                export,
            },
            schema: schema::canonicalize_tool_schema(&schema),
        };
        ensure!(
            !descriptors
                .iter()
                .any(|existing: &ToolDescriptor| existing.tool.key == tool.key),
            "Duplicate exact tool identity"
        );
        descriptors.push(version_descriptor(tool, &receipt.revision)?);
    }
    Ok(descriptors)
}

fn version_descriptors(
    tools: Vec<ScopedToolDescriptor>,
    receipt: &InstallReceipt,
) -> Result<Vec<ToolDescriptor>> {
    tools
        .into_iter()
        .map(|tool| version_descriptor(tool, &receipt.revision))
        .collect()
}

fn version_descriptor(
    tool: ScopedToolDescriptor,
    revision: &EntryRevision,
) -> Result<ToolDescriptor> {
    let mut hash = Sha256::new();
    hash.update(DESCRIPTOR_FORMAT);
    hash.update(serde_json::to_vec(&tool.schema)?);
    let reference = ToolRef {
        key: tool.key.clone(),
        revision: revision.clone(),
        contract: hex::encode(hash.finalize()),
    };
    Ok(ToolDescriptor { reference, tool })
}

fn descriptor_metadata(tools: &[ToolDescriptor]) -> Result<Vec<ToolMetadata>> {
    tools
        .iter()
        .map(|tool| {
            Ok(ToolMetadata {
                identifier: tool.tool.key.export.clone(),
                schema: tool.tool.schema.clone(),
                normalized_name: tool.tool.schema["name"]
                    .as_str()
                    .context("Tool schema has no name")?
                    .to_owned(),
            })
        })
        .collect()
}

fn sort_tools(tools: &mut [ToolDescriptor]) {
    tools.sort_by(|a, b| {
        let a = &a.tool.key;
        let b = &b.tool.key;
        (
            a.component_id.as_str(),
            &a.export.package_name,
            &a.export.interface_name,
            &a.export.function_name,
        )
            .cmp(&(
                b.component_id.as_str(),
                &b.export.package_name,
                &b.export.interface_name,
                &b.export.function_name,
            ))
    });
}

pub(crate) fn resolve_name<'a>(
    tools: &'a [ToolDescriptor],
    component: Option<&str>,
    name: &str,
) -> std::result::Result<&'a ToolDescriptor, ToolLookupError> {
    let matches: Vec<_> = tools
        .iter()
        .filter(|tool| {
            tool.tool.schema["name"].as_str() == Some(name)
                && component.is_none_or(|id| tool.tool.key.component_id.as_str() == id)
        })
        .collect();
    match matches.as_slice() {
        [] => Err(ToolLookupError::NotFound {
            tool: name.to_owned(),
        }),
        [tool] => Ok(tool),
        tools => Err(ToolLookupError::Ambiguous {
            tool: name.to_owned(),
            components: tools
                .iter()
                .map(|tool| tool.tool.key.component_id.as_str().to_owned())
                .collect(),
        }),
    }
}

fn require_name_reference(
    tools: &[ToolDescriptor],
    component: Option<&str>,
    name: &str,
    expected: &ToolRef,
) -> std::result::Result<(), ToolInvocationError> {
    let selected = resolve_name(tools, component, name)
        .map_err(|error| ToolInvocationError::Stale(error.into()))?;
    if selected.reference != *expected {
        return Err(ToolInvocationError::Stale(anyhow!(
            "Tool '{name}' changed since preparation"
        )));
    }
    Ok(())
}

fn invocation_error(error: anyhow::Error) -> ToolInvocationError {
    error
        .downcast::<ToolInvocationError>()
        .unwrap_or_else(ToolInvocationError::Unavailable)
}

fn invocation_function(
    manager: &LifecycleManager,
    component: &ComponentInstance,
    key: &ToolKey,
) -> Result<wasmtime::component::types::ComponentFunc> {
    let root = component.component.component_type();
    let item = if let Some(interface) = key
        .export
        .interface_name
        .as_deref()
        .filter(|name| !name.is_empty())
    {
        let (_, export) = root
            .exports(manager.runtime.as_ref())
            .find(|(name, _)| *name == interface)
            .context("Interface not found")?;
        let ComponentItem::ComponentInstance(instance) = export.ty else {
            bail!("Export is not an interface")
        };
        let function = instance
            .exports(manager.runtime.as_ref())
            .find(|(name, _)| *name == key.export.function_name)
            .map(|(_, export)| export.ty)
            .context("Function not found")?;
        function
    } else {
        root.exports(manager.runtime.as_ref())
            .find(|(name, _)| *name == key.export.function_name)
            .map(|(_, export)| export.ty)
            .context("Function not found")?
    };
    let ComponentItem::ComponentFunc(function) = item else {
        bail!("Export is not a function")
    };
    Ok(function)
}

fn is_store_conflict(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<StoreError>(),
        Some(StoreError::Conflict(_))
    )
}

#[cfg(test)]
mod tests;
