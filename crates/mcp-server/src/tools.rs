// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock, Tool};
use serde_json::{json, Value};
use tracing::{debug, error, info, instrument, warn};
use wassette::wasm_directory::{
    WasmDirectoryClient, DEFAULT_SEARCH_PAGE_SIZE, MAX_SEARCH_PAGE_SIZE,
};
use wassette::{format_error_chain, LifecycleManager};

use crate::components::{
    extract_args_from_request, get_component_tools, handle_component_call, handle_list_components,
    handle_load_component, handle_unload_component,
};

/// Handles a request to list available tools.
#[instrument(skip(lifecycle_manager))]
pub async fn handle_tools_list(
    lifecycle_manager: &LifecycleManager,
    disable_builtin_tools: bool,
) -> Result<Value> {
    debug!("Handling tools list request");

    let mut tools = get_component_tools(lifecycle_manager).await?;
    if !disable_builtin_tools {
        tools.extend(get_builtin_tools());
    }
    debug!(num_tools = %tools.len(), "Retrieved tools");

    let response = rmcp::model::ListToolsResult {
        tools,
        ..Default::default()
    };

    Ok(serde_json::to_value(response)?)
}

/// Check if a tool name is a builtin tool
pub fn is_builtin_tool(name: &str) -> bool {
    matches!(
        name,
        "load-component"
            | "unload-component"
            | "list-components"
            | "get-policy"
            | "grant-storage-permission"
            | "grant-network-permission"
            | "grant-environment-variable-permission"
            | "revoke-storage-permission"
            | "revoke-network-permission"
            | "revoke-environment-variable-permission"
            | "search-components"
            | "reset-permission"
    )
}

/// Sanitize tool arguments for logging by limiting string length and removing sensitive data
fn sanitize_args_for_logging(args: &Option<serde_json::Map<String, Value>>) -> String {
    const MAX_ARG_LENGTH: usize = 200;
    const MAX_TOTAL_LENGTH: usize = 1000;

    match args {
        None => "{}".to_string(),
        Some(map) => {
            let mut sanitized = serde_json::Map::new();
            let mut total_length = 0;

            for (key, value) in map {
                // Skip potentially sensitive keys
                if key.to_lowercase().contains("password")
                    || key.to_lowercase().contains("secret")
                    || key.to_lowercase().contains("token")
                    || key.to_lowercase().contains("key")
                {
                    sanitized.insert(key.clone(), json!("<redacted>"));
                    continue;
                }

                // Truncate long string values
                let sanitized_value = match value {
                    Value::String(s) if s.len() > MAX_ARG_LENGTH => {
                        json!(format!("{}... ({} chars)", &s[..MAX_ARG_LENGTH], s.len()))
                    }
                    _ => value.clone(),
                };

                // Check if adding this key-value pair would exceed the total length before insertion
                // The +20 accounts for JSON overhead (quotes, colons, commas, braces)
                if total_length + key.len() + 20 > MAX_TOTAL_LENGTH {
                    sanitized.insert("...".to_string(), json!("(truncated)"));
                    break;
                }

                sanitized.insert(key.clone(), sanitized_value);
                total_length += key.len() + 20;
            }

            serde_json::to_string(&sanitized).unwrap_or_else(|_| "{}".to_string())
        }
    }
}

