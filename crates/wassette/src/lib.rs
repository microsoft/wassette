use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use component2json::{
    component_exports_to_json_schema, create_placeholder_results, json_to_vals, vals_to_json,
};
use futures::stream::TryStreamExt;
use policy_mcp::PolicyParser;
use serde_json::Value;
use tokio::fs::DirEntry;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use tracing::{debug, error, info, instrument, warn};
use wasmtime::component::{Component, Linker};
use wasmtime::{Engine, Store};
use wasmtime_wasi::p2::WasiCtxBuilder;
use wasmtime_wasi_config::{WasiConfig, WasiConfigVariables};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

mod wasistate;
pub use wasistate::{create_wasi_state_template_from_policy, WasiStateTemplate};

struct WasiState {
    ctx: wasmtime_wasi::p2::WasiCtx,
    table: wasmtime_wasi::ResourceTable,
    http: wasmtime_wasi_http::WasiHttpCtx,
    wasi_config_vars: WasiConfigVariables,
}

impl wasmtime_wasi::p2::IoView for WasiState {
    fn table(&mut self) -> &mut wasmtime_wasi::ResourceTable {
        &mut self.table
    }
}

impl wasmtime_wasi::p2::WasiView for WasiState {
    fn ctx(&mut self) -> &mut wasmtime_wasi::p2::WasiCtx {
        &mut self.ctx
    }
}

impl WasiHttpView for WasiState {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }
}

impl WasiStateTemplate {
    fn build(&self) -> anyhow::Result<WasiState> {
        let mut ctx_builder = WasiCtxBuilder::new();
        if self.allow_stdout {
            ctx_builder.inherit_stdout();
        }
        if self.allow_stderr {
            ctx_builder.inherit_stderr();
        }
        ctx_builder.inherit_args();
        if self.allow_args {
            ctx_builder.inherit_args();
        }
        ctx_builder.inherit_network();
        ctx_builder.allow_tcp(self.network_perms.allow_tcp);
        ctx_builder.allow_udp(self.network_perms.allow_udp);
        ctx_builder.allow_ip_name_lookup(self.network_perms.allow_ip_name_lookup);
        for preopened_dir in &self.preopened_dirs {
            ctx_builder.preopened_dir(
                preopened_dir.host_path.as_path(),
                preopened_dir.guest_path.as_str(),
                preopened_dir.dir_perms,
                preopened_dir.file_perms,
            )?;
        }

        Ok(WasiState {
            ctx: ctx_builder.build(),
            table: wasmtime_wasi::ResourceTable::default(),
            http: WasiHttpCtx::new(),
            wasi_config_vars: WasiConfigVariables::from_iter(self.config_vars.clone()),
        })
    }
}

const DOWNLOADS_DIR: &str = "downloads";

#[derive(Debug, Clone)]
struct ToolInfo {
    component_id: String,
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

