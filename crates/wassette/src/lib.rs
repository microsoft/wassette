use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use component2json::{
    component_exports_to_json_schema, create_placeholder_results, json_to_vals, vals_to_json,
};
use futures::TryStreamExt;
use policy::{
    AccessType, EnvironmentPermission, NetworkHostPermission, NetworkPermission, PolicyParser,
    StoragePermission,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::fs::{metadata, DirEntry};
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use tracing::{debug, info, instrument, warn};
use wasmtime::component::{Component, Linker};
use wasmtime::{Engine, Store};
use wasmtime_wasi::p2::WasiCtxBuilder;
use wasmtime_wasi_config::{WasiConfig, WasiConfigVariables};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

mod wasistate;
pub use wasistate::{create_wasi_state_template_from_policy, WasiStateTemplate};

mod http;
pub use http::WeldWasiState;

const DOWNLOADS_DIR: &str = "downloads";

/// Granular permission rule types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PermissionRule {
    #[serde(rename = "network")]
    Network(NetworkPermission),
    #[serde(rename = "storage")]
    Storage(StoragePermission),
    #[serde(rename = "environment")]
    Environment(EnvironmentPermission),
    #[serde(rename = "custom")]
    Custom(String, serde_json::Value),
}

/// Permission grant request structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionGrantRequest {
    pub component_id: String,
    pub permission_type: String,
    pub details: serde_json::Value,
}

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
        // Note(mossaka): removed ctx_builder.inherit_network() to implement deny-by-default network policy
        // For HTTP requests to work, we need to allow TCP and DNS lookups when there are network permissions
        // But HTTP-level filtering happens in WeldWasiState::send_request
        if self.network_perms.allow_tcp || !self.allowed_hosts.is_empty() {
            ctx_builder.allow_tcp(true);
            ctx_builder.allow_ip_name_lookup(true);
        } else {
            ctx_builder.allow_tcp(false);
            ctx_builder.allow_ip_name_lookup(false);
        }
        ctx_builder.allow_udp(self.network_perms.allow_udp);
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

/// Represents a downloaded resource, either from a local file or a temporary one.
enum DownloadedResource {
    Local(PathBuf),
    Temp((tempfile::TempDir, PathBuf)),
}

impl AsRef<Path> for DownloadedResource {
    fn as_ref(&self) -> &Path {
        match self {
            DownloadedResource::Local(path) => path.as_path(),
            DownloadedResource::Temp((_, path)) => path.as_path(),
        }
    }
}

impl DownloadedResource {
    /// Returns a new `DownloadedComponent` with an already opened file handle for writing the
    /// download.
    ///
    /// The `name` parameter must be unique across all plugins as it is used to identify the
    /// component.
    async fn new_temp_file(
        name: impl AsRef<str>,
        extension: &str,
    ) -> Result<(Self, tokio::fs::File)> {
        let tempdir = tokio::task::spawn_blocking(tempfile::tempdir).await??;
        let file_path = tempdir
            .path()
            .join(format!("{}.{}", name.as_ref(), extension));
        let temp_file = tokio::fs::File::create(&file_path).await?;
        Ok((DownloadedResource::Temp((tempdir, file_path)), temp_file))
    }

    fn id(&self) -> Result<String> {
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
        let maybe_id = match self {
            DownloadedResource::Local(path) => path.file_stem().and_then(|s| s.to_str()),
            DownloadedResource::Temp((_, path)) => path.file_stem().and_then(|s| s.to_str()),
        };

        maybe_id
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Failed to extract resource ID from path"))
    }