/// Handles a tool call request without transport-specific notifications.
#[instrument(skip_all, fields(method_name = %req.name))]
pub async fn handle_tools_call(
    req: CallToolRequestParams,
    lifecycle_manager: &LifecycleManager,
    disable_builtin_tools: bool,
) -> Result<Value> {
    let start_time = Instant::now();
    let tool_name = req.name.to_string();
    let sanitized_args = sanitize_args_for_logging(&req.arguments);

    debug!(
        tool_name = %tool_name,
        arguments = %sanitized_args,
        "Tool invocation started"
    );

    let result = if disable_builtin_tools && is_builtin_tool(req.name.as_ref()) {
        // When builtin tools are disabled, reject calls to builtin tools
        warn!(
            tool_name = %tool_name,
            "Tool invocation rejected: built-in tools are disabled"
        );
        Err(anyhow::anyhow!("Built-in tools are disabled"))
    } else {
        // Handle builtin tools (if enabled) or component calls
        match req.name.as_ref() {
            "load-component" if !disable_builtin_tools => {
                handle_load_component(&req, lifecycle_manager).await
            }
            "unload-component" if !disable_builtin_tools => {
                handle_unload_component(&req, lifecycle_manager).await
            }
            "list-components" if !disable_builtin_tools => {
                handle_list_components(lifecycle_manager).await
            }
            "get-policy" if !disable_builtin_tools => {
                handle_get_policy(&req, lifecycle_manager).await
            }
            "grant-storage-permission" if !disable_builtin_tools => {
                handle_grant_storage_permission(&req, lifecycle_manager).await
            }
            "grant-network-permission" if !disable_builtin_tools => {
                handle_grant_network_permission(&req, lifecycle_manager).await
            }
            "grant-environment-variable-permission" if !disable_builtin_tools => {
                handle_grant_environment_variable_permission(&req, lifecycle_manager).await
            }
            "revoke-storage-permission" if !disable_builtin_tools => {
                handle_revoke_storage_permission(&req, lifecycle_manager).await
            }
            "revoke-network-permission" if !disable_builtin_tools => {
                handle_revoke_network_permission(&req, lifecycle_manager).await
            }
            "revoke-environment-variable-permission" if !disable_builtin_tools => {
                handle_revoke_environment_variable_permission(&req, lifecycle_manager).await
            }
            "search-components" if !disable_builtin_tools => {
                handle_search_component(&req, lifecycle_manager).await
            }
            "reset-permission" if !disable_builtin_tools => {
                handle_reset_permission(&req, lifecycle_manager).await
            }
            _ => handle_component_call(&req, lifecycle_manager).await,
        }
    };

    let duration = start_time.elapsed();

    match &result {
        Ok(_) => {
            debug!(
                tool_name = %tool_name,
                duration_ms = %duration.as_millis(),
                outcome = "success",
                "Tool invocation completed successfully"
            );
        }
        Err(e) => {
            error!(
                tool_name = %tool_name,
                duration_ms = %duration.as_millis(),
                outcome = "error",
                error = %format_error_chain(e),
                "Tool invocation failed"
            );
        }
    }

    match result {
        Ok(result) => Ok(serde_json::to_value(result)?),
        Err(e) => {
            let error_text = format!("Error: {}", format_error_chain(&e));
            let contents = vec![ContentBlock::text(error_text)];

            let error_result = CallToolResult::error(contents);
            Ok(serde_json::to_value(error_result)?)
        }
    }
}

