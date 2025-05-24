use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use component2json::{component_exports_to_json_schema, json_to_vals, vals_to_json};
use lifecycle_manager::LifecycleManager;
use lifecycle_proto::lifecycle::lifecycle_manager_service_server::{
    LifecycleManagerService, LifecycleManagerServiceServer,
};
use lifecycle_proto::lifecycle::{
    CallComponentRequest, CallComponentResponse, GetComponentRequest, GetComponentResponse,
    ListComponentsRequest, ListComponentsResponse, LoadComponentRequest, LoadComponentResponse,
    UnloadComponentRequest, UnloadComponentResponse,
};
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use wasmtime::component::Linker;
use wasmtime::Store;
use wasmtime_wasi::WasiCtxBuilder;
use wasmtime_wasi_config::{WasiConfig, WasiConfigVariables};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

const PATH_NOT_FILE_ERROR: &str = "Path is not a file";

struct WasiState {
    ctx: wasmtime_wasi::WasiCtx,
    table: wasmtime_wasi::ResourceTable,
    http: wasmtime_wasi_http::WasiHttpCtx,
    wasi_config_vars: WasiConfigVariables,
}

impl wasmtime_wasi::IoView for WasiState {
    fn table(&mut self) -> &mut wasmtime_wasi::ResourceTable {
        &mut self.table
    }
}

impl wasmtime_wasi::WasiView for WasiState {
    fn ctx(&mut self) -> &mut wasmtime_wasi::WasiCtx {
        &mut self.ctx
    }
}

impl WasiHttpView for WasiState {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }
}

struct WasiStateBuilder {
    ctx_builder: WasiCtxBuilder,
    table: Option<wasmtime_wasi::ResourceTable>,
    http: Option<wasmtime_wasi_http::WasiHttpCtx>,
    config_vars: WasiConfigVariables,
}

impl WasiStateBuilder {
    fn new() -> Self {
        let mut ctx_builder = WasiCtxBuilder::new();
        ctx_builder.inherit_stdout();
        ctx_builder.inherit_stderr();
        ctx_builder.inherit_args();
        ctx_builder.inherit_network();

        Self {
            ctx_builder,
            table: None,
            http: None,
            config_vars: WasiConfigVariables::new(),
        }
    }

    fn allow_tcp(mut self, allow: bool) -> Self {
        self.ctx_builder.allow_tcp(allow);
        self
    }

    fn allow_udp(mut self, allow: bool) -> Self {
        self.ctx_builder.allow_udp(allow);
        self
    }

    fn allow_ip_name_lookup(mut self, allow: bool) -> Self {
        self.ctx_builder.allow_ip_name_lookup(allow);
        self
    }

    fn config_vars(mut self, config_vars: WasiConfigVariables) -> Self {
        self.config_vars = config_vars;
        self
    }

    fn build(self) -> WasiState {
        let mut ctx_builder = self.ctx_builder;

        WasiState {
            ctx: ctx_builder.build(),
            table: self.table.unwrap_or_default(),
            http: self.http.unwrap_or_else(WasiHttpCtx::new),
            wasi_config_vars: self.config_vars,
        }
    }
}

#[derive(Clone)]
struct LifecycleManagerServiceImpl {
    manager: Arc<LifecycleManager>,
    plugin_dir: PathBuf,
    policy_file: Option<String>,
}

impl LifecycleManagerServiceImpl {
    async fn copy_to_dir(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let metadata = tokio::fs::metadata(&path).await?;
        if !metadata.is_file() {
            return Err(std::io::Error::other(PATH_NOT_FILE_ERROR));
        }
        // NOTE: We just checked this was a file by reading metadata so we can unwrap here
        let dest = self.plugin_dir.join(path.as_ref().file_name().unwrap());
        tokio::fs::copy(path, dest).await.map(|_| ())
    }

    async fn execute_component_call(
        &self,
        component: &wasmtime::component::Component,
        function_name: &str,
        parameters: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let mut linker = Linker::new(self.manager.engine.as_ref());
        wasmtime_wasi::add_to_linker_async(&mut linker)?;
        wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;
        wasmtime_wasi_config::add_to_linker(&mut linker, |h: &mut WasiState| {
            WasiConfig::from(&h.wasi_config_vars)
        })?;

        let (wasi_config_vars, storage_permissions) = if let Some(policy_path) = &self.policy_file {
            // Load environment variables from policy
            let env_vars = policy::load_policy(policy_path)?;
            
            // Get storage permissions from policy
            let storage_perms = policy::get_storage_permissions(policy_path)?;
            
            (WasiConfigVariables::from_iter(env_vars), storage_perms)
        } else {
            (WasiConfigVariables::new(), Vec::new())
        };

        // Create the WASI state builder
        let mut state_builder = WasiStateBuilder::new()
            .allow_tcp(true)
            .allow_udp(true)
            .allow_ip_name_lookup(true)
            .config_vars(wasi_config_vars);
        
        // Add storage permissions from policy
        for (path, access_mode) in storage_permissions {
            let path = PathBuf::from(path);
            
            // Configure directory/file access based on permissions
            if access_mode.contains('r') && access_mode.contains('w') {
                state_builder.ctx_builder.preopened_dir(path)?;
            } else if access_mode.contains('r') {
                // Read-only access
                state_builder.ctx_builder.preopened_dir_with_read_only(path)?;
            }
            // Note: We don't have write-only option in the current API
        }

        let state = state_builder.build();
        let mut store = Store::new(self.manager.engine.as_ref(), state);

        let instance = linker.instantiate_async(&mut store, component).await?;

        let params: serde_json::Value = serde_json::from_str(parameters)?;
        let argument_vals = json_to_vals(&params)?;

        let export = instance
            .get_export(&mut store, None, function_name)
            .ok_or_else(|| anyhow::anyhow!("Function not found: {}", function_name))?;

        let func = instance
            .get_func(&mut store, export)
            .ok_or_else(|| anyhow::anyhow!("Export is not a function: {}", function_name))?;

        let schema =
            component_exports_to_json_schema(component, self.manager.engine.as_ref(), true);
        let tools = schema
            .get("tools")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow::anyhow!("No tools found in component"))?;

