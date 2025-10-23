// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! The main `wassette(1)` command.

#![warn(missing_docs)]

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use clap::Parser;
use mcp_server::components::{
    handle_list_components, handle_load_component_cli, handle_unload_component_cli,
};
use mcp_server::tools::*;
use mcp_server::{
    handle_prompts_list, handle_resources_list, handle_tools_call, handle_tools_list,
    LifecycleManager,
};
use rmcp::model::{
    CallToolRequestParam, CallToolResult, ErrorData, ListPromptsResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParam, ServerCapabilities, ServerInfo, ToolsCapability,
};
use rmcp::service::{serve_server, RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::{stdio as stdio_transport, SseServer};
use rmcp::ServerHandler;
use serde_json::{json, Map, Value};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

mod commands;
mod config;
mod format;

use commands::{
    Cli, Commands, ComponentCommands, GrantPermissionCommands, PermissionCommands, PolicyCommands,
    RevokePermissionCommands, SecretCommands, Serve, Transport,
};
use format::{print_result, OutputFormat};

/// Represents the different types of tools available in the MCP server
#[derive(Debug, Clone, PartialEq)]
enum ToolName {
    LoadComponent,
    UnloadComponent,
    ListComponents,
    GetPolicy,
    GrantStoragePermission,
    GrantNetworkPermission,
    GrantEnvironmentVariablePermission,
    GrantMemoryPermission,
    RevokeStoragePermission,
    RevokeNetworkPermission,
    RevokeEnvironmentVariablePermission,
    ResetPermission,
}

impl ToolName {
    /// Get the tool name as a string constant
    const fn as_str(&self) -> &'static str {
        match self {
            Self::LoadComponent => Self::LOAD_COMPONENT,
            Self::UnloadComponent => Self::UNLOAD_COMPONENT,
            Self::ListComponents => Self::LIST_COMPONENTS,
            Self::GetPolicy => Self::GET_POLICY,
            Self::GrantStoragePermission => Self::GRANT_STORAGE_PERMISSION,
            Self::GrantNetworkPermission => Self::GRANT_NETWORK_PERMISSION,
            Self::GrantEnvironmentVariablePermission => Self::GRANT_ENVIRONMENT_VARIABLE_PERMISSION,
            Self::GrantMemoryPermission => Self::GRANT_MEMORY_PERMISSION,
            Self::RevokeStoragePermission => Self::REVOKE_STORAGE_PERMISSION,
            Self::RevokeNetworkPermission => Self::REVOKE_NETWORK_PERMISSION,
            Self::RevokeEnvironmentVariablePermission => {
                Self::REVOKE_ENVIRONMENT_VARIABLE_PERMISSION
            }
            Self::ResetPermission => Self::RESET_PERMISSION,
        }
    }

    // String constants for tool names
    const LOAD_COMPONENT: &'static str = "load-component";
    const UNLOAD_COMPONENT: &'static str = "unload-component";
    const LIST_COMPONENTS: &'static str = "list-components";
    const GET_POLICY: &'static str = "get-policy";
    const GRANT_STORAGE_PERMISSION: &'static str = "grant-storage-permission";
    const GRANT_NETWORK_PERMISSION: &'static str = "grant-network-permission";
    const GRANT_ENVIRONMENT_VARIABLE_PERMISSION: &'static str =
        "grant-environment-variable-permission";
    const GRANT_MEMORY_PERMISSION: &'static str = "grant-memory-permission";
    const REVOKE_STORAGE_PERMISSION: &'static str = "revoke-storage-permission";
    const REVOKE_NETWORK_PERMISSION: &'static str = "revoke-network-permission";
    const REVOKE_ENVIRONMENT_VARIABLE_PERMISSION: &'static str =
        "revoke-environment-variable-permission";
    const RESET_PERMISSION: &'static str = "reset-permission";
}

impl TryFrom<&str> for ToolName {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            Self::LOAD_COMPONENT => Ok(Self::LoadComponent),
            Self::UNLOAD_COMPONENT => Ok(Self::UnloadComponent),
            Self::LIST_COMPONENTS => Ok(Self::ListComponents),
            Self::GET_POLICY => Ok(Self::GetPolicy),
            Self::GRANT_STORAGE_PERMISSION => Ok(Self::GrantStoragePermission),
            Self::GRANT_NETWORK_PERMISSION => Ok(Self::GrantNetworkPermission),
            Self::GRANT_ENVIRONMENT_VARIABLE_PERMISSION => {
                Ok(Self::GrantEnvironmentVariablePermission)
            }
            Self::GRANT_MEMORY_PERMISSION => Ok(Self::GrantMemoryPermission),
            Self::REVOKE_STORAGE_PERMISSION => Ok(Self::RevokeStoragePermission),
            Self::REVOKE_NETWORK_PERMISSION => Ok(Self::RevokeNetworkPermission),
            Self::REVOKE_ENVIRONMENT_VARIABLE_PERMISSION => {
                Ok(Self::RevokeEnvironmentVariablePermission)
            }
            Self::RESET_PERMISSION => Ok(Self::ResetPermission),
            _ => Err(anyhow::anyhow!("Unknown tool name: {}", value)),
        }
    }
}