fn get_builtin_tools() -> Vec<Tool> {
    debug!("Getting builtin tools");
    vec![
        Tool::new_with_raw(
            Cow::Borrowed("load-component"),
            Some(Cow::Borrowed(
                "Loads a component from a direct path/URI or a wasm.directory package. Packages are selected by registry/repository or exact WIT identity (namespace:package[@version]) and resolved to a digest-pinned version.",
            )),
            Arc::new(
                serde_json::from_value(json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Direct file://, oci://, or https:// component source"
                        },
                        "package": {
                            "type": "string",
                            "description": "wasm.directory package as registry/repository, or an exact WIT identity namespace:package[@version] that must match exactly one package"
                        },
                        "version": {
                            "type": "string",
                            "description": "Optional exact indexed package tag; valid only with package"
                        }
                    },
                    "oneOf": [
                        {
                            "required": ["path"],
                            "not": {"anyOf": [{"required": ["package"]}, {"required": ["version"]}]}
                        },
                        {
                            "required": ["package"],
                            "not": {"required": ["path"]}
                        }
                    ]
                }))
                .unwrap_or_default(),
            ),
        ),
        Tool::new_with_raw(
            Cow::Borrowed("unload-component"),
            Some(Cow::Borrowed(
                "Unloads a tool or component.",
            )),
            Arc::new(
                serde_json::from_value(json!({
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"}
                    },
                    "required": ["id"]
                }))
                .unwrap_or_default(),
            ),
        ),
        Tool::new_with_raw(
            Cow::Borrowed("list-components"),
            Some(Cow::Borrowed(
                "Lists all currently loaded components or tools.",
            )),
            Arc::new(
                serde_json::from_value(json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }))
                .unwrap_or_default(),
            ),
        ),
        Tool::new_with_raw(
            Cow::Borrowed("get-policy"),
            Some(Cow::Borrowed(
                "Gets the policy information for a specific component",
            )),
            Arc::new(
                serde_json::from_value(json!({
                    "type": "object",
                    "properties": {
                        "component_id": {
                            "type": "string",
                            "description": "ID of the component to get policy for"
                        }
                    },
                    "required": ["component_id"]
                }))
                .unwrap_or_default(),
            ),
        ),
        Tool::new_with_raw(
            Cow::Borrowed("grant-storage-permission"),
            Some(Cow::Borrowed(
                "Grants storage access permission to a component, allowing it to read from and/or write to specific storage locations."
            )),
            Arc::new(
                serde_json::from_value(json!({
                    "type": "object",
                    "properties": {
                      "component_id": {
                        "type": "string",
                        "description": "ID of the component to grant storage permission to"
                      },
                      "details": {
                        "type": "object",
                        "properties": {
                          "uri": { 
                            "type": "string",
                            "description": "URI of the storage resource to grant access to. e.g. fs:///tmp/test"
                          },
                          "access": {
                            "type": "array",
                            "items": {
                              "type": "string",
                              "enum": ["read", "write"]
                            },
                            "description": "Access type for the storage resource, this must be an array of strings with values 'read' or 'write'"
                          }
                        },
                        "required": ["uri", "access"],
                        "additionalProperties": false
                      }
                    },
                    "required": ["component_id", "details"]
                  }))
                .unwrap_or_default(),
            ),
        ),
        Tool::new_with_raw(
            Cow::Borrowed("grant-network-permission"),
            Some(Cow::Borrowed(
                "Grants network access permission to a component, allowing it to make network requests to specific hosts."
            )),
            Arc::new(
                serde_json::from_value(json!({
                    "type": "object",
                    "properties": {
                      "component_id": {
                        "type": "string",
                        "description": "ID of the component to grant network permission to"
                      },
                      "details": {
                        "type": "object",
                        "properties": {
                          "host": { 
                            "type": "string",
                            "description": "Host to grant network access to"
                          }
                        },
                        "required": ["host"],
                        "additionalProperties": false
                      }
                    },
                    "required": ["component_id", "details"]
                  }))
                .unwrap_or_default(),
            ),
        ),
        Tool::new_with_raw(
            Cow::Borrowed("grant-environment-variable-permission"),
            Some(Cow::Borrowed(
                "Grants environment variable access permission to a component, allowing it to access specific environment variables."
            )),
            Arc::new(
                serde_json::from_value(json!({
                    "type": "object",
                    "properties": {
                      "component_id": {
                        "type": "string",
                        "description": "ID of the component to grant environment variable permission to"
                      },
                      "details": {
                        "type": "object",
                        "properties": {
                          "key": { 
                            "type": "string",
                            "description": "Environment variable key to grant access to"
                          }
                        },
                        "required": ["key"],
                        "additionalProperties": false
                      }
                    },
                    "required": ["component_id", "details"]
                  }))
                .unwrap_or_default(),
            ),
        ),
        Tool::new_with_raw(
            Cow::Borrowed("revoke-storage-permission"),
            Some(Cow::Borrowed(
                "Revokes all storage access permissions from a component for the specified URI path, removing both read and write access to that location."
            )),
            Arc::new(
                serde_json::from_value(json!({
                    "type": "object",
                    "properties": {
                      "component_id": {
                        "type": "string",
                        "description": "ID of the component to revoke storage permission from"
                      },
                      "details": {
                        "type": "object",
                        "properties": {
                          "uri": { 
                            "type": "string",
                            "description": "URI of the storage resource to revoke all access from. e.g. fs:///tmp/test"
                          }
                        },
                        "required": ["uri"],
                        "additionalProperties": false
                      }
                    },
                    "required": ["component_id", "details"]
                  }))
                .unwrap_or_default(),
            ),
        ),
        Tool::new_with_raw(
            Cow::Borrowed("revoke-network-permission"),
            Some(Cow::Borrowed(
                "Revokes network access permission from a component, removing its ability to make network requests to specific hosts."
            )),
            Arc::new(
                serde_json::from_value(json!({
                    "type": "object",
                    "properties": {
                      "component_id": {
                        "type": "string",
                        "description": "ID of the component to revoke network permission from"
                      },
                      "details": {
                        "type": "object",
                        "properties": {
                          "host": { 
                            "type": "string",
                            "description": "Host to revoke network access from"
                          }
                        },
                        "required": ["host"],
                        "additionalProperties": false
                      }
                    },
                    "required": ["component_id", "details"]
                  }))
                .unwrap_or_default(),
            ),
        ),
        Tool::new_with_raw(
            Cow::Borrowed("revoke-environment-variable-permission"),
            Some(Cow::Borrowed(
                "Revokes environment variable access permission from a component, removing its ability to access specific environment variables."
            )),
            Arc::new(
                serde_json::from_value(json!({
                    "type": "object",
                    "properties": {
                      "component_id": {
                        "type": "string",
                        "description": "ID of the component to revoke environment variable permission from"
                      },
                      "details": {
                        "type": "object",
                        "properties": {
                          "key": { 
                            "type": "string",
                            "description": "Environment variable key to revoke access from"
                          }
                        },
                        "required": ["key"],
                        "additionalProperties": false
                      }
                    },
                    "required": ["component_id", "details"]
                  }))
                .unwrap_or_default(),
            ),
        ),
        Tool::new_with_raw(
            Cow::Borrowed("reset-permission"),
            Some(Cow::Borrowed(
                "Resets all permissions for a component, removing all granted permissions and returning it to the default state."
            )),
            Arc::new(
                serde_json::from_value(json!({
                    "type": "object",
                    "properties": {
                      "component_id": {
                        "type": "string",
                        "description": "ID of the component to reset permissions for"
                      }
                    },
                    "required": ["component_id"]
                  }))
                .unwrap_or_default(),
            ),
        ),
        Tool::new_with_raw(
            Cow::Borrowed("search-components"),
            Some(Cow::Borrowed(
                "Searches wasm.directory for component packages. Results are discovery-only and do not install or expose components.",
            )),
            Arc::new(
                serde_json::from_value(json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Optional search query sent to wasm.directory"
                        },
                        "offset": {
                            "type": "integer",
                            "minimum": 0,
                            "description": "Offset into wasm.directory search results"
                        },
                        "limit": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": MAX_SEARCH_PAGE_SIZE,
                            "default": DEFAULT_SEARCH_PAGE_SIZE,
                            "description": "Maximum number of upstream records to fetch"
                        }
                    },
                    "required": []
                }))
                .unwrap_or_default(),
            ),
        ),
    ]
}

