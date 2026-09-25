// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! A security-oriented runtime that runs WebAssembly Components via MCP

#![warn(missing_docs)]
// The guard tests spawn `load_component`, whose `Send` obligation chain runs through
// resource resolution, staging and compilation. The next trait solver evaluates that
// chain past the default limit of 128 and reports it as
// `recursion_depth_exceeding_limit`, which the nightly coverage job turns into an
// error via `-D warnings`. Stable builds do not use the next solver, so `build` and
// `lint` stay green while `test coverage` fails to compile. Raising the limit is
// preferred over allowing the lint, since the note says it becomes a hard error later.
#![recursion_limit = "256"]

use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use component2json::{
    component_exports_to_json_schema, component_exports_to_json_schema_with_docs,
    component_exports_to_tools, component_exports_to_tools_with_docs, create_placeholder_results,
    extract_package_docs, json_to_vals, vals_to_json, FunctionIdentifier, ToolMetadata,
};
use etcetera::BaseStrategy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::fs::DirEntry;
use tokio::sync::{Mutex, RwLock, Semaphore};
use tracing::{debug, info, instrument, warn};
use wasmtime::component::{Component, InstancePre};
use wasmtime::Store;

mod component_storage;
mod config;
mod error_display;
mod http;
pub mod loader;
pub mod oci_multi_layer;
mod policy_internal;
mod runtime_context;
pub mod schema;
mod secrets;
mod wasistate;

use component_storage::ComponentStorage;
pub use config::{LifecycleBuilder, LifecycleConfig};
pub use error_display::format_error_chain;
pub use http::WassetteWasiState;
use loader::{ComponentResource, DownloadedResource};
use policy_internal::PolicyManager;
pub use policy_internal::{PermissionGrantRequest, PermissionRule, PolicyInfo};
use runtime_context::RuntimeContext;
pub use secrets::SecretsManager;
use wasistate::WasiState;
pub use wasistate::{
    create_wasi_state_template_from_policy, CustomResourceLimiter, PermissionError,
    WasiStateTemplate,
};

const DOWNLOADS_DIR: &str = "downloads";
const PRECOMPILED_EXT: &str = "cwasm";
const METADATA_EXT: &str = "metadata.json";

// Default timeout configurations
pub(crate) const DEFAULT_OCI_TIMEOUT_SECS: u64 = 30;
pub(crate) const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 30;
pub(crate) const DEFAULT_DOWNLOAD_CONCURRENCY: usize = 8;

/// Get the default secrets directory path based on the OS
pub(crate) fn get_default_secrets_dir() -> PathBuf {
    let dir_strategy = etcetera::choose_base_strategy();
    match dir_strategy {
        Ok(strategy) => strategy.config_dir().join("wassette").join("secrets"),
        Err(_) => {
            eprintln!("WARN: Unable to determine default secrets directory, using `secrets` directory in the current working directory");
            PathBuf::from("./secrets")
        }
    }
}

#[derive(Debug, Clone)]
struct ToolInfo {
    component_id: String,
    identifier: FunctionIdentifier,
    schema: Value,
}

/// Component metadata for fast startup without compilation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentMetadata {
    /// Component identifier
    pub component_id: String,
    /// Tool schemas for this component
    pub tool_schemas: Vec<Value>,
    /// Function identifiers
    pub function_identifiers: Vec<FunctionIdentifier>,
    /// Normalized tool names
    pub tool_names: Vec<String>,
    /// Validation stamp
    pub validation_stamp: ValidationStamp,
    /// Metadata creation timestamp
    pub created_at: u64,
}

/// Validation stamp to check if component has changed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationStamp {
    /// File size in bytes
    pub file_size: u64,
    /// File modification time (seconds since epoch)
    pub mtime: u64,
    /// Optional content hash (SHA256)
    pub content_hash: Option<String>,
}

#[derive(Clone, Default)]
struct ComponentRegistry {
    state: Arc<RwLock<ComponentRegistryState>>,
}

/// Per-component guards that serialize concurrent load attempts.
///
/// Every path that compiles a component is check-then-act: it tests the registry (or the
/// artifact's validation stamp) and then compiles. Without a guard, two callers can both
/// observe the component as absent and both compile it, duplicating wasmtime compilation
/// and racing on the same metadata and precompiled cache files. The on-demand load done by
/// [`LifecycleManager::ensure_component_loaded`] races the background restore in exactly
/// this way. The guard is keyed by component id so unrelated components still load
/// concurrently.
///
/// Entries are weak, so an id occupies the map only while a load of that component is in
/// flight. Holding strong references instead would grow the map forever in a long-running
/// server that loads and unloads distinct components.
#[derive(Clone, Default)]
struct ComponentLoadGuards {
    guards: Arc<Mutex<HashMap<String, Weak<Mutex<()>>>>>,
}

impl ComponentLoadGuards {
    /// Returns the guard for `component_id`, creating one if no load is currently using it.
    ///
    /// Callers keep the returned handle alive for as long as they hold (or wait for) the
    /// guard, which is what makes the weak entries safe: while any load of `component_id` is
    /// in flight the entry still upgrades, so every concurrent caller for that id is handed
    /// the very same mutex. A fresh mutex is only ever minted once no caller holds one, and
    /// therefore never runs concurrently with the mutex it replaces.
    async fn guard_for(&self, component_id: &str) -> Arc<Mutex<()>> {
        let mut guards = self.guards.lock().await;

        if let Some(existing) = guards.get(component_id).and_then(Weak::upgrade) {
            return existing;
        }

        // Nothing holds a guard for this id, so its entry (and any other entry left behind by
        // a finished load or an unloaded component) is dead weight. Reclaim them here: this
        // only runs on the path that is about to compile a component, which dwarfs a sweep of
        // a map holding at most one entry per component.
        guards.retain(|_, guard| guard.strong_count() > 0);

        let guard = Arc::new(Mutex::new(()));
        guards.insert(component_id.to_string(), Arc::downgrade(&guard));
        guard
    }

    /// Component ids currently tracked in the map, for tests that assert it does not grow
    /// without bound.
    #[cfg(test)]
    async fn tracked_ids(&self) -> Vec<String> {
        let guards = self.guards.lock().await;
        let mut ids: Vec<String> = guards.keys().cloned().collect();
        ids.sort();
        ids
    }
}

#[derive(Default)]
struct ComponentRegistryState {
    components: HashMap<String, ComponentInstance>,
    tool_map: HashMap<String, Vec<ToolInfo>>,
    component_map: HashMap<String, Vec<String>>,
}

impl std::fmt::Debug for ComponentRegistryState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentRegistryState")
            .field("components_len", &self.components.len())
            .field("tool_map", &self.tool_map)
            .field("component_map", &self.component_map)
            .finish()
    }
}

/// The returned status when loading a component
#[derive(Debug, PartialEq, Clone)]
pub enum LoadResult {
    /// Indicates that the component was loaded but replaced a currently loaded component
    Replaced,
    /// Indicates that the component did not exist and is now loaded
    New,
}

/// Detailed outcome for a component load operation.
#[derive(Debug, Clone)]
pub struct ComponentLoadOutcome {
    /// Identifier of the component that was processed.
    pub component_id: String,
    /// Whether the load replaced an existing component or was newly added.
    pub status: LoadResult,
    /// Normalized tool names exposed by the component after registration.
    pub tool_names: Vec<String>,
}

impl ComponentRegistry {
    fn new() -> Self {
        Self::default()
    }

    async fn upsert_component(
        &self,
        component_id: String,
        instance: ComponentInstance,
        tools: Vec<ToolMetadata>,
    ) -> Result<LoadResult> {
        let mut state = self.state.write().await;
        state.upsert_component(component_id, instance, tools)
    }

    async fn remove_component(&self, component_id: &str) -> Option<ComponentInstance> {
        let mut state = self.state.write().await;
        state.unregister_component(component_id)
    }

    async fn get_component(&self, component_id: &str) -> Option<ComponentInstance> {
        let state = self.state.read().await;
        state.components.get(component_id).cloned()
    }

    async fn contains_component(&self, component_id: &str) -> bool {
        self.state
            .read()
            .await
            .components
            .contains_key(component_id)
    }

    async fn list_components(&self) -> Vec<String> {
        let state = self.state.read().await;
        let mut ids: Vec<String> = state.components.keys().cloned().collect();
        ids.sort();
        ids
    }

    async fn tool_identifier(&self, tool_name: &str) -> Option<FunctionIdentifier> {
        let state = self.state.read().await;
        state
            .tool_map
            .get(tool_name)
            .and_then(|infos| infos.first().map(|info| info.identifier.clone()))
    }

    async fn tool_infos(&self, tool_name: &str) -> Option<Vec<ToolInfo>> {
        let state = self.state.read().await;
        state.tool_map.get(tool_name).cloned()
    }

    async fn list_tools(&self) -> Vec<Value> {
        let state = self.state.read().await;
        state
            .tool_map
            .values()
            .flat_map(|tools| tools.iter().map(|t| t.schema.clone()))
            .collect()
    }

    /// Registers cached tool metadata for a whole set of components under one registry write.
    ///
    /// Hydration has to become visible all at once. Registering one component at a time
    /// publishes a `tool_map` in which a tool exported by two installed components is
    /// momentarily owned by exactly one of them, and a lookup landing in that window resolves
    /// the tool to that component and runs it, even though the collision rule would refuse it
    /// once both are registered. Nothing revisits the tool map afterwards to withdraw the
    /// answer. Taking the write lock once for the entire batch removes the window: a reader
    /// holds the read lock either before this write or after it, so it sees the registry
    /// either as it was before hydration or with every hydrated component present, and the
    /// collision is visible in both.
    ///
    /// `artifact_present` is rechecked here, inside that same critical section, rather than
    /// only under each component's load guard. The guard is released when a component's
    /// metadata has been validated, so an unload can complete between validation and this
    /// write, and a batch that trusted the earlier check would put a component with no
    /// artifact and no instance back into the tool map. The recheck closes that: an unload
    /// removes the artifact before it deregisters the component, so either the removal is
    /// already visible here and the entry is skipped, or the unload's own deregistration is
    /// still to come and, because it needs this very lock, it necessarily runs after this
    /// write and takes the entry back out.
    ///
    /// The predicate is deliberately synchronous. It runs a `stat` per pending component while
    /// the write lock is held, which is bounded by the number of installed components and
    /// happens once at startup; awaiting inside the critical section would hold the lock
    /// across scheduler yields for no benefit.
    async fn register_cached_metadata_batch(
        &self,
        pending: Vec<PendingRegistration>,
        artifact_present: impl Fn(&Path) -> bool,
    ) -> usize {
        let mut state = self.state.write().await;
        let mut registered = 0;

        for entry in pending {
            if !artifact_present(&entry.artifact_path) {
                debug!(component_id = %entry.component_id, "Component removed before its cached metadata could be registered");
                continue;
            }

            if state.components.contains_key(&entry.component_id)
                || state.component_map.contains_key(&entry.component_id)
            {
                debug!(component_id = %entry.component_id, "Skipping cached metadata; component already registered");
                continue;
            }

            state.register_tools_only(&entry.component_id, entry.tools);
            debug!(component_id = %entry.component_id, "Registered tools from cached metadata");
            registered += 1;
        }

        registered
    }
}

/// A component whose cached metadata has been read and validated, waiting to be published
/// into the registry by [`ComponentRegistry::register_cached_metadata_batch`].
struct PendingRegistration {
    component_id: String,
    artifact_path: PathBuf,
    tools: Vec<ToolMetadata>,
}

impl ComponentRegistryState {
    fn upsert_component(
        &mut self,
        component_id: String,
        instance: ComponentInstance,
        tools: Vec<ToolMetadata>,
    ) -> Result<LoadResult> {
        let replaced = self.components.contains_key(&component_id);
        self.unregister_tools(&component_id);
        self.register_tools_only(&component_id, tools);
        self.components.insert(component_id, instance);

        Ok(if replaced {
            LoadResult::Replaced
        } else {
            LoadResult::New
        })
    }

    fn unregister_component(&mut self, component_id: &str) -> Option<ComponentInstance> {
        self.unregister_tools(component_id);
        self.components.remove(component_id)
    }

    fn unregister_tools(&mut self, component_id: &str) {
        if let Some(tools) = self.component_map.remove(component_id) {
            for tool_name in tools {
                if let Some(tool_infos) = self.tool_map.get_mut(&tool_name) {
                    tool_infos.retain(|info| info.component_id != component_id);
                    if tool_infos.is_empty() {
                        self.tool_map.remove(&tool_name);
                    }
                }
            }
        }
    }

