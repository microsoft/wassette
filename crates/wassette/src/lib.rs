// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! A security-oriented runtime that runs WebAssembly Components via MCP

#![warn(missing_docs)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use component2json::{
    component_exports_to_json_schema, component_exports_to_tools, create_placeholder_results,
    json_to_vals, vals_to_json, FunctionIdentifier, ToolMetadata,
};
use etcetera::BaseStrategy;
use policy::PolicyParser;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::fs::DirEntry;
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, info, instrument, warn};
use wasmtime::component::{Component, InstancePre, Linker};
use wasmtime::{Engine, Store};
use wasmtime_wasi_config::WasiConfig;

mod http;
mod loader;
mod policy_internal;
mod secrets;
mod wasistate;

pub use http::WassetteWasiState;
use loader::{ComponentResource, PolicyResource};
use policy_internal::PolicyRegistry;
pub use policy_internal::{PermissionGrantRequest, PermissionRule, PolicyInfo};
pub use secrets::SecretsManager;
use wasistate::WasiState;
pub use wasistate::{
    create_wasi_state_template_from_policy, CustomResourceLimiter, WasiStateTemplate,
};

const DOWNLOADS_DIR: &str = "downloads";
const PRECOMPILED_EXT: &str = "cwasm";
const METADATA_EXT: &str = "metadata.json";

