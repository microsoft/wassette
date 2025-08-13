// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Common server logic shared between wassette and weld binaries.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::LazyLock;

use anyhow::{Context, Result};
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

use crate::config::Config;

mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

const BIND_ADDRESS: &str = "127.0.0.1:9001";

// Create a static version string that can be used by clap
static VERSION_INFO: LazyLock<String> = LazyLock::new(format_build_info);

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

/// Configuration for server execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wasette/components
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_dir: Option<PathBuf>,
}

/// Transport type for the server
#[derive(Debug, Clone, PartialEq)]
pub enum TransportType {
    Stdio,
    Http,
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

/// Initialize logging based on transport type
pub fn initialize_logging(transport: TransportType) -> Result<()> {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| {
            "info,cranelift_codegen=warn,cranelift_entity=warn,cranelift_bforest=warn,cranelift_frontend=warn"
                .to_string()
                .into()
        });

    let registry = tracing_subscriber::registry().with(env_filter);

    match transport {
        TransportType::Stdio => {
            registry
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(std::io::stderr)
                        .with_ansi(false),
                )
                .init();
        }
        TransportType::Http => {
            registry.with(tracing_subscriber::fmt::layer()).init();
        }
    }

    Ok(())
}

/// Run the MCP server with the specified transport
pub async fn run_server(
    server_config: ServerConfig,
    transport: TransportType,
) -> Result<()> {
    initialize_logging(transport.clone())?;

    let config = Config::new(&server_config).context("Failed to load configuration")?;
    let lifecycle_manager = LifecycleManager::new(&config.plugin_dir).await?;
    let server = McpServer::new(lifecycle_manager);

    match transport {
        TransportType::Stdio => {
            tracing::info!("Starting MCP server with stdio transport");
            let transport = stdio_transport();
            let running_service = serve_server(server, transport).await?;

            tokio::signal::ctrl_c().await?;
            let _ = running_service.cancel().await;
        }
        TransportType::Http => {
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
    }

    tracing::info!("MCP server shutting down");
    Ok(())
}

/// Get version information string
pub fn get_version_info() -> &'static str {
    &VERSION_INFO
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