    /// Tool names in `tool_names` that an already-registered component exports, each
    /// paired with the ids of the components that already export it.
    fn find_tool_name_collisions(&self, tool_names: &[String]) -> Vec<(String, Vec<String>)> {
        tool_names
            .iter()
            .filter_map(|tool_name| {
                let existing = self.tool_map.get(tool_name)?;
                let mut component_ids: Vec<String> = existing
                    .iter()
                    .map(|info| info.component_id.clone())
                    .collect();
                component_ids.dedup();
                if component_ids.is_empty() {
                    None
                } else {
                    Some((tool_name.clone(), component_ids))
                }
            })
            .collect()
    }

    fn register_tools_only(&mut self, component_id: &str, tools: Vec<ToolMetadata>) {
        let incoming_names: Vec<String> = tools
            .iter()
            .map(|tool| tool.normalized_name.clone())
            .collect();

        for (tool_name, existing_component_ids) in self.find_tool_name_collisions(&incoming_names) {
            warn!(
                %component_id,
                %tool_name,
                existing_components = %existing_component_ids.join(", "),
                "Tool name collision: this tool name is already exported by another loaded \
                 component. The tool cannot be called while both components are loaded; unload \
                 one of them to make it callable again"
            );
        }

        let mut tool_names = Vec::with_capacity(incoming_names.len());

        for tool_metadata in tools {
            let ToolMetadata {
                identifier,
                schema,
                normalized_name,
            } = tool_metadata;

            let tool_info = ToolInfo {
                component_id: component_id.to_string(),
                identifier,
                schema,
            };

            self.tool_map
                .entry(normalized_name.clone())
                .or_default()
                .push(tool_info);
            tool_names.push(normalized_name);
        }

        self.component_map
            .insert(component_id.to_string(), tool_names);
    }
}

/// A manager that handles the dynamic lifecycle of WebAssembly components.
#[derive(Clone)]
pub struct LifecycleManager {
    runtime: Arc<RuntimeContext>,
    registry: ComponentRegistry,
    load_guards: ComponentLoadGuards,
    storage: ComponentStorage,
    policy_manager: PolicyManager,
    oci_client: Arc<oci_wasm::WasmClient>,
    http_client: reqwest::Client,
    secrets_manager: Arc<SecretsManager>,
}

/// A representation of a loaded component instance. It contains both the base component info and a
/// pre-instantiated component ready for execution
#[derive(Clone)]
pub struct ComponentInstance {
    component: Arc<Component>,
    instance_pre: Arc<InstancePre<WassetteWasiState<WasiState>>>,
    package_docs: Option<Value>,
}

impl LifecycleManager {
    /// Begin constructing a lifecycle manager with a fluent builder that
    /// validates configuration and applies sensible defaults.
    pub fn builder(component_dir: impl AsRef<Path>) -> LifecycleBuilder {
        LifecycleBuilder::new(component_dir.as_ref().to_path_buf())
    }

    /// Creates a lifecycle manager with default configuration and eager loading.
    #[instrument(skip_all, fields(component_dir = %component_dir.as_ref().display()))]
    pub async fn new(component_dir: impl AsRef<Path>) -> Result<Self> {
        Self::builder(component_dir).build().await
    }

    /// Creates an unloaded lifecycle manager; components remain unloaded until requested.
    #[instrument(skip_all, fields(component_dir = %component_dir.as_ref().display()))]
    pub async fn new_unloaded(component_dir: impl AsRef<Path>) -> Result<Self> {
        Self::builder(component_dir)
            .with_eager_loading(false)
            .build()
            .await
    }

    /// Construct a lifecycle manager from an explicit configuration without loading components.
    #[instrument(skip_all, fields(component_dir = %config.component_dir().display()))]
    pub async fn from_config(config: LifecycleConfig) -> Result<Self> {
        let (component_dir, secrets_dir, environment_vars, http_client, oci_client, _) =
            config.into_parts();

        let storage =
            ComponentStorage::new(component_dir.clone(), DEFAULT_DOWNLOAD_CONCURRENCY).await?;

        let runtime = Arc::new(RuntimeContext::initialize()?);

        let secrets_manager = Arc::new(SecretsManager::new(secrets_dir.clone()));
        secrets_manager.ensure_secrets_dir().await?;

        let environment_vars = Arc::new(environment_vars);
        let oci_client = Arc::new(oci_wasm::WasmClient::new(oci_client));

        let policy_manager = PolicyManager::new(
            storage.clone(),
            Arc::clone(&secrets_manager),
            Arc::clone(&environment_vars),
            Arc::clone(&oci_client),
            http_client.clone(),
        );

        Ok(Self {
            runtime,
            registry: ComponentRegistry::new(),
            load_guards: ComponentLoadGuards::default(),
            storage,
            policy_manager,
            oci_client,
            http_client,
            secrets_manager,
        })
    }

    /// Load every component present in the component directory, updating the registry and cache.
    ///
    /// Each component is compiled and registered while holding that component's load guard, so
    /// an eager startup load cannot re-register a component that a concurrent unload has already
    /// removed from disk and from the registry. The guard is per component id, so unrelated
    /// components are still compiled in parallel.
    #[instrument(skip(self))]
    pub async fn load_all_components(&self) -> Result<()> {
        let mut entries = tokio::fs::read_dir(self.storage.root()).await?;
        let mut load_futures = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            load_futures.push(self.load_entry_under_guard(entry));
        }

        for result in futures::future::join_all(load_futures).await {
            if let Err(error) = result {
                warn!(error = %format_error_chain(&error), "Failed to load component");
            }
        }