impl TryFrom<String> for ToolName {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl AsRef<str> for ToolName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Parse environment variable in KEY=VALUE format
fn parse_env_var(s: &str) -> Result<(String, String), String> {
    match s.split_once('=') {
        Some((key, value)) => {
            if key.is_empty() {
                Err("Environment variable key cannot be empty".to_string())
            } else {
                Ok((key.to_string(), value.to_string()))
            }
        }
        None => Err("Environment variable must be in KEY=VALUE format".to_string()),
    }
}

/// Load environment variables from a file (supports .env format)
fn load_env_file(path: &PathBuf) -> Result<HashMap<String, String>, anyhow::Error> {
    use std::fs;

    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read environment file: {}", path.display()))?;

    let mut env_vars = HashMap::new();

    for (line_num, line) in content.lines().enumerate() {
        let line = line.trim();

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Parse KEY=VALUE format
        match line.split_once('=') {
            Some((key, value)) => {
                let key = key.trim();
                let value = value.trim();

                if key.is_empty() {
                    bail!("Empty environment variable key at line {}", line_num + 1);
                }

                // Handle quoted values
                let value = if (value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\''))
                {
                    &value[1..value.len() - 1]
                } else {
                    value
                };

                env_vars.insert(key.to_string(), value.to_string());
            }
            None => {
                bail!(
                    "Invalid environment variable format at line {}: {}",
                    line_num + 1,
                    line
                );
            }
        }
    }

    Ok(env_vars)
}
mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

/// A security-oriented runtime that runs WebAssembly Components via MCP.
#[derive(Clone)]
pub struct McpServer {
    lifecycle_manager: LifecycleManager,
    peer: Arc<Mutex<Option<rmcp::Peer<rmcp::RoleServer>>>>,
    disable_builtin_tools: bool,
}

/// Handle CLI tool commands by creating appropriate tool call requests
async fn handle_tool_cli_command(
    lifecycle_manager: &LifecycleManager,
    tool_name: &str,
    args: Map<String, Value>,
    output_format: OutputFormat,
) -> Result<()> {
    let tool = ToolName::try_from(tool_name)?;

    let req = CallToolRequestParam {
        name: tool.as_str().to_string().into(),
        arguments: Some(args),
    };

    let result = match tool {
        ToolName::LoadComponent => handle_load_component_cli(&req, lifecycle_manager).await?,
        ToolName::UnloadComponent => handle_unload_component_cli(&req, lifecycle_manager).await?,
        ToolName::ListComponents => handle_list_components(lifecycle_manager).await?,
        ToolName::GetPolicy => handle_get_policy(&req, lifecycle_manager).await?,
        ToolName::GrantStoragePermission => {
            handle_grant_storage_permission(&req, lifecycle_manager).await?
        }
        ToolName::GrantNetworkPermission => {
            handle_grant_network_permission(&req, lifecycle_manager).await?
        }
        ToolName::GrantEnvironmentVariablePermission => {
            handle_grant_environment_variable_permission(&req, lifecycle_manager).await?
        }
        ToolName::GrantMemoryPermission => {
            handle_grant_memory_permission(&req, lifecycle_manager).await?
        }
        ToolName::RevokeStoragePermission => {
            handle_revoke_storage_permission(&req, lifecycle_manager).await?
        }
        ToolName::RevokeNetworkPermission => {
            handle_revoke_network_permission(&req, lifecycle_manager).await?
        }
        ToolName::RevokeEnvironmentVariablePermission => {
            handle_revoke_environment_variable_permission(&req, lifecycle_manager).await?
        }
        ToolName::ResetPermission => handle_reset_permission(&req, lifecycle_manager).await?,
    };

    // Print the result using the format module
    print_result(&result, output_format)?;

    // Exit with error code if the tool result indicates an error
    if result.is_error.unwrap_or(false) {
        std::process::exit(1);
    }

    Ok(())
}

/// Create LifecycleManager from plugin directory
///
/// For CLI responsiveness, we create an unloaded lifecycle manager which
/// initializes engine/linker without compiling/scanning all components.
/// Component metadata or lazy loads are used by individual handlers.
async fn create_lifecycle_manager(plugin_dir: Option<PathBuf>) -> Result<LifecycleManager> {
    let config = if let Some(dir) = plugin_dir {
        config::Config {
            plugin_dir: dir,
            secrets_dir: config::get_secrets_dir().unwrap_or_else(|_| {
                eprintln!("WARN: Unable to determine default secrets directory, using `secrets` directory in the current working directory");
                PathBuf::from("./secrets")
            }),
            environment_vars: std::collections::HashMap::new(),
            bind_address: "127.0.0.1:9001".to_string(),
        }
    } else {
        config::Config::from_serve(&crate::Serve {
            plugin_dir: None,
            transport: Default::default(),
            env_vars: vec![],
            env_file: None,
            disable_builtin_tools: false,
            bind_address: None,
        })
        .context("Failed to load configuration")?
    };

    // Use unloaded manager for fast CLI startup, but preserve custom secrets dir
    let config::Config {
        plugin_dir,
        secrets_dir,
        environment_vars,
        bind_address: _,
    } = config;

    LifecycleManager::builder(plugin_dir)
        .with_environment_vars(environment_vars)
        .with_secrets_dir(secrets_dir)
        .with_oci_client(oci_client::Client::default())
        .with_http_client(reqwest::Client::default())
        .with_eager_loading(false)
        .build()
        .await
}

