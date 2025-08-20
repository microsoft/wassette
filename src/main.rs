// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! The main `wassette(1)` command.

#![warn(missing_docs)]

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

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
    RevokePermissionCommands, Serve,
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
    RevokeStoragePermission,
    RevokeNetworkPermission,
    RevokeEnvironmentVariablePermission,
    ResetPermission,
}

impl TryFrom<&str> for ToolName {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "load-component" => Ok(Self::LoadComponent),
            "unload-component" => Ok(Self::UnloadComponent),
            "list-components" => Ok(Self::ListComponents),
            "get-policy" => Ok(Self::GetPolicy),
            "grant-storage-permission" => Ok(Self::GrantStoragePermission),
            "grant-network-permission" => Ok(Self::GrantNetworkPermission),
            "grant-environment-variable-permission" => Ok(Self::GrantEnvironmentVariablePermission),
            "revoke-storage-permission" => Ok(Self::RevokeStoragePermission),
            "revoke-network-permission" => Ok(Self::RevokeNetworkPermission),
            "revoke-environment-variable-permission" => {
                Ok(Self::RevokeEnvironmentVariablePermission)
            }
            "reset-permission" => Ok(Self::ResetPermission),
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
        match self {
            Self::LoadComponent => "load-component",
            Self::UnloadComponent => "unload-component",
            Self::ListComponents => "list-components",
            Self::GetPolicy => "get-policy",
            Self::GrantStoragePermission => "grant-storage-permission",
            Self::GrantNetworkPermission => "grant-network-permission",
            Self::GrantEnvironmentVariablePermission => "grant-environment-variable-permission",
            Self::RevokeStoragePermission => "revoke-storage-permission",
            Self::RevokeNetworkPermission => "revoke-network-permission",
            Self::RevokeEnvironmentVariablePermission => "revoke-environment-variable-permission",
            Self::ResetPermission => "reset-permission",
        }
    }
}