        info!("LifecycleManager finished loading components");
        Ok(())
    }

    /// Compile and register a single directory entry while holding that component's load guard.
    ///
    /// Compiling is not the only thing the guard has to cover: registering the result writes the
    /// same registry entry an unload removes, so both happen inside one critical section.
    /// Otherwise an unload could remove the artifact and the registry entry between the two, and
    /// this would re-register a component the unload had already reported as removed.
    ///
    /// Nothing reached from here takes the guard again, so this cannot deadlock:
    /// [`load_component_from_entry`] is a free function that only compiles, and the registry and
    /// the policy manager have their own locks.
    async fn load_entry_under_guard(&self, entry: DirEntry) -> Result<()> {
        let entry_path = entry.path();
        let is_wasm = entry_path
            .extension()
            .map(|ext| ext == "wasm")
            .unwrap_or(false);
        let is_file = entry
            .metadata()
            .await
            .map(|m| m.is_file())
            .context("unable to read file metadata")?;
        if !(is_file && is_wasm) {
            return Ok(());
        }

        let component_id = entry_path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(String::from)
            .context("wasm file didn't have a valid file name")?;

        let guard = self.load_guard(&component_id).await;
        let _guard = guard.lock().await;

        // An unload may have removed this component while we waited for the guard, in which
        // case there is nothing left to load.
        if !entry_path.exists() {
            debug!(component_id = %component_id, "Component removed while waiting for the load guard");
            return Ok(());
        }

        let Some((component_instance, name)) =
            load_component_from_entry(Arc::clone(&self.runtime), entry).await?
        else {
            return Ok(());
        };

        let tool_metadata = if let Some(ref package_docs) = component_instance.package_docs {
            component_exports_to_tools_with_docs(
                &component_instance.component,
                self.runtime.as_ref(),
                true,
                package_docs,
            )
        } else {
            component_exports_to_tools(&component_instance.component, self.runtime.as_ref(), true)
        };

        self.registry
            .upsert_component(name.clone(), component_instance, tool_metadata)
            .await
            .with_context(|| format!("Failed to register component in registry: {name}"))?;

        if let Err(error) = self.restore_policy_attachment(&name).await {
            warn!(component_id = %name, error = %format_error_chain(&error), "Failed to restore policy attachment");
        }

        Ok(())
    }

    async fn restore_policy_attachment(&self, component_id: &str) -> Result<()> {
        self.policy_manager.restore_from_disk(component_id).await
    }

    async fn resolve_component_resource(&self, uri: &str) -> Result<(String, DownloadedResource)> {
        // Show progress when running in CLI mode (stderr is a TTY)
        let show_progress = std::io::stderr().is_terminal();

        let resource = loader::load_resource_with_progress::<ComponentResource>(
            uri,
            &self.oci_client,
            &self.http_client,
            show_progress,
        )
        .await?;
        let id = resource.id()?;
        Ok((id, resource))
    }

    async fn stage_component_artifact(
        &self,
        component_id: &str,
        resource: DownloadedResource,
    ) -> Result<PathBuf> {
        let target_path = self.component_path(component_id);
        match resource {
            DownloadedResource::Local(path) if path == target_path => Ok(target_path),
            other => {
                self.storage
                    .install_component_artifact(component_id, other)
                    .await
            }
        }
    }

    /// Returns the per-component load guard, which serializes loading of `component_id`.
    ///
    /// The guard covers everything that reads or writes a component's on-disk artifacts:
    /// staging the `.wasm`, compiling it, writing its metadata and precompiled cache, and
    /// removing all of them again on unload. The mutex is not reentrant, so a caller that
    /// already holds it must call the `_locked` form of any helper rather than a wrapper
    /// that takes the guard itself.
    async fn load_guard(&self, component_id: &str) -> Arc<Mutex<()>> {
        self.load_guards.guard_for(component_id).await
    }

    /// Compiles and registers a component; the caller must hold the guard returned by
    /// [`Self::load_guard`] for `component_id`.
    ///
    /// This is the only place that compiles a component, so holding the guard here is what
    /// makes "compiled once" hold across the on-demand load path and the background restore.
    /// Every caller takes the guard itself first, because each of them has to do its own
    /// check-then-act (recheck the registry, or stage the artifact) inside the same critical
    /// section. Taking the guard again here would deadlock.
    ///
    /// This does not skip the work when the component is already registered, because an
    /// explicit reload has to recompile a changed artifact.
    async fn compile_and_register_component_locked(
        &self,
        component_id: &str,
        wasm_path: &Path,
    ) -> Result<ComponentLoadOutcome> {
        let (component, wasm_bytes) = self
            .load_component_optimized(wasm_path, component_id)
            .await?;

        let instance_pre = self
            .runtime
            .instantiate_pre(&component)
            .map_err(anyhow::Error::from)
            .context("failed to instantiate component")?;

        // Extract package docs from wasm bytes
        let package_docs = extract_package_docs(&wasm_bytes);

        let component_instance = ComponentInstance {
            component: Arc::new(component),
            instance_pre: Arc::new(instance_pre),
            package_docs: package_docs.clone(),
        };

        // Use package docs if available
        let tool_metadata = if let Some(ref docs) = package_docs {
            component_exports_to_tools_with_docs(
                &component_instance.component,
                self.runtime.as_ref(),
                true,
                docs,
            )
        } else {
            component_exports_to_tools(&component_instance.component, self.runtime.as_ref(), true)
        };

        let tool_names: Vec<String> = tool_metadata
            .iter()
            .map(|tool| tool.normalized_name.clone())
            .collect();

        if let Ok(validation_stamp) = self.storage.create_validation_stamp(wasm_path, false).await {
            if let Err(e) = self
                .save_component_metadata(component_id, &tool_metadata, validation_stamp)
                .await
            {
                warn!(%component_id, error = %format_error_chain(&e), "Failed to save component metadata");
            }
        }

        let load_result = self
            .registry
            .upsert_component(component_id.to_string(), component_instance, tool_metadata)
            .await?;

        if let Err(error) = self.policy_manager.restore_from_disk(component_id).await {
            warn!(%component_id, error = %format_error_chain(&error), "Failed to restore policy attachment");
        }

        Ok(ComponentLoadOutcome {
            component_id: component_id.to_string(),
            status: load_result,
            tool_names,
        })
    }

    /// Loads a new component from the given URI. This URI can be a file path, an OCI reference, or a URL.
    ///
    /// If a component with the given id already exists, it will be updated with the new component.
    /// Returns rich [`ComponentLoadOutcome`] information describing the loaded
    /// component and whether it replaced an existing instance.
    ///
    /// Staging replaces the component's `.wasm`, metadata and precompiled cache on disk, so it
    /// has to happen inside the same critical section as the compilation that reads them back.
    /// The guard is therefore taken before staging and held until the component is registered:
    /// otherwise an explicit reload could swap the artifact out from under a concurrent
    /// on-demand load or background restore, which would then compile a replaced artifact or
    /// deserialize a precompiled cache that no longer matches it.
    #[instrument(skip(self))]
    pub async fn load_component(&self, uri: &str) -> Result<ComponentLoadOutcome> {
        debug!(uri, "Loading component");
        // Resolving the resource is what yields the component id, so it necessarily happens
        // before the guard exists. It only writes to the downloads directory, never to the
        // component's own artifacts.
        let (component_id, resource) = self.resolve_component_resource(uri).await?;

        let guard = self.load_guard(&component_id).await;
        let _guard = guard.lock().await;

        let staged_path = self
            .stage_component_artifact(&component_id, resource)
            .await?;
        // We hold the guard, so this must be the `_locked` form; the guard is not reentrant.
        // A reload always recompiles: there is deliberately no registry recheck here.
        let outcome = self
            .compile_and_register_component_locked(&component_id, &staged_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to compile component from path: {}. Please ensure the file is a valid WebAssembly component.",
                    staged_path.display()
                )
            })?;

        info!(
            component_id = %outcome.component_id,
            status = ?outcome.status,
            tools = ?outcome.tool_names,
            "Successfully loaded component"
        );
        Ok(outcome)
    }

    /// Unloads the component with the specified id. This removes the component from the runtime
    /// and removes all associated files from disk, making it the reverse operation of load_component.
    /// This function fails if any files cannot be removed (except when they don't exist).
    ///
    /// Removing the artifacts and cleaning up the registry happen under the same per-component
    /// guard that loads take, so an unload cannot interleave with a load of the same component.
    /// Without it a concurrent on-demand load could re-register the component and rewrite its
    /// metadata and precompiled cache after the unload had already reported success, leaving a
    /// component registered with no files behind it.
    #[instrument(skip(self))]
    pub async fn unload_component(&self, id: &str) -> Result<()> {
        debug!("Unloading component and removing files from disk");

        // Nothing reached from here takes the guard again, so this cannot deadlock. The guard
        // map holds weak references, so the entry this creates dies with `guard` and is swept
        // by the next load that has to mint a mutex.
        let guard = self.load_guard(id).await;
        let _guard = guard.lock().await;

        // Remove files first, then clean up memory on success
        self.storage.remove_component_artifacts(id).await?;

        let policy_path = self.get_component_policy_path(id);
        self.storage
            .remove_if_exists(&policy_path, "policy file", id)
            .await?;

        let metadata_path = self.get_component_metadata_path(id);
        self.storage
            .remove_if_exists(&metadata_path, "policy metadata file", id)
            .await?;

        // Only cleanup memory after all files are successfully removed
        self.registry.remove_component(id).await;
        self.policy_manager.cleanup(id).await;

        info!(component_id = %id, "Component unloaded successfully");
        Ok(())
    }

    /// Returns the component ID for a given tool name.
    /// If there are multiple components with the same tool name, returns an error.
    #[instrument(skip(self))]
    pub async fn get_component_id_for_tool(&self, tool_name: &str) -> Result<String> {
        let tool_infos = self
            .registry
            .tool_infos(tool_name)
            .await
            .context("Tool not found")?;

        if tool_infos.len() > 1 {
            bail!(
                "Multiple components found for tool '{}': {}",
                tool_name,
                tool_infos
                    .iter()
                    .map(|info| info.component_id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        Ok(tool_infos[0].component_id.clone())
    }

    /// Lists all available tools across all components
    #[instrument(skip(self))]
    pub async fn list_tools(&self) -> Vec<Value> {
        self.registry.list_tools().await
    }

    /// Returns the schema for a specific tool owned by a component, if available
    #[instrument(skip(self))]
    pub async fn get_tool_schema_for_component(
        &self,
        component_id: &str,
        tool_name: &str,
    ) -> Option<Value> {
        let tool_infos = self.registry.tool_infos(tool_name).await?;
        tool_infos
            .iter()
            .find(|info| info.component_id == component_id)
            .map(|info| info.schema.clone())
    }

    /// Returns the requested component. Returns `None` if the component is not found.
    #[instrument(skip(self))]
    pub async fn get_component(&self, component_id: &str) -> Option<ComponentInstance> {
        self.registry.get_component(component_id).await
    }

    /// Lists all loaded components by their IDs
    #[instrument(skip(self))]
    pub async fn list_components(&self) -> Vec<String> {
        self.registry.list_components().await
    }

    /// Lists all known components by ID (union of loaded components and any
    /// `*.wasm` files present in the component directory). Does not compile components.
    #[instrument(skip(self))]
    pub async fn list_components_known(&self) -> Vec<String> {
        use std::collections::HashSet;
        let loaded = self.registry.list_components().await;
        let mut set: HashSet<String> = loaded.into_iter().collect();

        if let Ok(entries) = std::fs::read_dir(self.storage.root()) {
            for entry in entries.flatten() {
                let path = entry.path();

                // 1) Detect regular .wasm files
                let is_wasm = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| ext.eq_ignore_ascii_case("wasm"))
                    .unwrap_or(false);
                if is_wasm {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        set.insert(stem.to_string());
                        continue;
                    }
                }

                // 2) Detect metadata files ("<id>.metadata.json")
                if let Some(fname) = path.file_name().and_then(|s| s.to_str()) {
                    if fname.ends_with(&format!(".{METADATA_EXT}")) {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            if let Ok(meta) = serde_json::from_str::<ComponentMetadata>(&content) {
                                set.insert(meta.component_id);
                            }
                        }
                    }
                }
            }
        }

        let mut v: Vec<String> = set.into_iter().collect();
        v.sort();
        v
    }

    /// Gets the schema for a specific component
    #[instrument(skip(self))]
    pub async fn get_component_schema(&self, component_id: &str) -> Option<Value> {
        // Prefer live component schema if loaded
        if let Some(component_instance) = self.get_component(component_id).await {
            return Some(
                if let Some(ref package_docs) = component_instance.package_docs {
                    component_exports_to_json_schema_with_docs(
                        &component_instance.component,
                        self.runtime.as_ref(),
                        true,
                        package_docs,
                    )
                } else {
                    component_exports_to_json_schema(
                        &component_instance.component,
                        self.runtime.as_ref(),
                        true,
                    )
                },
            );
        }

        // Fallback to metadata-based schema without compiling the component
        match self.load_component_metadata(component_id).await {
            Ok(Some(metadata)) => {
                let component_path = self.component_path(component_id);
                if !ComponentStorage::validate_stamp(&component_path, &metadata.validation_stamp)
                    .await
                {
                    return None;
                }

                let tools: Vec<Value> = metadata
                    .tool_schemas
                    .into_iter()
                    .map(|schema| schema::canonicalize_tool_schema(&schema))
                    .collect();
                Some(serde_json::json!({
                    "tools": tools
                }))
            }
            _ => None,
        }
    }

    fn component_path(&self, component_id: &str) -> PathBuf {
        self.storage.component_path(component_id)
    }

    /// Get the path to precompiled component file
    fn component_precompiled_path(&self, component_id: &str) -> PathBuf {
        self.storage.precompiled_path(component_id)
    }

    pub(crate) fn get_component_policy_path(&self, component_id: &str) -> PathBuf {
        self.policy_manager.policy_path(component_id)
    }

    pub(crate) fn get_component_metadata_path(&self, component_id: &str) -> PathBuf {
        self.policy_manager.metadata_path(component_id)
    }

    /// Attach a policy to a component by URI.
    pub async fn attach_policy(&self, component_id: &str, policy_uri: &str) -> Result<()> {
        if !self.registry.contains_component(component_id).await {
            return Err(anyhow!("Component not found: {}", component_id));
        }
        self.policy_manager
            .attach_policy(component_id, policy_uri)
            .await
    }

    /// Detach any policy associated with the given component.
    pub async fn detach_policy(&self, component_id: &str) -> Result<()> {
        self.policy_manager.detach_policy(component_id).await
    }

    /// Retrieve policy metadata for a component if one is attached.
    pub async fn get_policy_info(&self, component_id: &str) -> Option<PolicyInfo> {
        self.policy_manager.get_policy_info(component_id).await
    }

    /// Grant a specific permission rule to a component.
    #[instrument(skip(self))]
    pub async fn grant_permission(
        &self,
        component_id: &str,
        permission_type: &str,
        details: &serde_json::Value,
    ) -> Result<()> {
        if !self.registry.contains_component(component_id).await {
            return Err(anyhow!("Component not found: {}", component_id));
        }
        self.policy_manager
            .grant_permission(component_id, permission_type, details)
            .await
    }

    /// Revoke a specific permission rule from a component.
    #[instrument(skip(self))]
    pub async fn revoke_permission(
        &self,
        component_id: &str,
        permission_type: &str,
        details: &serde_json::Value,
    ) -> Result<()> {
        if !self.registry.contains_component(component_id).await {
            return Err(anyhow!("Component not found: {}", component_id));
        }
        self.policy_manager
            .revoke_permission(component_id, permission_type, details)
            .await
    }

    /// Reset all permissions for a component to defaults.
    #[instrument(skip(self))]
    pub async fn reset_permission(&self, component_id: &str) -> Result<()> {
        if !self.registry.contains_component(component_id).await {
            return Err(anyhow!("Component not found: {}", component_id));
        }
        self.policy_manager.reset_permission(component_id).await
    }

    /// Revoke storage permission for a specific URI.
    #[instrument(skip(self))]
    pub async fn revoke_storage_permission_by_uri(
        &self,
        component_id: &str,
        uri: &str,
    ) -> Result<()> {
        if !self.registry.contains_component(component_id).await {
            return Err(anyhow!("Component not found: {}", component_id));
        }
        self.policy_manager
            .revoke_storage_permission_by_uri(component_id, uri)
            .await
    }

    /// Returns the component directory root on disk.
    pub fn component_root(&self) -> &Path {
        self.storage.root()
    }

    /// Ensure a specific component is loaded (compiled and instantiated) by its ID.
    /// If it's already loaded, this is a no-op. If the wasm file is not present in
    /// the component directory, an error is returned.
    ///
    /// Concurrent calls for the same component are serialized against each other and
    /// against the background restore, so the component is compiled and registered once
    /// no matter how many callers race. Calls for different components still proceed in
    /// parallel.
    #[instrument(skip(self))]
    pub async fn ensure_component_loaded(&self, component_id: &str) -> Result<()> {
        if self.registry.contains_component(component_id).await {
            return Ok(());
        }

        // Take the guard before looking at the artifact. An explicit reload of this component
        // stages its replacement under this same guard, and staging removes the old `.wasm`
        // before copying the new one in, so for part of a reload the artifact is legitimately
        // absent. Testing the path first would report the component as missing instead of
        // waiting for the reload that is already producing it.
        //
        // The original ordering existed so an unknown component id could not grow the guard
        // map. It no longer has to: the map holds `Weak` entries, so the entry minted here
        // dies with the `Arc` this function holds, and the next caller that has to mint a
        // mutex sweeps every dead entry before inserting. An id that is never loaded therefore
        // leaves nothing behind, and repeated lookups of unknown ids cannot accumulate.
        let guard = self.load_guard(component_id).await;
        let _guard = guard.lock().await;

        // Another caller, including the background restore, may have finished the load
        // while we waited for the guard.
        if self.registry.contains_component(component_id).await {
            return Ok(());
        }

        // An unload of this component may have completed while we waited for the guard, or it
        // may never have existed. Either way there is no artifact to compile.
        let entry_path = self.component_path(component_id);
        if !entry_path.exists() {
            bail!("Component not found: {}", component_id);
        }

        self.compile_and_register_component_locked(component_id, &entry_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to compile component from path: {}",
                    entry_path.display()
                )
            })?;

        Ok(())
    }

    /// Save component metadata to disk
    async fn save_component_metadata(
        &self,
        component_id: &str,
        tool_metadata: &[ToolMetadata],
        validation_stamp: ValidationStamp,
    ) -> Result<()> {
        let metadata = ComponentMetadata {
            component_id: component_id.to_string(),
            tool_schemas: tool_metadata.iter().map(|t| t.schema.clone()).collect(),
            function_identifiers: tool_metadata.iter().map(|t| t.identifier.clone()).collect(),
            tool_names: tool_metadata
                .iter()
                .map(|t| t.normalized_name.clone())
                .collect(),
            validation_stamp,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };

        self.storage.write_metadata(&metadata).await?;

        info!(component_id = %component_id, "Saved component metadata");
        Ok(())
    }

    /// Load component metadata from disk
    async fn load_component_metadata(
        &self,
        component_id: &str,
    ) -> Result<Option<ComponentMetadata>> {
        self.storage.read_metadata(component_id).await
    }

    /// Save precompiled component to disk
    async fn save_precompiled_component(
        &self,
        component_id: &str,
        wasm_bytes: &[u8],
    ) -> Result<()> {
        let precompiled_data = self
            .runtime
            .precompile_component(wasm_bytes)
            .map_err(anyhow::Error::from)
            .context("Failed to precompile component")?;

        self.storage
            .write_precompiled(component_id, &precompiled_data)
            .await?;

        info!(component_id = %component_id, "Saved precompiled component");
        Ok(())
    }

    /// Load component from precompiled cache or compile fresh
    async fn load_component_optimized(
        &self,
        wasm_path: &Path,
        component_id: &str,
    ) -> Result<(Component, Vec<u8>)> {
        let precompiled_path = self.component_precompiled_path(component_id);

        // Try to load from precompiled cache first
        if precompiled_path.exists() {
            match unsafe { Component::deserialize_file(self.runtime.as_ref(), &precompiled_path) } {
                Ok(component) => {
                    debug!(component_id = %component_id, "Loaded component from precompiled cache");
                    // Still need the wasm bytes for metadata/validation
                    let wasm_bytes = tokio::fs::read(wasm_path)
                        .await
                        .context("Failed to read wasm file")?;
                    return Ok((component, wasm_bytes));
                }
                Err(e) => {
                    warn!(%component_id, error = %format_error_chain(&e), "Failed to load precompiled component, falling back to compilation");
                }
            }
        }

        // Fall back to compilation
        let wasm_bytes = tokio::fs::read(wasm_path)
            .await
            .context("Failed to read wasm file")?;

        let component = Component::new(self.runtime.as_ref(), &wasm_bytes)
            .map_err(anyhow::Error::from)
            .context("Failed to compile component")?;

        // Save precompiled version for next time (async, don't block on this)
        if let Err(e) = self
            .save_precompiled_component(component_id, &wasm_bytes)
            .await
        {
            warn!(%component_id, error = %format_error_chain(&e), "Failed to save precompiled component");
        }

        debug!(component_id = %component_id, "Compiled component and saved to cache");
        Ok((component, wasm_bytes))
    }

    async fn get_wasi_state_for_component(
        &self,
        component_id: &str,
    ) -> Result<(WassetteWasiState<WasiState>, Option<CustomResourceLimiter>)> {
        let policy_template = self
            .policy_manager
            .template_for_component(component_id)
            .await;

        let wasi_state = policy_template.build()?;
        let allowed_hosts = policy_template.allowed_hosts.clone();
        let resource_limiter = wasi_state.resource_limiter.clone();

        let wassette_wasi_state = WassetteWasiState::new(wasi_state, allowed_hosts)?;
        Ok((wassette_wasi_state, resource_limiter))
    }

    /// Executes a function call on a WebAssembly component
    #[instrument(skip(self))]
    pub async fn execute_component_call(
        &self,
        component_id: &str,
        function_name: &str,
        parameters: &str,
    ) -> Result<String> {
        let start_time = Instant::now();

        debug!(
            component_id = %component_id,
            function_name = %function_name,
            "Starting WebAssembly component execution"
        );

        let component = self
            .get_component(component_id)
            .await
            .ok_or_else(|| anyhow!("Component not found: {}", component_id))?;

        let (state, resource_limiter) = self.get_wasi_state_for_component(component_id).await?;

        let mut store = Store::new(self.runtime.as_ref(), state);

        // Apply memory limits if configured in the policy by setting up a limiter closure
        // that extracts the resource limiter from the WasiState
        if resource_limiter.is_some() {
            store.limiter(|state: &mut WassetteWasiState<WasiState>| {
                // Extract the resource limiter from the inner state
                state
                    .inner
                    .resource_limiter
                    .as_mut()
                    .expect("Resource limiter should be present - checked above")
            });
        }

        let instantiation_start = Instant::now();
        let instance = component.instance_pre.instantiate_async(&mut store).await?;
        let instantiation_duration = instantiation_start.elapsed();

        debug!(
            component_id = %component_id,
            instantiation_ms = %instantiation_duration.as_millis(),
            "Component instance created"
        );

        // Use the new function identifier lookup instead of dot-splitting
        let function_id = self
            .registry
            .tool_identifier(function_name)
            .await
            .ok_or_else(|| anyhow!("Unknown tool name: {}", function_name))?;

        let (interface_name, func_name) = (
            function_id.interface_name.as_deref().unwrap_or(""),
            &function_id.function_name,
        );

        let func = if !interface_name.is_empty() {
            let interface_index = instance
                .get_export_index(&mut store, None, interface_name)
                .ok_or_else(|| anyhow!("Interface not found: {}", interface_name))?;

            let function_index = instance
                .get_export_index(&mut store, Some(&interface_index), func_name)
                .ok_or_else(|| {
                    anyhow!(
                        "Function not found in interface: {}.{}",
                        interface_name,
                        func_name
                    )
                })?;

            instance
                .get_func(&mut store, function_index)
                .ok_or_else(|| {
                    anyhow!(
                        "Function not found in interface: {}.{}",
                        interface_name,
                        func_name
                    )
                })?
        } else {
            let func_index = instance
                .get_export_index(&mut store, None, func_name)
                .ok_or_else(|| anyhow!("Function not found: {}", func_name))?;
            instance
                .get_func(&mut store, func_index)
                .ok_or_else(|| anyhow!("Function not found: {}", func_name))?
        };

        let params: serde_json::Value = serde_json::from_str(parameters)?;
        let func_type = func.ty(&store);
        let parameter_types = func_type
            .params()
            .map(|(name, ty)| (name.to_string(), ty))
            .collect::<Vec<_>>();
        let argument_vals = json_to_vals(&params, &parameter_types)?;

        let result_types = func_type.results().collect::<Vec<_>>();
        let mut results = create_placeholder_results(&result_types);

        let execution_start = Instant::now();

        // Execute the WASM function and capture any errors
        let call_result = func
            .call_async(&mut store, &argument_vals, &mut results)
            .await;

        let execution_duration = execution_start.elapsed();

        // If the call failed, check if it was due to a permission denial
        if let Err(e) = call_result {
            // Check if there was a permission error recorded during execution
            if let Some(perm_error) = store.data().get_last_permission_error() {
                // Return a more informative error with instructions
                return Err(anyhow!(perm_error.to_user_message(component_id)));
            }
            // Otherwise, return the original WASM execution error
            return Err(e.into());
        }

        let result_json = vals_to_json(&results);

        let total_duration = start_time.elapsed();

        debug!(
            component_id = %component_id,
            function_name = %function_name,
            total_duration_ms = %total_duration.as_millis(),
            instantiation_ms = %instantiation_duration.as_millis(),
            execution_ms = %execution_duration.as_millis(),
            "WebAssembly component execution completed"
        );

        if let Some(result_str) = result_json.as_str() {
            Ok(result_str.to_string())
        } else {
            Ok(serde_json::to_string(&result_json)?)
        }
    }

    /// Load existing components from component directory in the background with bounded parallelism
    /// Default concurrency is min(num_cpus, 4) if not specified
    #[instrument(skip(self, notify_fn))]
    pub async fn load_existing_components_async<F>(
        &self,
        concurrency: Option<usize>,
        notify_fn: Option<F>,
    ) -> Result<()>
    where
        F: Fn() + Send + Sync + 'static,
    {
        // First phase: Quick metadata-based registry population
        self.populate_registry_from_metadata().await?;

        let concurrency = concurrency.unwrap_or_else(|| std::cmp::min(num_cpus::get(), 4));

        info!(
            "Starting background component loading with concurrency: {}",
            concurrency
        );

        let semaphore = Arc::new(Semaphore::new(concurrency));
        let mut entries = tokio::fs::read_dir(self.storage.root()).await?;
        let mut load_futures = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let self_clone = self.clone();
            let semaphore = semaphore.clone();
            let notify_fn = notify_fn.as_ref().map(std::sync::Arc::new);

            let future = async move {
                let _permit = semaphore.acquire().await.unwrap();

                match self_clone.load_component_from_entry_optimized(entry).await {
                    Ok(true) => {
                        // Component was loaded, notify if callback provided
                        if let Some(notify) = notify_fn {
                            notify();
                        }
                    }
                    Ok(false) => {} // No component to load (not a .wasm file)
                    Err(e) => warn!("Failed to load component: {:#}", e),
                }
            };
            load_futures.push(future);
        }

        // Wait for all components to load
        futures::future::join_all(load_futures).await;
        info!("Background component loading completed");
        Ok(())
    }

    /// Populate tool registry from cached metadata without compiling components
    /// Registers tool metadata for every component on disk, without compiling anything.
    ///
    /// This makes tool names resolvable in a process that will never run the background
    /// restore, such as a one-shot CLI invocation. Components are registered from their
    /// cached metadata only when the validation stamp still matches, so a stale entry is
    /// skipped rather than trusted. Compilation still happens later, on first use.
    ///
    /// The pass has two phases. Each component's cached metadata is read and validated while
    /// holding that component's load guard, so this cannot race a load or unload of the same
    /// component; the guard is released before the next entry, so unrelated components are
    /// never serialized against each other. What every validated component produces is then
    /// published in a single registry write.
    ///
    /// The write is batched because the server starts answering requests while this runs, and
    /// registering one component at a time exposes a `tool_map` in which a tool exported by
    /// two installed components has only one owner. A lookup landing there resolves and runs
    /// the tool that the collision rule should refuse, and nothing revisits the tool map to
    /// take the answer back. With one write, a lookup sees either no hydrated component or
    /// all of them, and the collision is visible in both. The cost is that hydration becomes
    /// visible only once every component has been validated; validation reads metadata and
    /// stats an artifact, and compiles nothing.
    pub async fn populate_registry_from_metadata(&self) -> Result<()> {
        let mut entries = tokio::fs::read_dir(self.storage.root()).await?;
        let mut pending = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let entry_path = entry.path();
            let is_wasm = entry_path
                .extension()
                .map(|ext| ext == "wasm")
                .unwrap_or(false);

            if !is_wasm {
                continue;
            }

            let Some(component_id) = entry_path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };

            if let Some(prepared) = self
                .prepare_cached_metadata_under_guard(component_id, &entry_path)
                .await
            {
                pending.push(prepared);
            }
        }

        let loaded_count = self
            .registry
            .register_cached_metadata_batch(pending, |path| path.exists())
            .await;

        if loaded_count > 0 {
            info!(
                "Registered {} components from cached metadata",
                loaded_count
            );
        }

        Ok(())
    }

    /// Read and validate one component's cached tool metadata while holding its load guard.
    ///
    /// Reading the metadata and checking its validation stamp is a check-then-act over the
    /// very files a reload replaces and an unload removes, so both happen inside one critical
    /// section. Without the guard this could read the metadata of one artifact and stamp it
    /// against another that a reload has just staged in its place.
    ///
    /// The guard is not held until the result is registered, because the registration is
    /// batched with every other component's and holding one component's guard across another
    /// component's work would serialize ids that have nothing to do with each other. That is
    /// safe because [`ComponentRegistry::register_cached_metadata_batch`] rechecks the
    /// artifact inside its own write, which is what actually rules out reviving a component
    /// an unload has removed.
    ///
    /// Nothing reached from here takes the guard again, so this cannot deadlock. The guard is
    /// dropped when this returns, so the caller's loop does not hold one component's guard
    /// while handling the next.
    ///
    /// Returns the registration to publish, or `None` if there is nothing usable to register.
    async fn prepare_cached_metadata_under_guard(
        &self,
        component_id: &str,
        entry_path: &Path,
    ) -> Option<PendingRegistration> {
        let guard = self.load_guard(component_id).await;
        let _guard = guard.lock().await;

        // An unload may have completed while we waited for the guard. Its artifact is gone, so
        // there is nothing to register, whatever cached metadata may still be lying around.
        if !entry_path.exists() {
            debug!(component_id = %component_id, "Component removed while waiting for the load guard");
            return None;
        }

        let Ok(Some(metadata)) = self.load_component_metadata(component_id).await else {
            debug!(component_id = %component_id, "No valid cached metadata found, will load component later");
            return None;
        };

        // Validate that the component file hasn't changed
        if !ComponentStorage::validate_stamp(entry_path, &metadata.validation_stamp).await {
            debug!(component_id = %component_id, "No valid cached metadata found, will load component later");
            return None;
        }

        let tools: Vec<ToolMetadata> = metadata
            .function_identifiers
            .into_iter()
            .zip(metadata.tool_schemas)
            .zip(metadata.tool_names)
            .map(|((identifier, schema), normalized_name)| {
                let canonical = schema::canonicalize_tool_schema(&schema);
                ToolMetadata {
                    identifier,
                    schema: canonical,
                    normalized_name,
                }
            })
            .collect();

        Some(PendingRegistration {
            component_id: component_id.to_string(),
            artifact_path: entry_path.to_path_buf(),
            tools,
        })
    }

    /// Load a component from directory entry with optimization
    async fn load_component_from_entry_optimized(&self, entry: DirEntry) -> Result<bool> {
        let entry_path = entry.path();
        let is_file = entry
            .metadata()
            .await
            .map(|m| m.is_file())
            .context("unable to read file metadata")?;
        let is_wasm = entry_path
            .extension()
            .map(|ext| ext == "wasm")
            .unwrap_or(false);
        if !(is_file && is_wasm) {
            return Ok(false);
        }

        let component_id = entry_path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(String::from)
            .context("wasm file didn't have a valid file name")?;

        if self.registry.contains_component(&component_id).await {
            debug!(component_id = %component_id, "Component already loaded in memory");
            return Ok(false);
        }

        // Serialize against the on-demand load path: a `tools/call` that arrives before the
        // restore reaches this component compiles it itself, and without the shared guard
        // both passes would compile it and write the same metadata and cache files.
        let guard = self.load_guard(&component_id).await;
        let _guard = guard.lock().await;

        if self.registry.contains_component(&component_id).await {
            debug!(component_id = %component_id, "Component loaded while waiting for the load guard");
            return Ok(false);
        }

        // An unload may have removed this component while we waited for the guard, in which
        // case there is nothing left to restore.
        if !entry_path.exists() {
            debug!(component_id = %component_id, "Component removed while waiting for the load guard");
            return Ok(false);
        }

        let start_time = Instant::now();
        self.compile_and_register_component_locked(&component_id, &entry_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to compile component from path: {}",
                    entry_path.display()
                )
            })?;

        info!(component_id = %component_id, elapsed = ?start_time.elapsed(), "component loaded");
        Ok(true)
    }

    // Granular permission system methods
}