impl McpServer {
    /// Creates a new MCP server instance with the given lifecycle manager.
    ///
    /// # Arguments
    /// * `lifecycle_manager` - The lifecycle manager for handling component operations
    /// * `disable_builtin_tools` - Whether to disable built-in tools
    pub fn new(lifecycle_manager: LifecycleManager, disable_builtin_tools: bool) -> Self {
        Self {
            lifecycle_manager,
            peer: Arc::new(Mutex::new(None)),
            disable_builtin_tools,
        }
    }

    /// Store the peer for background notifications (called on first request)
    fn store_peer_if_empty(&self, peer: rmcp::Peer<rmcp::RoleServer>) {
        let mut peer_guard = self.peer.lock().unwrap();
        if peer_guard.is_none() {
            *peer_guard = Some(peer);
        }
    }

    /// Get a clone of the stored peer if available
    pub fn get_peer(&self) -> Option<rmcp::Peer<rmcp::RoleServer>> {
        self.peer.lock().unwrap().clone()
    }
}

#[allow(refining_impl_trait_reachable)]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability {
                    list_changed: Some(true),
                }),
                ..Default::default()
            },
            instructions: Some(
                r#"This server runs tools in sandboxed WebAssembly environments with no default access to host resources.

Key points:
- Tools must be loaded before use: "Load component from oci://registry/tool:version" or "file:///path/to/tool.wasm"
- When the server starts, it will load all tools present in the plugin directory.
- You can list loaded tools with 'list-components' tool.
- Each tool only accesses resources explicitly granted by a policy file (filesystem paths, network domains, etc.)
- You MUST never modify the policy file directly, use tools to grant permissions instead.
- Tools needs permission for that resource
- If access is denied, suggest alternatives within allowed permissions or propose to grant permission"#.to_string(),
            ),
            ..Default::default()
        }
    }

    fn call_tool<'a>(
        &'a self,
        params: CallToolRequestParam,
        ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<CallToolResult, ErrorData>> + Send + 'a>> {
        let peer_clone = ctx.peer.clone();

        // Store peer on first request
        self.store_peer_if_empty(peer_clone.clone());

        let disable_builtin_tools = self.disable_builtin_tools;
        Box::pin(async move {
            let result = handle_tools_call(
                params,
                &self.lifecycle_manager,
                peer_clone,
                disable_builtin_tools,
            )
            .await;
            match result {
                Ok(value) => serde_json::from_value(value).map_err(|e| {
                    ErrorData::parse_error(format!("Failed to parse result: {e}"), None)
                }),
                Err(err) => Err(ErrorData::parse_error(err.to_string(), None)),
            }
        })
    }

    fn list_tools<'a>(
        &'a self,
        _params: Option<PaginatedRequestParam>,
        ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<ListToolsResult, ErrorData>> + Send + 'a>> {
        // Store peer on first request
        self.store_peer_if_empty(ctx.peer.clone());

        let disable_builtin_tools = self.disable_builtin_tools;
        Box::pin(async move {
            let result = handle_tools_list(&self.lifecycle_manager, disable_builtin_tools).await;
            match result {
                Ok(value) => serde_json::from_value(value).map_err(|e| {
                    ErrorData::parse_error(format!("Failed to parse result: {e}"), None)
                }),
                Err(err) => Err(ErrorData::parse_error(err.to_string(), None)),
            }
        })
    }

    fn list_prompts<'a>(
        &'a self,
        _params: Option<PaginatedRequestParam>,
        ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<ListPromptsResult, ErrorData>> + Send + 'a>> {
        // Store peer on first request
        self.store_peer_if_empty(ctx.peer.clone());

        Box::pin(async move {
            let result = handle_prompts_list(serde_json::Value::Null).await;
            match result {
                Ok(value) => serde_json::from_value(value).map_err(|e| {
                    ErrorData::parse_error(format!("Failed to parse result: {e}"), None)
                }),
                Err(err) => Err(ErrorData::parse_error(err.to_string(), None)),
            }
        })
    }

    fn list_resources<'a>(
        &'a self,
        _params: Option<PaginatedRequestParam>,
        ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<ListResourcesResult, ErrorData>> + Send + 'a>> {
        // Store peer on first request
        self.store_peer_if_empty(ctx.peer.clone());

        Box::pin(async move {
            let result = handle_resources_list(serde_json::Value::Null).await;
            match result {
                Ok(value) => serde_json::from_value(value).map_err(|e| {
                    ErrorData::parse_error(format!("Failed to parse result: {e}"), None)
                }),
                Err(err) => Err(ErrorData::parse_error(err.to_string(), None)),
            }
        })
    }
}

