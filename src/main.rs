// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! The main `wassette(1)` command.

#![warn(missing_docs)]

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use mcp_server::{
    handle_prompts_list, handle_resources_list, handle_tools_call, handle_tools_list,
    LifecycleManager,
};
use serde_json::{json, Map, Value};
use rmcp::model::{
    CallToolRequestParam, CallToolResult, ErrorData, ListPromptsResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParam, ServerCapabilities, ServerInfo, ToolsCapability,
};
use rmcp::service::{serve_server, RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::{stdio as stdio_transport, SseServer};
use rmcp::ServerHandler;
use serde::{Deserialize, Serialize};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

mod config;

use std::sync::LazyLock;

// Create a static version string that can be used by clap
static VERSION_INFO: LazyLock<String> = LazyLock::new(format_build_info);
mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

const BIND_ADDRESS: &str = "127.0.0.1:9001";

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

#[derive(Parser, Debug)]
#[command(name = "wassette-mcp-server", about, long_about = None, version = VERSION_INFO.as_str())]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Begin handling requests over the specified protocol.
    Serve(Serve),
    /// Manage WebAssembly components.
    Component {
        #[command(subcommand)]
        command: ComponentCommands,
    },
    /// Manage component policies.
    Policy {
        #[command(subcommand)]
        command: PolicyCommands,
    },
    /// Manage component permissions.
    Permission {
        #[command(subcommand)]
        command: PermissionCommands,
    },
}

