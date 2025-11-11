// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! The main `wassette(1)` command.

#![warn(missing_docs)]

use anyhow::{bail, Context, Result};
use clap::Parser;
use mcp_server::{handle_tools_list, LifecycleManager};
use rmcp::service::serve_server;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::{stdio as stdio_transport, SseServer};
use serde_json::{json, Map};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

mod cli_handlers;
mod commands;
mod config;
mod format;
mod registry;
mod server;
mod tools;
mod utils;

use cli_handlers::{create_lifecycle_manager, handle_tool_cli_command};
use commands::{
    Cli, Commands, ComponentCommands, GrantPermissionCommands, PermissionCommands, PolicyCommands,
    RegistryCommands, RevokePermissionCommands, SecretCommands, ToolCommands, Transport,
};
use format::{print_result, OutputFormat};
use server::McpServer;
use tools::ToolName;
use utils::{format_build_info, load_component_registry, parse_env_var};

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
                    component_dir,
                    secrets_dir,
                    environment_vars,
                    bind_address,
                } = config;

                let lifecycle_manager = LifecycleManager::builder(component_dir)
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

                        // Spawn the server in a background task
                        let server_handle = tokio::spawn(async move {
                            axum::serve(tcp_listener, router)
                                .with_graceful_shutdown(async {
                                    tokio::signal::ctrl_c().await.unwrap()
                                })
                                .await
                        });

                        tracing::info!(
                            "MCP server is ready and listening on http://{}/mcp",
                            bind_address
                        );

                        // Wait for the server task to complete
                        let _ = server_handle.await;
                    }
                    Transport::Sse => {
                        tracing::info!(
                        "Starting MCP server on {} with SSE HTTP transport. Components will load in the background.",
                        bind_address
                    );
                        let ct = SseServer::serve(bind_address.parse().unwrap())
                            .await?
                            .with_service(move || server.clone());
                        tracing::info!(
                            "MCP server is ready and listening on http://{}/sse",
                            bind_address
                        );

                        tokio::signal::ctrl_c().await?;
                        ct.cancel();
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
                        &rmcp::model::CallToolResult {
                            content: Some(vec![rmcp::model::Content::text(content)]),
                            structured_content: None,
                            is_error: None,
                        },
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
                        &rmcp::model::CallToolResult {
                            content: Some(vec![rmcp::model::Content::text(content)]),
                            structured_content: None,
                            is_error: None,
                        },
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
                        let req = rmcp::model::CallToolRequestParam {
                            name: name.clone().into(),
                            arguments: Some(arguments),
                        };

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
                                eprintln!("Error invoking tool '{}': {}", name, e);
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
                        &rmcp::model::CallToolResult {
                            content: Some(vec![rmcp::model::Content::text(
                                serde_json::to_string_pretty(&result)?,
                            )]),
                            structured_content: None,
                            is_error: None,
                        },
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
