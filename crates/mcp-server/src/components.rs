// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::borrow::Cow;
use std::sync::Arc;

use anyhow::Result;
use futures::stream::{self, StreamExt};
use rmcp::model::{CallToolRequestParam, CallToolResult, Content, Tool};
use rmcp::{Peer, RoleServer};
use serde_json::{json, Value};
use tracing::{debug, error, info, instrument};
use wassette::LifecycleManager;

#[instrument(skip(lifecycle_manager))]
pub(crate) async fn get_component_tools(lifecycle_manager: &LifecycleManager) -> Result<Vec<Tool>> {
    debug!("Listing components");
    let component_ids = lifecycle_manager.list_components().await;

    info!(count = component_ids.len(), "Found components");
    let mut tools = Vec::new();

    for id in component_ids {
        debug!(component_id = %id, "Getting component details");
        if let Some(schema) = lifecycle_manager.get_component_schema(&id).await {
            if let Some(arr) = schema.get("tools").and_then(|v| v.as_array()) {
                let tool_count = arr.len();
                debug!(component_id = %id, tool_count, "Found tools in component");
                for tool_json in arr {
                    if let Some(tool) = parse_tool_schema(tool_json) {
                        tools.push(tool);
                    }
                }
            }
        }
    }
    info!(total_tools = tools.len(), "Total tools collected");
    Ok(tools)
}

#[instrument(skip(lifecycle_manager))]
pub(crate) async fn handle_load_component(
    req: &CallToolRequestParam,
    lifecycle_manager: &LifecycleManager,
    server_peer: Peer<RoleServer>,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'path'"))?;

    info!(path, "Loading component");

    let result = lifecycle_manager.load_component(path).await;

    match result {
        Ok((id, _load_result)) => {
            let status_text = serde_json::to_string(&json!({
                "status": "component loaded",
                "id": id
            }))?;

            let contents = vec![Content::text(status_text)];

            info!(
                component_id = %id,
                "Notifying server peer about tool list change after loading component"
            );
            // Notify the server peer about the tool list change
            if let Err(e) = server_peer.notify_tool_list_changed().await {
                error!(error = %e, "Failed to send tool list change notification");
            } else {
                info!(
                    component_id = %id,
                    "Sent tool list changed notification after loading component"
                );
            }

            Ok(CallToolResult {
                content: contents,
                is_error: None,
            })
        }
        Err(e) => {
            error!(error = %e, path, "Failed to load component");
            Err(anyhow::anyhow!(
                "Failed to load component: {}. Error: {}",
                path,
                e
            ))
        }
    }
}

#[instrument(skip(lifecycle_manager))]
pub(crate) async fn handle_unload_component(
    req: &CallToolRequestParam,
    lifecycle_manager: &LifecycleManager,
    server_peer: Peer<RoleServer>,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;

    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'id' in arguments"))?;

    info!(component_id = %id, "Unloading component");

    match lifecycle_manager.unload_component(id).await {
        Ok(()) => {
            let status_text = serde_json::to_string(&json!({
                "status": "component unloaded successfully",
                "id": id
            }))?;

            let contents = vec![Content::text(status_text)];

            if let Err(e) = server_peer.notify_tool_list_changed().await {
                error!(error = %e, "Failed to send tool list change notification");
            } else {
                info!(
                    component_id = %id,
                    "Sent tool list changed notification after unloading component"
                );
            }

            Ok(CallToolResult {
                content: contents,
                is_error: None,
            })
        }
        Err(e) => {
            error!(error = %e, "Failed to unload component");
            let error_text = serde_json::to_string(&json!({
                "status": "error",
                "message": format!("Failed to unload component: {}", e),
                "id": id
            }))?;

            let contents = vec![Content::text(error_text)];

            Ok(CallToolResult {
                content: contents,
                is_error: Some(true),
            })
        }
    }
}

#[instrument(skip(lifecycle_manager))]
pub(crate) async fn handle_component_call(
    req: &CallToolRequestParam,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;

    let method_name = req.name.to_string();
    info!(function_name = %method_name, "Calling function");

    let component_id = lifecycle_manager
        .get_component_id_for_tool(&method_name)
        .await
        .map_err(|e| {
            anyhow::anyhow!("Failed to find component for tool '{}': {}", method_name, e)
        })?;

    let result = lifecycle_manager
        .execute_component_call(&component_id, &method_name, &serde_json::to_string(&args)?)
        .await;

    match result {
        Ok(result_str) => {
            debug!("Component call successful");
            let contents = vec![Content::text(result_str)];

            Ok(CallToolResult {
                content: contents,
                is_error: None,
            })
        }
        Err(e) => {
            error!(error = %e, "Component call failed");
            Err(anyhow::anyhow!(e.to_string()))
        }
    }
}