impl LifecycleManager {
    /// Get the secrets manager
    pub fn secrets_manager(&self) -> &SecretsManager {
        &self.secrets_manager
    }

    /// List secrets for a component
    pub async fn list_component_secrets(
        &self,
        component_id: &str,
        show_values: bool,
    ) -> Result<std::collections::HashMap<String, Option<String>>> {
        self.secrets_manager
            .list_component_secrets(component_id, show_values)
            .await
    }

    /// Set secrets for a component
    pub async fn set_component_secrets(
        &self,
        component_id: &str,
        secrets: &[(String, String)],
    ) -> Result<()> {
        // Check if component exists in the component directory
        let component_path = self.component_path(component_id);
        if !component_path.exists() {
            bail!("Component not found: {}", component_id);
        }

        self.secrets_manager
            .set_component_secrets(component_id, secrets)
            .await
    }

    /// Delete secrets for a component
    pub async fn delete_component_secrets(
        &self,
        component_id: &str,
        keys: &[String],
    ) -> Result<()> {
        self.secrets_manager
            .delete_component_secrets(component_id, keys)
            .await
    }

    /// Load secrets for a component as environment variables
    pub async fn load_component_secrets(
        &self,
        component_id: &str,
    ) -> Result<std::collections::HashMap<String, String>> {
        self.secrets_manager
            .load_component_secrets(component_id)
            .await
    }
}