        let tool = tools
            .iter()
            .find(|t| t.get("name").and_then(|n| n.as_str()) == Some(function_name))
            .ok_or_else(|| anyhow::anyhow!("Tool not found"))?;

        let output_schema = tool["outputSchema"].clone();
        let mut results = json_to_vals(&output_schema)?;

        func.call_async(&mut store, &argument_vals, &mut results)
            .await?;

        let result_json = vals_to_json(&results);
        Ok(serde_json::to_string(&result_json)?)
    }
}

#[tonic::async_trait]
impl LifecycleManagerService for LifecycleManagerServiceImpl {
    async fn load_component(
        &self,
        request: Request<LoadComponentRequest>,
    ) -> Result<Response<LoadComponentResponse>, Status> {
        let req = request.into_inner();
        // Load the request file into the directory
        if let Err(e) = self.copy_to_dir(&req.path).await {
            let status = match e.kind() {
                ErrorKind::NotFound => {
                    Status::invalid_argument(format!("No file found at path {}", req.path))
                }
                ErrorKind::Other if e.to_string().contains(PATH_NOT_FILE_ERROR) => {
                    Status::invalid_argument(format!("Given path {} is not a file", req.path))
                }
                _ => Status::internal(e.to_string()),
            };
            return Err(status);
        }
        let (id, _) = self
            .manager
            .load_component(&req.path)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(LoadComponentResponse {
            status: "component loaded".to_string(),
            id,
        }))
    }

    async fn unload_component(
        &self,
        request: Request<UnloadComponentRequest>,
    ) -> Result<Response<UnloadComponentResponse>, Status> {
        let req = request.into_inner();
        self.manager.unload_component(&req.id).await;
        Ok(Response::new(UnloadComponentResponse {
            status: format!("component unloaded: {}", req.id),
        }))
    }

    async fn get_component(
        &self,
        request: Request<GetComponentRequest>,
    ) -> Result<Response<GetComponentResponse>, Status> {
        let req = request.into_inner();
        if req.id.is_empty() {
            return Err(Status::invalid_argument("ID field must be set"));
        }
        let component = self.manager.get_component(&req.id).await.ok_or_else(|| {
            Status::not_found(format!("Component with ID of {} not found", req.id))
        })?;

        let schema =
            component_exports_to_json_schema(&component, self.manager.engine.as_ref(), true);
        Ok(Response::new(GetComponentResponse {
            id: req.id.clone(),
            details: serde_json::to_string(&schema).unwrap_or_default(),
        }))
    }

    async fn list_components(
        &self,
        _request: Request<ListComponentsRequest>,
    ) -> Result<Response<ListComponentsResponse>, Status> {
        let ids = self.manager.list_components().await;
        Ok(Response::new(ListComponentsResponse { ids }))
    }

    async fn call_component(
        &self,
        request: Request<CallComponentRequest>,
    ) -> Result<Response<CallComponentResponse>, Status> {
        let req = request.into_inner();

        let component_id = self
            .manager
            .get_component_id_for_tool(&req.function_name)
            .await
            .map_err(|e| {
                Status::not_found(format!(
                    "Failed to find component for tool '{}': {}",
                    req.function_name, e
                ))
            })?;

        let component = self
            .manager
            .get_component(&component_id)
            .await
            .ok_or_else(|| {
                Status::not_found(format!("Component with ID {} not found", component_id))
            })?;

        match self
            .execute_component_call(&component, &req.function_name, &req.parameters)
            .await
        {
            Ok(result) => Ok(Response::new(CallComponentResponse {
                result: result.into_bytes(),
                error: String::new(),
            })),
            Err(e) => Ok(Response::new(CallComponentResponse {
                result: Vec::new(),
                error: e.to_string(),
            })),
        }
    }
}

pub struct WasmtimeD {
    addr: String,
    manager: Arc<LifecycleManager>,
    plugin_dir: PathBuf,
    policy_file: Option<String>,
}

impl WasmtimeD {
    pub async fn new(
        addr: String,
        plugin_dir: impl AsRef<Path>,
        policy_file: Option<&str>,
    ) -> anyhow::Result<Self> {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        config.async_support(true);
        let engine = Arc::new(wasmtime::Engine::new(&config)?);

        let manager = Arc::new(LifecycleManager::new(engine, &plugin_dir).await?);

        Ok(Self {
            addr,
            manager,
            plugin_dir: plugin_dir.as_ref().to_path_buf(),
            policy_file: policy_file.map(|s| s.to_string()),
        })
    }

    pub async fn serve(self) -> anyhow::Result<()> {
        let addr = self.addr.parse()?;
        let svc = LifecycleManagerServiceImpl {
            manager: self.manager,
            plugin_dir: self.plugin_dir,
            policy_file: self.policy_file,
        };

        tracing::info!("Daemon listening on {}", addr);
        Server::builder()
            .add_service(LifecycleManagerServiceServer::new(svc))
            .serve(addr)
            .await?;
        Ok(())
    }
}
