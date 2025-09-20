// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::borrow::Cow;
use std::sync::Arc;

use anyhow::Result;
use futures::stream::{self, StreamExt};
use rmcp::model::{CallToolRequestParam, CallToolResult, Content, Tool};
use rmcp::{Peer, RoleServer};
use serde_json::{json, Map, Value};
use tracing::{debug, error, info, instrument};
use wassette::{ComponentLoadOutcome, LifecycleManager, LoadResult};

#[instrument(skip(lifecycle_manager))]
pub(crate) async fn get_component_tools(lifecycle_manager: &LifecycleManager) -> Result<Vec<Tool>> {
    debug!("Listing components");
    // Use known components (loaded or present on disk) for fast listing
    let component_ids = lifecycle_manager.list_components_known().await;

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

    match lifecycle_manager.load_component(path).await {
        Ok(outcome) => {
            handle_tool_list_notification(Some(server_peer), &outcome.component_id, "load").await;
            create_load_component_success_result(&outcome)
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
            handle_tool_list_notification(Some(server_peer), id, "unload").await;
            create_component_success_result("unload", id)
        }
        Err(e) => {
            error!(error = %e, "Failed to unload component");
            Ok(create_component_error_result("unload", id, &e))
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

    let tool_schema = lifecycle_manager
        .get_tool_schema_for_component(&component_id, &method_name)
        .await;

    let result = lifecycle_manager
        .execute_component_call(&component_id, &method_name, &serde_json::to_string(&args)?)
        .await;

    match result {
        Ok(result_str) => {
            debug!("Component call successful");

            if let Some(raw_schema) = tool_schema
                .as_ref()
                .and_then(|schema| schema.get("outputSchema"))
            {
                if let Some(normalized_schema) = normalize_output_schema(raw_schema) {
                    let structured_value = parse_structured_result(&result_str);
                    let structured_value = align_structured_result_with_schema(
                        Some(&normalized_schema),
                        structured_value,
                    );
                    let contents = vec![Content::text(result_str)];

                    Ok(CallToolResult {
                        content: Some(contents),
                        structured_content: Some(structured_value),
                        is_error: Some(false),
                    })
                } else {
                    let contents = vec![Content::text(result_str)];

                    Ok(CallToolResult {
                        content: Some(contents),
                        structured_content: None,
                        is_error: Some(false),
                    })
                }
            } else {
                let contents = vec![Content::text(result_str)];

                Ok(CallToolResult {
                    content: Some(contents),
                    structured_content: None,
                    is_error: Some(false),
                })
            }
        }
        Err(e) => {
            error!(error = %e, "Component call failed");
            Err(anyhow::anyhow!(e.to_string()))
        }
    }
}

fn parse_structured_result(result: &str) -> Value {
    serde_json::from_str(result).unwrap_or_else(|_| Value::String(result.to_string()))
}

fn align_structured_result_with_schema(
    output_schema: Option<&Value>,
    structured_value: Value,
) -> Value {
    if let Some(schema) = output_schema {
        if schema.get("type").and_then(|v| v.as_str()) == Some("object") {
            if let Some(properties) = schema.get("properties").and_then(|v| v.as_object()) {
                if properties.contains_key("result") {
                    return match structured_value {
                        Value::Object(obj) => {
                            if obj.contains_key("result") {
                                Value::Object(obj)
                            } else {
                                let mut wrapper = Map::new();
                                wrapper.insert("result".to_string(), Value::Object(obj));
                                Value::Object(wrapper)
                            }
                        }
                        other => {
                            let mut wrapper = Map::new();
                            wrapper.insert("result".to_string(), other);
                            Value::Object(wrapper)
                        }
                    };
                }
            }
        }
    }

    structured_value
}

fn normalize_output_schema(schema: &Value) -> Option<Value> {
    if schema.is_null() {
        return None;
    }

    match schema {
        Value::Object(map) => {
            if map.get("type").and_then(|v| v.as_str()) == Some("object") {
                Some(Value::Object(map.clone()))
            } else {
                Some(wrap_schema_in_result(schema.clone()))
            }
        }
        _ => Some(wrap_schema_in_result(schema.clone())),
    }
}

fn wrap_schema_in_result(schema: Value) -> Value {
    let mut props = Map::new();
    props.insert("result".to_string(), schema);

    let mut wrapped = Map::new();
    wrapped.insert("type".to_string(), Value::String("object".to_string()));
    wrapped.insert("properties".to_string(), Value::Object(props));
    wrapped.insert(
        "required".to_string(),
        Value::Array(vec![Value::String("result".to_string())]),
    );
    Value::Object(wrapped)
}

#[instrument(skip(lifecycle_manager))]
pub async fn handle_list_components(
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    info!("Listing loaded components");

    // Use known components (loaded or present on disk) for fast listing
    let component_ids = lifecycle_manager.list_components_known().await;

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
        content: Some(contents),
        structured_content: None,
        is_error: None,
    })
}

