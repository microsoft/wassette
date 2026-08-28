// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! The main `wassette(1)` command.

#![warn(missing_docs)]
// `call_tool` in `server.rs` boxes an async block whose `Send` obligation chain runs
// through the whole tool-call path, and the next trait solver reports the resulting
// depth as `recursion_depth_exceeding_limit`. The default limit of 128 is not enough
// for that chain; see rust-lang/rust#159228.
#![recursion_limit = "256"]

use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{CommandFactory, Parser};
use clap_complete::{generate, shells};
use mcp_server::{handle_tools_list, LifecycleManager};
use rmcp::service::serve_server;
use rmcp::transport::stdio as stdio_transport;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use serde_json::{json, Map};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

mod cli_handlers;
mod commands;
mod config;
mod format;
mod manifest;
mod permission_synthesis;
mod provisioning_controller;
mod registry;
mod server;
mod tools;
mod utils;

use cli_handlers::{create_lifecycle_manager, handle_tool_cli_command};
use commands::{
    Cli, Commands, ComponentCommands, GrantPermissionCommands, PermissionCommands, PolicyCommands,
    RegistryCommands, RevokePermissionCommands, SecretCommands, Shell, ToolCommands, Transport,
};
use format::{print_result, OutputFormat};
use server::McpServer;
use tools::ToolName;
use utils::{format_build_info, load_component_registry, parse_env_var};

// Allow active HTTP connections five seconds to finish, leaving half of
// Docker's default ten-second stop grace period for final process teardown.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(unix)]
struct ShutdownSignals {
    sigint: tokio::signal::unix::Signal,
    sigterm: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl ShutdownSignals {
    fn new() -> Result<Self> {
        use tokio::signal::unix::{signal, SignalKind};

        Ok(Self {
            sigint: signal(SignalKind::interrupt()).context("Failed to install SIGINT handler")?,
            sigterm: signal(SignalKind::terminate())
                .context("Failed to install SIGTERM handler")?,
        })
    }

    async fn wait(&mut self) -> Result<()> {
        tokio::select! {
            result = self.sigint.recv() => {
                result.context("SIGINT signal stream closed unexpectedly")?;
                tracing::info!("Received SIGINT, starting graceful shutdown");
            }
            result = self.sigterm.recv() => {
                result.context("SIGTERM signal stream closed unexpectedly")?;
                tracing::info!("Received SIGTERM, starting graceful shutdown");
            }
        }
        Ok(())
    }
}

#[cfg(not(unix))]
struct ShutdownSignals {
    ctrl_c: tokio::signal::windows::CtrlC,
}

#[cfg(not(unix))]
impl ShutdownSignals {
    fn new() -> Result<Self> {
        Ok(Self {
            ctrl_c: tokio::signal::windows::ctrl_c().context("Failed to install Ctrl-C handler")?,
        })
    }

    async fn wait(&mut self) -> Result<()> {
        self.ctrl_c
            .recv()
            .await
            .context("Ctrl-C signal stream closed unexpectedly")?;
        tracing::info!("Received Ctrl-C, starting graceful shutdown");
        Ok(())
    }
}

// Health and info endpoint handlers
mod endpoints {
    use axum::http::StatusCode;
    use axum::Json;
    use serde_json::{json, Value};

    /// Health check endpoint - returns 200 OK if server is running
    pub async fn health() -> StatusCode {
        StatusCode::OK
    }

    /// Readiness check endpoint - returns 200 OK with JSON payload
    pub async fn ready() -> Json<Value> {
        Json(json!({
            "status": "ready"
        }))
    }

    /// Build info endpoint - returns build information
    pub async fn info() -> Json<Value> {
        let build_info = crate::utils::format_build_info();
        Json(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "build_info": build_info
        }))
    }
}

