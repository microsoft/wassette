// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! A security-oriented runtime that runs WebAssembly Components via MCP

#![warn(missing_docs)]

use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, info, instrument, warn};
use wasmtime::component::{Component, InstancePre};
use wasmtime::Store;

mod component_storage;
mod config;
mod http;
mod loader;
pub mod oci_multi_layer;
mod policy_internal;
mod runtime_context;
pub mod schema;
mod secrets;
mod wasistate;

use component_storage::ComponentStorage;
pub use config::{LifecycleBuilder, LifecycleConfig};
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

    async fn register_metadata_if_absent(
        &self,
        component_id: &str,
        tools: Vec<ToolMetadata>,
    ) -> Result<bool> {
        let mut state = self.state.write().await;

        if state.components.contains_key(component_id)
            || state.component_map.contains_key(component_id)
        {
            return Ok(false);
        }

        state.register_tools_only(component_id, tools);
        Ok(true)
    }
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

    fn register_tools_only(&mut self, component_id: &str, tools: Vec<ToolMetadata>) {
        let mut tool_names = Vec::new();

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
            storage,
            policy_manager,
            oci_client,
            http_client,
            secrets_manager,
        })
    }

    /// Load every component present in the component directory, updating the registry and cache.
    #[instrument(skip(self))]
    pub async fn load_all_components(&self) -> Result<()> {
        let loaded_components =
            load_components_parallel(self.storage.root(), Arc::clone(&self.runtime)).await?;

        let mut registered_ids = Vec::new();

        for (component_instance, name) in loaded_components {
            let tool_metadata = if let Some(ref package_docs) = component_instance.package_docs {
                component_exports_to_tools_with_docs(
                    &component_instance.component,
                    self.runtime.as_ref(),
                    true,
                    package_docs,
                )
            } else {
                component_exports_to_tools(
                    &component_instance.component,
                    self.runtime.as_ref(),
                    true,
                )
            };

            if let Err(error) = self
                .registry
                .upsert_component(name.clone(), component_instance, tool_metadata)
                .await
            {
                warn!(%name, %error, "Failed to register component in registry");
                continue;
            }

            registered_ids.push(name);
        }

        for component_id in registered_ids {
            if let Err(error) = self.restore_policy_attachment(&component_id).await {
                warn!(%component_id, %error, "Failed to restore policy attachment");
            }
        }

        info!("LifecycleManager finished loading components");
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

    async fn compile_and_register_component(
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
                warn!(%component_id, error = %e, "Failed to save component metadata");
            }
        }

        let load_result = self
            .registry
            .upsert_component(component_id.to_string(), component_instance, tool_metadata)
            .await?;

        if let Err(error) = self.policy_manager.restore_from_disk(component_id).await {
            warn!(%component_id, %error, "Failed to restore policy attachment");
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
    #[instrument(skip(self))]
    pub async fn load_component(&self, uri: &str) -> Result<ComponentLoadOutcome> {
        debug!(uri, "Loading component");
        let (component_id, resource) = self.resolve_component_resource(uri).await?;
        let staged_path = self
            .stage_component_artifact(&component_id, resource)
            .await?;
        let outcome = self
            .compile_and_register_component(&component_id, &staged_path)
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
    #[instrument(skip(self))]
    pub async fn unload_component(&self, id: &str) -> Result<()> {
        debug!("Unloading component and removing files from disk");

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
                let tools: Vec<Value> = metadata
                    .tool_schemas
                    .into_iter()
                    .map(|schema| schema::canonicalize_output_schema(&schema))
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
    #[instrument(skip(self))]
    pub async fn ensure_component_loaded(&self, component_id: &str) -> Result<()> {
        if self.registry.contains_component(component_id).await {
            return Ok(());
        }

        let entry_path = self.component_path(component_id);
        if !entry_path.exists() {
            bail!("Component not found: {}", component_id);
        }

        self.compile_and_register_component(component_id, &entry_path)
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
                    warn!(%component_id, error = %e, "Failed to load precompiled component, falling back to compilation");
                }
            }
        }

        // Fall back to compilation
        let wasm_bytes = tokio::fs::read(wasm_path)
            .await
            .context("Failed to read wasm file")?;

        let component = Component::new(self.runtime.as_ref(), &wasm_bytes)
            .context("Failed to compile component")?;

        // Save precompiled version for next time (async, don't block on this)
        if let Err(e) = self
            .save_precompiled_component(component_id, &wasm_bytes)
            .await
        {
            warn!(%component_id, error = %e, "Failed to save precompiled component");
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
        let argument_vals = json_to_vals(&params, &func.params(&store))?;

        let mut results = create_placeholder_results(&func.results(&store));

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
            return Err(e);
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
                    Err(e) => warn!("Failed to load component: {}", e),
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
    async fn populate_registry_from_metadata(&self) -> Result<()> {
        let mut entries = tokio::fs::read_dir(self.storage.root()).await?;
        let mut loaded_count = 0;

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

            // Try to load cached metadata
            if let Ok(Some(metadata)) = self.load_component_metadata(component_id).await {
                // Validate that the component file hasn't changed
                if ComponentStorage::validate_stamp(&entry_path, &metadata.validation_stamp).await {
                    let tool_metadata: Vec<ToolMetadata> = metadata
                        .function_identifiers
                        .into_iter()
                        .zip(metadata.tool_schemas)
                        .zip(metadata.tool_names)
                        .map(|((identifier, schema), normalized_name)| {
                            let canonical = schema::canonicalize_output_schema(&schema);
                            ToolMetadata {
                                identifier,
                                schema: canonical,
                                normalized_name,
                            }
                        })
                        .collect();

                    match self
                        .registry
                        .register_metadata_if_absent(component_id, tool_metadata)
                        .await
                    {
                        Ok(true) => {
                            loaded_count += 1;
                            debug!(component_id = %component_id, "Registered tools from cached metadata");
                            continue;
                        }
                        Ok(false) => {
                            debug!(component_id = %component_id, "Skipping cached metadata; component already registered");
                            continue;
                        }
                        Err(e) => {
                            warn!(%component_id, error = %e, "Failed to register tools from metadata");
                            continue;
                        }
                    }
                }
            }

            debug!(component_id = %component_id, "No valid cached metadata found, will load component later");
        }

        if loaded_count > 0 {
            info!(
                "Registered {} components from cached metadata",
                loaded_count
            );
        }

        Ok(())
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

        let start_time = Instant::now();
        self.compile_and_register_component(&component_id, &entry_path)
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
// Load components in parallel for improved startup performance
async fn load_components_parallel(
    component_dir: &Path,
    runtime: Arc<RuntimeContext>,
) -> Result<Vec<(ComponentInstance, String)>> {
    let mut entries = tokio::fs::read_dir(component_dir).await?;
    let mut load_futures = Vec::new();

    while let Some(entry) = entries.next_entry().await? {
        let runtime_clone = Arc::clone(&runtime);
        let future = async move {
            match load_component_from_entry(runtime_clone, entry).await {
                Ok(Some(result)) => Some(Ok(result)),
                Ok(None) => None,
                Err(e) => Some(Err(e)),
            }
        };
        load_futures.push(future);
    }

    let results = futures::future::join_all(load_futures).await;
    let mut components = Vec::new();

    for result in results.into_iter().flatten() {
        match result {
            Ok(component) => components.push(component),
            Err(e) => warn!("Failed to load component: {}", e),
        }
    }

    Ok(components)
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
}