/// Formats build information similar to agentgateway's version output
fn format_build_info() -> String {
    // Parse Rust version more robustly by looking for version pattern
    // Expected format: "rustc 1.88.0 (extra info)"
    let rust_version = built_info::RUSTC_VERSION
        .split_whitespace()
        .find(|part| part.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .unwrap_or("unknown");

    let build_profile = built_info::PROFILE;

    let build_status = if built_info::GIT_DIRTY.unwrap_or(false) {
        "Modified"
    } else {
        "Clean"
    };

    let git_tag = built_info::GIT_VERSION.unwrap_or("unknown");

    let git_revision = built_info::GIT_COMMIT_HASH.unwrap_or("unknown");
    let version = if built_info::GIT_DIRTY.unwrap_or(false) {
        format!("{git_revision}-dirty")
    } else {
        git_revision.to_string()
    };

    format!(
        "{} version.BuildInfo{{RustVersion:\"{}\", BuildProfile:\"{}\", BuildStatus:\"{}\", GitTag:\"{}\", Version:\"{}\", GitRevision:\"{}\"}}",
        built_info::PKG_VERSION,
        rust_version,
        build_profile,
        build_status,
        git_tag,
        version,
        git_revision
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle version flag
    if cli.version {
        println!("{}", format_build_info());
        return Ok(());
    }

    match &cli.command {
        Some(command) => match command {
            Commands::Serve(cfg) => {
                // Configure logging - use stderr for stdio transport to avoid interfering with MCP protocol
                let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    "info,cranelift_codegen=warn,cranelift_entity=warn,cranelift_bforest=warn,cranelift_frontend=warn"
                    .to_string()
                    .into()
                });

                let registry = tracing_subscriber::registry().with(env_filter);

                // Initialize logging based on transport type
                let transport: Transport = (&cfg.transport).into();
                match transport {
                    Transport::Stdio => {
                        registry
                            .with(
                                tracing_subscriber::fmt::layer()
                                    .with_writer(std::io::stderr)
                                    .with_ansi(false),
                            )
                            .init();
                    }
                    _ => registry.with(tracing_subscriber::fmt::layer()).init(),
                }

                let config =
                    config::Config::from_serve(cfg).context("Failed to load configuration")?;

                // Build the lifecycle manager without eagerly loading components so the
                // background loader is the single source of tool registration.
                let config::Config {
                    plugin_dir,
                    secrets_dir,
                    environment_vars,
                    bind_address,
                } = config;

                let lifecycle_manager = LifecycleManager::builder(plugin_dir)
                    .with_environment_vars(environment_vars)
                    .with_secrets_dir(secrets_dir)
                    .with_oci_client(oci_client::Client::default())
                    .with_http_client(reqwest::Client::default())
                    .with_eager_loading(false)
                    .build()
                    .await?;

                let server = McpServer::new(lifecycle_manager.clone(), cfg.disable_builtin_tools);

                // Start background component loading
                let server_clone = server.clone();
                let lifecycle_manager_clone = lifecycle_manager.clone();
                tokio::spawn(async move {
                    let notify_fn = move || {
                        // Notify clients when a new component is loaded (if peer is available)
                        if let Some(peer) = server_clone.get_peer() {
                            let peer_clone = peer.clone();
                            tokio::spawn(async move {
                                if let Err(e) = peer_clone.notify_tool_list_changed().await {
                                    tracing::warn!("Failed to notify tool list changed: {}", e);
                                }
                            });
                        }
                    };

                    if let Err(e) = lifecycle_manager_clone
                        .load_existing_components_async(None, Some(notify_fn))
                        .await
                    {
                        tracing::error!("Background component loading failed: {}", e);
                    }
                });

                match transport {
                    Transport::Stdio => {
                        tracing::info!("Starting MCP server with stdio transport. Components will load in the background.");
                        let transport = stdio_transport();
                        let running_service = serve_server(server, transport).await?;

                        tokio::signal::ctrl_c().await?;
                        let _ = running_service.cancel().await;
                    }
                    Transport::StreamableHttp => {
                        tracing::info!(
                        "Starting MCP server on {} with streamable HTTP transport. Components will load in the background.",
                        bind_address
                    );
                        let service = StreamableHttpService::new(
                            move || Ok(server.clone()),
                            LocalSessionManager::default().into(),
                            Default::default(),
                        );

                        let router = axum::Router::new().nest_service("/mcp", service);
                        let tcp_listener = tokio::net::TcpListener::bind(&bind_address).await?;
                        let _ = axum::serve(tcp_listener, router)
                            .with_graceful_shutdown(async {
                                tokio::signal::ctrl_c().await.unwrap()
                            })
                            .await;
                    }
                    Transport::Sse => {
                        tracing::info!(
                        "Starting MCP server on {} with SSE HTTP transport. Components will load in the background.",
                        bind_address
                    );
                        let ct = SseServer::serve(bind_address.parse().unwrap())
                            .await?
                            .with_service(move || server.clone());

                        tokio::signal::ctrl_c().await?;
                        ct.cancel();
                    }
                }

                tracing::info!("MCP server shutting down");
            }
            Commands::Component { command } => match command {
                ComponentCommands::Load { path, plugin_dir } => {
                    let plugin_dir = plugin_dir.clone().or_else(|| cli.plugin_dir.clone());
                    let lifecycle_manager = create_lifecycle_manager(plugin_dir).await?;
                    let mut args = Map::new();
                    args.insert("path".to_string(), json!(path));
                    handle_tool_cli_command(
                        &lifecycle_manager,
                        "load-component",
                        args,
                        OutputFormat::Json,
                    )
                    .await?;
                }
                ComponentCommands::Unload { id, plugin_dir } => {
                    let plugin_dir = plugin_dir.clone().or_else(|| cli.plugin_dir.clone());
                    let lifecycle_manager = create_lifecycle_manager(plugin_dir).await?;
                    let mut args = Map::new();
                    args.insert("id".to_string(), json!(id));
                    handle_tool_cli_command(
                        &lifecycle_manager,
                        "unload-component",
                        args,
                        OutputFormat::Json,
                    )
                    .await?;
                }
                ComponentCommands::List {
                    plugin_dir,
                    output_format,
                } => {
                    let plugin_dir = plugin_dir.clone().or_else(|| cli.plugin_dir.clone());
                    let lifecycle_manager = create_lifecycle_manager(plugin_dir).await?;
                    let args = Map::new();
                    handle_tool_cli_command(
                        &lifecycle_manager,
                        "list-components",
                        args,
                        *output_format,
                    )
                    .await?;
                }
            },
            Commands::Policy { command } => match command {
                PolicyCommands::Get {
                    component_id,
                    plugin_dir,
                    output_format,
                } => {
                    let plugin_dir = plugin_dir.clone().or_else(|| cli.plugin_dir.clone());
                    let lifecycle_manager = create_lifecycle_manager(plugin_dir).await?;
                    let mut args = Map::new();
                    args.insert("component_id".to_string(), json!(component_id));
                    handle_tool_cli_command(&lifecycle_manager, "get-policy", args, *output_format)
                        .await?;
                }
            },
            Commands::Permission { command } => match command {
                PermissionCommands::Grant { permission } => match permission {
                    GrantPermissionCommands::Storage {
                        component_id,
                        uri,
                        access,
                        plugin_dir,
                    } => {
                        let plugin_dir = plugin_dir.clone().or_else(|| cli.plugin_dir.clone());
                        let lifecycle_manager = create_lifecycle_manager(plugin_dir).await?;
                        let mut args = Map::new();
                        args.insert("component_id".to_string(), json!(component_id));
                        args.insert(
                            "details".to_string(),
                            json!({
                                "uri": uri,
                                "access": access
                            }),
                        );
                        handle_tool_cli_command(
                            &lifecycle_manager,
                            "grant-storage-permission",
                            args,
                            OutputFormat::Json,
                        )
                        .await?;
                    }
                    GrantPermissionCommands::Network {
                        component_id,
                        host,
                        plugin_dir,
                    } => {
                        let plugin_dir = plugin_dir.clone().or_else(|| cli.plugin_dir.clone());
                        let lifecycle_manager = create_lifecycle_manager(plugin_dir).await?;
                        let mut args = Map::new();
                        args.insert("component_id".to_string(), json!(component_id));
                        args.insert(
                            "details".to_string(),
                            json!({
                                "host": host
                            }),
                        );
                        handle_tool_cli_command(
                            &lifecycle_manager,
                            "grant-network-permission",
                            args,
                            OutputFormat::Json,
                        )
                        .await?;
                    }
                    GrantPermissionCommands::EnvironmentVariable {
                        component_id,
                        key,
                        plugin_dir,
                    } => {
                        let plugin_dir = plugin_dir.clone().or_else(|| cli.plugin_dir.clone());
                        let lifecycle_manager = create_lifecycle_manager(plugin_dir).await?;
                        let mut args = Map::new();
                        args.insert("component_id".to_string(), json!(component_id));
                        args.insert(
                            "details".to_string(),
                            json!({
                                "key": key
                            }),
                        );
                        handle_tool_cli_command(
                            &lifecycle_manager,
                            "grant-environment-variable-permission",
                            args,
                            OutputFormat::Json,
                        )
                        .await?;
                    }
                    GrantPermissionCommands::Memory {
                        component_id,
                        limit,
                        plugin_dir,
                    } => {
                        let plugin_dir = plugin_dir.clone().or_else(|| cli.plugin_dir.clone());
                        let lifecycle_manager = create_lifecycle_manager(plugin_dir).await?;
                        let mut args = Map::new();
                        args.insert("component_id".to_string(), json!(component_id));
                        args.insert(
                            "details".to_string(),
                            json!({
                                "resources": {
                                    "limits": {
                                        "memory": limit
                                    }
                                }
                            }),
                        );
                        handle_tool_cli_command(
                            &lifecycle_manager,
                            "grant-memory-permission",
                            args,
                            OutputFormat::Json,
                        )
                        .await?;
                    }
                },
                PermissionCommands::Revoke { permission } => match permission {
                    RevokePermissionCommands::Storage {
                        component_id,
                        uri,
                        plugin_dir,
                    } => {
                        let plugin_dir = plugin_dir.clone().or_else(|| cli.plugin_dir.clone());
                        let lifecycle_manager = create_lifecycle_manager(plugin_dir).await?;
                        let mut args = Map::new();
                        args.insert("component_id".to_string(), json!(component_id));
                        args.insert(
                            "details".to_string(),
                            json!({
                                "uri": uri
                            }),
                        );
                        handle_tool_cli_command(
                            &lifecycle_manager,
                            "revoke-storage-permission",
                            args,
                            OutputFormat::Json,
                        )
                        .await?;
                    }
                    RevokePermissionCommands::Network {
                        component_id,
                        host,
                        plugin_dir,
                    } => {
                        let plugin_dir = plugin_dir.clone().or_else(|| cli.plugin_dir.clone());
                        let lifecycle_manager = create_lifecycle_manager(plugin_dir).await?;
                        let mut args = Map::new();
                        args.insert("component_id".to_string(), json!(component_id));
                        args.insert(
                            "details".to_string(),
                            json!({
                                "host": host
                            }),
                        );
                        handle_tool_cli_command(
                            &lifecycle_manager,
                            "revoke-network-permission",
                            args,
                            OutputFormat::Json,
                        )
                        .await?;
                    }
                    RevokePermissionCommands::EnvironmentVariable {
                        component_id,
                        key,
                        plugin_dir,
                    } => {
                        let plugin_dir = plugin_dir.clone().or_else(|| cli.plugin_dir.clone());
                        let lifecycle_manager = create_lifecycle_manager(plugin_dir).await?;
                        let mut args = Map::new();
                        args.insert("component_id".to_string(), json!(component_id));
                        args.insert(
                            "details".to_string(),
                            json!({
                                "key": key
                            }),
                        );
                        handle_tool_cli_command(
                            &lifecycle_manager,
                            "revoke-environment-variable-permission",
                            args,
                            OutputFormat::Json,
                        )
                        .await?;
                    }
                },
                PermissionCommands::Reset {
                    component_id,
                    plugin_dir,
                } => {
                    let plugin_dir = plugin_dir.clone().or_else(|| cli.plugin_dir.clone());
                    let lifecycle_manager = create_lifecycle_manager(plugin_dir).await?;
                    let mut args = Map::new();
                    args.insert("component_id".to_string(), json!(component_id));
                    handle_tool_cli_command(
                        &lifecycle_manager,
                        "reset-permission",
                        args,
                        OutputFormat::Json,
                    )
                    .await?;
                }
            },
            Commands::Secret { command } => match command {
                SecretCommands::List {
                    component_id,
                    show_values,
                    yes,
                    plugin_dir,
                    output_format,
                } => {
                    let lifecycle_manager = create_lifecycle_manager(plugin_dir.clone()).await?;

                    // Prompt for confirmation if showing values
                    if *show_values && !*yes {
                        print!("Show secret values? [y/N]: ");
                        std::io::Write::flush(&mut std::io::stdout())?;
                        let mut input = String::new();
                        std::io::stdin().read_line(&mut input)?;
                        if !input.trim().eq_ignore_ascii_case("y") {
                            println!("Cancelled.");
                            return Ok(());
                        }
                    }

                    let secrets = lifecycle_manager
                        .list_component_secrets(component_id, *show_values)
                        .await?;

                    let result = if *show_values {
                        secrets
                            .into_iter()
                            .map(|(k, v)| {
                                json!({
                                    "key": k,
                                    "value": v.unwrap_or_else(|| "<not found>".to_string())
                                })
                            })
                            .collect::<Vec<_>>()
                    } else {
                        secrets
                            .into_keys()
                            .map(|k| json!({"key": k}))
                            .collect::<Vec<_>>()
                    };

                    print_result(
                        &rmcp::model::CallToolResult {
                            content: Some(vec![rmcp::model::Content::text(
                                serde_json::to_string_pretty(&json!({
                                    "component_id": component_id,
                                    "secrets": result
                                }))?,
                            )]),
                            structured_content: None,
                            is_error: None,
                        },
                        *output_format,
                    )?;
                }
                SecretCommands::Set {
                    component_id,
                    secrets,
                    plugin_dir,
                } => {
                    let lifecycle_manager = create_lifecycle_manager(plugin_dir.clone()).await?;
                    lifecycle_manager
                        .set_component_secrets(component_id, secrets)
                        .await?;

                    let result = json!({
                        "status": "success",
                        "component_id": component_id,
                        "message": format!("Set {} secret(s) for component", secrets.len())
                    });

                    print_result(
                        &rmcp::model::CallToolResult {
                            content: Some(vec![rmcp::model::Content::text(
                                serde_json::to_string_pretty(&result)?,
                            )]),
                            structured_content: None,
                            is_error: None,
                        },
                        OutputFormat::Json,
                    )?;
                }
                SecretCommands::Delete {
                    component_id,
                    keys,
                    plugin_dir,
                } => {
                    let lifecycle_manager = create_lifecycle_manager(plugin_dir.clone()).await?;
                    lifecycle_manager
                        .delete_component_secrets(component_id, keys)
                        .await?;

                    let result = json!({
                        "status": "success",
                        "component_id": component_id,
                        "message": format!("Deleted {} secret(s) from component", keys.len())
                    });

                    print_result(
                        &rmcp::model::CallToolResult {
                            content: Some(vec![rmcp::model::Content::text(
                                serde_json::to_string_pretty(&result)?,
                            )]),
                            structured_content: None,
                            is_error: None,
                        },
                        OutputFormat::Json,
                    )?;
                }
            },
            Commands::Inspect { path } => {
                use std::sync::Arc;

                use wasmtime::component::Component;
                use wasmtime::{Config, Engine};

                // Configure Wasmtime engine for component model
                let mut config = Config::new();
                config.wasm_component_model(true);
                config.async_support(true);
                let engine = Arc::new(Engine::new(&config)?);

                // Load the component
                let component = Arc::new(Component::from_file(&engine, path)?);

                // Try to extract package docs
                let wasm_bytes = std::fs::read(path)?;
                let package_docs = component2json::extract_package_docs(&wasm_bytes);

                // Generate schema
                let schema = if let Some(ref docs) = package_docs {
                    println!("Found package docs!");
                    component2json::component_exports_to_json_schema_with_docs(
                        &component, &engine, true, docs,
                    )
                } else {
                    println!("No package docs found, using auto-generated");
                    component2json::component_exports_to_json_schema(&component, &engine, true)
                };

                // Display tools information
                if let Some(arr) = schema["tools"].as_array() {
                    for t in arr {
                        let name = t["name"].as_str().unwrap_or("<unnamed>").to_string();
                        let description: Option<String> =
                            t["description"].as_str().map(|s| s.to_string());
                        let input_schema = t["inputSchema"].clone();
                        let output_schema = t["outputSchema"].clone();

                        println!("{name}, {description:?}");
                        println!(
                            "input schema: {}",
                            serde_json::to_string_pretty(&input_schema)?
                        );
                        println!(
                            "output schema: {}",
                            serde_json::to_string_pretty(&output_schema)?
                        );
                    }
                } else {
                    println!("No tools found in component");
                }
            }
        },
        None => {
            eprintln!("No command provided. Use --help for usage information.");
            std::process::exit(1);
        }
    }

    Ok(())
}