#[instrument(skip(_lifecycle_manager))]
pub(crate) async fn handle_search_component(
    req: &CallToolRequestParams,
    _lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;
    let query = match args.get("query") {
        Some(value) => Some(
            value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Argument 'query' must be a string"))?,
        ),
        None => None,
    };
    let offset = parse_search_integer(&args, "offset", 0)?;
    let limit = parse_search_integer(&args, "limit", DEFAULT_SEARCH_PAGE_SIZE)?;
    let page = WasmDirectoryClient::from_environment()?
        .search(query, offset, limit)
        .await?;

    let status_text = serde_json::to_string(&json!({
        "status": "success",
        "source": "wasm.directory",
        "discovery_only": true,
        "count": page.packages.len(),
        "upstream_count": page.upstream_count,
        "offset": page.offset,
        "limit": page.limit,
        "next_offset": page.next_offset,
        "may_have_more": page.may_have_more,
        "components": page.packages,
    }))?;

    let contents = vec![ContentBlock::text(status_text)];

    Ok(CallToolResult::success(contents))
}

fn parse_search_integer(
    args: &serde_json::Map<String, Value>,
    name: &str,
    default: usize,
) -> Result<usize> {
    let Some(value) = args.get(name) else {
        return Ok(default);
    };
    let value = value
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("Argument '{name}' must be a non-negative integer"))?;
    usize::try_from(value)
        .map_err(|_| anyhow::anyhow!("Argument '{name}' is too large for this platform"))
}

#[instrument(skip(lifecycle_manager))]
pub async fn handle_get_policy(
    req: &CallToolRequestParams,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;

    let component_id = args
        .get("component_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'component_id'"))?;

    info!("Getting policy for component {}", component_id);

    // Ensure the component is available (compile lazily if needed)
    lifecycle_manager
        .ensure_component_loaded(component_id)
        .await
        .with_context(|| format!("Component not found: {component_id}"))?;

    let policy_info = lifecycle_manager.get_policy_info(component_id).await;

    let status_text = if let Some(info) = policy_info {
        serde_json::to_string(&json!({
            "status": "policy found",
            "component_id": component_id,
            "policy_info": {
                "policy_id": info.policy_id,
                "source_uri": info.source_uri,
                "local_path": info.local_path,
                "created_at": info.created_at.duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default().as_secs()
            }
        }))?
    } else {
        serde_json::to_string(&json!({
            "status": "no policy found",
            "component_id": component_id
        }))?
    };

    let contents = vec![ContentBlock::text(status_text)];

    Ok(CallToolResult::success(contents))
}