async fn load_component_from_entry(
    runtime: Arc<RuntimeContext>,
    entry: DirEntry,
) -> Result<Option<(ComponentInstance, String)>> {
    let start_time = Instant::now();
    let is_file = entry
        .metadata()
        .await
        .map(|m| m.is_file())
        .context("unable to read file metadata")?;
    let is_wasm = entry
        .path()
        .extension()
        .map(|ext| ext == "wasm")
        .unwrap_or(false);
    if !(is_file && is_wasm) {
        return Ok(None);
    }
    let entry_path = entry.path();

    // Read wasm bytes to extract package docs
    let wasm_bytes = tokio::fs::read(&entry_path)
        .await
        .context("Failed to read wasm file")?;

    // Extract package docs before spawning blocking task
    let package_docs = extract_package_docs(&wasm_bytes);

    let runtime_for_component = Arc::clone(&runtime);
    let component = tokio::task::spawn_blocking(move || {
        Component::from_file(runtime_for_component.as_ref(), entry_path)
    })
    .await??;
    let name = entry
        .path()
        .file_stem()
        .and_then(|s| s.to_str())
        .map(String::from)
        .context("wasm file didn't have a valid file name")?;
    info!(component_id = %name, elapsed = ?start_time.elapsed(), "component loaded");
    let instance_pre = runtime.instantiate_pre(&component)?;
    Ok(Some((
        ComponentInstance {
            component: Arc::new(component),
            instance_pre: Arc::new(instance_pre),
            package_docs,
        },
        name,
    )))
}

#[cfg(test)]
mod tests {
    use std::ops::Deref;
    use std::path::PathBuf;
    use std::process::Command;

    use policy::PolicyParser;
    use test_log::test;

    use super::*;

    pub(crate) const TEST_COMPONENT_ID: &str = "fetch_rs";

    /// Helper struct for keeping a reference to the temporary directory used for testing the
    /// lifecycle manager
    pub(crate) struct TestLifecycleManager {
        pub manager: LifecycleManager,
        _tempdir: tempfile::TempDir,
    }

    impl TestLifecycleManager {
        pub async fn load_test_component(&self) -> Result<()> {
            let component_path = build_example_component().await?;

            self.manager
                .load_component(&format!("file://{}", component_path.to_str().unwrap()))
                .await?;

            Ok(())
        }
    }

    impl Deref for TestLifecycleManager {
        type Target = LifecycleManager;

        fn deref(&self) -> &Self::Target {
            &self.manager
        }
    }

    pub(crate) async fn create_test_manager() -> Result<TestLifecycleManager> {
        let tempdir = tempfile::tempdir()?;
        let manager = LifecycleManager::new(&tempdir).await?;
        Ok(TestLifecycleManager {
            manager,
            _tempdir: tempdir,
        })
    }

    pub(crate) async fn build_example_component() -> Result<PathBuf> {
        let cwd = std::env::current_dir()?;
        println!("CWD: {}", cwd.display());
        let component_path =
            cwd.join("../../examples/fetch-rs/target/wasm32-wasip2/release/fetch_rs.wasm");

        if !component_path.exists() {
            let status = Command::new("cargo")
                .current_dir(cwd.join("../../examples/fetch-rs"))
                .args(["build", "--release", "--target", "wasm32-wasip2"])
                .status()
                .context("Failed to execute cargo component build")?;

            if !status.success() {
                anyhow::bail!("Failed to compile fetch-rs component");
            }
        }

        if !component_path.exists() {
            anyhow::bail!(
                "Component file not found after build: {}",
                component_path.display()
            );
        }

        Ok(component_path)
    }

    #[test(tokio::test)]
    async fn test_lifecycle_manager_tool_registry() -> Result<()> {
        let manager = create_test_manager().await?;

        let temp_dir = tempfile::tempdir()?;
        let component_path = temp_dir.path().join("mock_component.wasm");
        std::fs::write(&component_path, b"mock wasm bytes")?;

        let load_result = manager
            .load_component(component_path.to_str().unwrap())
            .await;
        assert!(load_result.is_err()); // Expected since we're using invalid WASM

        let lookup_result = manager.get_component_id_for_tool("non-existent").await;
        assert!(lookup_result.is_err());

        Ok(())
    }

    #[test(tokio::test)]
    async fn test_new_manager() -> Result<()> {
        let _manager = create_test_manager().await?;
        Ok(())
    }

    #[test(tokio::test)]
    async fn test_load_and_unload_component() -> Result<()> {
        let manager = create_test_manager().await?;

        let load_result = manager.load_component("/path/to/nonexistent").await;
        assert!(load_result.is_err());

        manager.load_test_component().await?;

        let loaded_components = manager.list_components().await;
        assert_eq!(loaded_components.len(), 1);

        manager.unload_component(TEST_COMPONENT_ID).await?;

        let loaded_components = manager.list_components().await;
        assert!(loaded_components.is_empty());

        Ok(())
    }

    #[test(tokio::test)]
    async fn test_get_component() -> Result<()> {
        let manager = create_test_manager().await?;
        assert!(manager.get_component("non-existent").await.is_none());

        manager.load_test_component().await?;

        manager
            .get_component(TEST_COMPONENT_ID)
            .await
            .expect("Should be able to get a component we just loaded");
        Ok(())
    }

    #[test(tokio::test)]
    async fn test_duplicate_component_id() -> Result<()> {
        let manager = create_test_manager().await?;

        manager.load_test_component().await?;

        let components = manager.list_components().await;
        assert_eq!(components.len(), 1);
        assert_eq!(components[0], TEST_COMPONENT_ID);

        // Load again and make sure we still only have one

        manager.load_test_component().await?;
        let components = manager.list_components().await;
        assert_eq!(components.len(), 1);
        assert_eq!(components[0], TEST_COMPONENT_ID);

        Ok(())
    }

    #[test(tokio::test)]
    async fn test_component_reload() -> Result<()> {
        let manager = create_test_manager().await?;
        let component_path = build_example_component().await?;

        manager
            .load_component(&format!("file://{}", component_path.to_str().unwrap()))
            .await?;

        let component_id = manager.get_component_id_for_tool("fetch").await?;
        assert_eq!(component_id, TEST_COMPONENT_ID);

        manager
            .load_component(&format!("file://{}", component_path.to_str().unwrap()))
            .await?;

        let component_id = manager.get_component_id_for_tool("fetch").await?;
        assert_eq!(component_id, TEST_COMPONENT_ID);

        Ok(())
    }

    #[test(tokio::test)]
    async fn test_component_path_update() -> Result<()> {
        let manager = create_test_manager().await?;

        let component_id = "test-component";
        let expected_path = manager.component_root().join("test-component.wasm");
        let actual_path = manager.component_path(component_id);

        assert_eq!(actual_path, expected_path);
        Ok(())
    }