#[cfg(test)]
mod version_tests {
    use super::*;

    /// Formats build information similar to agentgateway's version output
    fn format_build_info() -> String {
        // Parse Rust version more robustly by looking for version pattern
        // Expected format: "rustc 1.88.0 (extra info)"
        let rust_version = built_info::RUSTC_VERSION
            .split_whitespace()
            .find(|part| part.chars().next().is_some_and(|c| c.is_ascii_digit()))
            .unwrap_or("unknown");

        let build_profile = built_info::PROFILE;

        let build_status = if built_info::GIT_DIRTY.unwrap_or(false) {
            "Modified"
        } else {
            "Clean"
        };

        let git_tag = built_info::GIT_VERSION.unwrap_or("unknown");

        let git_revision = built_info::GIT_COMMIT_HASH.unwrap_or("unknown");
        let version = if built_info::GIT_DIRTY.unwrap_or(false) {
            format!("{git_revision}-dirty")
        } else {
            git_revision.to_string()
        };

        format!(
            "{} version.BuildInfo{{RustVersion:\"{}\", BuildProfile:\"{}\", BuildStatus:\"{}\", GitTag:\"{}\", Version:\"{}\", GitRevision:\"{}\"}}",
            built_info::PKG_VERSION,
            rust_version,
            build_profile,
            build_status,
            git_tag,
            version,
            git_revision
        )
    }