#[instrument(skip(lifecycle_manager))]
pub async fn handle_list_components(
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    info!("Listing loaded components");

    let component_ids = lifecycle_manager.list_components().await;

    let components_info = stream::iter(component_ids)
        .map(|id| async move {
            debug!(component_id = %id, "Getting component details");
            if let Some(schema) = lifecycle_manager.get_component_schema(&id).await {
                let tools_count = schema
                    .get("tools")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.len())
                    .unwrap_or(0);

                json!({
                    "id": id,
                    "tools_count": tools_count,
                    "schema": schema
                })
            } else {
                json!({
                    "id": id,
                    "tools_count": 0,
                    "schema": null
                })
            }
        })
        .buffer_unordered(50)
        .collect::<Vec<_>>()
        .await;

    let result_text = serde_json::to_string(&json!({
        "components": components_info,
        "total": components_info.len()
    }))?;

    let contents = vec![Content::text(result_text)];

    Ok(CallToolResult {
        content: contents,
        is_error: None,
    })
}

pub(crate) fn extract_args_from_request(
    req: &CallToolRequestParam,
) -> Result<serde_json::Map<String, Value>> {
    let params_value = serde_json::to_value(&req.arguments)?;

    match params_value {
        Value::Object(map) => Ok(map),
        _ => Err(anyhow::anyhow!(
            "Parameters are not in expected object format"
        )),
    }
}

/// CLI-specific version of handle_load_component that doesn't require server peer notifications
#[instrument(skip(lifecycle_manager))]
pub async fn handle_load_component_cli(
    req: &CallToolRequestParam,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'path'"))?;

    info!(path, "Loading component (CLI mode)");

    let result = lifecycle_manager.load_component(path).await;

    match result {
        Ok((id, _load_result)) => {
            let status_text = serde_json::to_string(&json!({
                "status": "component loaded",
                "id": id
            }))?;

            let contents = vec![Content::text(status_text)];

            info!(component_id = %id, "Component loaded successfully in CLI mode");

            Ok(CallToolResult {
                content: contents,
                is_error: None,
            })
        }
        Err(e) => {
            error!(error = %e, path, "Failed to load component");
            Err(anyhow::anyhow!(
                "Failed to load component: {}. Error: {}",
                path,
                e
            ))
        }
    }
}

/// CLI-specific version of handle_unload_component that doesn't require server peer notifications
#[instrument(skip(lifecycle_manager))]
pub async fn handle_unload_component_cli(
    req: &CallToolRequestParam,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;

    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'id' in arguments"))?;

    info!(component_id = %id, "Unloading component (CLI mode)");

    match lifecycle_manager.unload_component(id).await {
        Ok(()) => {
            let status_text = serde_json::to_string(&json!({
                "status": "component unloaded successfully",
                "id": id
            }))?;

            let contents = vec![Content::text(status_text)];

            info!(component_id = %id, "Component unloaded successfully in CLI mode");

            Ok(CallToolResult {
                content: contents,
                is_error: None,
            })
        }
        Err(e) => {
            error!(error = %e, "Failed to unload component");
            let error_text = serde_json::to_string(&json!({
                "status": "error",
                "message": format!("Failed to unload component: {}", e),
                "id": id
            }))?;

            let contents = vec![Content::text(error_text)];

            Ok(CallToolResult {
                content: contents,
                is_error: Some(true),
            })
        }
    }
}

#[instrument]
fn parse_tool_schema(tool_json: &Value) -> Option<Tool> {
    let name = tool_json
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("<unnamed>");

    let description = tool_json
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("No description available");

    let input_schema = tool_json.get("inputSchema").cloned().unwrap_or(json!({}));

    debug!(tool_name = %name, "Parsed tool schema");

    Some(Tool {
        name: Cow::Owned(name.to_string()),
        description: Some(Cow::Owned(description.to_string())),
        input_schema: Arc::new(serde_json::from_value(input_schema).unwrap_or_default()),
        annotations: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tool_schema() {
        let tool_json = json!({
            "name": "test-tool",
            "description": "Test tool description",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "test": {"type": "string"}
                }
            }
        });

        let tool = parse_tool_schema(&tool_json).unwrap();

        assert_eq!(tool.name, "test-tool");
        assert_eq!(tool.description, Some("Test tool description".into()));

        let schema_json = serde_json::to_value(&*tool.input_schema).unwrap();
        let expected = json!({
             "type": "object",
            "properties": {
                "test": {"type": "string"}
            }
        });
        assert_eq!(schema_json, expected);
    }
}