fn main() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Failed to create Tokio runtime")?;
    let result = runtime.block_on(run());
    // Tokio's blocking stdin reader can outlive a canceled stdio service, and it
    // never returns once the service is gone, so this budget is always spent in
    // full on the stdio path. Keep it just wide enough for the blocking work
    // that can still finish, which measures at roughly 15ms, rather than
    // charging every stdio shutdown for a whole second of waiting.
    runtime.shutdown_timeout(Duration::from_millis(100));
    result
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    // Handle version flag
    if cli.version {
        println!("{}", format_build_info());
        return Ok(());
    }

    match &cli.command {
        Some(command) => match command {
            Commands::Run(cfg) => {
                let mut shutdown_signals = ShutdownSignals::new()?;

                // Configure logging - use stderr for stdio transport to avoid interfering with MCP protocol
                let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    "info,cranelift_codegen=warn,cranelift_entity=warn,cranelift_bforest=warn,cranelift_frontend=warn"
                    .to_string()
                    .into()
                });

                tracing_subscriber::registry()
                    .with(env_filter)
                    .with(
                        tracing_subscriber::fmt::layer()
                            .with_writer(std::io::stderr)
                            .with_ansi(false),
                    )
                    .init();

                let config = config::Config::from_run(cfg, cli.component_dir.as_deref())
                    .context("Failed to load configuration")?;

                // Build the lifecycle manager without eagerly loading components so the
                // background loader is the single source of tool registration.
                let config::Config {
                    component_dir,
                    secrets_dir,
                    environment_vars,
                    bind_address: _,
                    allowed_hosts: _,
                    legacy_sessions,
                    json_response: _,
                } = config;

                let lifecycle_manager = tokio::select! {
                    result = LifecycleManager::builder(component_dir)
                        .with_environment_vars(environment_vars)
                        .with_secrets_dir(secrets_dir)
                        .with_oci_client(oci_client::Client::default())
                        .with_http_client(reqwest::Client::default())
                        .with_eager_loading(false)
                        .build() => result?,
                    result = shutdown_signals.wait() => {
                        result?;
                        tracing::info!("MCP server shutting down");
                        return Ok(());
                    }
                };

                let server = McpServer::new(
                    lifecycle_manager.clone(),
                    cfg.disable_builtin_tools,
                    legacy_sessions,
                );

                // Start background component loading
                let server_clone = server.clone();
                let lifecycle_manager_clone = lifecycle_manager.clone();
                tokio::spawn(async move {
                    // Announce newly loaded components to session peers and to
                    // stateless `subscriptions/listen` streams alike.
                    let notify_fn = move || server_clone.publish_tool_list_changed();

                    if let Err(e) = lifecycle_manager_clone
                        .load_existing_components_async(None, Some(notify_fn))
                        .await
                    {
                        tracing::error!("Background component loading failed: {:#}", e);
                    }
                });

                tracing::info!("Starting MCP server with stdio transport. Components will load in the background.");
                let transport = stdio_transport();
                let running_service = tokio::select! {
                    result = serve_server(server, transport) => result?,
                    result = shutdown_signals.wait() => {
                        result?;
                        tracing::info!("MCP server shutting down");
                        return Ok(());
                    }
                };

                shutdown_signals.wait().await?;
                let _ = running_service.cancel().await;

                tracing::info!("MCP server shutting down");
            }
            Commands::Serve(cfg) => {
                let mut shutdown_signals = ShutdownSignals::new()?;

                // Configure logging for HTTP-based transports
                let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    "info,cranelift_codegen=warn,cranelift_entity=warn,cranelift_bforest=warn,cranelift_frontend=warn"
                    .to_string()
                    .into()
                });

                tracing_subscriber::registry()
                    .with(env_filter)
                    .with(tracing_subscriber::fmt::layer())
                    .init();

                let config = config::Config::from_serve(cfg, cli.component_dir.as_deref())
                    .context("Failed to load configuration")?;

                // Parse and validate manifest if provided
                let manifest = if let Some(manifest_path) = &cfg.manifest {
                    let m = manifest::ProvisioningManifest::from_file(manifest_path)
                        .context("Failed to parse provisioning manifest")?;

                    tracing::info!(
                        "Validating provisioning manifest from: {}",
                        manifest_path.display()
                    );
                    m.validate().context("Manifest validation failed")?;

                    tracing::info!(
                        "Successfully validated manifest with {} component(s)",
                        m.components.len()
                    );
                    Some(m)
                } else {
                    None
                };

                // Build the lifecycle manager without eagerly loading components so the
                // background loader is the single source of tool registration.
                let config::Config {
                    component_dir,
                    secrets_dir,
                    environment_vars,
                    bind_address,
                    allowed_hosts,
                    legacy_sessions,
                    json_response,
                } = config;

                // Keep a clone of component_dir for provisioning
                let component_dir_path = component_dir.clone();

                let lifecycle_manager = tokio::select! {
                    result = LifecycleManager::builder(component_dir)
                        .with_environment_vars(environment_vars)
                        .with_secrets_dir(secrets_dir)
                        .with_oci_client(oci_client::Client::default())
                        .with_http_client(reqwest::Client::default())
                        .with_eager_loading(false)
                        .build() => result?,
                    result = shutdown_signals.wait() => {
                        result?;
                        tracing::info!("MCP server shutting down");
                        return Ok(());
                    }
                };

                // Provision components from manifest if provided
                if let Some(manifest) = &manifest {
                    tracing::info!("Provisioning components from manifest...");

                    let provisioner = provisioning_controller::ProvisioningController::new(
                        manifest,
                        &lifecycle_manager,
                        lifecycle_manager.secrets_manager(),
                        &component_dir_path,
                    );

                    tokio::select! {
                        result = provisioner.provision() => {
                            result.context("Component provisioning failed")?;
                        }
                        result = shutdown_signals.wait() => {
                            result?;
                            tracing::info!("MCP server shutting down");
                            return Ok(());
                        }
                    }

                    tracing::info!("All components provisioned successfully");
                }

                let server = McpServer::new(
                    lifecycle_manager.clone(),
                    cfg.disable_builtin_tools,
                    legacy_sessions,
                );

                // Start background component loading
                let server_clone = server.clone();
                let lifecycle_manager_clone = lifecycle_manager.clone();
                tokio::spawn(async move {
                    // Announce newly loaded components to session peers and to
                    // stateless `subscriptions/listen` streams alike.
                    let notify_fn = move || server_clone.publish_tool_list_changed();

                    if let Err(e) = lifecycle_manager_clone
                        .load_existing_components_async(None, Some(notify_fn))
                        .await
                    {
                        tracing::error!("Background component loading failed: {:#}", e);
                    }
                });

                let transport: Transport = (&cfg.transport).into();
                match transport {
                    Transport::StreamableHttp => {
                        tracing::info!(
                        "Starting MCP server on {} with streamable HTTP transport. Components will load in the background.",
                        bind_address
                    );
                        // The transport accepts loopback Host headers only unless told
                        // otherwise, so a server addressed as `http://wassette:9001/mcp`
                        // is refused before MCP dispatch until allowed_hosts is set.
                        let http_config = match allowed_hosts.as_deref() {
                            Some(hosts) if !hosts.is_empty() => {
                                tracing::info!(
                                    allowed_hosts = ?hosts,
                                    "Accepting these Host values on the MCP endpoint"
                                );
                                StreamableHttpServerConfig::default()
                                    .with_allowed_hosts(hosts.to_vec())
                            }
                            _ => StreamableHttpServerConfig::default(),
                        };

                        // Override only what the operator chose, so the `Host`
                        // allow list resolved above stays in place. A literal
                        // struct here would silently drop it.
                        let cancellation_token = CancellationToken::new();
                        let http_config = http_config
                            .with_legacy_session_mode(legacy_sessions)
                            .with_json_response(json_response)
                            .with_cancellation_token(cancellation_token.clone());

                        let service = StreamableHttpService::new(
                            move || Ok(server.clone()),
                            LocalSessionManager::default().into(),
                            http_config,
                        );

                        let router = axum::Router::new()
                            .nest_service("/mcp", service)
                            .route("/health", axum::routing::get(endpoints::health))
                            .route("/ready", axum::routing::get(endpoints::ready))
                            .route("/info", axum::routing::get(endpoints::info));
                        let tcp_listener = tokio::net::TcpListener::bind(&bind_address).await?;

                        // Spawn the server in a background task
                        let (shutdown_result_tx, shutdown_result_rx) =
                            tokio::sync::oneshot::channel();
                        let mut server_handle = tokio::spawn(async move {
                            axum::serve(tcp_listener, router)
                                .with_graceful_shutdown(async move {
                                    let result = shutdown_signals.wait().await;
                                    cancellation_token.cancel();
                                    let _ = shutdown_result_tx.send(result);
                                })
                                .await
                        });

                        tracing::info!(
                            "MCP server is ready and listening on http://{}/mcp",
                            bind_address
                        );
                        tracing::info!("Health check available at http://{}/health", bind_address);
                        tracing::info!(
                            "Readiness check available at http://{}/ready",
                            bind_address
                        );
                        tracing::info!("Build info available at http://{}/info", bind_address);

                        shutdown_result_rx
                            .await
                            .context("MCP server stopped before receiving a shutdown signal")??;

                        // Wait for active connections to finish.
                        match tokio::time::timeout(DRAIN_TIMEOUT, &mut server_handle).await {
                            Ok(result) => result??,
                            Err(_) => {
                                tracing::warn!(
                                    "HTTP connection drain deadline passed; dropping remaining connections"
                                );
                                server_handle.abort();
                            }
                        }
                    }
                }

                tracing::info!("MCP server shutting down");
            }
            Commands::Component { command } => match command {
                ComponentCommands::Load {
                    path,
                    component_dir,
                } => {
                    let component_dir = component_dir.clone().or_else(|| cli.component_dir.clone());
                    let lifecycle_manager = create_lifecycle_manager(component_dir).await?;
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
                ComponentCommands::Unload { id, component_dir } => {
                    let component_dir = component_dir.clone().or_else(|| cli.component_dir.clone());
                    let lifecycle_manager = create_lifecycle_manager(component_dir).await?;
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
                    component_dir,
                    output_format,
                } => {
                    let component_dir = component_dir.clone().or_else(|| cli.component_dir.clone());
                    let lifecycle_manager = create_lifecycle_manager(component_dir).await?;
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
                    component_dir,
                    output_format,
                } => {
                    let component_dir = component_dir.clone().or_else(|| cli.component_dir.clone());
                    let lifecycle_manager = create_lifecycle_manager(component_dir).await?;
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
                        component_dir,
                    } => {
                        let component_dir =
                            component_dir.clone().or_else(|| cli.component_dir.clone());
                        let lifecycle_manager = create_lifecycle_manager(component_dir).await?;
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
                        component_dir,
                    } => {
                        let component_dir =
                            component_dir.clone().or_else(|| cli.component_dir.clone());
                        let lifecycle_manager = create_lifecycle_manager(component_dir).await?;
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
                        component_dir,
                    } => {
                        let component_dir =
                            component_dir.clone().or_else(|| cli.component_dir.clone());
                        let lifecycle_manager = create_lifecycle_manager(component_dir).await?;
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
                        component_dir,
                    } => {
                        let component_dir =
                            component_dir.clone().or_else(|| cli.component_dir.clone());
                        let lifecycle_manager = create_lifecycle_manager(component_dir).await?;
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
                        component_dir,
                    } => {
                        let component_dir =
                            component_dir.clone().or_else(|| cli.component_dir.clone());
                        let lifecycle_manager = create_lifecycle_manager(component_dir).await?;
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
                        component_dir,
                    } => {
                        let component_dir =
                            component_dir.clone().or_else(|| cli.component_dir.clone());
                        let lifecycle_manager = create_lifecycle_manager(component_dir).await?;
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
                        component_dir,
                    } => {
                        let component_dir =
                            component_dir.clone().or_else(|| cli.component_dir.clone());
                        let lifecycle_manager = create_lifecycle_manager(component_dir).await?;
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
                    component_dir,
                } => {
                    let component_dir = component_dir.clone().or_else(|| cli.component_dir.clone());
                    let lifecycle_manager = create_lifecycle_manager(component_dir).await?;
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
                    component_dir,
                    output_format,
                } => {
                    let lifecycle_manager = create_lifecycle_manager(component_dir.clone()).await?;

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
                        &rmcp::model::CallToolResult::success(vec![
                            rmcp::model::ContentBlock::text(serde_json::to_string_pretty(
                                &json!({
                                    "component_id": component_id,
                                    "secrets": result
                                }),
                            )?),
                        ]),
                        *output_format,
                    )?;
                }
                SecretCommands::Set {
                    component_id,
                    secrets,
                    component_dir,
                } => {
                    let lifecycle_manager = create_lifecycle_manager(component_dir.clone()).await?;
                    lifecycle_manager
                        .set_component_secrets(component_id, secrets)
                        .await?;

                    let result = json!({
                        "status": "success",
                        "component_id": component_id,
                        "message": format!("Set {} secret(s) for component", secrets.len())
                    });

                    print_result(
                        &rmcp::model::CallToolResult::success(vec![
                            rmcp::model::ContentBlock::text(serde_json::to_string_pretty(&result)?),
                        ]),
                        OutputFormat::Json,
                    )?;
                }
                SecretCommands::Delete {
                    component_id,
                    keys,
                    component_dir,
                } => {
                    let lifecycle_manager = create_lifecycle_manager(component_dir.clone()).await?;
                    lifecycle_manager
                        .delete_component_secrets(component_id, keys)
                        .await?;

                    let result = json!({
                        "status": "success",
                        "component_id": component_id,
                        "message": format!("Deleted {} secret(s) from component", keys.len())
                    });

                    print_result(
                        &rmcp::model::CallToolResult::success(vec![
                            rmcp::model::ContentBlock::text(serde_json::to_string_pretty(&result)?),
                        ]),
                        OutputFormat::Json,
                    )?;
                }
            },
            Commands::Tool { command } => match command {
                ToolCommands::List {
                    component_dir,
                    output_format,
                } => {
                    let component_dir = component_dir.clone().or_else(|| cli.component_dir.clone());
                    let lifecycle_manager = create_lifecycle_manager(component_dir).await?;

                    let result = handle_tools_list(&lifecycle_manager, false).await?;

                    let tools_result: rmcp::model::ListToolsResult =
                        serde_json::from_value(result)?;

                    let content = serde_json::to_string_pretty(&json!({
                        "tools": tools_result.tools.iter().map(|t| {
                            json!({
                                "name": t.name,
                                "description": t.description,
                                "input_schema": t.input_schema,
                                "output_schema": t.output_schema,
                            })
                        }).collect::<Vec<_>>()
                    }))?;

                    print_result(
                        &rmcp::model::CallToolResult::success(vec![
                            rmcp::model::ContentBlock::text(content),
                        ]),
                        *output_format,
                    )?;
                }
                ToolCommands::Read {
                    name,
                    component_dir,
                    output_format,
                } => {
                    let component_dir = component_dir.clone().or_else(|| cli.component_dir.clone());
                    let lifecycle_manager = create_lifecycle_manager(component_dir).await?;

                    let result = handle_tools_list(&lifecycle_manager, false).await?;
                    let tools_result: rmcp::model::ListToolsResult =
                        serde_json::from_value(result)?;

                    let tool = tools_result
                        .tools
                        .iter()
                        .find(|t| t.name == name.as_str())
                        .ok_or_else(|| anyhow::anyhow!("Tool not found: {}", name))?;

                    let content = serde_json::to_string_pretty(&json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": tool.input_schema,
                        "output_schema": tool.output_schema,
                    }))?;

                    print_result(
                        &rmcp::model::CallToolResult::success(vec![
                            rmcp::model::ContentBlock::text(content),
                        ]),
                        *output_format,
                    )?;
                }
                ToolCommands::Invoke {
                    name,
                    args,
                    component_dir,
                    output_format,
                } => {
                    let component_dir = component_dir.clone().or_else(|| cli.component_dir.clone());
                    let lifecycle_manager = create_lifecycle_manager(component_dir).await?;

                    let arguments = if let Some(args_str) = args {
                        let parsed: serde_json::Value = serde_json::from_str(args_str)
                            .context("Failed to parse arguments as JSON")?;

                        if let serde_json::Value::Object(map) = parsed {
                            map
                        } else {
                            bail!("Arguments must be a JSON object");
                        }
                    } else {
                        serde_json::Map::new()
                    };

                    if let Ok(tool_name) = ToolName::try_from(name.as_str()) {
                        handle_tool_cli_command(
                            &lifecycle_manager,
                            tool_name.as_str(),
                            arguments,
                            *output_format,
                        )
                        .await?;
                    } else {
                        let req = rmcp::model::CallToolRequestParams::new(name.clone())
                            .with_arguments(arguments);

                        use mcp_server::components::handle_component_call;
                        let result = handle_component_call(&req, &lifecycle_manager).await;

                        match result {
                            Ok(tool_result) => {
                                print_result(&tool_result, *output_format)?;

                                if tool_result.is_error.unwrap_or(false) {
                                    std::process::exit(1);
                                }
                            }
                            Err(e) => {
                                eprintln!("Error invoking tool '{}': {:#}", name, e);
                                std::process::exit(1);
                            }
                        }
                    }
                }
            },
            Commands::Inspect {
                component_id,
                component_dir,
            } => {
                let component_dir = component_dir.clone().or_else(|| cli.component_dir.clone());
                let lifecycle_manager = create_lifecycle_manager(component_dir).await?;

                // Get the component schema from the lifecycle manager
                let schema = lifecycle_manager
                    .get_component_schema(component_id)
                    .await
                    .context(format!(
                    "Component '{}' not found. Use 'component load' to load the component first.",
                    component_id
                ))?;

                // Display tools information
                if let Some(arr) = schema["tools"].as_array() {
                    for t in arr {
                        // The tool info is nested in properties.result
                        let tool_info = &t["properties"]["result"];
                        let name = tool_info["name"]
                            .as_str()
                            .unwrap_or("<unnamed>")
                            .to_string();
                        let description: Option<String> =
                            tool_info["description"].as_str().map(|s| s.to_string());
                        let input_schema = tool_info["inputSchema"].clone();
                        let output_schema = tool_info["outputSchema"].clone();

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
            Commands::Registry { command } => match command {
                RegistryCommands::Search {
                    query,
                    output_format,
                } => {
                    let components = load_component_registry()?;
                    let results = registry::search_components(&components, query.as_deref());

                    let result = json!({
                        "status": "success",
                        "count": results.len(),
                        "components": results
                    });

                    print_result(
                        &rmcp::model::CallToolResult::success(vec![
                            rmcp::model::ContentBlock::text(serde_json::to_string_pretty(&result)?),
                        ]),
                        *output_format,
                    )?;
                }
                RegistryCommands::Get {
                    component,
                    plugin_dir,
                } => {
                    let components = load_component_registry()?;

                    // Find the component by name or URI
                    let registry_component =
                        registry::find_component_by_name_or_uri(&components, component)
                            .ok_or_else(|| {
                                anyhow::anyhow!(
                                    "Component '{}' not found in registry. Use 'wassette registry search' to list available components.",
                                    component
                                )
                            })?;

                    // Use the existing load-component functionality
                    let plugin_dir = plugin_dir.clone().or_else(|| cli.component_dir.clone());
                    let lifecycle_manager = create_lifecycle_manager(plugin_dir).await?;
                    let mut args = Map::new();
                    args.insert("path".to_string(), json!(registry_component.uri));
                    handle_tool_cli_command(
                        &lifecycle_manager,
                        "load-component",
                        args,
                        OutputFormat::Json,
                    )
                    .await?;
                }
            },
            Commands::Autocomplete { shell } => {
                let mut cmd = Cli::command();
                let bin_name = cmd.get_name().to_string();

                match shell {
                    Shell::Bash => {
                        generate(shells::Bash, &mut cmd, &bin_name, &mut std::io::stdout());
                    }
                    Shell::Zsh => {
                        generate(shells::Zsh, &mut cmd, &bin_name, &mut std::io::stdout());
                    }
                    Shell::Fish => {
                        generate(shells::Fish, &mut cmd, &bin_name, &mut std::io::stdout());
                    }
                    Shell::PowerShell => {
                        generate(
                            shells::PowerShell,
                            &mut cmd,
                            &bin_name,
                            &mut std::io::stdout(),
                        );
                    }
                    Shell::Elvish => {
                        generate(shells::Elvish, &mut cmd, &bin_name, &mut std::io::stdout());
                    }
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
mod cli_tests {
    use clap::Parser;

    use super::*;

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

        // Test run command (local stdio)
        let args = vec!["wassette", "run"];
        let cli = Cli::try_parse_from(args).unwrap();
        matches!(cli.command, Some(Commands::Run(_)));

        // Test serve command (remote HTTP)
        let args = vec!["wassette", "serve"];
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

    /// `--json-response` is optional-valued, so a bare flag and an explicit
    /// value must both parse and must mean different things.
    ///
    /// Exercised through the parser rather than the built binary on purpose: a
    /// `--help` invocation exits successfully before clap ever constructs a
    /// `Serve`, so it would pass without proving either form was understood.
    #[test]
    fn test_serve_json_response_value_is_optional() {
        for (args, expected) in [
            (vec!["wassette", "serve", "--json-response"], Some(true)),
            (
                vec!["wassette", "serve", "--json-response=false"],
                Some(false),
            ),
            (vec!["wassette", "serve"], None),
        ] {
            let cli = Cli::try_parse_from(&args).expect("serve args should parse");
            if let Some(Commands::Serve(serve)) = cli.command {
                assert_eq!(
                    serve.json_response, expected,
                    "unexpected json_response for {args:?}"
                );
            } else {
                panic!("expected a serve command for {args:?}");
            }
        }
    }

    #[test]
    fn test_autocomplete_parsing() {
        // Test autocomplete bash
        let args = vec!["wassette", "autocomplete", "bash"];
        let cli = Cli::try_parse_from(args).unwrap();
        if let Some(Commands::Autocomplete { shell }) = cli.command {
            assert!(matches!(shell, Shell::Bash));
        } else {
            panic!("Expected autocomplete command");
        }

        // Test autocomplete zsh
        let args = vec!["wassette", "autocomplete", "zsh"];
        let cli = Cli::try_parse_from(args).unwrap();
        if let Some(Commands::Autocomplete { shell }) = cli.command {
            assert!(matches!(shell, Shell::Zsh));
        } else {
            panic!("Expected autocomplete command");
        }

        // Test autocomplete fish
        let args = vec!["wassette", "autocomplete", "fish"];
        let cli = Cli::try_parse_from(args).unwrap();
        if let Some(Commands::Autocomplete { shell }) = cli.command {
            assert!(matches!(shell, Shell::Fish));
        } else {
            panic!("Expected autocomplete command");
        }

        // Test autocomplete powershell
        let args = vec!["wassette", "autocomplete", "power-shell"];
        let cli = Cli::try_parse_from(args).unwrap();
        if let Some(Commands::Autocomplete { shell }) = cli.command {
            assert!(matches!(shell, Shell::PowerShell));
        } else {
            panic!("Expected autocomplete command");
        }

        // Test autocomplete elvish
        let args = vec!["wassette", "autocomplete", "elvish"];
        let cli = Cli::try_parse_from(args).unwrap();
        if let Some(Commands::Autocomplete { shell }) = cli.command {
            assert!(matches!(shell, Shell::Elvish));
        } else {
            panic!("Expected autocomplete command");
        }
    }
}
