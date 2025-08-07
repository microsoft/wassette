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
use futures::TryStreamExt;
use policy::PolicyParser;
use serde_json::Value;
use tokio::fs::DirEntry;
use tokio::sync::RwLock;
use tracing::{debug, info, instrument, warn};
use wasmtime::component::{Component, Linker};
use wasmtime::{Engine, Store};
use wasmtime_wasi_config::WasiConfig;

mod http;
mod loader;
mod policy_internal;
mod wasistate;

pub use http::WassetteWasiState;
use loader::{ComponentResource, PolicyResource};
use policy_internal::PolicyRegistry;
pub use policy_internal::{PermissionGrantRequest, PermissionRule, PolicyInfo};
use wasistate::WasiState;
pub use wasistate::{create_wasi_state_template_from_policy, WasiStateTemplate};

const DOWNLOADS_DIR: &str = "downloads";

#[derive(Debug, Clone)]
struct ToolInfo {
    component_id: String,
    identifier: FunctionIdentifier,
    schema: Value,
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
    components: Arc<RwLock<HashMap<String, Arc<Component>>>>,
    registry: Arc<RwLock<ComponentRegistry>>,
    policy_registry: Arc<RwLock<PolicyRegistry>>,
    oci_client: Arc<oci_wasm::WasmClient>,
    http_client: reqwest::Client,
    plugin_dir: PathBuf,
}

impl LifecycleManager {
    /// Creates a lifecycle manager from configuration parameters
    /// This is the primary way to create a LifecycleManager for most use cases
    #[instrument(skip_all, fields(plugin_dir = %plugin_dir.as_ref().display()))]
    pub async fn new(plugin_dir: impl AsRef<Path>) -> Result<Self> {
        Self::new_with_clients(
            plugin_dir,
            oci_client::Client::default(),
            reqwest::Client::default(),
        )
        .await
    }

    /// Creates a lifecycle manager from configuration parameters with custom clients
    #[instrument(skip_all)]
    pub async fn new_with_clients(
        plugin_dir: impl AsRef<Path>,
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
        let engine = Arc::new(wasmtime::Engine::new(&config)?);

        // Create the lifecycle manager
        Self::new_with_policy(engine, components_dir, oci_client, http_client).await
    }

    /// Creates a lifecycle manager with custom clients and WASI state template
    #[instrument(skip_all)]
    async fn new_with_policy(
        engine: Arc<Engine>,
        plugin_dir: impl AsRef<Path>,
        oci_client: oci_client::Client,
        http_client: reqwest::Client,
    ) -> Result<Self> {
        info!("Creating new LifecycleManager");

        let mut registry = ComponentRegistry::new();
        let mut components = HashMap::new();
        let mut policy_registry = PolicyRegistry::default();

        let loaded_components =
            tokio_stream::wrappers::ReadDirStream::new(tokio::fs::read_dir(&plugin_dir).await?)
                .map_err(anyhow::Error::from)
                .try_filter_map(|entry| {
                    let value = engine.clone();
                    async move { load_component_from_entry(value, entry).await }
                })
                .try_collect::<Vec<_>>()
                .await?;

        for (component, name) in loaded_components.into_iter() {
            let tool_metadata = component_exports_to_tools(&component, &engine, true);
            registry
                .register_tools(&name, tool_metadata)
                .context("unable to insert component into registry")?;
            components.insert(name.clone(), Arc::new(component));

            // Check for co-located policy file and restore policy association
            let policy_path = plugin_dir.as_ref().join(format!("{name}.policy.yaml"));
            if policy_path.exists() {
                match tokio::fs::read_to_string(&policy_path).await {
                    Ok(policy_content) => match PolicyParser::parse_str(&policy_content) {
                        Ok(policy) => {
                            match wasistate::create_wasi_state_template_from_policy(
                                &policy,
                                plugin_dir.as_ref(),
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
            components: Arc::new(RwLock::new(components)),
            registry: Arc::new(RwLock::new(registry)),
            policy_registry: Arc::new(RwLock::new(policy_registry)),
            oci_client: Arc::new(oci_wasm::WasmClient::new(oci_client)),
            http_client,
            plugin_dir: plugin_dir.as_ref().to_path_buf(),
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

        let wasm_bytes = tokio::fs::read(downloaded_resource.as_ref())
            .await
            .context("Failed to read component file")?;

        let component = Component::new(&self.engine, wasm_bytes).map_err(|e| anyhow::anyhow!("Failed to compile component from path: {}. Error: {}. Please ensure the file is a valid WebAssembly component.", downloaded_resource.as_ref().display(), e))?;
        let id = downloaded_resource.id()?;
        let tool_metadata = component_exports_to_tools(&component, &self.engine, true);

        {
            let mut registry_write = self.registry.write().await;
            registry_write.unregister_component(&id);
            registry_write.register_tools(&id, tool_metadata)?;
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

        let res = self
            .components
            .write()
            .await
            .insert(id.clone(), Arc::new(component))
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
    pub async fn get_component(&self, component_id: &str) -> Option<Arc<Component>> {
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
        let component = self.get_component(component_id).await?;
        Some(component_exports_to_json_schema(
            &component,
            self.engine.as_ref(),
            true,
        ))
    }

    fn component_path(&self, component_id: &str) -> PathBuf {
        self.plugin_dir.join(format!("{component_id}.wasm"))
    }

    async fn get_wasi_state_for_component(
        &self,
        component_id: &str,
    ) -> Result<WassetteWasiState<WasiState>> {
        let policy_registry = self.policy_registry.read().await;

        let policy_template = policy_registry
            .component_policies
            .get(component_id)
            .cloned()
            .unwrap_or_else(Self::create_default_policy_template);

        let wasi_state = policy_template.build()?;
        let allowed_hosts = policy_template.allowed_hosts.clone();

        WassetteWasiState::new(wasi_state, allowed_hosts)
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

        let state = self.get_wasi_state_for_component(component_id).await?;

        let mut linker = Linker::new(self.engine.as_ref());
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;

        // Use the standard HTTP linker - filtering happens at WasiHttpView level
        wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;

        wasmtime_wasi_config::add_to_linker(
            &mut linker,
            |h: &mut WassetteWasiState<WasiState>| WasiConfig::from(&h.inner.wasi_config_vars),
        )?;

        let mut store = Store::new(self.engine.as_ref(), state);

        let instance = linker.instantiate_async(&mut store, &component).await?;

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

    // Granular permission system methods
}

async fn load_component_from_entry(
    engine: Arc<Engine>,
    entry: DirEntry,
) -> Result<Option<(Component, String)>> {
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
    Ok(Some((component, name)))
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
        let template = create_wasi_state_template_from_policy(&policy, temp_dir.path())?;

        assert_eq!(template.allowed_hosts.len(), 2);
        assert!(template.allowed_hosts.contains("api.example.com"));
        assert!(template.allowed_hosts.contains("cdn.example.com"));

        Ok(())
    }
}
