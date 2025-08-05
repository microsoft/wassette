// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::borrow::Cow;
use std::sync::Arc;

use anyhow::Result;
use rmcp::model::{CallToolRequestParam, CallToolResult, Content, Tool};
use rmcp::{Peer, RoleServer};
use serde_json::{json, Value};
use tracing::{debug, error, info, instrument};
use wassette::LifecycleManager;

use crate::components::{
    extract_args_from_request, get_component_tools, handle_component_call, handle_list_components,
    handle_load_component, handle_unload_component,
};

/// Handles a request to list available tools.
#[instrument(skip(lifecycle_manager))]
pub async fn handle_tools_list(lifecycle_manager: &LifecycleManager) -> Result<Value> {
    debug!("Handling tools list request");

    let mut tools = get_component_tools(lifecycle_manager).await?;
    tools.extend(get_builtin_tools());
    debug!(num_tools = %tools.len(), "Retrieved tools");

    let response = rmcp::model::ListToolsResult {
        tools,
        next_cursor: None,
    };

    Ok(serde_json::to_value(response)?)
}

/// Handles a tool call request.
#[instrument(skip_all, fields(method_name = %req.name))]
pub async fn handle_tools_call(
    req: CallToolRequestParam,
    lifecycle_manager: &LifecycleManager,
    server_peer: Option<Peer<RoleServer>>,
) -> Result<Value> {
    info!("Handling tool call");

    let result = match req.name.as_ref() {
        "load-component" => handle_load_component(&req, lifecycle_manager, server_peer).await,
        "unload-component" => handle_unload_component(&req, lifecycle_manager, server_peer).await,
        "list-components" => handle_list_components(lifecycle_manager).await,
        "get-policy" => handle_get_policy(&req, lifecycle_manager).await,
        "grant-storage-permission" => {
            handle_grant_storage_permission(&req, lifecycle_manager).await
        }
        "grant-network-permission" => {
            handle_grant_network_permission(&req, lifecycle_manager).await
        }
        "grant-environment-variable-permission" => {
            handle_grant_environment_variable_permission(&req, lifecycle_manager).await
        }
        _ => handle_component_call(&req, lifecycle_manager).await,
    };

    if let Err(ref e) = result {
        error!(error = ?e, "Tool call failed");
    }

    match result {
        Ok(result) => Ok(serde_json::to_value(result)?),
        Err(e) => {
            let error_text = format!("Error: {e}");
            let contents = vec![Content::text(error_text)];

            let error_result = CallToolResult {
                content: contents,
                is_error: Some(true),
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
            annotations: None,
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
            annotations: None,
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
            annotations: None,
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
            annotations: None,
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
            annotations: None,
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
            annotations: None,
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
            annotations: None,
        },
    ]
}

#[instrument(skip(lifecycle_manager))]
async fn handle_get_policy(
    req: &CallToolRequestParam,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;

    let component_id = args
        .get("component_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'component_id'"))?;

    info!("Getting policy for component {}", component_id);

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
        is_error: None,
    })
}

#[instrument(skip(lifecycle_manager))]
async fn handle_grant_storage_permission(
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

    info!("Granting storage permission to component {}", component_id);

    let result = lifecycle_manager
        .grant_permission(component_id, "storage", details)
        .await;

    match result {
        Ok(()) => {
            let status_text = serde_json::to_string(&json!({
                "status": "permission granted",
                "component_id": component_id,
                "permission_type": "storage",
                "details": details
            }))?;

            let contents = vec![Content::text(status_text)];

            Ok(CallToolResult {
                content: contents,
                is_error: None,
            })
        }
        Err(e) => {
            error!("Failed to grant storage permission: {}", e);
            Err(anyhow::anyhow!(
                "Failed to grant storage permission to component {}: {}",
                component_id,
                e
            ))
        }
    }
}

#[instrument(skip(lifecycle_manager))]
async fn handle_grant_network_permission(
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

    info!("Granting network permission to component {}", component_id);

    let result = lifecycle_manager
        .grant_permission(component_id, "network", details)
        .await;

    match result {
        Ok(()) => {
            let status_text = serde_json::to_string(&json!({
                "status": "permission granted",
                "component_id": component_id,
                "permission_type": "network",
                "details": details
            }))?;

            let contents = vec![Content::text(status_text)];

            Ok(CallToolResult {
                content: contents,
                is_error: None,
            })
        }
        Err(e) => {
            error!("Failed to grant network permission: {}", e);
            Err(anyhow::anyhow!(
                "Failed to grant network permission to component {}: {}",
                component_id,
                e
            ))
        }
    }
}

#[instrument(skip(lifecycle_manager))]
async fn handle_grant_environment_variable_permission(
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

    info!(
        "Granting environment variable permission to component {}",
        component_id
    );

    let result = lifecycle_manager
        .grant_permission(component_id, "environment", details)
        .await;

    match result {
        Ok(()) => {
            let status_text = serde_json::to_string(&json!({
                "status": "permission granted",
                "component_id": component_id,
                "permission_type": "environment",
                "details": details
            }))?;

            let contents = vec![Content::text(status_text)];

            Ok(CallToolResult {
                content: contents,
                is_error: None,
            })
        }
        Err(e) => {
            error!("Failed to grant environment variable permission: {}", e);
            Err(anyhow::anyhow!(
                "Failed to grant environment variable permission to component {}: {}",
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
        assert_eq!(tools.len(), 7);
        assert!(tools.iter().any(|t| t.name == "load-component"));
        assert!(tools.iter().any(|t| t.name == "unload-component"));
        assert!(tools.iter().any(|t| t.name == "list-components"));
        assert!(tools.iter().any(|t| t.name == "get-policy"));
        assert!(tools.iter().any(|t| t.name == "grant-storage-permission"));
        assert!(tools.iter().any(|t| t.name == "grant-network-permission"));
        assert!(tools
            .iter()
            .any(|t| t.name == "grant-environment-variable-permission"));
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
}