    #[test(tokio::test)]
    async fn test_cached_tool_schema_preserves_tool_fields() -> Result<()> {
        let manager = create_test_manager().await?;
        let component_id = "cached-component";
        let component_path = manager.component_path(component_id);
        tokio::fs::write(&component_path, b"cached component").await?;

        let tool_schema = serde_json::json!({
            "name": "cached-tool",
            "description": "A cached tool",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "value": {"type": "string"}
                },
                "required": ["value"]
            },
            "outputSchema": {
                "type": "object",
                "properties": {
                    "result": {"type": "string"}
                },
                "required": ["result"]
            }
        });
        let metadata = ComponentMetadata {
            component_id: component_id.to_string(),
            tool_schemas: vec![tool_schema],
            function_identifiers: vec![FunctionIdentifier {
                package_name: None,
                interface_name: None,
                function_name: "cached-tool".to_string(),
            }],
            tool_names: vec!["cached-tool".to_string()],
            validation_stamp: manager
                .storage
                .create_validation_stamp(&component_path, false)
                .await?,
            created_at: 0,
        };
        manager.storage.write_metadata(&metadata).await?;

        let component_schema = manager
            .get_component_schema(component_id)
            .await
            .context("cached component schema should be available")?;
        let cached_tool = &component_schema["tools"][0];
        assert_eq!(cached_tool["name"], "cached-tool");
        assert_eq!(cached_tool["description"], "A cached tool");
        assert_eq!(
            cached_tool["inputSchema"],
            metadata.tool_schemas[0]["inputSchema"]
        );
        assert_eq!(
            cached_tool["outputSchema"],
            serde_json::json!({
                "type": "object",
                "properties": {
                    "result": {"type": "string"}
                },
                "required": ["result"]
            })
        );

        manager.populate_registry_from_metadata().await?;
        let registered_tools = manager.list_tools().await;
        assert_eq!(registered_tools, vec![cached_tool.clone()]);

        tokio::fs::write(&component_path, b"changed component").await?;
        assert!(manager.get_component_schema(component_id).await.is_none());

        Ok(())
    }

    /// Counts `tracing` events whose message contains `needle`, which lets a test observe
    /// how many times a code path ran without changing the code under test.
    #[derive(Clone)]
    struct MessageCounter {
        needle: &'static str,
        count: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl MessageCounter {
        fn new(needle: &'static str) -> Self {
            Self {
                needle,
                count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }
        }

        fn count(&self) -> usize {
            self.count.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for MessageCounter {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            struct MessageVisitor<'a> {
                needle: &'a str,
                matched: bool,
            }

            impl tracing::field::Visit for MessageVisitor<'_> {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    if field.name() == "message" && format!("{value:?}").contains(self.needle) {
                        self.matched = true;
                    }
                }
            }

            let mut visitor = MessageVisitor {
                needle: self.needle,
                matched: false,
            };
            event.record(&mut visitor);
            if visitor.matched {
                self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }

    /// `ensure_component_loaded` is on the tool-call path, so several requests can race to
    /// load the same not-yet-restored component. Without a per-component guard each racer
    /// compiles the component and they all write the same metadata and cache files.
    ///
    /// Compilation is not directly observable from the crate's API, so this counts the
    /// "Saved component metadata" event, which `compile_and_register_component_locked` emits
    /// exactly once per pass.
    #[tokio::test]
    async fn test_ensure_component_loaded_compiles_once_under_concurrency() -> Result<()> {
        let manager = create_test_manager().await?;
        let source = build_example_component().await?;
        tokio::fs::copy(&source, manager.component_path(TEST_COMPONENT_ID)).await?;
        assert!(manager.list_components().await.is_empty());

        let counter = MessageCounter::new("Saved component metadata");
        let subscriber = tracing_subscriber::layer::SubscriberExt::with(
            tracing_subscriber::registry(),
            counter.clone(),
        );
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);

        let results = futures::future::join_all(
            (0..4).map(|_| manager.ensure_component_loaded(TEST_COMPONENT_ID)),
        )
        .await;
        for result in results {
            result?;
        }

        assert_eq!(
            manager.list_components().await,
            vec![TEST_COMPONENT_ID.to_string()]
        );
        assert_eq!(
            counter.count(),
            1,
            "the component should be compiled and registered exactly once"
        );

        Ok(())
    }

    /// The interesting race is not between two `ensure_component_loaded` callers, it is
    /// between an on-demand load and the background restore: a `tools/call` that arrives
    /// before the restore reaches a component compiles it on demand while the restore is
    /// compiling the very same component, and both passes write the same metadata and
    /// precompiled cache files. The guard therefore has to sit on every path that compiles a
    /// component, not just on `ensure_component_loaded`.
    ///
    /// As above, compilation is observed through the "Saved component metadata" event that
    /// `compile_and_register_component_locked` emits exactly once per pass.
    #[tokio::test]
    async fn test_ensure_component_loaded_compiles_once_against_background_restore() -> Result<()> {
        let manager = create_test_manager().await?;
        let source = build_example_component().await?;
        tokio::fs::copy(&source, manager.component_path(TEST_COMPONENT_ID)).await?;
        assert!(manager.list_components().await.is_empty());

        let counter = MessageCounter::new("Saved component metadata");
        let subscriber = tracing_subscriber::layer::SubscriberExt::with(
            tracing_subscriber::registry(),
            counter.clone(),
        );
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);

        let (restore, on_demand) = tokio::join!(
            manager.load_existing_components_async(None, None::<fn()>),
            manager.ensure_component_loaded(TEST_COMPONENT_ID),
        );
        restore?;
        on_demand?;

        assert_eq!(
            manager.list_components().await,
            vec![TEST_COMPONENT_ID.to_string()]
        );
        assert_eq!(
            counter.count(),
            1,
            "the on-demand load and the background restore should compile the component once \
             between them"
        );

        Ok(())
    }

    /// Two callers racing to load the same component must be handed the same mutex, or the
    /// guard stops guarding and both compile.
    #[tokio::test]
    async fn test_load_guards_hand_out_one_mutex_per_active_component() {
        let guards = ComponentLoadGuards::default();

        let first = guards.guard_for("component-a").await;
        let _held = first.lock().await;
        let second = guards.guard_for("component-a").await;

        assert!(
            Arc::ptr_eq(&first, &second),
            "a caller arriving while a load is in flight must wait on that load's mutex"
        );
    }

    /// The map is keyed by component id, so a server that loads and unloads distinct
    /// components over its lifetime would grow it without bound if entries outlived the loads
    /// that created them.
    #[tokio::test]
    async fn test_load_guards_do_not_retain_entries_for_inactive_components() {
        let guards = ComponentLoadGuards::default();

        {
            let gone = guards.guard_for("gone").await;
            let _held = gone.lock().await;
            assert_eq!(guards.tracked_ids().await, vec!["gone".to_string()]);
        }

        let still_here = guards.guard_for("still-here").await;
        let _held = still_here.lock().await;

        assert_eq!(
            guards.tracked_ids().await,
            vec!["still-here".to_string()],
            "an id with no load in flight should not be retained"
        );
    }

    /// The same reclamation seen through the manager: a component that was loaded and then
    /// unloaded leaves nothing behind in the guard map.
    #[tokio::test]
    async fn test_unloaded_components_are_not_retained_by_the_load_guards() -> Result<()> {
        let manager = create_test_manager().await?;
        let source = build_example_component().await?;
        let other_component_id = "other_component";
        tokio::fs::copy(&source, manager.component_path(TEST_COMPONENT_ID)).await?;
        tokio::fs::copy(&source, manager.component_path(other_component_id)).await?;

        manager.ensure_component_loaded(TEST_COMPONENT_ID).await?;
        manager.unload_component(TEST_COMPONENT_ID).await?;
        manager.ensure_component_loaded(other_component_id).await?;

        assert_eq!(
            manager.load_guards.tracked_ids().await,
            vec![other_component_id.to_string()],
            "the unloaded component should not keep an entry in the guard map"
        );

        Ok(())
    }

    /// Staging replaces the component's `.wasm`, metadata and precompiled cache on disk, and
    /// the compiler then reads those very files back, so staging has to happen inside the
    /// same critical section as the compilation. If the guard were taken around compilation
    /// only, an explicit reload could delete and rewrite the artifact while an on-demand load
    /// or the background restore held the guard, and the serialized compiler would read a
    /// replaced artifact or a precompiled cache that no longer matches it.
    ///
    /// That interleaving is not directly observable from outside the crate, so this test pins
    /// the ordering that rules it out: while another load holds the guard for this component,
    /// a reload must not have touched any of its files. Releasing the guard then has to let
    /// the same reload through and recompile, which also pins that holding the guard earlier
    /// did not turn a reload into an early return.
    #[tokio::test]
    async fn test_load_component_stages_the_artifact_under_the_load_guard() -> Result<()> {
        let manager = create_test_manager().await?;
        let source_dir = tempfile::tempdir()?;
        let source = source_dir.path().join(format!("{TEST_COMPONENT_ID}.wasm"));
        tokio::fs::copy(build_example_component().await?, &source).await?;

        // Stand-ins for the artifacts of an already installed component. Staging removes all
        // three before copying the new ones in, so any of them changing while the guard is
        // held elsewhere is proof that staging ran outside the critical section.
        let artifact_path = manager.component_path(TEST_COMPONENT_ID);
        let metadata_path = manager.storage.metadata_path(TEST_COMPONENT_ID);
        let precompiled_path = manager.component_precompiled_path(TEST_COMPONENT_ID);
        tokio::fs::write(&artifact_path, b"stale artifact").await?;
        tokio::fs::write(&metadata_path, b"stale metadata").await?;
        tokio::fs::write(&precompiled_path, b"stale precompiled cache").await?;

        // Stand in for a concurrent on-demand load or background restore that holds the guard
        // and is about to compile.
        let guard = manager.load_guard(TEST_COMPONENT_ID).await;
        let held = guard.lock().await;

        let reload = tokio::spawn({
            let manager = manager.manager.clone();
            let uri = format!("file://{}", source.display());
            async move { manager.load_component(&uri).await }
        });

        // Resolving a `file://` source and staging it take microseconds, so a reload that
        // stages outside the guard has long finished doing so by the end of this loop. The
        // comparisons are booleans because a mismatch here is a whole component's bytes.
        for _ in 0..10 {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            assert!(
                tokio::fs::read(&artifact_path).await? == b"stale artifact",
                "a reload must not replace the artifact while another load holds the guard"
            );
            assert!(
                tokio::fs::read(&metadata_path).await? == b"stale metadata",
                "a reload must not remove the metadata while another load holds the guard"
            );
            assert!(
                tokio::fs::read(&precompiled_path).await? == b"stale precompiled cache",
                "a reload must not remove the cache while another load holds the guard"
            );
        }

        drop(held);

        let outcome = reload.await??;
        assert_eq!(outcome.component_id, TEST_COMPONENT_ID);
        assert!(
            tokio::fs::read(&artifact_path).await? == tokio::fs::read(&source).await?,
            "the reload must still stage the new artifact once it holds the guard"
        );
        assert!(
            tokio::fs::read(&metadata_path).await? != b"stale metadata",
            "the reload must still recompile and rewrite the component's metadata"
        );
        assert_eq!(
            manager.list_components().await,
            vec![TEST_COMPONENT_ID.to_string()]
        );

        Ok(())
    }

    /// Unload removes a component's artifacts and its registry entry, both of which a load of
    /// the same component is simultaneously creating, so it has to take the same guard.
    ///
    /// As above, the ordering is what is observable: while a load holds the guard, an unload
    /// must not have removed anything yet, and releasing the guard has to let it through.
    #[tokio::test]
    async fn test_unload_component_removes_artifacts_under_the_load_guard() -> Result<()> {
        let manager = create_test_manager().await?;
        manager.load_test_component().await?;

        let artifact_path = manager.component_path(TEST_COMPONENT_ID);
        assert!(artifact_path.exists());

        let guard = manager.load_guard(TEST_COMPONENT_ID).await;
        let held = guard.lock().await;

        let unload = tokio::spawn({
            let manager = manager.manager.clone();
            async move { manager.unload_component(TEST_COMPONENT_ID).await }
        });

        for _ in 0..10 {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            assert!(
                artifact_path.exists(),
                "an unload must not remove the artifact while a load holds the guard"
            );
            assert_eq!(
                manager.list_components().await,
                vec![TEST_COMPONENT_ID.to_string()],
                "an unload must not deregister the component while a load holds the guard"
            );
        }

        drop(held);

        unload.await??;
        assert!(!artifact_path.exists());
        assert!(manager.list_components().await.is_empty());

        Ok(())
    }

    /// The damaging outcome of an unguarded unload: an on-demand load compiles while unload
    /// removes the files, so the load writes the metadata and precompiled cache and registers
    /// the component *after* the unload reported success. The component is then registered
    /// with no artifact behind it, and the files the unload promised to delete are back.
    ///
    /// The load is started first and given a head start so that the unload lands during
    /// compilation, which is the window that matters. Under the guard the unload waits for
    /// the load to finish and then removes everything it created, so the end state is the
    /// same no matter which one wins: nothing registered and no files left.
    #[tokio::test]
    async fn test_unload_during_on_demand_load_leaves_no_half_removed_component() -> Result<()> {
        let manager = create_test_manager().await?;
        let source = build_example_component().await?;
        tokio::fs::copy(&source, manager.component_path(TEST_COMPONENT_ID)).await?;
        assert!(manager.list_components().await.is_empty());

        let load = tokio::spawn({
            let manager = manager.manager.clone();
            async move { manager.ensure_component_loaded(TEST_COMPONENT_ID).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        manager.unload_component(TEST_COMPONENT_ID).await?;

        // The load may win the guard and be undone by the unload, or lose it and find the
        // component gone. Both are consistent; only the end state is asserted.
        let _ = load.await?;

        assert!(
            manager.list_components().await.is_empty(),
            "a component must not stay registered after its unload reported success"
        );
        assert!(
            !manager.component_path(TEST_COMPONENT_ID).exists(),
            "the component artifact must not survive its unload"
        );
        assert!(
            !manager.storage.metadata_path(TEST_COMPONENT_ID).exists(),
            "the component metadata must not be rewritten after its unload reported success"
        );
        assert!(
            !manager
                .component_precompiled_path(TEST_COMPONENT_ID)
                .exists(),
            "the precompiled cache must not be rewritten after its unload reported success"
        );

        Ok(())
    }

    /// Builds a component directory holding an installed component's artifact, cached metadata
    /// and precompiled cache, then returns a manager over it whose registry is still empty.
    ///
    /// That is the state a fresh CLI process, or a server about to run its background restore,
    /// starts from: everything is on disk and nothing is registered yet.
    async fn create_manager_over_installed_component() -> Result<TestLifecycleManager> {
        let tempdir = tempfile::tempdir()?;
        let source = build_example_component().await?;

        let installer = LifecycleManager::new_unloaded(&tempdir).await?;
        installer
            .load_component(&format!("file://{}", source.display()))
            .await?;
        drop(installer);

        let manager = LifecycleManager::new_unloaded(&tempdir).await?;
        assert!(manager.list_components().await.is_empty());
        assert!(manager.list_tools().await.is_empty());

        Ok(TestLifecycleManager {
            manager,
            _tempdir: tempdir,
        })
    }

    /// `load_all_components` is the eager startup path, and registering what it compiles writes
    /// the very registry entry an unload removes. Without the per-component guard an unload can
    /// remove a component's files and deregister it while this pass is still compiling, and the
    /// pass then re-registers a component that the unload already reported as gone.
    ///
    /// The interleaving is not directly observable, so the ordering that rules it out is what is
    /// pinned: while another load holds the guard for this component, the eager pass must not
    /// have registered it. The window is generous because without the guard the pass registers
    /// as soon as it has compiled, which takes far less than this.
    #[tokio::test]
    async fn test_load_all_components_registers_under_the_load_guard() -> Result<()> {
        let manager = create_test_manager().await?;
        let source = build_example_component().await?;
        tokio::fs::copy(&source, manager.component_path(TEST_COMPONENT_ID)).await?;
        assert!(manager.list_components().await.is_empty());

        // Stand in for a concurrent load or unload of this component that holds the guard.
        let guard = manager.load_guard(TEST_COMPONENT_ID).await;
        let held = guard.lock().await;

        let load_all = tokio::spawn({
            let manager = manager.manager.clone();
            async move { manager.load_all_components().await }
        });

        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            assert!(
                manager.list_components().await.is_empty(),
                "the eager load must not register a component while another load holds its guard"
            );
        }
        assert!(
            !load_all.is_finished(),
            "the eager load must still be waiting for the guard"
        );

        drop(held);

        load_all.await??;
        assert_eq!(
            manager.list_components().await,
            vec![TEST_COMPONENT_ID.to_string()],
            "the eager load must still register the component once it holds the guard"
        );

        Ok(())
    }

    /// Hydrating the registry from cached metadata is a check-then-act over the same files and
    /// registry entry an unload removes, so it has to hold the same per-component guard. The
    /// server runs it from the background restore while live requests may be unloading
    /// components.
    ///
    /// As above, the ordering is what is observable: while another load holds the guard for this
    /// component, the hydration pass must not have registered its tools, and releasing the guard
    /// has to let it through. Hydration compiles nothing, so without the guard it finishes
    /// almost immediately.
    #[tokio::test]
    async fn test_populate_registry_from_metadata_registers_under_the_load_guard() -> Result<()> {
        let manager = create_manager_over_installed_component().await?;

        let guard = manager.load_guard(TEST_COMPONENT_ID).await;
        let held = guard.lock().await;

        let hydrate = tokio::spawn({
            let manager = manager.manager.clone();
            async move { manager.populate_registry_from_metadata().await }
        });

        for _ in 0..20 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            assert!(
                manager.list_tools().await.is_empty(),
                "hydration must not register tools while another load holds the component's guard"
            );
        }
        assert!(
            !hydrate.is_finished(),
            "hydration must still be waiting for the guard"
        );

        drop(held);

        hydrate.await??;
        assert!(
            !manager.list_tools().await.is_empty(),
            "hydration must still register the cached tools once it holds the guard"
        );

        Ok(())
    }

    /// The damaging outcome of unguarded hydration: an unload completes between the validation
    /// of a component's cached metadata and the registration of it. The component is put back
    /// into the tool map with no artifact and no instance behind it, and because nothing else
    /// ever revisits the tool map that entry is permanent.
    ///
    /// The unload is stood in for by holding the guard and removing the artifact, which is the
    /// state an unload leaves behind for a hydration pass that is waiting on the guard.
    #[tokio::test]
    async fn test_populate_registry_from_metadata_does_not_resurrect_an_unloaded_component(
    ) -> Result<()> {
        let manager = create_manager_over_installed_component().await?;

        let guard = manager.load_guard(TEST_COMPONENT_ID).await;
        let held = guard.lock().await;

        let hydrate = tokio::spawn({
            let manager = manager.manager.clone();
            async move { manager.populate_registry_from_metadata().await }
        });

        // Give the pass time to reach the guard, then let the "unload" win it.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        tokio::fs::remove_file(manager.component_path(TEST_COMPONENT_ID)).await?;
        drop(held);

        hydrate.await??;

        assert!(
            manager.list_tools().await.is_empty(),
            "a component removed from disk must not be reintroduced into the tool map"
        );
        assert!(
            manager.list_components().await.is_empty(),
            "a component removed from disk must not be reintroduced into the registry"
        );

        Ok(())
    }

    /// Writes the on-disk state a previous process leaves behind for an installed component:
    /// the artifact plus the cached tool metadata beside it. Neither has to be a real
    /// WebAssembly component, because hydrating the registry from cached metadata reads the
    /// metadata and validates the artifact's stamp without ever compiling it.
    async fn install_cached_component(
        component_dir: &Path,
        component_id: &str,
        tool_name: &str,
    ) -> Result<()> {
        let storage = ComponentStorage::new(component_dir.to_path_buf(), 1).await?;
        let artifact = storage.component_path(component_id);
        tokio::fs::write(&artifact, format!("stand-in artifact for {component_id}")).await?;

        let validation_stamp = storage.create_validation_stamp(&artifact, false).await?;
        storage
            .write_metadata(&ComponentMetadata {
                component_id: component_id.to_string(),
                tool_schemas: vec![serde_json::json!({
                    "name": tool_name,
                    "description": "a cached tool",
                    "inputSchema": { "type": "object" },
                })],
                function_identifiers: vec![FunctionIdentifier {
                    package_name: None,
                    interface_name: None,
                    function_name: tool_name.to_string(),
                }],
                tool_names: vec![tool_name.to_string()],
                validation_stamp,
                created_at: 0,
            })
            .await?;

        Ok(())
    }

    /// A tool name exported by two installed components must never resolve to one of them.
    /// Hydration is the first phase of the background restore and the server answers requests
    /// while it runs, so publishing one component at a time opens a window in which only the
    /// first of the pair is in the tool map. A lookup landing there resolves the tool to that
    /// component and runs it, even though the collision rule would refuse it once both are
    /// registered, and nothing revisits the tool map afterwards to withdraw the answer.
    ///
    /// Holding one component's load guard stops hydration at that component, which is the
    /// interleaving. Which of the pair the directory scan reaches first is not something the
    /// test can choose, so it runs the scenario once per component id: for whichever id the
    /// scan reaches second, the run that blocks on it leaves the other one already published
    /// by a per-component write. One of the two runs therefore reproduces the window whatever
    /// the directory order is, and with a single batched write neither does.
    #[tokio::test]
    async fn test_hydration_never_exposes_one_side_of_a_tool_name_collision() -> Result<()> {
        const COLLIDING_TOOL: &str = "shared-tool";
        const COMPONENT_IDS: [&str; 2] = ["collide-alpha", "collide-beta"];

        let tempdir = tempfile::tempdir()?;
        for component_id in COMPONENT_IDS {
            install_cached_component(tempdir.path(), component_id, COLLIDING_TOOL).await?;
        }

        for blocked in COMPONENT_IDS {
            let manager = LifecycleManager::new_unloaded(&tempdir).await?;
            assert!(manager.list_tools().await.is_empty());

            // Stand in for a concurrent load or unload of this component. Hydration reaches it
            // and waits, with the other component either already handled or still to come.
            let guard = manager.load_guard(blocked).await;
            let held = guard.lock().await;

            let hydrate = tokio::spawn({
                let manager = manager.clone();
                async move { manager.populate_registry_from_metadata().await }
            });

            for _ in 0..10 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                assert!(
                    manager.list_tools().await.is_empty(),
                    "hydration blocked on {blocked} must not publish part of the registry"
                );
                assert!(
                    manager
                        .get_component_id_for_tool(COLLIDING_TOOL)
                        .await
                        .is_err(),
                    "a tool exported by two installed components must not resolve to one of \
                     them while hydration is blocked on {blocked}"
                );
            }
            assert!(
                !hydrate.is_finished(),
                "hydration must still be waiting for the guard"
            );

            drop(held);
            hydrate.await??;

            let error = manager
                .get_component_id_for_tool(COLLIDING_TOOL)
                .await
                .expect_err("the collision must be reported once hydration has finished");
            assert!(
                error.to_string().contains("Multiple components"),
                "unexpected error resolving the colliding tool: {error:#}"
            );
        }

        Ok(())
    }

    /// An explicit reload stages its replacement under the load guard, and staging removes the
    /// old `.wasm` before copying the new one in. A concurrent tool call must wait for that
    /// reload rather than deciding the component is missing, so the guard has to be taken
    /// before the artifact is looked at.
    ///
    /// The staging window is reproduced exactly: the guard is held and the artifact is absent,
    /// which is what a caller arriving mid-reload sees.
    #[tokio::test]
    async fn test_ensure_component_loaded_waits_for_a_reload_that_is_staging() -> Result<()> {
        let manager = create_test_manager().await?;
        let source = build_example_component().await?;
        let artifact_path = manager.component_path(TEST_COMPONENT_ID);
        tokio::fs::copy(&source, &artifact_path).await?;

        // Stand in for a reload that holds the guard and has removed the old artifact.
        let guard = manager.load_guard(TEST_COMPONENT_ID).await;
        let held = guard.lock().await;
        tokio::fs::remove_file(&artifact_path).await?;

        let on_demand = tokio::spawn({
            let manager = manager.manager.clone();
            async move { manager.ensure_component_loaded(TEST_COMPONENT_ID).await }
        });

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(
            !on_demand.is_finished(),
            "a tool call arriving while a reload is staging must wait for it, not fail"
        );

        // The reload finishes staging and releases the guard.
        tokio::fs::copy(&source, &artifact_path).await?;
        drop(held);

        on_demand.await??;
        assert_eq!(
            manager.list_components().await,
            vec![TEST_COMPONENT_ID.to_string()]
        );

        Ok(())
    }

    /// Taking the guard before the artifact check means unknown component ids now reach the
    /// guard map. They must not accumulate there: the entries are weak, so each dies with the
    /// `Arc` the failed call held, and the next caller that has to mint a mutex sweeps them.
    #[tokio::test]
    async fn test_unknown_components_are_not_retained_by_the_load_guards() -> Result<()> {
        let manager = create_test_manager().await?;

        for index in 0..32 {
            let missing = format!("never-installed-{index}");
            assert!(
                manager.ensure_component_loaded(&missing).await.is_err(),
                "an id with no artifact must still fail"
            );
            assert!(
                manager.load_guards.tracked_ids().await.len() <= 1,
                "failed lookups of unknown ids must not accumulate in the guard map"
            );
        }

        Ok(())
    }

    #[test(tokio::test)]
    async fn test_get_wasi_state_for_component_with_policy() -> Result<()> {
        let manager = create_test_manager().await?;
        manager.load_test_component().await?;

        // Create and attach a policy
        let policy_content = r#"
version: "1.0"
description: "Test policy"
permissions:
  network:
    allow:
      - host: "example.com"
"#;
        let policy_path = manager.component_root().join("test-policy.yaml");
        tokio::fs::write(&policy_path, policy_content).await?;

        let policy_uri = format!("file://{}", policy_path.display());
        manager
            .attach_policy(TEST_COMPONENT_ID, &policy_uri)
            .await?;

        // Test getting WASI state for component with attached policy
        let _wasi_state = manager
            .get_wasi_state_for_component(TEST_COMPONENT_ID)
            .await?;

        Ok(())
    }

    #[test(tokio::test)]
    async fn test_policy_restoration_on_startup() -> Result<()> {
        let tempdir = tempfile::tempdir()?;

        // Create a component file
        let component_content = if let Ok(content) =
            std::fs::read("examples/fetch-rs/target/wasm32-wasip2/debug/fetch_rs.wasm")
        {
            content
        } else {
            let path = build_example_component().await?;
            std::fs::read(path)?
        };
        let component_path = tempdir.path().join("test-component.wasm");
        std::fs::write(&component_path, component_content)?;

        // Create a co-located policy file
        let policy_content = r#"
version: "1.0"
description: "Test policy"
permissions:
  network:
    allow:
      - host: "example.com"
"#;
        let policy_path = tempdir.path().join("test-component.policy.yaml");
        std::fs::write(&policy_path, policy_content)?;

        // Create a new LifecycleManager to test policy restoration
        let manager = LifecycleManager::new(&tempdir).await?;

        // Check if policy was restored
        let policy_info = manager.get_policy_info("test-component").await;
        assert!(policy_info.is_some());

        Ok(())
    }

    #[test(tokio::test)]
    async fn test_policy_file_not_found_error() -> Result<()> {
        let manager = create_test_manager().await?;
        manager.load_test_component().await?;

        let non_existent_uri = "file:///non/existent/policy.yaml";

        // Test attaching non-existent policy file
        let result = manager
            .attach_policy(TEST_COMPONENT_ID, non_existent_uri)
            .await;
        assert!(result.is_err());

        Ok(())
    }

    #[test(tokio::test)]
    async fn test_policy_invalid_uri_scheme() -> Result<()> {
        let manager = create_test_manager().await?;
        manager.load_test_component().await?;

        let invalid_uri = "invalid-scheme://policy.yaml";

        // Test attaching policy with invalid URI scheme
        let result = manager.attach_policy(TEST_COMPONENT_ID, invalid_uri).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unsupported policy scheme"));

        Ok(())
    }

    #[test(tokio::test)]
    async fn test_execute_component_call_with_per_component_policy() -> Result<()> {
        let manager = create_test_manager().await?;
        manager.load_test_component().await?;

        // Test execution with default policy (no explicit policy attached)
        // This tests that the execution works with the default policy
        let result = manager
            .execute_component_call(
                TEST_COMPONENT_ID,
                "fetch",
                r#"{"url": "https://example.com"}"#,
            )
            .await;

        // The call might fail due to network restrictions in test environment,
        // but it should at least attempt to execute (not fail due to component not found)
        // We just verify the call was made successfully in terms of component lookup
        match result {
            Ok(_) => {} // Success
            Err(e) => {
                // Should not be a component lookup error
                assert!(!e.to_string().contains("Component not found"));
            }
        }

        Ok(())
    }

    #[test(tokio::test)]
    async fn test_wasi_state_template_allowed_hosts() -> Result<()> {
        // Test that WasiStateTemplate correctly stores allowed hosts from policy
        let policy_content = r#"
version: "1.0"
description: "Test policy with network permissions"
permissions:
  network:
    allow:
      - host: "api.example.com"
      - host: "cdn.example.com"
"#;
        let policy = PolicyParser::parse_str(policy_content)?;

        let temp_dir = tempfile::tempdir()?;
        let env_vars = HashMap::new(); // Empty environment for test
        let template =
            create_wasi_state_template_from_policy(&policy, temp_dir.path(), &env_vars, None)?;

        assert_eq!(template.allowed_hosts.len(), 2);
        assert!(template.allowed_hosts.contains("api.example.com"));
        assert!(template.allowed_hosts.contains("cdn.example.com"));

        Ok(())
    }

    // Revoke permission system tests

    #[test(tokio::test)]
    async fn test_revoke_permission_network() -> Result<()> {
        let manager = create_test_manager().await?;
        manager.load_test_component().await?;

        // Grant network permission first
        let details = serde_json::json!({"host": "api.example.com"});
        manager
            .grant_permission(TEST_COMPONENT_ID, "network", &details)
            .await?;

        // Verify permission was granted
        let policy_path = manager.get_component_policy_path(TEST_COMPONENT_ID);
        let policy_content = tokio::fs::read_to_string(&policy_path).await?;
        assert!(policy_content.contains("api.example.com"));

        // Revoke the network permission
        manager
            .revoke_permission(TEST_COMPONENT_ID, "network", &details)
            .await?;

        // Verify permission was revoked
        let policy_content = tokio::fs::read_to_string(&policy_path).await?;
        assert!(!policy_content.contains("api.example.com"));

        Ok(())
    }

    #[test(tokio::test)]
    async fn test_revoke_permission_storage() -> Result<()> {
        let manager = create_test_manager().await?;
        manager.load_test_component().await?;

        // Grant storage permission first
        let details = serde_json::json!({"uri": "fs:///tmp/test", "access": ["read", "write"]});
        manager
            .grant_permission(TEST_COMPONENT_ID, "storage", &details)
            .await?;

        // Verify permission was granted
        let policy_path = manager.get_component_policy_path(TEST_COMPONENT_ID);
        let policy_content = tokio::fs::read_to_string(&policy_path).await?;
        assert!(policy_content.contains("fs:///tmp/test"));

        // Revoke the storage permission
        manager
            .revoke_permission(TEST_COMPONENT_ID, "storage", &details)
            .await?;

        // Verify permission was revoked
        let policy_content = tokio::fs::read_to_string(&policy_path).await?;
        assert!(!policy_content.contains("fs:///tmp/test"));

        Ok(())
    }

    #[test(tokio::test)]
    async fn test_revoke_permission_environment() -> Result<()> {
        let manager = create_test_manager().await?;
        manager.load_test_component().await?;

        // Grant environment permission first
        let details = serde_json::json!({"key": "API_KEY"});
        manager
            .grant_permission(TEST_COMPONENT_ID, "environment", &details)
            .await?;

        // Verify permission was granted
        let policy_path = manager.get_component_policy_path(TEST_COMPONENT_ID);
        let policy_content = tokio::fs::read_to_string(&policy_path).await?;
        assert!(policy_content.contains("API_KEY"));

        // Revoke the environment permission
        manager
            .revoke_permission(TEST_COMPONENT_ID, "environment", &details)
            .await?;

        // Verify permission was revoked
        let policy_content = tokio::fs::read_to_string(&policy_path).await?;
        assert!(!policy_content.contains("API_KEY"));

        Ok(())
    }

    #[test(tokio::test)]
    async fn test_reset_permission() -> Result<()> {
        let manager = create_test_manager().await?;
        manager.load_test_component().await?;

        // Grant multiple permissions first
        let network_details = serde_json::json!({"host": "api.example.com"});
        manager
            .grant_permission(TEST_COMPONENT_ID, "network", &network_details)
            .await?;

        let storage_details = serde_json::json!({"uri": "fs:///tmp/test", "access": ["read"]});
        manager
            .grant_permission(TEST_COMPONENT_ID, "storage", &storage_details)
            .await?;

        let env_details = serde_json::json!({"key": "API_KEY"});
        manager
            .grant_permission(TEST_COMPONENT_ID, "environment", &env_details)
            .await?;

        // Verify permissions were granted
        let policy_path = manager.get_component_policy_path(TEST_COMPONENT_ID);
        assert!(policy_path.exists());

        // Reset all permissions
        manager.reset_permission(TEST_COMPONENT_ID).await?;

        // Verify policy file was removed
        assert!(!policy_path.exists());

        // Verify metadata file was also removed
        let metadata_path = manager.get_component_metadata_path(TEST_COMPONENT_ID);
        assert!(!metadata_path.exists());

        Ok(())
    }

    #[test(tokio::test)]
    async fn test_revoke_permission_component_not_found() -> Result<()> {
        let manager = create_test_manager().await?;

        // Try to revoke permission from non-existent component
        let details = serde_json::json!({"host": "api.example.com"});
        let result = manager
            .revoke_permission("non-existent", "network", &details)
            .await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Component not found"));

        Ok(())
    }

    #[test(tokio::test)]
    async fn test_reset_permission_component_not_found() -> Result<()> {
        let manager = create_test_manager().await?;

        // Try to reset permissions for non-existent component
        let result = manager.reset_permission("non-existent").await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Component not found"));

        Ok(())
    }

    #[test(tokio::test)]
    async fn test_grant_revoke_grant_cycle() -> Result<()> {
        let manager = create_test_manager().await?;
        manager.load_test_component().await?;

        let details = serde_json::json!({"host": "api.example.com"});

        // Grant permission
        manager
            .grant_permission(TEST_COMPONENT_ID, "network", &details)
            .await?;

        let policy_path = manager.get_component_policy_path(TEST_COMPONENT_ID);
        let policy_content = tokio::fs::read_to_string(&policy_path).await?;
        assert!(policy_content.contains("api.example.com"));

        // Revoke permission
        manager
            .revoke_permission(TEST_COMPONENT_ID, "network", &details)
            .await?;

        let policy_content = tokio::fs::read_to_string(&policy_path).await?;
        assert!(!policy_content.contains("api.example.com"));

        // Grant permission again
        manager
            .grant_permission(TEST_COMPONENT_ID, "network", &details)
            .await?;

        let policy_content = tokio::fs::read_to_string(&policy_path).await?;
        assert!(policy_content.contains("api.example.com"));

        Ok(())
    }

    #[test(tokio::test)]
    async fn test_set_secrets_component_not_found() -> Result<()> {
        let manager = create_test_manager().await?;

        // Try to set secrets for non-existent component
        let secrets = vec![("KEY".to_string(), "value".to_string())];
        let result = manager
            .set_component_secrets("non-existent-component", &secrets)
            .await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Component not found"));

        Ok(())
    }

    fn tool_metadata(name: &str) -> ToolMetadata {
        ToolMetadata {
            identifier: FunctionIdentifier {
                package_name: None,
                interface_name: None,
                function_name: name.to_string(),
            },
            normalized_name: name.to_string(),
            schema: serde_json::json!({ "name": name }),
        }
    }

    fn tool_metadatas(names: &[&str]) -> Vec<ToolMetadata> {
        names.iter().copied().map(tool_metadata).collect()
    }

    #[test]
    fn test_find_tool_name_collisions_reports_nothing_for_distinct_names() {
        let mut state = ComponentRegistryState::default();
        state.register_tools_only("weather", tool_metadatas(&["get-weather"]));

        assert!(state
            .find_tool_name_collisions(&["delete-file".to_string()])
            .is_empty());
    }

    #[test]
    fn test_find_tool_name_collisions_reports_incumbent_component() {
        let mut state = ComponentRegistryState::default();
        state.register_tools_only("get-weather-js", tool_metadatas(&["get-weather"]));

        let collisions = state
            .find_tool_name_collisions(&["get-weather".to_string(), "unique-tool".to_string()]);

        assert_eq!(
            collisions,
            vec![(
                "get-weather".to_string(),
                vec!["get-weather-js".to_string()]
            )]
        );
    }

    #[test]
    fn test_find_tool_name_collisions_ignores_component_reloading_itself() {
        let mut state = ComponentRegistryState::default();
        let names = ["get-weather", "get-forecast"];
        state.register_tools_only("get-weather-js", tool_metadatas(&names));

        // A reload evicts the previous registration before re-registering.
        state.unregister_tools("get-weather-js");

        let incoming: Vec<String> = names.iter().map(|name| name.to_string()).collect();
        assert!(state.find_tool_name_collisions(&incoming).is_empty());
    }

    #[test]
    fn test_find_tool_name_collisions_reports_every_incumbent() {
        let mut state = ComponentRegistryState::default();
        state.register_tools_only("filesystem-rs", tool_metadatas(&["delete-file"]));
        state.register_tools_only("github-js", tool_metadatas(&["delete-file"]));

        let collisions = state.find_tool_name_collisions(&["delete-file".to_string()]);

        assert_eq!(
            collisions,
            vec![(
                "delete-file".to_string(),
                vec!["filesystem-rs".to_string(), "github-js".to_string()]
            )]
        );
    }

    /// Collects formatted tracing output so a test can assert on emitted events.
    #[derive(Clone, Default)]
    struct CapturedLogs(Arc<std::sync::Mutex<Vec<u8>>>);

    impl CapturedLogs {
        fn contents(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl std::io::Write for CapturedLogs {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn test_register_tools_only_warns_on_collision() {
        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .with_writer(logs.clone())
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            let mut state = ComponentRegistryState::default();
            state.register_tools_only("get-weather-js", tool_metadatas(&["get-weather"]));
            assert!(!logs.contents().contains("Tool name collision"));

            state.register_tools_only(
                "get-open-meteo-weather-js",
                tool_metadatas(&["get-weather"]),
            );
        });

        let captured = logs.contents();
        assert!(captured.contains("Tool name collision"), "{captured}");
        assert!(
            captured.contains("component_id=get-open-meteo-weather-js"),
            "{captured}"
        );
        assert!(captured.contains("tool_name=get-weather"), "{captured}");
        assert!(
            captured.contains("existing_components=get-weather-js"),
            "{captured}"
        );
    }
}