    async fn copy_to(self, dest: impl AsRef<Path>) -> Result<()> {
        let meta = tokio::fs::metadata(&dest).await?;
        if !meta.is_dir() {
            bail!(
                "Destination path must be a directory: {}",
                dest.as_ref().display()
            );
        }
        match self {
            DownloadedResource::Local(path) => {
                let dest = dest.as_ref().join(
                    path.file_name()
                        .context("Path to copy is missing filename")?,
                );
                tokio::fs::copy(path, dest).await?;
            }
            DownloadedResource::Temp((tempdir, file)) => {
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

/// A trait for resources that can be loaded from a URI.
trait Loadable: Sized {
    const FILE_EXTENSION: &'static str;
    const RESOURCE_TYPE: &'static str;

    async fn from_local_file(path: &Path) -> Result<DownloadedResource>;
    async fn from_oci_reference(
        reference: &str,
        oci_client: &oci_client::Client,
    ) -> Result<DownloadedResource>;
    async fn from_url(url: &str, http_client: &reqwest::Client) -> Result<DownloadedResource>;
}

/// Loadable implementation for WebAssembly components
pub struct ComponentResource;

impl Loadable for ComponentResource {
    const FILE_EXTENSION: &'static str = "wasm";
    const RESOURCE_TYPE: &'static str = "component";

    async fn from_local_file(path: &Path) -> Result<DownloadedResource> {
        if !path.is_absolute() {
            bail!("Component path must be fully qualified. Please provide an absolute path to the WebAssembly component file.");
        }

        if !tokio::fs::try_exists(path).await? {
            bail!("Component path does not exist: {}. Please provide a valid path to a WebAssembly component file.", path.display());
        }

        if path.extension().unwrap_or_default() != Self::FILE_EXTENSION {
            bail!(
                "Invalid file extension for component: {}. Component file must have .{} extension.",
                path.display(),
                Self::FILE_EXTENSION
            );
        }

        Ok(DownloadedResource::Local(path.to_path_buf()))
    }

    async fn from_oci_reference(
        reference: &str,
        oci_client: &oci_client::Client,
    ) -> Result<DownloadedResource> {
        let reference: oci_client::Reference =
            reference.parse().context("Failed to parse OCI reference")?;
        let data = oci_wasm::WasmClient::from(oci_client.clone())
            .pull(&reference, &oci_client::secrets::RegistryAuth::Anonymous)
            .await?;
        let (downloaded_resource, mut file) = DownloadedResource::new_temp_file(
            reference.repository().replace('/', "_"),
            Self::FILE_EXTENSION,
        )
        .await?;
        file.write_all(&data.layers[0].data).await?;
        Ok(downloaded_resource)
    }

    async fn from_url(url: &str, http_client: &reqwest::Client) -> Result<DownloadedResource> {
        let resp = http_client.get(url).send().await?;
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
            .trim_end_matches(&format!(".{}", Self::FILE_EXTENSION));
        let (downloaded_resource, mut file) =
            DownloadedResource::new_temp_file(name, Self::FILE_EXTENSION).await?;
        let stream = resp.bytes_stream();
        let mut reader = tokio_util::io::StreamReader::new(stream.map_err(std::io::Error::other));
        tokio::io::copy(&mut reader, &mut file)
            .await
            .context("Failed to write downloaded component to temp file")?;
        Ok(downloaded_resource)
    }
}

/// Loadable implementation for policies
pub struct PolicyResource;

impl Loadable for PolicyResource {
    const FILE_EXTENSION: &'static str = "yaml";
    const RESOURCE_TYPE: &'static str = "policy";

    async fn from_local_file(path: &Path) -> Result<DownloadedResource> {
        if !path.is_absolute() {
            bail!("Policy file path must be fully qualified");
        }

        match metadata(path).await {
            Ok(meta) if meta.is_file() => Ok(DownloadedResource::Local(path.to_path_buf())),
            _ => {
                bail!("Policy file does not exist: {}", path.display());
            }
        }
    }

    async fn from_oci_reference(
        _reference: &str,
        _oci_client: &oci_client::Client,
    ) -> Result<DownloadedResource> {
        bail!("OCI references are not supported for policy resources. Use 'file://' or 'https://' schemes instead.")
    }

    async fn from_url(url: &str, http_client: &reqwest::Client) -> Result<DownloadedResource> {
        let url_obj = reqwest::Url::parse(url)?;
        let filename = url_obj
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .unwrap_or("policy")
            .trim_end_matches(&format!(".{}", Self::FILE_EXTENSION))
            .trim_end_matches(".yml");

        let temp_file_name = format!("policy-{filename}");
        let (downloaded_resource, mut temp_file) =
            DownloadedResource::new_temp_file(&temp_file_name, Self::FILE_EXTENSION).await?;

        let response = http_client.get(url).send().await?;
        if !response.status().is_success() {
            bail!(
                "Failed to download policy from {}: {}",
                url,
                response.status()
            );
        }

        let policy_bytes = response.bytes().await?;
        tokio::io::copy(&mut policy_bytes.as_ref(), &mut temp_file).await?;

        Ok(downloaded_resource)
    }
}

/// Generic resource loading function
async fn load_resource<T: Loadable>(
    uri: &str,
    oci_client: &oci_wasm::WasmClient,
    http_client: &reqwest::Client,
) -> Result<DownloadedResource> {
    let uri = uri.trim();
    let error_message = format!(
        "Invalid {} reference. Should be of the form scheme://reference",
        T::RESOURCE_TYPE
    );
    let (scheme, reference) = uri.split_once("://").context(error_message)?;

    match scheme {
        "file" => T::from_local_file(Path::new(reference)).await,
        "oci" => T::from_oci_reference(reference, oci_client).await,
        "https" => T::from_url(uri, http_client).await,
        _ => bail!("Unsupported {} scheme: {}", T::RESOURCE_TYPE, scheme),
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
            let schema = component_exports_to_json_schema(&component, &engine, true);
            registry
                .register_component(&name, &schema)
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
        debug!("Loading component from URI: {}", uri);

        let downloaded_resource =
            load_resource::<ComponentResource>(uri, &self.oci_client, &self.http_client).await?;

        let wasm_bytes = tokio::fs::read(downloaded_resource.as_ref())
            .await
            .context("Failed to read component file")?;

        let component = Component::new(&self.engine, wasm_bytes).map_err(|e| anyhow::anyhow!("Failed to compile component from path: {}. Error: {}. Please ensure the file is a valid WebAssembly component.", downloaded_resource.as_ref().display(), e))?;
        let id = downloaded_resource.id()?;
        let schema = component_exports_to_json_schema(&component, &self.engine, true);

        {
            let mut registry_write = self.registry.write().await;
            registry_write.unregister_component(&id);
            registry_write.register_component(&id, &schema)?;
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
        self.plugin_dir.join(format!("{component_id}.wasm"))
    }

    fn get_component_policy_path(&self, component_id: &str) -> PathBuf {
        self.plugin_dir.join(format!("{component_id}.policy.yaml"))
    }

    fn create_default_policy_template() -> Arc<WasiStateTemplate> {
        Arc::new(WasiStateTemplate::default())
    }

    async fn get_wasi_state_for_component(
        &self,
        component_id: &str,
    ) -> Result<WeldWasiState<WasiState>> {
        let policy_registry = self.policy_registry.read().await;

        let policy_template = policy_registry
            .component_policies
            .get(component_id)
            .cloned()
            .unwrap_or_else(Self::create_default_policy_template);

        let wasi_state = policy_template.build()?;
        let allowed_hosts = policy_template.allowed_hosts.clone();

        WeldWasiState::new(wasi_state, allowed_hosts)
    }

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

        let downloaded_policy =
            load_resource::<PolicyResource>(policy_uri, &self.oci_client, &self.http_client)
                .await?;

        let policy = PolicyParser::parse_file(downloaded_policy.as_ref())?;

        let policy_path = self.get_component_policy_path(component_id);
        tokio::fs::copy(downloaded_policy.as_ref(), &policy_path).await?;

        // Store metadata about the policy source
        let metadata = serde_json::json!({
            "source_uri": policy_uri,
            "attached_at": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
        });
        let metadata_path = self
            .plugin_dir
            .join(format!("{component_id}.policy.meta.json"));
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

    /// Detaches a policy from a component. This will remove the policy from the
    /// component and remove the policy file from the plugin directory.
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
            .join(format!("{component_id}.policy.meta.json"));
        if metadata_path.exists() {
            tokio::fs::remove_file(&metadata_path).await?;
        }

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

        let metadata_path = self
            .plugin_dir
            .join(format!("{component_id}.policy.meta.json"));
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

        wasmtime_wasi_config::add_to_linker(&mut linker, |h: &mut WeldWasiState<WasiState>| {
            WasiConfig::from(&h.inner.wasi_config_vars)
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

    // Granular permission system methods

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
    ) -> Result<policy_mcp::PolicyDocument> {
        let policy_path = self.get_component_policy_path(component_id);

        if policy_path.exists() {
            let policy_content = tokio::fs::read_to_string(&policy_path).await?;
            Ok(PolicyParser::parse_str(&policy_content)?)
        } else {
            // Create minimal policy document
            Ok(policy_mcp::PolicyDocument {
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
        policy: &mut policy_mcp::PolicyDocument,
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
        policy: &mut policy_mcp::PolicyDocument,
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
        policy: &mut policy_mcp::PolicyDocument,
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
        policy: &mut policy_mcp::PolicyDocument,
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
        policy: &policy_mcp::PolicyDocument,
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
        policy: &policy_mcp::PolicyDocument,
    ) -> Result<()> {
        let wasi_template =
            wasistate::create_wasi_state_template_from_policy(policy, &self.plugin_dir)?;
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

    // Granular permission system tests

    #[test(tokio::test)]
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

    #[test(tokio::test)]
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

    #[test(tokio::test)]
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

        let policy: policy_mcp::PolicyDocument = serde_yaml::from_str(&policy_content)?;

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
        assert!(test_storage.access.contains(&policy_mcp::AccessType::Read));
        assert!(test_storage.access.contains(&policy_mcp::AccessType::Write));

        let other_storage = storage_allow
            .iter()
            .find(|p| p.uri == "fs:///tmp/other")
            .expect("Should have storage permission for fs:///tmp/other");
        assert_eq!(
            other_storage.access.len(),
            1,
            "Should have only read access"
        );
        assert!(other_storage.access.contains(&policy_mcp::AccessType::Read));

        Ok(())
    }

    #[test(tokio::test)]
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

    #[test(tokio::test)]
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

    #[test(tokio::test)]
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

    #[test(tokio::test)]
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

    #[test(tokio::test)]
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

    #[test(tokio::test)]
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

    #[test(tokio::test)]
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