    #[test]
    fn test_version_format_contains_required_fields() {
        let version_info = format_build_info();

        // Check that the version output contains expected components
        assert!(version_info.contains("version.BuildInfo"));
        assert!(version_info.contains("RustVersion"));
        assert!(version_info.contains("BuildProfile"));
        assert!(version_info.contains("BuildStatus"));
        assert!(version_info.contains("GitTag"));
        assert!(version_info.contains("Version"));
        assert!(version_info.contains("GitRevision"));
    }

    #[test]
    fn test_version_contains_cargo_version() {
        let version_info = format_build_info();
        // This test ensures the Homebrew formula test will pass by checking the version info contains package version
        assert!(version_info.contains(built_info::PKG_VERSION));
    }
}

#[cfg(test)]
mod cli_tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn test_tool_name_from_str() {
        assert_eq!(
            ToolName::try_from("load-component").unwrap(),
            ToolName::LoadComponent
        );
        assert_eq!(
            ToolName::try_from("unload-component").unwrap(),
            ToolName::UnloadComponent
        );
        assert_eq!(
            ToolName::try_from("list-components").unwrap(),
            ToolName::ListComponents
        );
        assert_eq!(
            ToolName::try_from("get-policy").unwrap(),
            ToolName::GetPolicy
        );
        assert_eq!(
            ToolName::try_from("grant-storage-permission").unwrap(),
            ToolName::GrantStoragePermission
        );
        assert_eq!(
            ToolName::try_from("grant-network-permission").unwrap(),
            ToolName::GrantNetworkPermission
        );
        assert_eq!(
            ToolName::try_from("grant-environment-variable-permission").unwrap(),
            ToolName::GrantEnvironmentVariablePermission
        );
        assert_eq!(
            ToolName::try_from("grant-memory-permission").unwrap(),
            ToolName::GrantMemoryPermission
        );
        assert_eq!(
            ToolName::try_from("revoke-storage-permission").unwrap(),
            ToolName::RevokeStoragePermission
        );
        assert_eq!(
            ToolName::try_from("revoke-network-permission").unwrap(),
            ToolName::RevokeNetworkPermission
        );
        assert_eq!(
            ToolName::try_from("revoke-environment-variable-permission").unwrap(),
            ToolName::RevokeEnvironmentVariablePermission
        );
        assert_eq!(
            ToolName::try_from("reset-permission").unwrap(),
            ToolName::ResetPermission
        );

