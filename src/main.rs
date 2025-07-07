use std::env;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use anyhow::Result;
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
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

const BIND_ADDRESS: &str = "127.0.0.1:9001";

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Serve {
        #[arg(long, default_value_t = get_component_dir().into_os_string().into_string().unwrap())]
        plugin_dir: String,

        /// Enable stdio transport
        #[arg(long)]
        stdio: bool,

        /// Enable HTTP transport
        #[arg(long)]
        http: bool,
    },
}

/// Get the default component directory path based on the OS
fn get_component_dir() -> PathBuf {
    if cfg!(target_os = "windows") {
        let local_app_data = env::var("LOCALAPPDATA")
            .unwrap_or_else(|_| env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".to_string()));
        PathBuf::from(local_app_data)
            .join("wassette")
            .join("components")
    } else if cfg!(target_os = "macos") {
        let home = env::var("HOME").unwrap_or_else(|_| "/".to_string());
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("wassette")
            .join("components")
    } else {
        let xdg_data_home = env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
            let home = env::var("HOME").unwrap_or_else(|_| "/".to_string());
            format!("{home}/.local/share")
        });
        PathBuf::from(xdg_data_home)
            .join("wassette")
            .join("components")
    }
}

#[derive(Clone)]
pub struct McpServer {
    lifecycle_manager: LifecycleManager,
    peer: Option<rmcp::service::Peer<RoleServer>>,
}

impl McpServer {
    pub fn new(lifecycle_manager: LifecycleManager) -> Self {
        Self {
            lifecycle_manager,
            peer: None,
        }
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
            ..Default::default()
        }
    }

    fn call_tool<'a>(
        &'a self,
        params: CallToolRequestParam,
        _ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<CallToolResult, ErrorData>> + Send + 'a>> {
        let peer_clone = self.peer.clone();

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
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    "info,cranelift_codegen=warn,cranelift_entity=warn,cranelift_bforest=warn,cranelift_frontend=warn"
                        .to_string()
                        .into()
                }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cli = Cli::parse();

    match &cli.command {
        Commands::Serve {
            plugin_dir,
            stdio,
            http,
        } => {
            let components_dir = PathBuf::from(plugin_dir);

            let lifecycle_manager = LifecycleManager::new(&components_dir).await?;

            let server = McpServer::new(lifecycle_manager);

            match (*stdio, *http) {
                (false, false) => {
                    // Default case: use stdio transport
                    tracing::info!("Starting MCP server with stdio transport (default)");
                    let transport = stdio_transport();
                    let running_service = serve_server(server, transport).await?;

                    tokio::signal::ctrl_c().await?;
                    let _ = running_service.cancel().await;
                }
                (true, false) => {
                    // Stdio transport only
                    tracing::info!("Starting MCP server with stdio transport");
                    let transport = stdio_transport();
                    let running_service = serve_server(server, transport).await?;

                    tokio::signal::ctrl_c().await?;
                    let _ = running_service.cancel().await;
                }
                (false, true) => {
                    // HTTP transport only
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
                (true, true) => {
                    return Err(anyhow::anyhow!(
                        "Running both stdio and HTTP transports simultaneously is not supported. Please choose one."
                    ));
                }
            }

            tracing::info!("MCP server shutting down");
        }
    }

    Ok(())
}
