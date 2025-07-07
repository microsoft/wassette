use std::borrow::Cow;
use std::sync::Arc;

use anyhow::Result;
use rmcp::model::{CallToolRequestParam, CallToolResult, Content, Tool};
use rmcp::{Peer, RoleServer};
use serde_json::{json, Value};
use tracing::{debug, error, info, instrument};
use weld::LifecycleManager;

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
        "attach-policy" => handle_attach_policy(&req, lifecycle_manager).await,
        "detach-policy" => handle_detach_policy(&req, lifecycle_manager).await,
        "get-policy" => handle_get_policy(&req, lifecycle_manager).await,
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
            description: Cow::Borrowed(
                "Dynamically loads a new tool or component from either the filesystem or OCI registries.",
            ),
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
        },
        Tool {
            name: Cow::Borrowed("unload-component"),
            description: Cow::Borrowed(
                "Unloads a tool or component.",
            ),
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
        },
        Tool {
            name: Cow::Borrowed("list-components"),
            description: Cow::Borrowed(
                "Lists all currently loaded components or tools.",
            ),
            input_schema: Arc::new(
                serde_json::from_value(json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }))
                .unwrap_or_default(),
            ),
        },
        Tool {
            name: Cow::Borrowed("attach-policy"),
            description: Cow::Borrowed(
                "Attaches a capability policy to a specific component / plugin, or updates an existing policy",
            ),
            input_schema: Arc::new(
                serde_json::from_value(json!({
                    "type": "object",
                    "properties": {
                        "component_id": {
                            "type": "string",
                            "description": "ID of the component to attach policy to"
                        },
                        "policy_uri": {
                            "type": "string", 
                            "description": "URI of the policy file (file://, oci://, or https://)"
                        }
                    },
                    "required": ["component_id", "policy_uri"]
                }))
                .unwrap_or_default(),
            ),
        },
        Tool {
            name: Cow::Borrowed("detach-policy"),
            description: Cow::Borrowed(
                "Removes the capability policy from a component (reverts to the default policy)",
            ),
            input_schema: Arc::new(
                serde_json::from_value(json!({
                    "type": "object",
                    "properties": {
                        "component_id": {
                            "type": "string",
                            "description": "ID of the component to detach policy from"
                        }
                    },
                    "required": ["component_id"]
                }))
                .unwrap_or_default(),
            ),
        },
        Tool {
            name: Cow::Borrowed("get-policy"),
            description: Cow::Borrowed(
                "Gets the policy information for a specific component",
            ),
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
        },
    ]
}

#[instrument(skip(lifecycle_manager))]
async fn handle_attach_policy(
    req: &CallToolRequestParam,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;

    let component_id = args
        .get("component_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'component_id'"))?;

    let policy_uri = args
        .get("policy_uri")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'policy_uri'"))?;

    info!(
        "Attaching policy {} to component {}",
        policy_uri, component_id
    );

    let result = lifecycle_manager
        .attach_policy(component_id, policy_uri)
        .await;

    match result {
        Ok(()) => {
            let status_text = serde_json::to_string(&json!({
                "status": "policy attached",
                "component_id": component_id,
                "policy_uri": policy_uri
            }))?;

            let contents = vec![Content::text(status_text)];

            Ok(CallToolResult {
                content: contents,
                is_error: None,
            })
        }
        Err(e) => {
            error!("Failed to attach policy: {}", e);
            Err(anyhow::anyhow!(
                "Failed to attach policy {} to component {}: {}",
                policy_uri,
                component_id,
                e
            ))
        }
    }
}

#[instrument(skip(lifecycle_manager))]
async fn handle_detach_policy(
    req: &CallToolRequestParam,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;

    let component_id = args
        .get("component_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'component_id'"))?;

    info!("Detaching policy from component {}", component_id);

    let result = lifecycle_manager.detach_policy(component_id).await;

    match result {
        Ok(()) => {
            let status_text = serde_json::to_string(&json!({
                "status": "policy detached",
                "component_id": component_id
            }))?;

            let contents = vec![Content::text(status_text)];

            Ok(CallToolResult {
                content: contents,
                is_error: None,
            })
        }
        Err(e) => {
            error!("Failed to detach policy: {}", e);
            Err(anyhow::anyhow!(
                "Failed to detach policy from component {}: {}",
                component_id,
                e
            ))
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_builtin_tools() {
        let tools = get_builtin_tools();
        assert_eq!(tools.len(), 6);
        assert!(tools.iter().any(|t| t.name == "load-component"));
        assert!(tools.iter().any(|t| t.name == "unload-component"));
        assert!(tools.iter().any(|t| t.name == "list-components"));
        assert!(tools.iter().any(|t| t.name == "attach-policy"));
        assert!(tools.iter().any(|t| t.name == "detach-policy"));
        assert!(tools.iter().any(|t| t.name == "get-policy"));
    }
}
