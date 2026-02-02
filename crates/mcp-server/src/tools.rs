// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use rmcp::model::{CallToolRequestParam, CallToolResult, Content, Tool};
use rmcp::{Peer, RoleServer};
use serde_json::{json, Value};
use tracing::{debug, error, info, instrument, warn};
use wassette::LifecycleManager;

use crate::components::{
    extract_args_from_request, get_component_tools, handle_component_call, handle_list_components,
    handle_load_component, handle_unload_component,
};

/// The list of components that Wassette knows about
const COMPONENT_LIST: &str = include_str!("../../../component-registry.json");

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
        next_cursor: None,
    };

    Ok(serde_json::to_value(response)?)
}

/// Check if a tool name is a builtin tool
fn is_builtin_tool(name: &str) -> bool {
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

/// Handles a tool call request.
#[instrument(skip_all, fields(method_name = %req.name))]
pub async fn handle_tools_call(
    req: CallToolRequestParam,
    lifecycle_manager: &LifecycleManager,
    server_peer: Peer<RoleServer>,
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
                handle_load_component(&req, lifecycle_manager, server_peer).await
            }
            "unload-component" if !disable_builtin_tools => {
                handle_unload_component(&req, lifecycle_manager, server_peer).await
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
                error = %e,
                "Tool invocation failed"
            );
        }
    }

    match result {
        Ok(result) => Ok(serde_json::to_value(result)?),
        Err(e) => {
            let error_text = format!("Error: {e}");
            let contents = vec![Content::text(error_text)];

            let error_result = CallToolResult {
                content: contents,
                structured_content: None,
                is_error: Some(true),
                meta: None,
            };
            Ok(serde_json::to_value(error_result)?)
        }
    }
}