    fn register_component(&mut self, component_id: &str, schema: &Value) -> Result<()> {
        let tools = schema["tools"]
            .as_array()
            .context("Schema does not contain tools array")?;

        let mut component_tools = Vec::new();

        for tool in tools {
            let name = tool["name"]
                .as_str()
                .context("Tool name is not a string")?
                .to_string();

            let tool_info = ToolInfo {
                component_id: component_id.to_string(),
                schema: tool.clone(),
            };

            self.tool_map
                .entry(name.clone())
                .or_default()
                .push(tool_info);

            component_tools.push(name);
        }

        self.component_map
            .insert(component_id.to_string(), component_tools);
        Ok(())
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

#[derive(Default)]
struct PolicyRegistry {
    component_policies: HashMap<String, Arc<WasiStateTemplate>>,
}

#[derive(Debug, Clone)]
pub struct PolicyInfo {
    pub policy_id: String,
    pub source_uri: String,
    pub local_path: PathBuf,
    pub component_id: String,
    pub created_at: std::time::SystemTime,
}

enum DownloadedPolicy {
    Local(PathBuf),
    Temp((tempfile::TempDir, PathBuf)),
}

impl AsRef<Path> for DownloadedPolicy {
    fn as_ref(&self) -> &Path {
        match self {
            DownloadedPolicy::Local(path) => path.as_path(),
            DownloadedPolicy::Temp((_, path)) => path.as_path(),
        }
    }
}

impl DownloadedPolicy {
    async fn new_temp_file(name: impl AsRef<str>) -> Result<(Self, tokio::fs::File)> {
        let tempdir = tokio::task::spawn_blocking(tempfile::tempdir).await??;
        let file_path = tempdir.path().join(format!("{}.yaml", name.as_ref()));
        let temp_file = tokio::fs::File::create(&file_path).await?;
        Ok((DownloadedPolicy::Temp((tempdir, file_path)), temp_file))
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
        Self::new_with_policy(
            engine,
            components_dir,
            oci_client,
            http_client,
            WasiStateTemplate::default(),
        )
        .await
    }

    /// Creates a lifecycle manager with custom clients and WASI state template
    #[instrument(skip_all)]
    async fn new_with_policy(
        engine: Arc<Engine>,
        plugin_dir: impl AsRef<Path>,
        oci_cli: oci_client::Client,
        http_client: reqwest::Client,
        _wasi_state_template: WasiStateTemplate,
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
            let schema = component_exports_to_json_schema(&component, &engine, true);
            registry
                .register_component(&name, &schema)
                .context("unable to insert component into registry")?;
            components.insert(name.clone(), Arc::new(component));

            // Check for co-located policy file and restore policy association
            let policy_path = plugin_dir.as_ref().join(format!("{}.policy.yaml", name));
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
            oci_client: Arc::new(oci_wasm::WasmClient::new(oci_cli)),
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
        debug!("Loading component from path");
        let uri = uri.trim();
        let (scheme, reference) = uri
            .split_once("://")
            .context("Invalid component reference. Should be of the form scheme://reference")?;

        let file = match scheme {
            "file" => self.load_file(reference).await?,
            "oci" => self.load_oci(reference).await?,
            "https" => self.load_url(uri).await?,
            _ => bail!("Unsupported component scheme: {}", scheme),
        };

        // Read the file so we can use the bytes first to parse the wit and then to load and compile
        // the component
        let wasm_bytes = tokio::fs::read(file.as_ref())
            .await
            .context("Failed to read component file")?;

        // NOTE(thomastaylor312): Unfortunately the rust tooling (and I think some of the others),
        // doesn't preserve the package ID from the wit world defined for the component. It just
        // ends up as "root-component". So for now we rely on the file name to give us a unique ID
        // for the component.
        // let decoded = wit_parser::decoding::decode(&wasm_bytes)
        //     .map_err(|e| anyhow::anyhow!("Failed to decode component from path: {}. Error: {}. Please ensure the file is a valid WebAssembly component.", file.as_ref().display(), e))?;

        // let pkg_id = decoded.package();
        // // SAFETY: The package ID is guaranteed to be valid because we just decoded it
        // let pkg = decoded.resolve().packages.get(pkg_id).unwrap();
        // // Format the package name without the colon so it is valid on all systems. We are using the
        // // package name as a unique key on the filesystem as well
        // let id = format!("{}-{}", pkg.name.namespace, pkg.name.name);

        // Load the component to see if it is valid
        let component = Component::new(&self.engine, wasm_bytes).map_err(|e| anyhow::anyhow!("Failed to compile component from path: {}. Error: {}. Please ensure the file is a valid WebAssembly component.", file.as_ref().display(), e))?;
        let id = file.id()?;
        let schema = component_exports_to_json_schema(&component, &self.engine, true);

        {
            let mut registry_write = self.registry.write().await;
            registry_write.unregister_component(&id);
            registry_write.register_component(&id, &schema)?;
        }

        // Now that we've gotten here, we know everything is valid, so copy the component to the directory
        if let Err(e) = file.copy_to(&self.plugin_dir).await {
            // Unregister the component if copy failed
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

    async fn load_file(&self, path: impl AsRef<Path>) -> Result<DownloadedComponent> {
        // Validate that the path is fully qualified
        if !path.as_ref().is_absolute() {
            error!("Component path must be fully qualified");
            bail!("Component path must be fully qualified. Please provide an absolute path to the WebAssembly component file.");
        }

        // Validate path exists
        if !tokio::fs::try_exists(path.as_ref()).await? {
            error!("Component path does not exist: {}", path.as_ref().display());
            bail!("Component path does not exist: {}. Please provide a valid path to a WebAssembly component file.", path.as_ref().display());
        }

        // Validate file extension
        if path.as_ref().extension().unwrap_or_default() != "wasm" {
            error!("Invalid file extension for component");
            bail!("Invalid file extension for component: {}. Component file must have .wasm extension.", path.as_ref().display());
        }

        Ok(DownloadedComponent::Local(path.as_ref().to_path_buf()))
    }

    async fn load_oci(&self, reference: &str) -> Result<DownloadedComponent> {
        let reference: oci_client::Reference =
            reference.parse().context("Failed to parse OCI reference")?;
        let data = self
            .oci_client
            .pull(&reference, &oci_client::secrets::RegistryAuth::Anonymous)
            .await?;
        let (downloaded_component, mut file) =
            DownloadedComponent::new_temp_file(reference.repository().replace('/', "_")).await?;
        // Per the wasm OCI spec, the component data is in the first layer, which is also validated
        // by the library
        file.write_all(&data.layers[0].data).await?;
        Ok(downloaded_component)
    }

    async fn load_url(&self, url: &str) -> Result<DownloadedComponent> {
        let resp = self.http_client.get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!(
                "Failed to download component from URL: {}. Status code: {}\nBody: {}",
                url,
                status,
                body
            );
        }
        let name = resp
            .url()
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .context("Failed to discover name from URL")?
            .trim_end_matches(".wasm");
        let (downloaded_component, mut file) = DownloadedComponent::new_temp_file(name).await?;
        let stream = resp.bytes_stream();
        let mut reader = tokio_util::io::StreamReader::new(stream.map_err(std::io::Error::other));
        tokio::io::copy(&mut reader, &mut file)
            .await
            .context("Failed to write downloaded component to temp file")?;
        Ok(downloaded_component)
    }

    /// Unloads the component with the specified id. This does not remove the installed component,
    /// only unloads it from the runtime. Use [`LifecycleManager::uninstall_component`] to remove
    /// the component from the system.
    #[instrument(skip(self))]
    pub async fn unload_component(&self, id: &str) {
        debug!("Unloading component");
        self.components.write().await.remove(id);
        self.registry.write().await.unregister_component(id);
    }

    /// Uninstalls the component from the system. This removes the component from the runtime and
    /// removes the component from disk.
    #[instrument(skip(self))]
    pub async fn uninstall_component(&self, id: &str) -> Result<()> {
        debug!("Uninstalling component");
        self.unload_component(id).await;
        let component_file = self.component_path(id);
        tokio::fs::remove_file(&component_file)
            .await
            .context(format!(
                "Failed to remove component file at {}. Please remove the file manually.",
                component_file.display()
            ))
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
        self.plugin_dir.join(format!("{}.wasm", component_id))
    }

    fn get_component_policy_path(&self, component_id: &str) -> PathBuf {
        self.plugin_dir
            .join(format!("{}.policy.yaml", component_id))
    }

    fn create_default_policy_template() -> Arc<WasiStateTemplate> {
        Arc::new(WasiStateTemplate::default())
    }

    async fn get_wasi_state_for_component(&self, component_id: &str) -> Result<WasiState> {
        let policy_registry = self.policy_registry.read().await;

        let policy_template = policy_registry
            .component_policies
            .get(component_id)
            .cloned()
            .unwrap_or_else(|| Self::create_default_policy_template());

        policy_template.build()
    }

    pub async fn attach_policy(&self, component_id: &str, policy_uri: &str) -> Result<()> {
        info!(component_id, policy_uri, "Attaching policy to component");

        if !self.components.read().await.contains_key(component_id) {
            return Err(anyhow!("Component not found: {}", component_id));
        }

        let downloaded_policy = self.download_policy(policy_uri).await?;

        let policy_content = tokio::fs::read_to_string(downloaded_policy.as_ref()).await?;
        let policy = PolicyParser::parse_str(&policy_content)?;

        let policy_path = self.get_component_policy_path(component_id);
        tokio::fs::copy(downloaded_policy.as_ref(), &policy_path).await?;

        // Store metadata about the policy source
        let metadata = serde_json::json!({
            "source_uri": policy_uri,
            "attached_at": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
        });
        let metadata_path = self
            .plugin_dir
            .join(format!("{}.policy.meta.json", component_id));
        tokio::fs::write(&metadata_path, serde_json::to_string_pretty(&metadata)?).await?;

        let wasi_template =
            wasistate::create_wasi_state_template_from_policy(&policy, &self.plugin_dir)?;
        self.policy_registry
            .write()
            .await
            .component_policies
            .insert(component_id.to_string(), Arc::new(wasi_template));

        info!(component_id, policy_uri, "Policy attached successfully");
        Ok(())
    }

    pub async fn detach_policy(&self, component_id: &str) -> Result<()> {
        info!(component_id, "Detaching policy from component");

        self.policy_registry
            .write()
            .await
            .component_policies
            .remove(component_id);

        let policy_path = self.get_component_policy_path(component_id);
        if policy_path.exists() {
            tokio::fs::remove_file(&policy_path).await?;
        }

        let metadata_path = self
            .plugin_dir
            .join(format!("{}.policy.meta.json", component_id));
        if metadata_path.exists() {
            tokio::fs::remove_file(&metadata_path).await?;
        }

        info!(component_id, "Policy detached successfully");
        Ok(())
    }

    pub async fn get_policy_info(&self, component_id: &str) -> Option<PolicyInfo> {
        let policy_path = self.get_component_policy_path(component_id);
        if !policy_path.exists() {
            return None;
        }

        let metadata_path = self
            .plugin_dir
            .join(format!("{}.policy.meta.json", component_id));
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
            policy_id: format!("{}-policy", component_id),
            source_uri,
            local_path: policy_path,
            component_id: component_id.to_string(),
            created_at,
        })
    }

    async fn download_policy(&self, uri: &str) -> Result<DownloadedPolicy> {
        let uri = uri.trim();
        let (scheme, reference) = uri
            .split_once("://")
            .context("Invalid policy URI. Should be of the form scheme://reference")?;

        match scheme {
            "file" => self.load_policy_from_file(reference).await,
            "oci" => self.load_policy_from_oci(reference).await,
            "https" => self.load_policy_from_url(uri).await,
            _ => bail!("Unsupported policy scheme: {}", scheme),
        }
    }

    async fn load_policy_from_file(&self, path: &str) -> Result<DownloadedPolicy> {
        let path = Path::new(path);
        if !path.is_absolute() {
            bail!("Policy file path must be fully qualified");
        }

        if !path.exists() {
            bail!("Policy file does not exist: {}", path.display());
        }

        Ok(DownloadedPolicy::Local(path.to_path_buf()))
    }

    async fn load_policy_from_oci(&self, reference: &str) -> Result<DownloadedPolicy> {
        let temp_file_name = format!("policy-{}", reference.replace('/', "-").replace(':', "-"));
        let (_downloaded_policy, _temp_file) =
            DownloadedPolicy::new_temp_file(&temp_file_name).await?;

        // For now, OCI policy pulling is not implemented - we'll use a simple approach
        // In a full implementation, this would use the OCI client to pull policy artifacts
        bail!("OCI policy pulling not implemented yet. Use file:// or https:// URIs for now.");
    }

    async fn load_policy_from_url(&self, url: &str) -> Result<DownloadedPolicy> {
        let url_obj = reqwest::Url::parse(url)?;
        let filename = url_obj
            .path_segments()
            .and_then(|segments| segments.last())
            .unwrap_or("policy")
            .trim_end_matches(".yaml")
            .trim_end_matches(".yml");

        let temp_file_name = format!("policy-{}", filename);
        let (downloaded_policy, mut temp_file) =
            DownloadedPolicy::new_temp_file(&temp_file_name).await?;

        let response = self.http_client.get(url).send().await?;
        if !response.status().is_success() {
            bail!(
                "Failed to download policy from {}: {}",
                url,
                response.status()
            );
        }

        let policy_bytes = response.bytes().await?;
        tokio::io::copy(&mut policy_bytes.as_ref(), &mut temp_file).await?;

        Ok(downloaded_policy)
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
        wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;
        wasmtime_wasi_config::add_to_linker(&mut linker, |h: &mut WasiState| {
            WasiConfig::from(&h.wasi_config_vars)
        })?;

        let mut store = Store::new(self.engine.as_ref(), state);

        let instance = linker.instantiate_async(&mut store, &component).await?;

        let (interface_name, func_name) =
            function_name.split_once(".").unwrap_or(("", function_name));

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

// Helper struct for tracking a component that has been downloaded. Allows for cleanup in the event
// of a failure. This works because we don't want to remove a local file, so that will just drop.
// But a temp file will be removed when the temp dir is dropped.
enum DownloadedComponent {
    Local(PathBuf),
    Temp((tempfile::TempDir, PathBuf)),
}

impl AsRef<Path> for DownloadedComponent {
    fn as_ref(&self) -> &Path {
        match self {
            DownloadedComponent::Local(path) => path.as_path(),
            DownloadedComponent::Temp((_, path)) => path.as_path(),
        }
    }
}

impl DownloadedComponent {
    /// Returns a new `DownloadedComponent` with an already opened file handle for writing the
    /// download.
    ///
    /// The `name` parameter must be unique across all plugins as it is used to identify the
    /// component. It should be provided without the `.wasm` extension, as it will be added
    /// automatically.
    async fn new_temp_file(name: impl AsRef<str>) -> Result<(Self, tokio::fs::File)> {
        let tempdir = tokio::task::spawn_blocking(tempfile::tempdir).await??;
        let file_path = tempdir.path().join(format!("{}.wasm", name.as_ref()));
        let temp_file = tokio::fs::File::create(&file_path).await?;
        Ok((DownloadedComponent::Temp((tempdir, file_path)), temp_file))
    }

    /// Returns the ID of the component based on the file name.
    fn id(&self) -> Result<String> {
        let maybe_id = match self {
            DownloadedComponent::Local(path) => path.file_stem().and_then(|s| s.to_str()),
            DownloadedComponent::Temp((_, path)) => path.file_stem().and_then(|s| s.to_str()),
        };

        maybe_id
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Failed to extract component ID from path"))
    }

    /// Copies the downloaded component to the given destination directory, consuming the
    /// `DownloadedComponent` and returning the component ID.
    ///
    /// If the given path is not a directory or does not exist, it will return an error.
    async fn copy_to(self, dest: impl AsRef<Path>) -> Result<()> {
        // Ensure the destination is a directory and exists
        let meta = tokio::fs::metadata(&dest).await?;
        if !meta.is_dir() {
            bail!(
                "Destination path must be a directory: {}",
                dest.as_ref().display()
            );
        }
        match self {
            DownloadedComponent::Local(path) => {
                let dest = dest.as_ref().join(
                    path.file_name()
                        .context("Path to copy is missing filename")?,
                );
                tokio::fs::copy(path, dest).await?;
            }
            DownloadedComponent::Temp((tempdir, file)) => {
                let dest = dest.as_ref().join(
                    file.file_name()
                        .context("Path to copy is missing filename")?,
                );
                tokio::fs::rename(file, dest).await?;
                tokio::task::spawn_blocking(move || tempdir.close())
                    .await?
                    .context("Failed to clean up temporary download file")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Deref;
    use std::path::PathBuf;
    use std::process::Command;

    use serde_json::json;
    use test_log::test;

    use super::*;

    const TEST_COMPONENT_ID: &str = "fetch_rs";

    /// Helper struct for keeping a reference to the temporary directory used for testing the
    /// lifecycle manager
    struct TestLifecycleManager {
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

    async fn create_test_manager() -> Result<TestLifecycleManager> {
        let tempdir = tempfile::tempdir()?;
        let manager = LifecycleManager::new(&tempdir).await?;
        Ok(TestLifecycleManager {
            manager,
            _tempdir: tempdir,
        })
    }

    async fn build_example_component() -> Result<PathBuf> {
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

    #[test]
    fn test_component_registry() {
        let mut registry = ComponentRegistry::new();

        // Test registering a component with tools
        let schema = json!({
            "tools": [
                {
                    "name": "tool1",
                    "description": "Test tool 1"
                },
                {
                    "name": "tool2",
                    "description": "Test tool 2"
                }
            ]
        });

        registry.register_component("comp1", &schema).unwrap();

        // Test tool lookup
        let tool1_info = registry.get_tool_info("tool1").unwrap();
        assert_eq!(tool1_info[0].component_id, "comp1");

        // Test listing tools
        let tools = registry.list_tools();
        assert_eq!(tools.len(), 2);

        // Test registering another component with overlapping tool name
        let schema2 = json!({
            "tools": [
                {
                    "name": "tool1",
                    "description": "Test tool 1 from comp2"
                }
            ]
        });

        registry.register_component("comp2", &schema2).unwrap();

        // Verify tool1 now has two components
        let tool1_info = registry.get_tool_info("tool1").unwrap();
        assert_eq!(tool1_info.len(), 2);

        // Test unregistering a component
        registry.unregister_component("comp1");

        // Verify tool2 is gone and tool1 only has one component
        assert!(registry.get_tool_info("tool2").is_none());
        let tool1_info = registry.get_tool_info("tool1").unwrap();
        assert_eq!(tool1_info.len(), 1);
        assert_eq!(tool1_info[0].component_id, "comp2");
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

        manager.unload_component(TEST_COMPONENT_ID).await;

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

    #[test(tokio::test)]
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

    #[test(tokio::test)]
    async fn test_get_wasi_state_for_component_default_policy() -> Result<()> {
        let manager = create_test_manager().await?;

        // Test getting WASI state for component without attached policy (should use default)
        let _wasi_state = manager
            .get_wasi_state_for_component("test-component")
            .await?;

        // Should not fail and return a valid WasiState
        // We can't directly inspect the WasiState, but we can verify it was created successfully
        assert!(true); // If we get here without error, the test passes

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

        // Should not fail and return a valid WasiState with the policy applied
        assert!(true); // If we get here without error, the test passes

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
}