impl ToolName {
    /// Get the tool name as a string (convenience method that delegates to AsRef)
    fn as_str(&self) -> &str {
        self.as_ref()
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

const BIND_ADDRESS: &str = "127.0.0.1:9001";

/// A security-oriented runtime that runs WebAssembly Components via MCP.
#[derive(Clone)]
pub struct McpServer {
    lifecycle_manager: LifecycleManager,
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
async fn create_lifecycle_manager(plugin_dir: Option<PathBuf>) -> Result<LifecycleManager> {
    let config = if let Some(dir) = plugin_dir {
        config::Config {
            plugin_dir: dir,
            environment_vars: std::collections::HashMap::new(),
        }
    } else {
        config::Config::from_serve(&crate::Serve {
            plugin_dir: None,
            stdio: false,
            sse: false,
            streamable_http: false,
            env_vars: vec![],
            env_file: None,
        })
        .context("Failed to load configuration")?
    };

    LifecycleManager::new_with_env(&config.plugin_dir, config.environment_vars).await
}

impl McpServer {
    /// Creates a new MCP server instance with the given lifecycle manager.
    ///
    /// # Arguments
    /// * `lifecycle_manager` - The lifecycle manager for handling component operations
    pub fn new(lifecycle_manager: LifecycleManager) -> Self {
        Self { lifecycle_manager }
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

        Box::pin(async move {
            let result = handle_tools_call(params, &self.lifecycle_manager, peer_clone).await;
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
        _ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<ListToolsResult, ErrorData>> + Send + 'a>> {
        Box::pin(async move {
            let result = handle_tools_list(&self.lifecycle_manager).await;
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
        _ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<ListPromptsResult, ErrorData>> + Send + 'a>> {
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
        _ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<ListResourcesResult, ErrorData>> + Send + 'a>> {
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
                // Initialize logging based on transport type
                let (use_stdio_transport, use_streamable_http) = match (
                    cfg.stdio,
                    cfg.sse,
                    cfg.streamable_http,
                ) {
                    (false, false, false) => (true, false), // Default case: use stdio transport
                    (true, false, false) => (true, false),  // Stdio transport only
                    (false, true, false) => (false, false), // SSE transport only
                    (false, false, true) => (false, true),  // Streamable HTTP transport only
                    _ => {
                        return Err(anyhow::anyhow!(
                        "Running multiple transports simultaneously is not supported. Please choose one of: --stdio, --sse, or --streamable-http."
                    ));
                    }
                };

                // Configure logging - use stderr for stdio transport to avoid interfering with MCP protocol
                let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    "info,cranelift_codegen=warn,cranelift_entity=warn,cranelift_bforest=warn,cranelift_frontend=warn"
                        .to_string()
                        .into()
                });

                let registry = tracing_subscriber::registry().with(env_filter);

                if use_stdio_transport {
                    registry
                        .with(
                            tracing_subscriber::fmt::layer()
                                .with_writer(std::io::stderr)
                                .with_ansi(false),
                        )
                        .init();
                } else {
                    registry.with(tracing_subscriber::fmt::layer()).init();
                }

                let config =
                    config::Config::from_serve(cfg).context("Failed to load configuration")?;

                let lifecycle_manager =
                    LifecycleManager::new_with_env(&config.plugin_dir, config.environment_vars)
                        .await?;

                let server = McpServer::new(lifecycle_manager);

                if use_stdio_transport {
                    tracing::info!("Starting MCP server with stdio transport");
                    let transport = stdio_transport();
                    let running_service = serve_server(server, transport).await?;

                    tokio::signal::ctrl_c().await?;
                    let _ = running_service.cancel().await;
                } else if use_streamable_http {
                    tracing::info!(
                        "Starting MCP server on {} with streamable HTTP transport",
                        BIND_ADDRESS
                    );
                    let service = StreamableHttpService::new(
                        move || Ok(server.clone()),
                        LocalSessionManager::default().into(),
                        Default::default(),
                    );

                    let router = axum::Router::new().nest_service("/mcp", service);
                    let tcp_listener = tokio::net::TcpListener::bind(BIND_ADDRESS).await?;
                    let _ = axum::serve(tcp_listener, router)
                        .with_graceful_shutdown(async { tokio::signal::ctrl_c().await.unwrap() })
                        .await;
                } else {
                    tracing::info!(
                        "Starting MCP server on {} with SSE HTTP transport",
                        BIND_ADDRESS
                    );
                    let ct = SseServer::serve(BIND_ADDRESS.parse().unwrap())
                        .await?
                        .with_service(move || server.clone());

                    tokio::signal::ctrl_c().await?;
                    ct.cancel();
                }

                tracing::info!("MCP server shutting down");
            }
            Commands::Component { command } => match command {
                ComponentCommands::Load { path, plugin_dir } => {
                    let lifecycle_manager = create_lifecycle_manager(plugin_dir.clone()).await?;
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
                    let lifecycle_manager = create_lifecycle_manager(plugin_dir.clone()).await?;
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
                    let lifecycle_manager = create_lifecycle_manager(plugin_dir.clone()).await?;
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
                    let lifecycle_manager = create_lifecycle_manager(plugin_dir.clone()).await?;
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
                        let lifecycle_manager =
                            create_lifecycle_manager(plugin_dir.clone()).await?;
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
                        let lifecycle_manager =
                            create_lifecycle_manager(plugin_dir.clone()).await?;
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
                        let lifecycle_manager =
                            create_lifecycle_manager(plugin_dir.clone()).await?;
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
                },
                PermissionCommands::Revoke { permission } => match permission {
                    RevokePermissionCommands::Storage {
                        component_id,
                        uri,
                        plugin_dir,
                    } => {
                        let lifecycle_manager =
                            create_lifecycle_manager(plugin_dir.clone()).await?;
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
                        let lifecycle_manager =
                            create_lifecycle_manager(plugin_dir.clone()).await?;
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
                        let lifecycle_manager =
                            create_lifecycle_manager(plugin_dir.clone()).await?;
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
                    let lifecycle_manager = create_lifecycle_manager(plugin_dir.clone()).await?;
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
        assert!(version_info.contains("0.2.0"));
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