#[derive(Subcommand, Debug)]
enum ComponentCommands {
    /// Load a WebAssembly component from a file path or OCI registry.
    Load {
        /// Path to the component (file:// or oci://)
        path: String,
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
    },
    /// Unload a WebAssembly component.
    Unload {
        /// Component ID to unload
        id: String,
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
    },
    /// List all loaded components.
    List {
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum PolicyCommands {
    /// Get policy information for a component.
    Get {
        /// Component ID to get policy for
        component_id: String,
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum PermissionCommands {
    /// Grant permissions to a component.
    Grant {
        #[command(subcommand)]
        permission: GrantPermissionCommands,
    },
    /// Revoke permissions from a component.
    Revoke {
        #[command(subcommand)]
        permission: RevokePermissionCommands,
    },
    /// Reset all permissions for a component.
    Reset {
        /// Component ID to reset permissions for
        component_id: String,
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum GrantPermissionCommands {
    /// Grant storage access permission.
    Storage {
        /// Component ID to grant permission to
        component_id: String,
        /// Storage URI (e.g., fs:///tmp/workspace)
        uri: String,
        /// Access types (read, write, or both)
        #[arg(long, value_delimiter = ',')]
        access: Vec<String>,
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
    },
    /// Grant network access permission.
    Network {
        /// Component ID to grant permission to
        component_id: String,
        /// Host to grant access to
        host: String,
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
    },
    /// Grant environment variable access permission.
    #[command(name = "environment-variable")]
    EnvironmentVariable {
        /// Component ID to grant permission to
        component_id: String,
        /// Environment variable key
        key: String,
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum RevokePermissionCommands {
    /// Revoke storage access permission.
    Storage {
        /// Component ID to revoke permission from
        component_id: String,
        /// Storage URI to revoke access from
        uri: String,
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
    },
    /// Revoke network access permission.
    Network {
        /// Component ID to revoke permission from
        component_id: String,
        /// Host to revoke access from
        host: String,
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
    },
    /// Revoke environment variable access permission.
    #[command(name = "environment-variable")]
    EnvironmentVariable {
        /// Component ID to revoke permission from
        component_id: String,
        /// Environment variable key to revoke access from
        key: String,
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
    },
}

#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
struct Serve {
    /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wasette/components
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    plugin_dir: Option<PathBuf>,

    /// Enable stdio transport
    #[arg(long)]
    #[serde(skip)]
    stdio: bool,

    /// Enable SSE transport
    #[arg(long)]
    #[serde(skip)]
    sse: bool,

    /// Enable streamable HTTP transport  
    #[arg(long)]
    #[serde(skip)]
    streamable_http: bool,
}

/// A security-oriented runtime that runs WebAssembly Components via MCP.
#[derive(Clone)]
pub struct McpServer {
    lifecycle_manager: LifecycleManager,
}

/// Handle CLI commands by creating appropriate tool call requests
async fn handle_cli_command(
    lifecycle_manager: &LifecycleManager,
    tool_name: &str,
    args: Map<String, Value>,
) -> Result<()> {
    let req = CallToolRequestParam {
        name: tool_name.to_string().into(),
        arguments: Some(args),
    };

    let result = match tool_name {
        "load-component" | "unload-component" => {
            // These commands require a Peer, but we can't easily create one for CLI usage
            // For now, return an error suggesting to use the MCP server
            return Err(anyhow::anyhow!(
                "The '{}' command is only available through the MCP server interface. Please use 'wassette serve' and connect with an MCP client.",
                tool_name
            ));
        }
        "list-components" => {
            // Import the function directly
            use mcp_server::components::handle_list_components;
            handle_list_components(lifecycle_manager).await?
        }
        "get-policy" => {
            use mcp_server::tools::handle_get_policy;
            handle_get_policy(&req, lifecycle_manager).await?
        }
        "grant-storage-permission" => {
            use mcp_server::tools::handle_grant_storage_permission;
            handle_grant_storage_permission(&req, lifecycle_manager).await?
        }
        "grant-network-permission" => {
            use mcp_server::tools::handle_grant_network_permission;
            handle_grant_network_permission(&req, lifecycle_manager).await?
        }
        "grant-environment-variable-permission" => {
            use mcp_server::tools::handle_grant_environment_variable_permission;
            handle_grant_environment_variable_permission(&req, lifecycle_manager).await?
        }
        "revoke-storage-permission" => {
            use mcp_server::tools::handle_revoke_storage_permission;
            handle_revoke_storage_permission(&req, lifecycle_manager).await?
        }
        "revoke-network-permission" => {
            use mcp_server::tools::handle_revoke_network_permission;
            handle_revoke_network_permission(&req, lifecycle_manager).await?
        }
        "revoke-environment-variable-permission" => {
            use mcp_server::tools::handle_revoke_environment_variable_permission;
            handle_revoke_environment_variable_permission(&req, lifecycle_manager).await?
        }
        "reset-permission" => {
            use mcp_server::tools::handle_reset_permission;
            handle_reset_permission(&req, lifecycle_manager).await?
        }
        _ => {
            return Err(anyhow::anyhow!("Unknown command: {}", tool_name));
        }
    };
    
    // Print the result content
    for content in result.content {
        // Convert content to text and print
        if let Some(text) = content.as_text() {
            println!("{}", text.text);
        }
    }
    
    Ok(())
}

/// Create LifecycleManager from plugin directory
async fn create_lifecycle_manager(plugin_dir: Option<PathBuf>) -> Result<LifecycleManager> {
    let config = if let Some(dir) = plugin_dir {
        config::Config { plugin_dir: dir }
    } else {
        config::Config::new(&Serve { 
            plugin_dir: None, 
            stdio: false, 
            http: false 
        }).context("Failed to load configuration")?
    };
    
    LifecycleManager::new(&config.plugin_dir).await
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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
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

            let config = config::Config::new(cfg).context("Failed to load configuration")?;

            let lifecycle_manager = LifecycleManager::new(&config.plugin_dir).await?;

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
        Commands::Component { command } => {
            match command {
                ComponentCommands::Load { path, plugin_dir } => {
                    let lifecycle_manager = create_lifecycle_manager(plugin_dir.clone()).await?;
                    let mut args = Map::new();
                    args.insert("path".to_string(), json!(path));
                    handle_cli_command(&lifecycle_manager, "load-component", args).await?;
                }
                ComponentCommands::Unload { id, plugin_dir } => {
                    let lifecycle_manager = create_lifecycle_manager(plugin_dir.clone()).await?;
                    let mut args = Map::new();
                    args.insert("id".to_string(), json!(id));
                    handle_cli_command(&lifecycle_manager, "unload-component", args).await?;
                }
                ComponentCommands::List { plugin_dir } => {
                    let lifecycle_manager = create_lifecycle_manager(plugin_dir.clone()).await?;
                    let args = Map::new();
                    handle_cli_command(&lifecycle_manager, "list-components", args).await?;
                }
            }
        }
        Commands::Policy { command } => {
            match command {
                PolicyCommands::Get { component_id, plugin_dir } => {
                    let lifecycle_manager = create_lifecycle_manager(plugin_dir.clone()).await?;
                    let mut args = Map::new();
                    args.insert("component_id".to_string(), json!(component_id));
                    handle_cli_command(&lifecycle_manager, "get-policy", args).await?;
                }
            }
        }
        Commands::Permission { command } => {
            match command {
                PermissionCommands::Grant { permission } => {
                    match permission {
                        GrantPermissionCommands::Storage { component_id, uri, access, plugin_dir } => {
                            let lifecycle_manager = create_lifecycle_manager(plugin_dir.clone()).await?;
                            let mut args = Map::new();
                            args.insert("component_id".to_string(), json!(component_id));
                            args.insert("details".to_string(), json!({
                                "uri": uri,
                                "access": access
                            }));
                            handle_cli_command(&lifecycle_manager, "grant-storage-permission", args).await?;
                        }
                        GrantPermissionCommands::Network { component_id, host, plugin_dir } => {
                            let lifecycle_manager = create_lifecycle_manager(plugin_dir.clone()).await?;
                            let mut args = Map::new();
                            args.insert("component_id".to_string(), json!(component_id));
                            args.insert("details".to_string(), json!({
                                "host": host
                            }));
                            handle_cli_command(&lifecycle_manager, "grant-network-permission", args).await?;
                        }
                        GrantPermissionCommands::EnvironmentVariable { component_id, key, plugin_dir } => {
                            let lifecycle_manager = create_lifecycle_manager(plugin_dir.clone()).await?;
                            let mut args = Map::new();
                            args.insert("component_id".to_string(), json!(component_id));
                            args.insert("details".to_string(), json!({
                                "key": key
                            }));
                            handle_cli_command(&lifecycle_manager, "grant-environment-variable-permission", args).await?;
                        }
                    }
                }
                PermissionCommands::Revoke { permission } => {
                    match permission {
                        RevokePermissionCommands::Storage { component_id, uri, plugin_dir } => {
                            let lifecycle_manager = create_lifecycle_manager(plugin_dir.clone()).await?;
                            let mut args = Map::new();
                            args.insert("component_id".to_string(), json!(component_id));
                            args.insert("details".to_string(), json!({
                                "uri": uri
                            }));
                            handle_cli_command(&lifecycle_manager, "revoke-storage-permission", args).await?;
                        }
                        RevokePermissionCommands::Network { component_id, host, plugin_dir } => {
                            let lifecycle_manager = create_lifecycle_manager(plugin_dir.clone()).await?;
                            let mut args = Map::new();
                            args.insert("component_id".to_string(), json!(component_id));
                            args.insert("details".to_string(), json!({
                                "host": host
                            }));
                            handle_cli_command(&lifecycle_manager, "revoke-network-permission", args).await?;
                        }
                        RevokePermissionCommands::EnvironmentVariable { component_id, key, plugin_dir } => {
                            let lifecycle_manager = create_lifecycle_manager(plugin_dir.clone()).await?;
                            let mut args = Map::new();
                            args.insert("component_id".to_string(), json!(component_id));
                            args.insert("details".to_string(), json!({
                                "key": key
                            }));
                            handle_cli_command(&lifecycle_manager, "revoke-environment-variable-permission", args).await?;
                        }
                    }
                }
                PermissionCommands::Reset { component_id, plugin_dir } => {
                    let lifecycle_manager = create_lifecycle_manager(plugin_dir.clone()).await?;
                    let mut args = Map::new();
                    args.insert("component_id".to_string(), json!(component_id));
                    handle_cli_command(&lifecycle_manager, "reset-permission", args).await?;
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod version_tests {
    use super::*;

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