        // Test invalid tool name
        assert!(ToolName::try_from("invalid-tool").is_err());
    }

    #[test]
    fn test_tool_name_as_str() {
        assert_eq!(ToolName::LoadComponent.as_str(), "load-component");
        assert_eq!(ToolName::UnloadComponent.as_str(), "unload-component");
        assert_eq!(ToolName::ListComponents.as_str(), "list-components");
        assert_eq!(ToolName::GetPolicy.as_str(), "get-policy");
        assert_eq!(
            ToolName::GrantStoragePermission.as_str(),
            "grant-storage-permission"
        );
        assert_eq!(
            ToolName::GrantNetworkPermission.as_str(),
            "grant-network-permission"
        );
        assert_eq!(
            ToolName::GrantEnvironmentVariablePermission.as_str(),
            "grant-environment-variable-permission"
        );
        assert_eq!(
            ToolName::GrantMemoryPermission.as_str(),
            "grant-memory-permission"
        );
        assert_eq!(
            ToolName::RevokeStoragePermission.as_str(),
            "revoke-storage-permission"
        );
        assert_eq!(
            ToolName::RevokeNetworkPermission.as_str(),
            "revoke-network-permission"
        );
        assert_eq!(
            ToolName::RevokeEnvironmentVariablePermission.as_str(),
            "revoke-environment-variable-permission"
        );
        assert_eq!(ToolName::ResetPermission.as_str(), "reset-permission");
    }

    #[test]
    fn test_tool_name_roundtrip() {
        let test_cases = [
            ToolName::LoadComponent,
            ToolName::UnloadComponent,
            ToolName::ListComponents,
            ToolName::GetPolicy,
            ToolName::GrantStoragePermission,
            ToolName::GrantNetworkPermission,
            ToolName::GrantEnvironmentVariablePermission,
            ToolName::GrantMemoryPermission,
            ToolName::RevokeStoragePermission,
            ToolName::RevokeNetworkPermission,
            ToolName::RevokeEnvironmentVariablePermission,
            ToolName::ResetPermission,
        ];

        for tool in test_cases {
            let str_repr = tool.as_str();
            let parsed = ToolName::try_from(str_repr).unwrap();
            assert_eq!(tool, parsed);
        }
    }

    #[test]
    fn test_cli_command_parsing() {
        // Test component commands
        let args = vec!["wassette", "component", "list"];
        let cli = Cli::try_parse_from(args).unwrap();
        matches!(cli.command, Some(Commands::Component { .. }));

        // Test policy commands
        let args = vec!["wassette", "policy", "get", "test-component"];
        let cli = Cli::try_parse_from(args).unwrap();
        matches!(cli.command, Some(Commands::Policy { .. }));

        // Test permission commands
        let args = vec![
            "wassette",
            "permission",
            "grant",
            "storage",
            "test-component",
            "fs:///tmp",
            "--access",
            "read",
        ];
        let cli = Cli::try_parse_from(args).unwrap();
        matches!(cli.command, Some(Commands::Permission { .. }));

        // Test serve command still works
        let args = vec!["wassette", "serve", "--sse"];
        let cli = Cli::try_parse_from(args).unwrap();
        matches!(cli.command, Some(Commands::Serve(_)));
    }

    #[test]
    fn test_permission_grant_storage_parsing() {
        let args = vec![
            "wassette",
            "permission",
            "grant",
            "storage",
            "test-component",
            "fs:///tmp/test",
            "--access",
            "read,write",
        ];
        let cli = Cli::try_parse_from(args).unwrap();

        if let Some(Commands::Permission {
            command:
                PermissionCommands::Grant {
                    permission:
                        GrantPermissionCommands::Storage {
                            component_id,
                            uri,
                            access,
                            ..
                        },
                },
        }) = cli.command
        {
            assert_eq!(component_id, "test-component");
            assert_eq!(uri, "fs:///tmp/test");
            assert_eq!(access, vec!["read", "write"]);
        } else {
            panic!("Expected storage grant command");
        }
    }

    #[test]
    fn test_permission_revoke_network_parsing() {
        let args = vec![
            "wassette",
            "permission",
            "revoke",
            "network",
            "test-component",
            "example.com",
        ];
        let cli = Cli::try_parse_from(args).unwrap();

        if let Some(Commands::Permission {
            command:
                PermissionCommands::Revoke {
                    permission:
                        RevokePermissionCommands::Network {
                            component_id, host, ..
                        },
                },
        }) = cli.command
        {
            assert_eq!(component_id, "test-component");
            assert_eq!(host, "example.com");
        } else {
            panic!("Expected network revoke command");
        }
    }
}
