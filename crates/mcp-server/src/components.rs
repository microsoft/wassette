use std::borrow::Cow;
use std::sync::Arc;

use anyhow::Result;
use futures::stream::{self, StreamExt};
use rmcp::model::{CallToolRequestParam, CallToolResult, Content, Tool};
use rmcp::{Peer, RoleServer};
use serde_json::{json, Value};
use tracing::{debug, error, info, instrument};
use weld::LifecycleManager;

#[instrument(skip(lifecycle_manager))]
pub(crate) async fn get_component_tools(lifecycle_manager: &LifecycleManager) -> Result<Vec<Tool>> {
    debug!("Listing components");
    let component_ids = lifecycle_manager.list_components().await;

    info!("Found {} components", component_ids.len());
    let mut tools = Vec::new();

    for id in component_ids {
        debug!("Getting component details for {}", id);
        if let Some(schema) = lifecycle_manager.get_component_schema(&id).await {
            if let Some(arr) = schema.get("tools").and_then(|v| v.as_array()) {
                let tool_count = arr.len();
                debug!("Found {} tools in component {}", tool_count, id);
                for tool_json in arr {
                    if let Some(tool) = parse_tool_schema(tool_json) {
                        tools.push(tool);
                    }
                }
            }
        }
    }
    info!("Total tools collected: {}", tools.len());
    Ok(tools)
}

#[instrument(skip(lifecycle_manager))]
pub(crate) async fn handle_load_component(
    req: &CallToolRequestParam,
    lifecycle_manager: &LifecycleManager,
    server_peer: Option<Peer<RoleServer>>,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'path'"))?;

    info!("Loading component from path {}", path);

    let result = lifecycle_manager.load_component(path).await;

    match result {
        Ok((id, _load_result)) => {
            let status_text = serde_json::to_string(&json!({
                "status": "component loaded",
                "id": id
            }))?;

            let contents = vec![Content::text(status_text)];

            if let Some(peer) = server_peer {
                if let Err(e) = peer.notify_tool_list_changed().await {
                    error!("Failed to send tool list change notification: {}", e);
                } else {
                    info!(
                        "Sent tool list changed notification after loading component {}",
                        id
                    );
                }
            }

            Ok(CallToolResult {
                content: contents,
                is_error: None,
            })
        }
        Err(e) => {
            error!("Failed to load component: {}", e);
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
    server_peer: Option<Peer<RoleServer>>,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;

    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'id' in arguments"))?;

    info!("Unloading component {}", id);
    lifecycle_manager.unload_component(id).await;

    let status_text = serde_json::to_string(&json!({
        "status": "component unloaded",
        "id": id
    }))?;

    let contents = vec![Content::text(status_text)];

    if let Some(peer) = server_peer {
        if let Err(e) = peer.notify_tool_list_changed().await {
            error!("Failed to send tool list change notification: {}", e);
        } else {
            info!(
                "Sent tool list changed notification after unloading component {}",
                id
            );
        }
    }

    Ok(CallToolResult {
        content: contents,
        is_error: None,
    })
}

#[instrument(skip(lifecycle_manager))]
pub(crate) async fn handle_component_call(
    req: &CallToolRequestParam,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;

    let method_name = req.name.to_string();
    info!("Calling function {}", method_name);

    let component_id = lifecycle_manager
        .get_component_id_for_tool(&method_name)
        .await
        .map_err(|e| {
            anyhow::anyhow!("Failed to find component for tool '{}': {}", method_name, e)
        })?;

    let component = lifecycle_manager
        .get_component(&component_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("Component with ID {} not found", component_id))?;

    let result = lifecycle_manager
        .execute_component_call(&component, &method_name, &serde_json::to_string(&args)?)
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
            error!("Component call failed: {}", e);
            Err(anyhow::anyhow!(e.to_string()))
        }
    }
}

#[instrument(skip(lifecycle_manager))]
pub(crate) async fn handle_list_components(
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    info!("Listing loaded components");

    let component_ids = lifecycle_manager.list_components().await;

    let components_info = stream::iter(component_ids)
        .then(|id| async move {
            debug!("Getting component details for {}", id);
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

fn extract_args_from_request(req: &CallToolRequestParam) -> Result<serde_json::Map<String, Value>> {
    let params_value = serde_json::to_value(&req.arguments)?;

    match params_value {
        Value::Object(map) => Ok(map),
        _ => Err(anyhow::anyhow!(
            "Parameters are not in expected object format"
        )),
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

    debug!("Parsed tool schema for {}", name);

    Some(Tool {
        name: Cow::Owned(name.to_string()),
        description: Cow::Owned(description.to_string()),
        input_schema: Arc::new(serde_json::from_value(input_schema).unwrap_or_default()),
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
        assert_eq!(tool.description, "Test tool description");

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
