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
use rmcp::model::{
    CallToolRequestParam, CallToolResult, ErrorData, ListPromptsResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParam, ServerCapabilities, ServerInfo, ToolsCapability,
};
use rmcp::service::{serve_server, RequestContext, RoleServer};
use rmcp::transport::{stdio as stdio_transport, SseServer};
use rmcp::ServerHandler;
use serde::{Deserialize, Serialize};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

mod config;

// Include build-time information
mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

const BIND_ADDRESS: &str = "127.0.0.1:9001";

/// Formats build information similar to agentgateway's version output
fn format_build_info() -> String {
    let rust_version = built_info::RUSTC_VERSION
        .split_whitespace()
        .nth(1)
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
        "{} {} version.BuildInfo{{RustVersion:\"{}\", BuildProfile:\"{}\", BuildStatus:\"{}\", GitTag:\"{}\", Version:\"{}\", GitRevision:\"{}\"}}",
        built_info::PKG_NAME,
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
#[command(name = "wassette", about, long_about = None)]
struct Cli {
    /// Print version information
    #[arg(long, short = 'V')]
    version: bool,
    
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Begin handling requests over the specified protocol.
    Serve(Serve),
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

    /// Enable HTTP transport
    #[arg(long)]
    #[serde(skip)]
    http: bool,
}

/// A security-oriented runtime that runs WebAssembly Components via MCP.
#[derive(Clone)]
pub struct McpServer {
    lifecycle_manager: LifecycleManager,
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

    // Handle version flag
    if cli.version {
        println!("{}", format_build_info());
        return Ok(());
    }

    // Handle command or show help if no command provided
    match cli.command {
        Some(command) => match &command {
        Commands::Serve(cfg) => {
            // Initialize logging based on transport type
            let use_stdio_transport = match (cfg.stdio, cfg.http) {
                (false, false) => true, // Default case: use stdio transport
                (true, false) => true,  // Stdio transport only
                (false, true) => false, // HTTP transport only
                (true, true) => {
                    return Err(anyhow::anyhow!(
                        "Running both stdio and HTTP transports simultaneously is not supported. Please choose one."
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
            } else {
                tracing::info!(
                    "Starting MCP server on {} with HTTP transport",
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
        },
        None => {
            // Show help if no command provided
            return Err(anyhow::anyhow!("No command provided. Use --help for usage information."));
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
        assert!(version_info.contains("wassette-mcp-server"));
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
        // This test ensures the Homebrew formula test will pass
        assert!(version_info.contains(&format!("wassette-mcp-server {}", built_info::PKG_VERSION)));
    }
}