pub(crate) fn extract_args_from_request(
    req: &CallToolRequestParam,
) -> Result<serde_json::Map<String, Value>> {
    match &req.arguments {
        Some(args) => {
            let params_value = serde_json::to_value(args)?;
            match params_value {
                Value::Object(map) => Ok(map),
                _ => Err(anyhow::anyhow!(
                    "Parameters are not in expected object format"
                )),
            }
        }
        None => Ok(serde_json::Map::new()),
    }
}

/// Create successful result for component operations
fn create_component_success_result(
    operation_name: &str,
    component_id: &str,
) -> Result<CallToolResult> {
    let status_text = serde_json::to_string(&json!({
        "status": format!("component {}ed successfully", operation_name),
        "id": component_id
    }))?;

    let contents = vec![Content::text(status_text)];

    Ok(CallToolResult {
        content: Some(contents),
        structured_content: None,
        is_error: None,
    })
}

fn create_load_component_success_result(outcome: &ComponentLoadOutcome) -> Result<CallToolResult> {
    let status = match outcome.status {
        LoadResult::New => "component loaded successfully",
        LoadResult::Replaced => "component reloaded successfully",
    };

    let status_text = serde_json::to_string(&json!({
        "status": status,
        "id": &outcome.component_id,
        "tools": &outcome.tool_names,
    }))?;

    let contents = vec![Content::text(status_text)];

    Ok(CallToolResult {
        content: Some(contents),
        structured_content: None,
        is_error: None,
    })
}

/// Create error result for component operations
fn create_component_error_result(
    operation_name: &str,
    operation_arg: &str,
    error: &anyhow::Error,
) -> CallToolResult {
    let error_text = serde_json::to_string(&json!({
        "status": "error",
        "message": format!("Failed to {} component: {}", operation_name, error),
        "id": operation_arg
    }))
    .unwrap_or_else(|_| {
        format!("{{\"status\":\"error\",\"message\":\"Failed to {operation_name} component\"}}",)
    });

    let contents = vec![Content::text(error_text)];

    CallToolResult {
        content: Some(contents),
        structured_content: None,
        is_error: Some(true),
    }
}