/// Generic helper for handling grant permission requests
async fn handle_grant_permission_generic(
    req: &CallToolRequestParams,
    lifecycle_manager: &LifecycleManager,
    permission_type: &str,
    permission_display_name: &str,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;

    let component_id = args
        .get("component_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'component_id'"))?;

    let details = args
        .get("details")
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'details'"))?;

    info!(
        "Granting {} permission to component {}",
        permission_display_name, component_id
    );

    // Ensure component is loaded (lazy compile)
    lifecycle_manager
        .ensure_component_loaded(component_id)
        .await
        .with_context(|| format!("Component not found: {component_id}"))?;

    let result = lifecycle_manager
        .grant_permission(component_id, permission_type, details)
        .await;

    match result {
        Ok(()) => {
            let status_text = serde_json::to_string(&json!({
                "status": "permission granted successfully",
                "component_id": component_id,
                "permission_type": permission_display_name,
                "details": details
            }))?;

            let contents = vec![ContentBlock::text(status_text)];

            Ok(CallToolResult::success(contents))
        }
        Err(e) => {
            error!(
                "Failed to grant {} permission: {}",
                permission_display_name, e
            );
            Err(anyhow::anyhow!(
                "Failed to grant {} permission to component {}: {}",
                permission_display_name,
                component_id,
                e
            ))
        }
    }
}

#[instrument(skip(lifecycle_manager))]
pub async fn handle_grant_storage_permission(
    req: &CallToolRequestParams,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    handle_grant_permission_generic(req, lifecycle_manager, "storage", "storage").await
}

#[instrument(skip(lifecycle_manager))]
pub async fn handle_grant_network_permission(
    req: &CallToolRequestParams,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    handle_grant_permission_generic(req, lifecycle_manager, "network", "network").await
}

#[instrument(skip(lifecycle_manager))]
pub async fn handle_grant_environment_variable_permission(
    req: &CallToolRequestParams,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    handle_grant_permission_generic(
        req,
        lifecycle_manager,
        "environment",
        "environment variable",
    )
    .await
}

#[instrument(skip(lifecycle_manager))]
pub async fn handle_grant_memory_permission(
    req: &CallToolRequestParams,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    handle_grant_permission_generic(req, lifecycle_manager, "resource", "memory").await
}

/// Generic helper for handling revoke permission requests
async fn handle_revoke_permission_generic(
    req: &CallToolRequestParams,
    lifecycle_manager: &LifecycleManager,
    permission_type: &str,
    permission_display_name: &str,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;

    let component_id = args
        .get("component_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'component_id'"))?;

    let details = args
        .get("details")
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'details'"))?;

    info!(
        "Revoking {} permission from component {}",
        permission_display_name, component_id
    );

    lifecycle_manager
        .ensure_component_loaded(component_id)
        .await
        .with_context(|| format!("Component not found: {component_id}"))?;

    let result = lifecycle_manager
        .revoke_permission(component_id, permission_type, details)
        .await;

    match result {
        Ok(()) => {
            let status_text = serde_json::to_string(&json!({
                "status": "permission revoked",
                "component_id": component_id,
                "permission_type": permission_display_name,
                "details": details
            }))?;

            let contents = vec![ContentBlock::text(status_text)];

            Ok(CallToolResult::success(contents))
        }
        Err(e) => {
            error!(
                "Failed to revoke {} permission: {}",
                permission_display_name, e
            );
            Err(anyhow::anyhow!(
                "Failed to revoke {} permission from component {}: {}",
                permission_display_name,
                component_id,
                e
            ))
        }
    }
}

#[instrument(skip(lifecycle_manager))]
pub async fn handle_revoke_storage_permission(
    req: &CallToolRequestParams,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;

    let component_id = args
        .get("component_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'component_id'"))?;

    let details = args
        .get("details")
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'details'"))?;

    let uri = details
        .get("uri")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'uri' field in details"))?;

    info!(
        "Revoking all storage permissions for URI {} from component {}",
        uri, component_id
    );

    lifecycle_manager
        .ensure_component_loaded(component_id)
        .await
        .with_context(|| format!("Component not found: {component_id}"))?;

    let result = lifecycle_manager
        .revoke_storage_permission_by_uri(component_id, uri)
        .await;

    match result {
        Ok(()) => {
            let status_text = serde_json::to_string(&json!({
                "status": "permission revoked successfully",
                "component_id": component_id,
                "uri": uri,
                "message": "All access (read and write) to the specified URI has been revoked"
            }))?;

            let contents = vec![ContentBlock::text(status_text)];

            Ok(CallToolResult::success(contents))
        }
        Err(e) => {
            error!("Failed to revoke storage permission: {}", e);
            Err(anyhow::anyhow!(
                "Failed to revoke storage permission from component {}: {}",
                component_id,
                e
            ))
        }
    }
}

#[instrument(skip(lifecycle_manager))]
pub async fn handle_revoke_network_permission(
    req: &CallToolRequestParams,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    handle_revoke_permission_generic(req, lifecycle_manager, "network", "network").await
}

#[instrument(skip(lifecycle_manager))]
pub async fn handle_revoke_environment_variable_permission(
    req: &CallToolRequestParams,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    handle_revoke_permission_generic(
        req,
        lifecycle_manager,
        "environment",
        "environment variable",
    )
    .await
}