/// Get the default secrets directory path based on the OS
fn get_default_secrets_dir() -> PathBuf {
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

#[derive(Debug, Default)]
struct ComponentRegistry {
    tool_map: HashMap<String, Vec<ToolInfo>>,
    component_map: HashMap<String, Vec<String>>,
}

/// The returned status when loading a component
#[derive(Debug, PartialEq)]
pub enum LoadResult {
    /// Indicates that the component was loaded but replaced a currently loaded component
    Replaced,
    /// Indicates that the component did not exist and is now loaded
    New,
}

impl ComponentRegistry {
    fn new() -> Self {
        Self::default()
    }

    fn register_tools(&mut self, component_id: &str, tools: Vec<ToolMetadata>) -> Result<()> {
        let mut tool_names = Vec::new();

        for tool_metadata in tools {
            let tool_info = ToolInfo {
                component_id: component_id.to_string(),
                identifier: tool_metadata.identifier,
                schema: tool_metadata.schema,
            };

            self.tool_map
                .entry(tool_metadata.normalized_name.clone())
                .or_default()
                .push(tool_info);
            tool_names.push(tool_metadata.normalized_name);
        }

        self.component_map
            .insert(component_id.to_string(), tool_names);
        Ok(())
    }

    fn get_function_identifier(&self, tool_name: &str) -> Option<&FunctionIdentifier> {
        self.tool_map
            .get(tool_name)
            .and_then(|tool_infos| tool_infos.first())
            .map(|tool_info| &tool_info.identifier)
    }

    fn unregister_component(&mut self, component_id: &str) {
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

    fn get_tool_info(&self, tool_name: &str) -> Option<&Vec<ToolInfo>> {
        self.tool_map.get(tool_name)
    }

    fn list_tools(&self) -> Vec<Value> {
        self.tool_map
            .values()
            .flat_map(|tools| tools.iter().map(|t| t.schema.clone()))
            .collect()
    }
}

/// A manager that handles the dynamic lifecycle of WebAssembly components.
#[derive(Clone)]
pub struct LifecycleManager {
    engine: Arc<Engine>,
    linker: Arc<Linker<WassetteWasiState<WasiState>>>,
    components: Arc<RwLock<HashMap<String, ComponentInstance>>>,
    registry: Arc<RwLock<ComponentRegistry>>,
    policy_registry: Arc<RwLock<PolicyRegistry>>,
    oci_client: Arc<oci_wasm::WasmClient>,
    http_client: reqwest::Client,
    plugin_dir: PathBuf,
    environment_vars: HashMap<String, String>,
    secrets_manager: Arc<SecretsManager>,
}

/// A representation of a loaded component instance. It contains both the base component info and a
/// pre-instantiated component ready for execution
#[derive(Clone)]
pub struct ComponentInstance {
    component: Arc<Component>,
    instance_pre: Arc<InstancePre<WassetteWasiState<WasiState>>>,
}

impl LifecycleManager {
    /// Creates a lifecycle manager from configuration parameters
    /// This is the primary way to create a LifecycleManager for most use cases
    #[instrument(skip_all, fields(plugin_dir = %plugin_dir.as_ref().display()))]
    pub async fn new(plugin_dir: impl AsRef<Path>) -> Result<Self> {
        // Use default secrets directory for backward compatibility
        let default_secrets_dir = get_default_secrets_dir();

        Self::new_with_config(
            plugin_dir,
            HashMap::new(), // Empty environment variables for backward compatibility
            default_secrets_dir,
            oci_client::Client::default(),
            reqwest::Client::default(),
        )
        .await
    }

    /// Creates an unloaded lifecycle manager that initializes the engine/linker but does not scan/compile components
    /// This enables fast startup with components loaded in the background
    #[instrument(skip_all, fields(plugin_dir = %plugin_dir.as_ref().display()))]
    pub async fn new_unloaded(plugin_dir: impl AsRef<Path>) -> Result<Self> {
        Self::new_unloaded_with_env(
            plugin_dir,
            HashMap::new(), // Empty environment variables for backward compatibility
        )
        .await
    }

    /// Creates an unloaded lifecycle manager with environment variables
    #[instrument(skip_all, fields(plugin_dir = %plugin_dir.as_ref().display()))]
    pub async fn new_unloaded_with_env(
        plugin_dir: impl AsRef<Path>,
        environment_vars: HashMap<String, String>,
    ) -> Result<Self> {
        Self::new_unloaded_with_clients(
            plugin_dir,
            environment_vars,
            oci_client::Client::default(),
            reqwest::Client::default(),
        )
        .await
    }

    /// Creates an unloaded lifecycle manager with custom clients
    #[instrument(skip_all)]
    pub async fn new_unloaded_with_clients(
        plugin_dir: impl AsRef<Path>,
        environment_vars: HashMap<String, String>,
        oci_client: oci_client::Client,
        http_client: reqwest::Client,
    ) -> Result<Self> {
        let components_dir = plugin_dir.as_ref();

        if !components_dir.exists() {
            std::fs::create_dir_all(components_dir)?;
        }

        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        config.async_support(true);
        // Enable Wasmtime's built-in compilation cache for faster recompilation
        // Note: cache_config_load_default may not be available in this wasmtime version
        // if let Err(e) = config.cache_config_load_default() {
        //     warn!("Failed to load default cache config: {}", e);
        // }
        let engine = Arc::new(wasmtime::Engine::new(&config)?);

        Self::new_unloaded_with_policy(
            engine,
            components_dir,
            environment_vars,
            oci_client,
            http_client,
        )
        .await
    }

    /// Creates an unloaded lifecycle manager with custom clients and WASI state template
    #[instrument(skip_all)]
    async fn new_unloaded_with_policy(
        engine: Arc<Engine>,
        plugin_dir: impl AsRef<Path>,
        environment_vars: HashMap<String, String>,
        oci_client: oci_client::Client,
        http_client: reqwest::Client,
    ) -> Result<Self> {
        info!("Creating new unloaded LifecycleManager");

        let registry = ComponentRegistry::new();
        let components = HashMap::new();
        let policy_registry = PolicyRegistry::default();

        let mut linker = Linker::new(engine.as_ref());
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
        wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;
        wasmtime_wasi_config::add_to_linker(
            &mut linker,
            |h: &mut WassetteWasiState<WasiState>| WasiConfig::from(&h.inner.wasi_config_vars),
        )?;

        let linker = Arc::new(linker);

        // Initialize secrets manager with default directory for backward compatibility
        let secrets_dir = get_default_secrets_dir();
        let secrets_manager = Arc::new(SecretsManager::new(secrets_dir));
        secrets_manager.ensure_secrets_dir().await?;

        // Make sure the plugin dir exists and also create a subdirectory for temporary staging of downloaded files
        tokio::fs::create_dir_all(&plugin_dir)
            .await
            .context("Failed to create plugin directory")?;
        tokio::fs::create_dir_all(plugin_dir.as_ref().join(DOWNLOADS_DIR))
            .await
            .context("Failed to create downloads directory")?;

        info!("Unloaded LifecycleManager initialized successfully");
        Ok(Self {
            engine,
            linker,
            components: Arc::new(RwLock::new(components)),
            registry: Arc::new(RwLock::new(registry)),
            policy_registry: Arc::new(RwLock::new(policy_registry)),
            oci_client: Arc::new(oci_wasm::WasmClient::new(oci_client)),
            http_client,
            plugin_dir: plugin_dir.as_ref().to_path_buf(),
            environment_vars,
            secrets_manager,
        })
    }

    /// Creates a lifecycle manager from configuration parameters with environment variables
    #[instrument(skip_all, fields(plugin_dir = %plugin_dir.as_ref().display()))]
    pub async fn new_with_env(
        plugin_dir: impl AsRef<Path>,
        environment_vars: HashMap<String, String>,
    ) -> Result<Self> {
        // Use default secrets directory
        let default_secrets_dir = get_default_secrets_dir();

        Self::new_with_config(
            plugin_dir,
            environment_vars,
            default_secrets_dir,
            oci_client::Client::default(),
            reqwest::Client::default(),
        )
        .await
    }

    /// Creates a lifecycle manager from full configuration
    #[instrument(skip_all, fields(plugin_dir = %plugin_dir.as_ref().display()))]
    pub async fn new_with_config(
        plugin_dir: impl AsRef<Path>,
        environment_vars: HashMap<String, String>,
        secrets_dir: impl AsRef<Path>,
        oci_client: oci_client::Client,
        http_client: reqwest::Client,
    ) -> Result<Self> {
        Self::new_with_policy(
            plugin_dir,
            environment_vars,
            secrets_dir,
            oci_client,
            http_client,
        )
        .await
    }

    /// Creates a lifecycle manager from configuration parameters with custom clients
    #[instrument(skip_all)]
    pub async fn new_with_clients(
        plugin_dir: impl AsRef<Path>,
        environment_vars: HashMap<String, String>,
        oci_client: oci_client::Client,
        http_client: reqwest::Client,
    ) -> Result<Self> {
        // Use default secrets directory for backward compatibility
        let default_secrets_dir = get_default_secrets_dir();

        Self::new_with_policy(
            plugin_dir,
            environment_vars,
            default_secrets_dir,
            oci_client,
            http_client,
        )
        .await
    }

    /// Creates a lifecycle manager with custom clients and WASI state template
    #[instrument(skip_all)]
    async fn new_with_policy(
        plugin_dir: impl AsRef<Path>,
        environment_vars: HashMap<String, String>,
        secrets_dir: impl AsRef<Path>,
        oci_client: oci_client::Client,
        http_client: reqwest::Client,
    ) -> Result<Self> {
        let components_dir = plugin_dir.as_ref();

        if !components_dir.exists() {
            fs::create_dir_all(components_dir)?;
        }

        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        config.async_support(true);
        // Enable Wasmtime's built-in compilation cache for faster recompilation
        // Note: cache_config_load_default may not be available in this wasmtime version
        // if let Err(e) = config.cache_config_load_default() {
        //     warn!("Failed to load default cache config: {}", e);
        // }
        let engine = Arc::new(wasmtime::Engine::new(&config)?);

        info!("Creating new LifecycleManager");

        let mut registry = ComponentRegistry::new();
        let mut components = HashMap::new();
        let mut policy_registry = PolicyRegistry::default();

        // Create secrets manager
        let secrets_manager = Arc::new(SecretsManager::new(secrets_dir.as_ref().to_path_buf()));
        secrets_manager.ensure_secrets_dir().await?;

        let mut linker = Linker::new(engine.as_ref());
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;

        // Use the standard HTTP linker - filtering happens at WasiHttpView level
        wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;

        wasmtime_wasi_config::add_to_linker(
            &mut linker,
            |h: &mut WassetteWasiState<WasiState>| WasiConfig::from(&h.inner.wasi_config_vars),
        )?;

        let linker = Arc::new(linker);

        let loaded_components =
            load_components_parallel(plugin_dir.as_ref(), &engine, &linker).await?;

        for (component_instance, name) in loaded_components.into_iter() {
            let tool_metadata =
                component_exports_to_tools(&component_instance.component, &engine, true);
            registry
                .register_tools(&name, tool_metadata)
                .context("unable to insert component into registry")?;
            components.insert(name.clone(), component_instance);

            // Check for co-located policy file and restore policy association
            let policy_path = plugin_dir.as_ref().join(format!("{name}.policy.yaml"));
            if policy_path.exists() {
                match tokio::fs::read_to_string(&policy_path).await {
                    Ok(policy_content) => match PolicyParser::parse_str(&policy_content) {
                        Ok(policy) => {
                            match wasistate::create_wasi_state_template_from_policy(
                                &policy,
                                plugin_dir.as_ref(),
                                &environment_vars,
                                None, // No secrets during initial loading
                            ) {
                                Ok(wasi_template) => {
                                    policy_registry
                                        .component_policies
                                        .insert(name.clone(), Arc::new(wasi_template));
                                    info!(component_id = %name, "Restored policy association from co-located file");
                                }
                                Err(e) => {
                                    warn!(component_id = %name, error = %e, "Failed to create WASI template from policy");
                                }
                            }
                        }
                        Err(e) => {
                            warn!(component_id = %name, error = %e, "Failed to parse co-located policy file");
                        }
                    },
                    Err(e) => {
                        warn!(component_id = %name, error = %e, "Failed to read co-located policy file");
                    }
                }
            }
        }

        // Make sure the plugin dir exists and also create a subdirectory for temporary staging of downloaded files
        tokio::fs::create_dir_all(&plugin_dir)
            .await
            .context("Failed to create plugin directory")?;
        tokio::fs::create_dir_all(plugin_dir.as_ref().join(DOWNLOADS_DIR))
            .await
            .context("Failed to create downloads directory")?;

        info!("LifecycleManager initialized successfully");
        Ok(Self {
            engine,
            linker,
            components: Arc::new(RwLock::new(components)),
            registry: Arc::new(RwLock::new(registry)),
            policy_registry: Arc::new(RwLock::new(policy_registry)),
            oci_client: Arc::new(oci_wasm::WasmClient::new(oci_client)),
            http_client,
            plugin_dir: plugin_dir.as_ref().to_path_buf(),
            environment_vars,
            secrets_manager,
        })
    }

    /// Loads a new component from the given URI. This URI can be a file path, an OCI reference, or a URL.
    ///
    /// If a component with the given id already exists, it will be updated with the new component.
    /// Returns the new ID and whether or not this component was replaced.
    #[instrument(skip(self))]
    pub async fn load_component(&self, uri: &str) -> Result<(String, LoadResult)> {
        debug!(uri, "Loading component");

        let downloaded_resource =
            loader::load_resource::<ComponentResource>(uri, &self.oci_client, &self.http_client)
                .await?;

        // Use optimized loading for manual component loading too
        let id = downloaded_resource.id()?;
        let (component, _wasm_bytes) = self.load_component_optimized(downloaded_resource.as_ref(), &id).await
            .map_err(|e| anyhow::anyhow!("Failed to compile component from path: {}. Error: {}. Please ensure the file is a valid WebAssembly component.", downloaded_resource.as_ref().display(), e))?;
        // Pre-instantiate the component
        let instance_pre = self.linker.instantiate_pre(&component)?;
        let tool_metadata = component_exports_to_tools(&component, &self.engine, true);

        {
            let mut registry_write = self.registry.write().await;
            registry_write.unregister_component(&id);
            registry_write.register_tools(&id, tool_metadata.clone())?;
        }

        if let Err(e) = downloaded_resource.copy_to(&self.plugin_dir).await {
            let mut registry_write = self.registry.write().await;
            registry_write.unregister_component(&id);
            bail!(
                "Failed to copy component to destination: {}. Error: {}",
                self.plugin_dir.display(),
                e
            );
        }

        // Save metadata for future startups
        if let Ok(validation_stamp) =
            Self::create_validation_stamp(&self.component_path(&id), false).await
        {
            if let Err(e) = self
                .save_component_metadata(&id, &tool_metadata, validation_stamp)
                .await
            {
                warn!(component_id = %id, error = %e, "Failed to save component metadata");
            }
        }

        let res = self
            .components
            .write()
            .await
            .insert(
                id.clone(),
                ComponentInstance {
                    component: Arc::new(component),
                    instance_pre: Arc::new(instance_pre),
                },
            )
            .map(|_| LoadResult::Replaced)
            .unwrap_or(LoadResult::New);

        info!("Successfully loaded component");
        Ok((id, res))
    }

    /// Helper function to remove a file with consistent logging and error handling
    async fn remove_file_if_exists(
        &self,
        file_path: &std::path::Path,
        file_type: &str,
        component_id: &str,
    ) -> Result<()> {
        match tokio::fs::remove_file(file_path).await {
            Ok(()) => {
                debug!(
                    component_id = %component_id,
                    path = %file_path.display(),
                    "Removed {}", file_type
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!(
                    component_id = %component_id,
                    path = %file_path.display(),
                    "{} already absent", file_type
                );
            }
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Failed to remove {} at {}: {}",
                    file_type,
                    file_path.display(),
                    e
                ));
            }
        }
        Ok(())
    }

    /// Unloads the component with the specified id. This removes the component from the runtime
    /// and removes all associated files from disk, making it the reverse operation of load_component.
    /// This function fails if any files cannot be removed (except when they don't exist).
    #[instrument(skip(self))]
    pub async fn unload_component(&self, id: &str) -> Result<()> {
        debug!("Unloading component and removing files from disk");

        // Remove files first, then clean up memory on success
        let component_file = self.component_path(id);
        self.remove_file_if_exists(&component_file, "component file", id)
            .await?;

        let policy_path = self.get_component_policy_path(id);
        self.remove_file_if_exists(&policy_path, "policy file", id)
            .await?;

        let metadata_path = self.get_component_metadata_path(id);
        self.remove_file_if_exists(&metadata_path, "policy metadata file", id)
            .await?;

        // Remove new cache files
        let component_metadata_path = self.component_metadata_path(id);
        self.remove_file_if_exists(&component_metadata_path, "component metadata file", id)
            .await?;

        let precompiled_path = self.component_precompiled_path(id);
        self.remove_file_if_exists(&precompiled_path, "precompiled component file", id)
            .await?;

        // Only cleanup memory after all files are successfully removed
        self.components.write().await.remove(id);
        self.registry.write().await.unregister_component(id);
        self.cleanup_policy_registry(id).await;

        info!(component_id = %id, "Component unloaded successfully");
        Ok(())
    }

    /// Returns the component ID for a given tool name.
    /// If there are multiple components with the same tool name, returns an error.
    #[instrument(skip(self))]
    pub async fn get_component_id_for_tool(&self, tool_name: &str) -> Result<String> {
        let registry = self.registry.read().await;
        let tool_infos = registry
            .get_tool_info(tool_name)
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
        self.registry.read().await.list_tools()
    }

    /// Returns the requested component. Returns `None` if the component is not found.
    #[instrument(skip(self))]
    pub async fn get_component(&self, component_id: &str) -> Option<ComponentInstance> {
        self.components.read().await.get(component_id).cloned()
    }

    /// Lists all loaded components by their IDs
    #[instrument(skip(self))]
    pub async fn list_components(&self) -> Vec<String> {
        self.components.read().await.keys().cloned().collect()
    }

    /// Gets the schema for a specific component
    #[instrument(skip(self))]
    pub async fn get_component_schema(&self, component_id: &str) -> Option<Value> {
        let component_instance = self.get_component(component_id).await?;
        Some(component_exports_to_json_schema(
            &component_instance.component,
            self.engine.as_ref(),
            true,
        ))
    }

    fn component_path(&self, component_id: &str) -> PathBuf {
        self.plugin_dir.join(format!("{component_id}.wasm"))
    }

    /// Get the path to component metadata file
    fn component_metadata_path(&self, component_id: &str) -> PathBuf {
        self.plugin_dir
            .join(format!("{component_id}.{METADATA_EXT}"))
    }

    /// Get the path to precompiled component file
    fn component_precompiled_path(&self, component_id: &str) -> PathBuf {
        self.plugin_dir
            .join(format!("{component_id}.{PRECOMPILED_EXT}"))
    }

    /// Create validation stamp for a file
    async fn create_validation_stamp(
        file_path: &Path,
        compute_hash: bool,
    ) -> Result<ValidationStamp> {
        let metadata = tokio::fs::metadata(file_path)
            .await
            .context("Failed to read file metadata")?;

        let file_size = metadata.len();
        let mtime = metadata
            .modified()
            .context("Failed to get modification time")?
            .duration_since(std::time::UNIX_EPOCH)
            .context("Invalid modification time")?
            .as_secs();

        let content_hash = if compute_hash {
            let content = tokio::fs::read(file_path)
                .await
                .context("Failed to read file for hashing")?;
            let mut hasher = Sha256::new();
            hasher.update(&content);
            Some(format!("{:x}", hasher.finalize()))
        } else {
            None
        };

        Ok(ValidationStamp {
            file_size,
            mtime,
            content_hash,
        })
    }

    /// Check if validation stamp matches current file
    async fn validate_stamp(file_path: &Path, stamp: &ValidationStamp) -> bool {
        let Ok(metadata) = tokio::fs::metadata(file_path).await else {
            return false;
        };

        if metadata.len() != stamp.file_size {
            return false;
        }

        let Ok(mtime) = metadata
            .modified()
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))
            .and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))
            })
            .map(|d| d.as_secs())
        else {
            return false;
        };

        if mtime != stamp.mtime {
            return false;
        }

        // If we have a content hash, verify it
        if let Some(expected_hash) = &stamp.content_hash {
            let Ok(content) = tokio::fs::read(file_path).await else {
                return false;
            };
            let mut hasher = Sha256::new();
            hasher.update(&content);
            let actual_hash = format!("{:x}", hasher.finalize());
            return &actual_hash == expected_hash;
        }

        true
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

        let metadata_path = self.component_metadata_path(component_id);
        let metadata_json = serde_json::to_string_pretty(&metadata)
            .context("Failed to serialize component metadata")?;

        tokio::fs::write(&metadata_path, metadata_json)
            .await
            .context("Failed to write component metadata")?;

        info!(component_id = %component_id, "Saved component metadata");
        Ok(())
    }

    /// Load component metadata from disk
    async fn load_component_metadata(
        &self,
        component_id: &str,
    ) -> Result<Option<ComponentMetadata>> {
        let metadata_path = self.component_metadata_path(component_id);

        if !metadata_path.exists() {
            return Ok(None);
        }

        let metadata_content = tokio::fs::read_to_string(&metadata_path)
            .await
            .context("Failed to read component metadata")?;

        let metadata: ComponentMetadata = serde_json::from_str(&metadata_content)
            .context("Failed to parse component metadata")?;

        Ok(Some(metadata))
    }

    /// Save precompiled component to disk
    async fn save_precompiled_component(
        &self,
        component_id: &str,
        wasm_bytes: &[u8],
    ) -> Result<()> {
        let precompiled_data = self
            .engine
            .precompile_component(wasm_bytes)
            .context("Failed to precompile component")?;

        let precompiled_path = self.component_precompiled_path(component_id);
        tokio::fs::write(&precompiled_path, precompiled_data)
            .await
            .context("Failed to write precompiled component")?;

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
            match unsafe { Component::deserialize_file(&self.engine, &precompiled_path) } {
                Ok(component) => {
                    debug!(component_id = %component_id, "Loaded component from precompiled cache");
                    // Still need the wasm bytes for metadata/validation
                    let wasm_bytes = tokio::fs::read(wasm_path)
                        .await
                        .context("Failed to read wasm file")?;
                    return Ok((component, wasm_bytes));
                }
                Err(e) => {
                    warn!(component_id = %component_id, error = %e, "Failed to load precompiled component, falling back to compilation");
                }
            }
        }

        // Fall back to compilation
        let wasm_bytes = tokio::fs::read(wasm_path)
            .await
            .context("Failed to read wasm file")?;

        let component =
            Component::new(&self.engine, &wasm_bytes).context("Failed to compile component")?;

        // Save precompiled version for next time (async, don't block on this)
        if let Err(e) = self
            .save_precompiled_component(component_id, &wasm_bytes)
            .await
        {
            warn!(component_id = %component_id, error = %e, "Failed to save precompiled component");
        }

        debug!(component_id = %component_id, "Compiled component and saved to cache");
        Ok((component, wasm_bytes))
    }

    async fn get_wasi_state_for_component(
        &self,
        component_id: &str,
    ) -> Result<(WassetteWasiState<WasiState>, Option<CustomResourceLimiter>)> {
        let policy_registry = self.policy_registry.read().await;

        let policy_template = policy_registry
            .component_policies
            .get(component_id)
            .cloned()
            .unwrap_or_else(Self::create_default_policy_template);

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
        let component = self
            .get_component(component_id)
            .await
            .ok_or_else(|| anyhow!("Component not found: {}", component_id))?;

        let (state, resource_limiter) = self.get_wasi_state_for_component(component_id).await?;

        let mut store = Store::new(self.engine.as_ref(), state);

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

        let instance = component.instance_pre.instantiate_async(&mut store).await?;

        // Use the new function identifier lookup instead of dot-splitting
        let function_id = self
            .registry
            .read()
            .await
            .get_function_identifier(function_name)
            .ok_or_else(|| anyhow!("Unknown tool name: {}", function_name))?
            .clone();

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

        func.call_async(&mut store, &argument_vals, &mut results)
            .await?;

        let result_json = vals_to_json(&results);

        if let Some(result_str) = result_json.as_str() {
            Ok(result_str.to_string())
        } else {
            Ok(serde_json::to_string(&result_json)?)
        }
    }

    /// Load existing components from plugin directory in the background with bounded parallelism
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
        let mut entries = tokio::fs::read_dir(&self.plugin_dir).await?;
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
        let mut entries = tokio::fs::read_dir(&self.plugin_dir).await?;
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
                if Self::validate_stamp(&entry_path, &metadata.validation_stamp).await {
                    // Register tools from cached metadata
                    let mut registry_write = self.registry.write().await;
                    let tool_metadata: Vec<ToolMetadata> = metadata
                        .function_identifiers
                        .into_iter()
                        .zip(metadata.tool_schemas)
                        .zip(metadata.tool_names)
                        .map(|((identifier, schema), normalized_name)| ToolMetadata {
                            identifier,
                            schema,
                            normalized_name,
                        })
                        .collect();

                    if let Err(e) = registry_write.register_tools(component_id, tool_metadata) {
                        warn!(component_id = %component_id, error = %e, "Failed to register tools from metadata");
                        continue;
                    }

                    loaded_count += 1;
                    debug!(component_id = %component_id, "Registered tools from cached metadata");
                    continue;
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

        let start_time = Instant::now();

        // Check if component is already loaded in memory (from metadata)
        if self.components.read().await.contains_key(&component_id) {
            debug!(component_id = %component_id, "Component already loaded in memory");
            return Ok(false);
        }

        // Load component using optimized path (precompiled cache or fresh compilation)
        let (component, _wasm_bytes) = self
            .load_component_optimized(&entry_path, &component_id)
            .await?;

        let instance_pre = self
            .linker
            .instantiate_pre(&component)
            .context("failed to instantiate component")?;

        let component_instance = ComponentInstance {
            component: Arc::new(component),
            instance_pre: Arc::new(instance_pre),
        };

        // Get tool metadata
        let tool_metadata =
            component_exports_to_tools(&component_instance.component, &self.engine, true);

        // Register tools (only if not already registered from metadata)
        {
            let mut registry_write = self.registry.write().await;
            if !registry_write.component_map.contains_key(&component_id) {
                registry_write
                    .register_tools(&component_id, tool_metadata.clone())
                    .context("unable to insert component into registry")?;
            }
        }

        // Store component in memory
        {
            let mut components_write = self.components.write().await;
            components_write.insert(component_id.clone(), component_instance);
        }

        // Create validation stamp and save metadata for future startups
        if let Ok(validation_stamp) = Self::create_validation_stamp(&entry_path, false).await {
            if let Err(e) = self
                .save_component_metadata(&component_id, &tool_metadata, validation_stamp)
                .await
            {
                warn!(component_id = %component_id, error = %e, "Failed to save component metadata");
            }
        }

        // Handle co-located policy file
        let policy_path = self.plugin_dir.join(format!("{component_id}.policy.yaml"));
        if policy_path.exists() {
            match tokio::fs::read_to_string(&policy_path).await {
                Ok(policy_content) => match PolicyParser::parse_str(&policy_content) {
                    Ok(policy) => {
                        match wasistate::create_wasi_state_template_from_policy(
                            &policy,
                            &self.plugin_dir,
                            &self.environment_vars,
                            None,
                        ) {
                            Ok(wasi_template) => {
                                let mut policy_registry_write = self.policy_registry.write().await;
                                policy_registry_write
                                    .component_policies
                                    .insert(component_id.clone(), Arc::new(wasi_template));
                                info!(component_id = %component_id, "Restored policy association from co-located file");
                            }
                            Err(e) => {
                                warn!(component_id = %component_id, error = %e, "Failed to create WASI template from policy");
                            }
                        }
                    }
                    Err(e) => {
                        warn!(component_id = %component_id, error = %e, "Failed to parse co-located policy file");
                    }
                },
                Err(e) => {
                    warn!(component_id = %component_id, error = %e, "Failed to read co-located policy file");
                }
            }
        }

        info!(component_id = %component_id, elapsed = ?start_time.elapsed(), "component loaded");
        Ok(true)
    }

    // Granular permission system methods
}
// Load components in parallel for improved startup performance
async fn load_components_parallel(
    plugin_dir: &Path,
    engine: &Arc<Engine>,
    linker: &Arc<Linker<WassetteWasiState<WasiState>>>,
) -> Result<Vec<(ComponentInstance, String)>> {
    let mut entries = tokio::fs::read_dir(plugin_dir).await?;
    let mut load_futures = Vec::new();

    while let Some(entry) = entries.next_entry().await? {
        let engine = engine.clone();
        let linker = linker.clone();
        let future = async move {
            match load_component_from_entry(engine, &linker, entry).await {
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
    /// Revoke storage permission from a component by URI (removes all access types for that URI)
    #[instrument(skip(self))]
    pub async fn revoke_storage_permission_by_uri(
        &self,
        component_id: &str,
        uri: &str,
    ) -> Result<()> {
        info!(
            component_id,
            uri, "Revoking storage permission by URI from component"
        );
        if !self.components.read().await.contains_key(component_id) {
            return Err(anyhow!("Component not found: {}", component_id));
        }

        if uri.is_empty() {
            return Err(anyhow!("Storage URI cannot be empty"));
        }

        let mut policy = self.load_or_create_component_policy(component_id).await?;
        self.remove_storage_permission_by_uri_from_policy(&mut policy, uri)?;
        self.save_component_policy(component_id, &policy).await?;
        self.update_policy_registry(component_id, &policy).await?;

        info!(component_id, uri, "Storage permission revoked successfully");
        Ok(())
    }

    /// Remove all storage permissions for a specific URI from policy
    fn remove_storage_permission_by_uri_from_policy(
        &self,
        policy: &mut policy::PolicyDocument,
        uri: &str,
    ) -> Result<()> {
        if let Some(storage_perms) = &mut policy.permissions.storage {
            if let Some(allow_set) = &mut storage_perms.allow {
                allow_set.retain(|perm| perm.uri != uri);
                // Clean up empty structures
                if allow_set.is_empty() {
                    storage_perms.allow = None;
                }
            }
        }
        Ok(())
    }

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
    engine: Arc<Engine>,
    linker: &Linker<WassetteWasiState<WasiState>>,
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
    let component =
        tokio::task::spawn_blocking(move || Component::from_file(&engine, entry_path)).await??;
    let name = entry
        .path()
        .file_stem()
        .and_then(|s| s.to_str())
        .map(String::from)
        .context("wasm file didn't have a valid file name")?;
    info!(component_id = %name, elapsed = ?start_time.elapsed(), "component loaded");
    let instance_pre = linker.instantiate_pre(&component)?;
    Ok(Some((
        ComponentInstance {
            component: Arc::new(component),
            instance_pre: Arc::new(instance_pre),
        },
        name,
    )))
}

#[cfg(test)]
mod tests {
    use std::ops::Deref;
    use std::path::PathBuf;
    use std::process::Command;

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
        let expected_path = manager.plugin_dir.join("test-component.wasm");
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
        let policy_path = manager.plugin_dir.join("test-policy.yaml");
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
}