/// Handle tool list change notification
async fn handle_tool_list_notification(
    server_peer: Option<Peer<RoleServer>>,
    component_id: &str,
    operation_name: &str,
) {
    if let Some(peer) = server_peer {
        if let Err(e) = peer.notify_tool_list_changed().await {
            error!(error = %e, "Failed to send tool list change notification");
        } else {
            info!(
                component_id = %component_id,
                "Sent tool list changed notification after {}ing component", operation_name
            );
        }
    } else {
        info!(component_id = %component_id, "Component {}ed successfully in CLI mode", operation_name);
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

    match lifecycle_manager.load_component(path).await {
        Ok(outcome) => {
            handle_tool_list_notification(None, &outcome.component_id, "load").await;
            create_load_component_success_result(&outcome)
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
            handle_tool_list_notification(None, id, "unload").await;
            create_component_success_result("unload", id)
        }
        Err(e) => {
            error!(error = %e, "Failed to unload component");
            Ok(create_component_error_result("unload", id, &e))
        }
    }
}

#[instrument]
pub(crate) fn parse_tool_schema(tool_json: &Value) -> Option<Tool> {
    let name = tool_json
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("<unnamed>");

    let description = tool_json
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("No description available");

    let input_schema = tool_json.get("inputSchema").cloned().unwrap_or(json!({}));

    // Extract outputSchema if present for MCP structured output support
    // MCP Inspector requires outputSchema.type to be "object" if provided.
    // To ensure compatibility, wrap any non-object output schema into an
    // object schema under a "result" property.
    let output_schema_arc = tool_json
        .get("outputSchema")
        .and_then(normalize_output_schema)
        .and_then(|normalized| match normalized {
            Value::Object(map) => Some(Arc::new(map)),
            _ => None,
        });

    debug!(
        tool_name = %name,
        has_output_schema = output_schema_arc.is_some(),
        "Parsed tool schema"
    );

    Some(Tool {
        name: Cow::Owned(name.to_string()),
        description: Some(Cow::Owned(description.to_string())),
        input_schema: Arc::new(serde_json::from_value(input_schema).unwrap_or_default()),
        output_schema: output_schema_arc,
        annotations: None,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

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
        // Verify that output_schema is None when not provided
        assert!(tool.output_schema.is_none());

        let schema_json = serde_json::to_value(&*tool.input_schema).unwrap();
        let expected = json!({
             "type": "object",
            "properties": {
                "test": {"type": "string"}
            }
        });
        assert_eq!(schema_json, expected);
    }

    #[test]
    fn test_extract_args_from_request() {
        let req = CallToolRequestParam {
            name: "test-tool".into(),
            arguments: Some(serde_json::Map::from_iter([
                ("path".to_string(), json!("/test/path")),
                ("id".to_string(), json!("test-id")),
            ])),
        };

        let args = extract_args_from_request(&req).unwrap();
        assert_eq!(args.get("path").unwrap(), "/test/path");
        assert_eq!(args.get("id").unwrap(), "test-id");
    }

    #[test]
    fn test_extract_args_from_request_none() {
        let req = CallToolRequestParam {
            name: "test-tool".into(),
            arguments: None,
        };

        let args = extract_args_from_request(&req).unwrap();
        assert!(args.is_empty());
    }

    #[test]
    fn test_parse_tool_schema_minimal() {
        let tool_json = json!({
            "name": "minimal-tool"
        });

        let tool = parse_tool_schema(&tool_json).unwrap();

        assert_eq!(tool.name, "minimal-tool");
        assert_eq!(tool.description, Some("No description available".into()));
    }

    #[test]
    fn test_parse_structured_result_with_object() {
        let json_str = r#"{"ok":{"message":"hello"}}"#;
        let parsed = parse_structured_result(json_str);
        assert_eq!(parsed, json!({"ok": {"message": "hello"}}));
    }

    #[test]
    fn test_parse_structured_result_with_text() {
        let parsed = parse_structured_result("plain text");
        assert_eq!(parsed, json!("plain text"));
    }

    #[test]
    fn test_normalize_output_schema_wraps_scalar() {
        let inner = json!({"type": "string"});
        let normalized = normalize_output_schema(&inner).unwrap();
        assert_eq!(
            normalized,
            json!({
                "type": "object",
                "properties": {"result": inner},
                "required": ["result"]
            })
        );
    }

    #[test]
    fn test_normalize_output_schema_handles_null() {
        assert!(normalize_output_schema(&Value::Null).is_none());
    }

    #[test]
    fn test_align_structured_result_with_schema_wraps_missing_result() {
        let schema = json!({
            "type": "object",
            "properties": {
                "result": {"type": "string"}
            },
            "required": ["result"]
        });

        let aligned =
            align_structured_result_with_schema(Some(&schema), Value::String("hello".into()));
        assert_eq!(aligned, json!({"result": "hello"}));
    }

    #[test]
    fn test_align_structured_result_with_schema_respects_existing_result() {
        let schema = json!({
            "type": "object",
            "properties": {
                "result": {"type": "string"}
            },
            "required": ["result"]
        });

        let original = json!({"result": {"ok": "16"}});
        let aligned = align_structured_result_with_schema(Some(&schema), original.clone());
        assert_eq!(aligned, original);
    }

    #[test]
    fn test_parse_tool_schema_no_name() {
        let tool_json = json!({
            "description": "Test description"
        });

        let tool = parse_tool_schema(&tool_json).unwrap();

        assert_eq!(tool.name, "<unnamed>");
        assert_eq!(tool.description, Some("Test description".into()));
    }

    #[test]
    fn test_parse_tool_schema_with_output_schema() {
        let tool_json = json!({
            "name": "weather-tool",
            "description": "Get weather data",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "location": {"type": "string"}
                },
                "required": ["location"]
            },
            "outputSchema": {
                "type": "object",
                "properties": {
                    "temperature": {"type": "number"},
                    "conditions": {"type": "string"}
                },
                "required": ["temperature", "conditions"]
            }
        });

        let tool = parse_tool_schema(&tool_json).unwrap();

        assert_eq!(tool.name, "weather-tool");
        // Verify that the description is now the original description (no enhancement needed)
        assert_eq!(tool.description.as_ref().unwrap(), "Get weather data");

        // Verify that output_schema is correctly set
        assert!(tool.output_schema.is_some());
        let output_schema_json =
            serde_json::to_value(&**tool.output_schema.as_ref().unwrap()).unwrap();
        let expected_output = json!({
            "type": "object",
            "properties": {
                "temperature": {"type": "number"},
                "conditions": {"type": "string"}
            },
            "required": ["temperature", "conditions"]
        });
        assert_eq!(output_schema_json, expected_output);

        let schema_json = serde_json::to_value(&*tool.input_schema).unwrap();
        let expected_input = json!({
            "type": "object",
            "properties": {
                "location": {"type": "string"}
            },
            "required": ["location"]
        });
        assert_eq!(schema_json, expected_input);
    }

    #[test]
    fn test_parse_tool_schema_integration_with_component2json() {
        // This test uses the same structure that component2json generates
        // to verify the integration works properly
        let component_generated_tool = json!({
            "name": "fetch",
            "description": "Auto-generated schema for function 'fetch'",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string"
                    }
                },
                "required": ["url"]
            },
            "outputSchema": {
                "oneOf": [
                    {
                        "type": "object",
                        "properties": {
                            "ok": {
                                "type": "string"
                            }
                        },
                        "required": ["ok"]
                    },
                    {
                        "type": "object",
                        "properties": {
                            "err": {
                                "type": "string"
                            }
                        },
                        "required": ["err"]
                    }
                ]
            }
        });

        let tool = parse_tool_schema(&component_generated_tool).unwrap();

        assert_eq!(tool.name, "fetch");
        // Verify that the description is now the original description (no enhancement needed)
        assert_eq!(
            tool.description.as_ref().unwrap(),
            "Auto-generated schema for function 'fetch'"
        );

        // Verify that output_schema is correctly set
        assert!(tool.output_schema.is_some());
        let output_schema_json =
            serde_json::to_value(&**tool.output_schema.as_ref().unwrap()).unwrap();
        let expected_output = json!({
            "type": "object",
            "properties": {
                "result": {
                    "oneOf": [
                        {
                            "type": "object",
                            "properties": {
                                "ok": {"type": "string"}
                            },
                            "required": ["ok"]
                        },
                        {
                            "type": "object",
                            "properties": {
                                "err": {"type": "string"}
                            },
                            "required": ["err"]
                        }
                    ]
                }
            },
            "required": ["result"]
        });
        assert_eq!(output_schema_json, expected_output);

        // Verify input schema is correctly parsed
        let input_schema_json = serde_json::to_value(&*tool.input_schema).unwrap();
        let expected_input = json!({
            "type": "object",
            "properties": {
                "url": {"type": "string"}
            },
            "required": ["url"]
        });
        assert_eq!(input_schema_json, expected_input);
    }
}