#[instrument(skip(lifecycle_manager))]
pub async fn handle_reset_permission(
    req: &CallToolRequestParams,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;

    let component_id = args
        .get("component_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'component_id'"))?;

    info!("Resetting all permissions for component {}", component_id);

    lifecycle_manager
        .ensure_component_loaded(component_id)
        .await
        .with_context(|| format!("Component not found: {component_id}"))?;

    let result = lifecycle_manager.reset_permission(component_id).await;

    match result {
        Ok(()) => {
            let status_text = serde_json::to_string(&json!({
                "status": "permissions reset successfully",
                "component_id": component_id
            }))?;

            let contents = vec![ContentBlock::text(status_text)];

            Ok(CallToolResult::success(contents))
        }
        Err(e) => {
            error!("Failed to reset permissions: {}", e);
            Err(anyhow::anyhow!(
                "Failed to reset permissions for component {}: {}",
                component_id,
                e
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_builtin_tools() {
        let tools = get_builtin_tools();
        assert_eq!(tools.len(), 12);
        assert!(tools.iter().any(|t| t.name == "load-component"));
        assert!(tools.iter().any(|t| t.name == "unload-component"));
        assert!(tools.iter().any(|t| t.name == "list-components"));
        assert!(tools.iter().any(|t| t.name == "get-policy"));
        assert!(tools.iter().any(|t| t.name == "grant-storage-permission"));
        assert!(tools.iter().any(|t| t.name == "grant-network-permission"));
        assert!(tools
            .iter()
            .any(|t| t.name == "grant-environment-variable-permission"));
        assert!(tools.iter().any(|t| t.name == "revoke-storage-permission"));
        assert!(tools.iter().any(|t| t.name == "revoke-network-permission"));
        assert!(tools
            .iter()
            .any(|t| t.name == "revoke-environment-variable-permission"));
        assert!(tools.iter().any(|t| t.name == "reset-permission"));
        assert!(tools.iter().any(|t| t.name == "search-components"));

        let search_tool = tools
            .iter()
            .find(|tool| tool.name == "search-components")
            .unwrap();
        let schema = serde_json::to_value(search_tool).unwrap();
        assert!(schema["inputSchema"]["properties"]["offset"].is_object());
        assert!(schema["inputSchema"]["properties"]["limit"].is_object());

        let load_tool = tools
            .iter()
            .find(|tool| tool.name == "load-component")
            .unwrap();
        let schema = serde_json::to_value(load_tool).unwrap();
        assert!(schema["inputSchema"]["properties"]["path"].is_object());
        assert!(schema["inputSchema"]["properties"]["package"].is_object());
        assert!(schema["inputSchema"]["properties"]["version"].is_object());
        assert!(schema["inputSchema"]["oneOf"].is_array());
    }

    #[tokio::test]
    async fn test_grant_network_permission_integration() -> Result<()> {
        // Create a test lifecycle manager
        let tempdir = tempfile::tempdir()?;
        let lifecycle_manager = wassette::LifecycleManager::new(&tempdir).await?;

        // Test the grant_network_permission tool call
        let mut args = serde_json::Map::new();
        args.insert("component_id".to_string(), json!("test-component"));
        args.insert("details".to_string(), json!({"host": "api.example.com"}));

        let req = CallToolRequestParams::new("grant-network-permission").with_arguments(args);

        // This should fail because the component doesn't exist, but it tests the flow
        let result = handle_grant_network_permission(&req, &lifecycle_manager).await;

        // The result should be an error because the component doesn't exist
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Component not found"));

        Ok(())
    }

    #[tokio::test]
    async fn test_grant_storage_permission_integration() -> Result<()> {
        // Create a test lifecycle manager
        let tempdir = tempfile::tempdir()?;
        let lifecycle_manager = wassette::LifecycleManager::new(&tempdir).await?;

        // Test the grant_storage_permission tool call
        let mut args = serde_json::Map::new();
        args.insert("component_id".to_string(), json!("test-component"));
        args.insert(
            "details".to_string(),
            json!({"uri": "file:///tmp/test", "access": ["read", "write"]}),
        );

        let req = CallToolRequestParams::new("grant-storage-permission").with_arguments(args);

        // This should fail because the component doesn't exist, but it tests the flow
        let result = handle_grant_storage_permission(&req, &lifecycle_manager).await;

        // The result should be an error because the component doesn't exist
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Component not found"));

        Ok(())
    }

    #[tokio::test]
    async fn test_grant_permission_missing_arguments() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let lifecycle_manager = wassette::LifecycleManager::new(&tempdir).await?;

        // Test with missing component_id for network permission
        let mut args = serde_json::Map::new();
        args.insert("details".to_string(), json!({"host": "api.example.com"}));

        let req = CallToolRequestParams::new("grant-network-permission").with_arguments(args);

        let result = handle_grant_network_permission(&req, &lifecycle_manager).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing required argument: 'component_id'"));

        // Test with missing details for network permission
        let mut args = serde_json::Map::new();
        args.insert("component_id".to_string(), json!("test-component"));

        let req = CallToolRequestParams::new("grant-network-permission").with_arguments(args);

        let result = handle_grant_network_permission(&req, &lifecycle_manager).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing required argument: 'details'"));

        // Test with missing component_id for storage permission
        let mut args = serde_json::Map::new();
        args.insert(
            "details".to_string(),
            json!({"uri": "file:///tmp/test", "access": ["read"]}),
        );

        let req = CallToolRequestParams::new("grant-storage-permission").with_arguments(args);

        let result = handle_grant_storage_permission(&req, &lifecycle_manager).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing required argument: 'component_id'"));

        // Test with missing details for storage permission
        let mut args = serde_json::Map::new();
        args.insert("component_id".to_string(), json!("test-component"));

        let req = CallToolRequestParams::new("grant-storage-permission").with_arguments(args);

        let result = handle_grant_storage_permission(&req, &lifecycle_manager).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing required argument: 'details'"));

        Ok(())
    }

    // Revoke permission system tests

    #[tokio::test]
    async fn test_revoke_permission_network() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let lifecycle_manager = wassette::LifecycleManager::new(&tempdir).await?;

        // Test the revoke-network-permission tool call
        let mut args = serde_json::Map::new();
        args.insert("component_id".to_string(), json!("test-component"));
        args.insert("details".to_string(), json!({"host": "api.example.com"}));

        let req = CallToolRequestParams::new("revoke-network-permission").with_arguments(args);

        // This should fail because the component doesn't exist, but it tests the flow
        let result = handle_revoke_network_permission(&req, &lifecycle_manager).await;

        // The result should be an error because the component doesn't exist
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Component not found"));

        Ok(())
    }

    #[tokio::test]
    async fn test_revoke_storage_permission_integration() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let lifecycle_manager = wassette::LifecycleManager::new(&tempdir).await?;

        // Test the revoke-storage-permission tool call
        let mut args = serde_json::Map::new();
        args.insert("component_id".to_string(), json!("test-component"));
        args.insert(
            "details".to_string(),
            json!({"uri": "fs:///tmp/test", "access": ["read", "write"]}),
        );

        let req = CallToolRequestParams::new("revoke-storage-permission").with_arguments(args);

        // This should fail because the component doesn't exist, but it tests the flow
        let result = handle_revoke_storage_permission(&req, &lifecycle_manager).await;

        // The result should be an error because the component doesn't exist
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Component not found"));

        Ok(())
    }

    #[tokio::test]
    async fn test_revoke_environment_variable_permission_integration() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let lifecycle_manager = wassette::LifecycleManager::new(&tempdir).await?;

        // Test the revoke-environment-variable-permission tool call
        let mut args = serde_json::Map::new();
        args.insert("component_id".to_string(), json!("test-component"));
        args.insert("details".to_string(), json!({"key": "API_KEY"}));

        let req = CallToolRequestParams::new("revoke-environment-variable-permission")
            .with_arguments(args);

        // This should fail because the component doesn't exist, but it tests the flow
        let result = handle_revoke_environment_variable_permission(&req, &lifecycle_manager).await;

        // The result should be an error because the component doesn't exist
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Component not found"));

        Ok(())
    }

    #[tokio::test]
    async fn test_reset_permission_integration() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let lifecycle_manager = wassette::LifecycleManager::new(&tempdir).await?;

        // Test the reset-permission tool call
        let mut args = serde_json::Map::new();
        args.insert("component_id".to_string(), json!("test-component"));

        let req = CallToolRequestParams::new("reset-permission").with_arguments(args);

        // This should fail because the component doesn't exist, but it tests the flow
        let result = handle_reset_permission(&req, &lifecycle_manager).await;

        // The result should be an error because the component doesn't exist
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Component not found"));

        Ok(())
    }

    #[tokio::test]
    async fn test_revoke_permission_missing_arguments() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let lifecycle_manager = wassette::LifecycleManager::new(&tempdir).await?;

        // Test with missing component_id for revoke network permission
        let mut args = serde_json::Map::new();
        args.insert("details".to_string(), json!({"host": "api.example.com"}));

        let req = CallToolRequestParams::new("revoke-network-permission").with_arguments(args);

        let result = handle_revoke_network_permission(&req, &lifecycle_manager).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing required argument: 'component_id'"));

        // Test with missing details for revoke network permission
        let mut args = serde_json::Map::new();
        args.insert("component_id".to_string(), json!("test-component"));

        let req = CallToolRequestParams::new("revoke-network-permission").with_arguments(args);

        let result = handle_revoke_network_permission(&req, &lifecycle_manager).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing required argument: 'details'"));

        // Test with missing component_id for reset permission
        let args = serde_json::Map::new();

        let req = CallToolRequestParams::new("reset-permission").with_arguments(args);

        let result = handle_reset_permission(&req, &lifecycle_manager).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing required argument: 'component_id'"));

        Ok(())
    }

    #[test]
    fn test_sanitize_args_for_logging_redacts_sensitive_keys() {
        let mut args = serde_json::Map::new();
        args.insert("url".to_string(), json!("https://example.com"));
        args.insert("api_key".to_string(), json!("secret-key-123"));
        args.insert("password".to_string(), json!("my-password"));
        args.insert("token".to_string(), json!("bearer-token"));

        let sanitized = sanitize_args_for_logging(&Some(args));

        assert!(sanitized.contains("\"url\""));
        assert!(sanitized.contains("https://example.com"));
        assert!(sanitized.contains("<redacted>"));
        assert!(!sanitized.contains("secret-key-123"));
        assert!(!sanitized.contains("my-password"));
        assert!(!sanitized.contains("bearer-token"));
    }

    #[test]
    fn test_sanitize_args_for_logging_truncates_long_strings() {
        let mut args = serde_json::Map::new();
        let long_string = "a".repeat(300);
        args.insert("data".to_string(), json!(long_string));

        let sanitized = sanitize_args_for_logging(&Some(args));

        assert!(sanitized.contains("300 chars"));
        assert!(!sanitized.contains(&"a".repeat(300)));
    }

    #[test]
    fn test_sanitize_args_for_logging_handles_empty() {
        let sanitized = sanitize_args_for_logging(&None);
        assert_eq!(sanitized, "{}");

        let empty_args = serde_json::Map::new();
        let sanitized = sanitize_args_for_logging(&Some(empty_args));
        assert_eq!(sanitized, "{}");
    }

    #[test]
    fn test_sanitize_args_for_logging_preserves_normal_data() {
        let mut args = serde_json::Map::new();
        args.insert("name".to_string(), json!("test"));
        args.insert("count".to_string(), json!(42));
        args.insert("enabled".to_string(), json!(true));

        let sanitized = sanitize_args_for_logging(&Some(args));

        assert!(sanitized.contains("\"name\""));
        assert!(sanitized.contains("test"));
        assert!(sanitized.contains("\"count\""));
        assert!(sanitized.contains("42"));
        assert!(sanitized.contains("\"enabled\""));
        assert!(sanitized.contains("true"));
    }

    #[test]
    fn search_pagination_uses_defaults_and_rejects_invalid_values() {
        let empty = serde_json::Map::new();
        assert_eq!(parse_search_integer(&empty, "offset", 0).unwrap(), 0);
        assert_eq!(
            parse_search_integer(&empty, "limit", DEFAULT_SEARCH_PAGE_SIZE).unwrap(),
            DEFAULT_SEARCH_PAGE_SIZE
        );

        for value in [json!(-1), json!(1.5), json!("10")] {
            let args = serde_json::Map::from_iter([("offset".to_owned(), value)]);
            assert!(parse_search_integer(&args, "offset", 0).is_err());
        }
    }

    #[tokio::test]
    async fn search_rejects_non_string_query_without_network_access() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let lifecycle_manager = wassette::LifecycleManager::new(&tempdir).await?;
        let req = CallToolRequestParams::new("search-components").with_arguments(
            serde_json::Map::from_iter([("query".to_owned(), json!(42))]),
        );

        let error = handle_search_component(&req, &lifecycle_manager)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("must be a string"));
        Ok(())
    }

    #[tokio::test]
    async fn search_rejects_limits_over_api_max_without_network_access() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let lifecycle_manager = wassette::LifecycleManager::new(&tempdir).await?;
        let req = CallToolRequestParams::new("search-components").with_arguments(
            serde_json::Map::from_iter([("limit".to_owned(), json!(101))]),
        );

        let error = handle_search_component(&req, &lifecycle_manager)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("between 1 and 100"));
        Ok(())
    }
}