fn get_builtin_tools() -> Vec<Tool> {
    debug!("Getting builtin tools");
    vec![
        Tool {
            name: Cow::Borrowed("load-component"),
            description: Some(Cow::Borrowed(
                "Dynamically loads a new tool or component from either the filesystem or OCI registries.",
            )),
            input_schema: Arc::new(
                serde_json::from_value(json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"}
                    },
                    "required": ["path"]
                }))
                .unwrap_or_default(),
            ),
            output_schema: None,
            annotations: None,
            title: None,
            icons: None,
            meta: None,
        },
        Tool {
            name: Cow::Borrowed("unload-component"),
            description: Some(Cow::Borrowed(
                "Unloads a tool or component.",
            )),
            input_schema: Arc::new(
                serde_json::from_value(json!({
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"}
                    },
                    "required": ["id"]
                }))
                .unwrap_or_default(),
            ),
            output_schema: None,
            annotations: None,
            title: None,
            icons: None,
            meta: None,
        },
        Tool {
            name: Cow::Borrowed("list-components"),
            description: Some(Cow::Borrowed(
                "Lists all currently loaded components or tools.",
            )),
            input_schema: Arc::new(
                serde_json::from_value(json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }))
                .unwrap_or_default(),
            ),
            output_schema: None,
            annotations: None,
            title: None,
            icons: None,
            meta: None,
        },
        Tool {
            name: Cow::Borrowed("get-policy"),
            description: Some(Cow::Borrowed(
                "Gets the policy information for a specific component",
            )),
            input_schema: Arc::new(
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
            output_schema: None,
            annotations: None,
            title: None,
            icons: None,
            meta: None,
        },
        Tool {
            name: Cow::Borrowed("grant-storage-permission"),
            description: Some(Cow::Borrowed(
                "Grants storage access permission to a component, allowing it to read from and/or write to specific storage locations."
            )),
            input_schema: Arc::new(
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
            output_schema: None,
            annotations: None,
            title: None,
            icons: None,
            meta: None,
        },
        Tool {
            name: Cow::Borrowed("grant-network-permission"),
            description: Some(Cow::Borrowed(
                "Grants network access permission to a component, allowing it to make network requests to specific hosts."
            )),
            input_schema: Arc::new(
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
            output_schema: None,
            annotations: None,
            title: None,
            icons: None,
            meta: None,
        },
        Tool {
            name: Cow::Borrowed("grant-environment-variable-permission"),
            description: Some(Cow::Borrowed(
                "Grants environment variable access permission to a component, allowing it to access specific environment variables."
            )),
            input_schema: Arc::new(
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
            output_schema: None,
            annotations: None,
            title: None,
            icons: None,
            meta: None,
        },
        Tool {
            name: Cow::Borrowed("revoke-storage-permission"),
            description: Some(Cow::Borrowed(
                "Revokes all storage access permissions from a component for the specified URI path, removing both read and write access to that location."
            )),
            input_schema: Arc::new(
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
            output_schema: None,
            annotations: None,
            title: None,
            icons: None,
            meta: None,
        },
        Tool {
            name: Cow::Borrowed("revoke-network-permission"),
            description: Some(Cow::Borrowed(
                "Revokes network access permission from a component, removing its ability to make network requests to specific hosts."
            )),
            input_schema: Arc::new(
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
            output_schema: None,
            annotations: None,
            title: None,
            icons: None,
            meta: None,
        },
        Tool {
            name: Cow::Borrowed("revoke-environment-variable-permission"),
            description: Some(Cow::Borrowed(
                "Revokes environment variable access permission from a component, removing its ability to access specific environment variables."
            )),
            input_schema: Arc::new(
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
            output_schema: None,
            annotations: None,
            title: None,
            icons: None,
            meta: None,
        },
        Tool {
            name: Cow::Borrowed("reset-permission"),
            description: Some(Cow::Borrowed(
                "Resets all permissions for a component, removing all granted permissions and returning it to the default state."
            )),
            input_schema: Arc::new(
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
            output_schema: None,
            annotations: None,
            title: None,
            icons: None,
            meta: None,
        },
        Tool {
            name: Cow::Borrowed("search-components"),
            description: Some(Cow::Borrowed(
                "Lists all known components that can be fetched and loaded. Optionally filter by a search query.",
            )),
            input_schema: Arc::new(
                serde_json::from_value(json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Optional search query to filter components by name, description, or URI"
                        }
                    },
                    "required": []
                }))
                .unwrap_or_default(),
            ),
            output_schema: None,
            annotations: None,
            title: None,
            icons: None,
            meta: None,
        },
    ]
}

/// Calculate a relevance score for a component based on query terms
/// Higher scores indicate better matches
fn calculate_relevance_score(component: &Value, query_terms: &[String]) -> u32 {
    let name = component["name"].as_str().unwrap_or("").to_lowercase();
    let description = component["description"]
        .as_str()
        .unwrap_or("")
        .to_lowercase();
    let uri = component["uri"].as_str().unwrap_or("").to_lowercase();

    let mut score = 0u32;

    for term in query_terms {
        // Exact name match gets highest score
        if name == term.as_str() {
            score += 100;
        } else if name.starts_with(term) {
            score += 50;
        } else if name.contains(term) {
            score += 20;
        }

        // Description matches get medium score
        if description.starts_with(term) {
            score += 15;
        } else if description.contains(term) {
            score += 10;
        }

        // URI matches get lower score
        if uri.contains(term) {
            score += 5;
        }
    }

    score
}

#[instrument(skip(_lifecycle_manager))]
pub(crate) async fn handle_search_component(
    req: &CallToolRequestParam,
    _lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;

    // Extract the optional query parameter
    let query = args.get("query").and_then(|v| v.as_str());

    // Parse the component list
    let components_value: Value = serde_json::from_str(COMPONENT_LIST)?;
    let all_components = components_value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Component registry is not an array"))?;

    // Filter and rank components based on query
    let filtered_components: Vec<Value> = if let Some(q) = query {
        // Split query into words for multi-term matching
        let query_terms: Vec<String> = q
            .split_whitespace()
            .map(|term| term.to_lowercase())
            .collect();

        if query_terms.is_empty() {
            all_components.to_vec()
        } else {
            // Calculate relevance scores and filter out non-matches
            let mut scored_components: Vec<(u32, &Value)> = all_components
                .iter()
                .map(|component| {
                    let score = calculate_relevance_score(component, &query_terms);
                    (score, component)
                })
                .filter(|(score, _)| *score > 0)
                .collect();

            // Sort by relevance score (descending)
            scored_components.sort_by(|a, b| b.0.cmp(&a.0));

            // Extract components in ranked order
            scored_components
                .into_iter()
                .map(|(_, component)| (*component).clone())
                .collect()
        }
    } else {
        all_components.to_vec()
    };

    let status_text = serde_json::to_string(&json!({
        "status": "Component list found",
        "components": filtered_components,
    }))?;

    let contents = vec![Content::text(status_text)];

    Ok(CallToolResult {
        content: contents,
        structured_content: None,
        is_error: None,
        meta: None,
    })
}

#[instrument(skip(lifecycle_manager))]
pub async fn handle_get_policy(
    req: &CallToolRequestParam,
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
        .map_err(|e| anyhow::anyhow!("Component not found: {} ({})", component_id, e))?;

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

    let contents = vec![Content::text(status_text)];

    Ok(CallToolResult {
        content: contents,
        structured_content: None,
        is_error: None,
        meta: None,
    })
}

/// Generic helper for handling grant permission requests
async fn handle_grant_permission_generic(
    req: &CallToolRequestParam,
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
        .map_err(|e| anyhow::anyhow!("Component not found: {} ({})", component_id, e))?;

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

            let contents = vec![Content::text(status_text)];

            Ok(CallToolResult {
                content: contents,
                structured_content: None,
                is_error: None,
                meta: None,
            })
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
    req: &CallToolRequestParam,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    handle_grant_permission_generic(req, lifecycle_manager, "storage", "storage").await
}

#[instrument(skip(lifecycle_manager))]
pub async fn handle_grant_network_permission(
    req: &CallToolRequestParam,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    handle_grant_permission_generic(req, lifecycle_manager, "network", "network").await
}

#[instrument(skip(lifecycle_manager))]
pub async fn handle_grant_environment_variable_permission(
    req: &CallToolRequestParam,
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
    req: &CallToolRequestParam,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    handle_grant_permission_generic(req, lifecycle_manager, "resource", "memory").await
}

/// Generic helper for handling revoke permission requests
async fn handle_revoke_permission_generic(
    req: &CallToolRequestParam,
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
        .map_err(|e| anyhow::anyhow!("Component not found: {} ({})", component_id, e))?;

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

            let contents = vec![Content::text(status_text)];

            Ok(CallToolResult {
                content: contents,
                structured_content: None,
                is_error: None,
                meta: None,
            })
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
    req: &CallToolRequestParam,
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
        .map_err(|e| anyhow::anyhow!("Component not found: {} ({})", component_id, e))?;

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

            let contents = vec![Content::text(status_text)];

            Ok(CallToolResult {
                content: contents,
                structured_content: None,
                is_error: None,
                meta: None,
            })
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
    req: &CallToolRequestParam,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    handle_revoke_permission_generic(req, lifecycle_manager, "network", "network").await
}

#[instrument(skip(lifecycle_manager))]
pub async fn handle_revoke_environment_variable_permission(
    req: &CallToolRequestParam,
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
    req: &CallToolRequestParam,
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
        .map_err(|e| anyhow::anyhow!("Component not found: {} ({})", component_id, e))?;

    let result = lifecycle_manager.reset_permission(component_id).await;

    match result {
        Ok(()) => {
            let status_text = serde_json::to_string(&json!({
                "status": "permissions reset successfully",
                "component_id": component_id
            }))?;

            let contents = vec![Content::text(status_text)];

            Ok(CallToolResult {
                content: contents,
                structured_content: None,
                is_error: None,
                meta: None,
            })
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

        let req = CallToolRequestParam {
            name: "grant-network-permission".into(),
            arguments: Some(args),
        };

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

        let req = CallToolRequestParam {
            name: "grant-storage-permission".into(),
            arguments: Some(args),
        };

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

        let req = CallToolRequestParam {
            name: "grant-network-permission".into(),
            arguments: Some(args),
        };

        let result = handle_grant_network_permission(&req, &lifecycle_manager).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing required argument: 'component_id'"));

        // Test with missing details for network permission
        let mut args = serde_json::Map::new();
        args.insert("component_id".to_string(), json!("test-component"));

        let req = CallToolRequestParam {
            name: "grant-network-permission".into(),
            arguments: Some(args),
        };

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

        let req = CallToolRequestParam {
            name: "grant-storage-permission".into(),
            arguments: Some(args),
        };

        let result = handle_grant_storage_permission(&req, &lifecycle_manager).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing required argument: 'component_id'"));

        // Test with missing details for storage permission
        let mut args = serde_json::Map::new();
        args.insert("component_id".to_string(), json!("test-component"));

        let req = CallToolRequestParam {
            name: "grant-storage-permission".into(),
            arguments: Some(args),
        };

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

        let req = CallToolRequestParam {
            name: "revoke-network-permission".into(),
            arguments: Some(args),
        };

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

        let req = CallToolRequestParam {
            name: "revoke-storage-permission".into(),
            arguments: Some(args),
        };

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

        let req = CallToolRequestParam {
            name: "revoke-environment-variable-permission".into(),
            arguments: Some(args),
        };

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

        let req = CallToolRequestParam {
            name: "reset-permission".into(),
            arguments: Some(args),
        };

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

        let req = CallToolRequestParam {
            name: "revoke-network-permission".into(),
            arguments: Some(args),
        };

        let result = handle_revoke_network_permission(&req, &lifecycle_manager).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing required argument: 'component_id'"));

        // Test with missing details for revoke network permission
        let mut args = serde_json::Map::new();
        args.insert("component_id".to_string(), json!("test-component"));

        let req = CallToolRequestParam {
            name: "revoke-network-permission".into(),
            arguments: Some(args),
        };

        let result = handle_revoke_network_permission(&req, &lifecycle_manager).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing required argument: 'details'"));

        // Test with missing component_id for reset permission
        let args = serde_json::Map::new();

        let req = CallToolRequestParam {
            name: "reset-permission".into(),
            arguments: Some(args),
        };

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

    #[tokio::test]
    async fn test_search_component_without_query() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let lifecycle_manager = wassette::LifecycleManager::new(&tempdir).await?;

        // Test without query - should return all components
        let args = serde_json::Map::new();
        let req = CallToolRequestParam {
            name: "search-components".into(),
            arguments: Some(args),
        };

        let result = handle_search_component(&req, &lifecycle_manager).await?;

        // Parse the result
        let content_json = serde_json::to_value(&result.content)?;
        let text = content_json[0]["text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No text in content"))?;

        let response: Value = serde_json::from_str(text)?;
        assert_eq!(response["status"], "Component list found");

        let components = response["components"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Components is not an array"))?;

        // Should return all 12 components from component-registry.json
        assert_eq!(components.len(), 12);

        Ok(())
    }

    #[tokio::test]
    async fn test_search_component_with_query() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let lifecycle_manager = wassette::LifecycleManager::new(&tempdir).await?;

        // Test with query - search for "weather"
        let mut args = serde_json::Map::new();
        args.insert("query".to_string(), json!("weather"));
        let req = CallToolRequestParam {
            name: "search-components".into(),
            arguments: Some(args),
        };

        let result = handle_search_component(&req, &lifecycle_manager).await?;

        // Parse the result
        let content_json = serde_json::to_value(&result.content)?;
        let text = content_json[0]["text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No text in content"))?;

        let response: Value = serde_json::from_str(text)?;
        assert_eq!(response["status"], "Component list found");

        let components = response["components"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Components is not an array"))?;

        // Should return 2 weather components
        assert_eq!(components.len(), 2);

        // Verify both have "weather" in their name or description
        for component in components {
            let name = component["name"].as_str().unwrap_or("").to_lowercase();
            let description = component["description"]
                .as_str()
                .unwrap_or("")
                .to_lowercase();
            let uri = component["uri"].as_str().unwrap_or("").to_lowercase();

            assert!(
                name.contains("weather")
                    || description.contains("weather")
                    || uri.contains("weather"),
                "Component should contain 'weather': {:?}",
                component
            );
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_search_component_case_insensitive() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let lifecycle_manager = wassette::LifecycleManager::new(&tempdir).await?;

        // Test case insensitivity - search with uppercase
        let mut args = serde_json::Map::new();
        args.insert("query".to_string(), json!("WEATHER"));
        let req = CallToolRequestParam {
            name: "search-components".into(),
            arguments: Some(args),
        };

        let result = handle_search_component(&req, &lifecycle_manager).await?;

        let content_json = serde_json::to_value(&result.content)?;
        let text = content_json[0]["text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No text in content"))?;

        let response: Value = serde_json::from_str(text)?;
        let components = response["components"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Components is not an array"))?;

        // Should still return 2 weather components
        assert_eq!(components.len(), 2);

        Ok(())
    }

    #[tokio::test]
    async fn test_search_component_no_results() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let lifecycle_manager = wassette::LifecycleManager::new(&tempdir).await?;

        // Test with query that matches nothing
        let mut args = serde_json::Map::new();
        args.insert("query".to_string(), json!("nonexistent"));
        let req = CallToolRequestParam {
            name: "search-components".into(),
            arguments: Some(args),
        };

        let result = handle_search_component(&req, &lifecycle_manager).await?;

        let content_json = serde_json::to_value(&result.content)?;
        let text = content_json[0]["text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No text in content"))?;

        let response: Value = serde_json::from_str(text)?;
        let components = response["components"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Components is not an array"))?;

        // Should return no components
        assert_eq!(components.len(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn test_search_component_multi_term() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let lifecycle_manager = wassette::LifecycleManager::new(&tempdir).await?;

        // Test multi-term search
        let mut args = serde_json::Map::new();
        args.insert("query".to_string(), json!("weather rust"));
        let req = CallToolRequestParam {
            name: "search-components".into(),
            arguments: Some(args),
        };

        let result = handle_search_component(&req, &lifecycle_manager).await?;

        let content_json = serde_json::to_value(&result.content)?;
        let text = content_json[0]["text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No text in content"))?;

        let response: Value = serde_json::from_str(text)?;
        let components = response["components"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Components is not an array"))?;

        // Should match components with either "weather" or "rust"
        // Weather Server, Open-Meteo Weather, arXiv Research (Rust), Fetch (Rust),
        // Filesystem (Rust), Brave Search (Rust), Context7 (Rust)
        assert_eq!(components.len(), 7);

        Ok(())
    }

    #[tokio::test]
    async fn test_search_component_relevance_ranking() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let lifecycle_manager = wassette::LifecycleManager::new(&tempdir).await?;

        // Test relevance ranking - search for "server"
        // "Weather Server" and "Time Server" have "server" in the name
        // Other components might have it in description
        let mut args = serde_json::Map::new();
        args.insert("query".to_string(), json!("server"));
        let req = CallToolRequestParam {
            name: "search-components".into(),
            arguments: Some(args),
        };

        let result = handle_search_component(&req, &lifecycle_manager).await?;

        let content_json = serde_json::to_value(&result.content)?;
        let text = content_json[0]["text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No text in content"))?;

        let response: Value = serde_json::from_str(text)?;
        let components = response["components"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Components is not an array"))?;

        // Should have at least 2 components (Weather Server, Time Server)
        assert!(components.len() >= 2);

        // First two results should have "server" in the name (highest relevance)
        let first_name = components[0]["name"].as_str().unwrap_or("").to_lowercase();
        let second_name = components[1]["name"].as_str().unwrap_or("").to_lowercase();

        assert!(
            first_name.contains("server"),
            "First result should have 'server' in name: {}",
            first_name
        );
        assert!(
            second_name.contains("server"),
            "Second result should have 'server' in name: {}",
            second_name
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_search_component_integration_end_to_end() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let lifecycle_manager = wassette::LifecycleManager::new(&tempdir).await?;

        // Test 1: No query returns all components
        let req1 = CallToolRequestParam {
            name: "search-components".into(),
            arguments: Some(serde_json::Map::new()),
        };
        let result1 = handle_search_component(&req1, &lifecycle_manager).await?;
        let content1_json = serde_json::to_value(&result1.content)?;
        let text1 = content1_json[0]["text"].as_str().unwrap();
        let response1: Value = serde_json::from_str(text1)?;
        assert_eq!(response1["components"].as_array().unwrap().len(), 12);

        // Test 2: Query with single term
        let mut args2 = serde_json::Map::new();
        args2.insert("query".to_string(), json!("python"));
        let req2 = CallToolRequestParam {
            name: "search-components".into(),
            arguments: Some(args2),
        };
        let result2 = handle_search_component(&req2, &lifecycle_manager).await?;
        let content2_json = serde_json::to_value(&result2.content)?;
        let text2 = content2_json[0]["text"].as_str().unwrap();
        let response2: Value = serde_json::from_str(text2)?;
        let components2 = response2["components"].as_array().unwrap();
        assert_eq!(components2.len(), 1);
        assert!(components2[0]["name"].as_str().unwrap().contains("Python"));

        // Test 3: Query with no matches
        let mut args3 = serde_json::Map::new();
        args3.insert("query".to_string(), json!("xyz123notfound"));
        let req3 = CallToolRequestParam {
            name: "search-components".into(),
            arguments: Some(args3),
        };
        let result3 = handle_search_component(&req3, &lifecycle_manager).await?;
        let content3_json = serde_json::to_value(&result3.content)?;
        let text3 = content3_json[0]["text"].as_str().unwrap();
        let response3: Value = serde_json::from_str(text3)?;
        assert_eq!(response3["components"].as_array().unwrap().len(), 0);

        // Test 4: Verify ranking - exact name match should come first
        let mut args4 = serde_json::Map::new();
        args4.insert("query".to_string(), json!("fetch"));
        let req4 = CallToolRequestParam {
            name: "search-components".into(),
            arguments: Some(args4),
        };
        let result4 = handle_search_component(&req4, &lifecycle_manager).await?;
        let content4_json = serde_json::to_value(&result4.content)?;
        let text4 = content4_json[0]["text"].as_str().unwrap();
        let response4: Value = serde_json::from_str(text4)?;
        let components4 = response4["components"].as_array().unwrap();
        // "Fetch" component should be first (exact name match)
        assert_eq!(components4[0]["name"].as_str().unwrap(), "Fetch");

        Ok(())
    }
}
