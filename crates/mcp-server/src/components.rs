// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::sync::Arc;

use anyhow::{Context, Result};
use futures::stream::{self, StreamExt};
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock, Tool};
use serde_json::{json, Value};
use tracing::{debug, error, info, instrument};
use wassette::schema::canonicalize_output_schema;
use wassette::tool_result::present_tool_output;
use wassette::wasm_directory::{PackageSelector, WasmDirectoryClient};
use wassette::{format_error_chain, ComponentLoadOutcome, LifecycleManager, LoadResult};

#[instrument(skip(lifecycle_manager))]
pub(crate) async fn get_component_tools(lifecycle_manager: &LifecycleManager) -> Result<Vec<Tool>> {
    debug!("Listing components");
    let tools: Vec<_> = lifecycle_manager
        .list_tool_descriptors()
        .await?
        .into_iter()
        .filter_map(|descriptor| parse_tool_schema(&descriptor.schema))
        .collect();
    info!(total_tools = tools.len(), "Total tools collected");
    Ok(tools)
}

#[instrument(skip(lifecycle_manager))]
pub(crate) async fn handle_load_component(
    req: &CallToolRequestParams,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;
    let path = args
        .get("path")
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Argument 'path' must be a string"))
        })
        .transpose()?;
    let package = args
        .get("package")
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Argument 'package' must be a string"))
        })
        .transpose()?;
    let version = args
        .get("version")
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Argument 'version' must be a string"))
        })
        .transpose()?;

    match (path, package) {
        (Some(_), Some(_)) => {
            anyhow::bail!("Provide exactly one of 'path' or 'package'");
        }
        (Some(path), None) if version.is_none() => {
            debug!(
                path,
                operation = "load-component",
                "Component load operation started"
            );
            match lifecycle_manager.load_component(path).await {
                Ok(outcome) => {
                    info!(
                        path,
                        component_id = %outcome.component_id,
                        operation = "load-component",
                        "Component loaded successfully"
                    );
                    create_load_component_success_result(&outcome)
                }
                Err(error) => {
                    let error = error.context(format!("Failed to load component: {path}"));
                    error!(
                        path,
                        operation = "load-component",
                        error = %format_error_chain(&error),
                        "Component load operation failed"
                    );
                    Err(error)
                }
            }
        }
        (Some(_), None) => anyhow::bail!("Argument 'version' requires a 'package'"),
        (None, Some(package)) => {
            let selector = PackageSelector::parse(package).context(
                "Argument 'package' must be a registry/repository or namespace:package[@version] \
                 identity",
            )?;
            let directory = WasmDirectoryClient::from_environment()?;
            match lifecycle_manager
                .load_package(&directory, &selector, version)
                .await
            {
                Ok((resolved, outcome)) => {
                    info!(
                        package = %resolved.package_id,
                        component_id = %outcome.component_id,
                        operation = "load-component",
                        "Package loaded successfully"
                    );
                    create_package_load_success_result(&resolved, &outcome)
                }
                Err(error) => {
                    let error = error.context(format!("Failed to load package: {package}"));
                    error!(
                        package,
                        operation = "load-component",
                        error = %format_error_chain(&error),
                        "Package load operation failed"
                    );
                    Err(error)
                }
            }
        }
        (None, None) if version.is_some() => {
            anyhow::bail!("Argument 'version' requires a 'package'")
        }
        (None, None) => anyhow::bail!("Provide exactly one of 'path' or 'package'"),
    }
}

#[instrument(skip(lifecycle_manager))]
pub(crate) async fn handle_unload_component(
    req: &CallToolRequestParams,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'id' in arguments"))?;

    debug!(
        component_id = %id,
        operation = "unload-component",
        "Component unload operation started"
    );

    match lifecycle_manager.unload_component(id).await {
        Ok(()) => {
            info!(
                component_id = %id,
                operation = "unload-component",
                "Component unloaded successfully"
            );
            create_component_success_result("unload", id)
        }
        Err(e) => {
            error!(
                component_id = %id,
                operation = "unload-component",
                error = %format_error_chain(&e),
                "Component unload operation failed"
            );
            Ok(create_component_error_result("unload", id, &e))
        }
    }
}

#[instrument(skip(lifecycle_manager))]
pub async fn handle_component_call(
    req: &CallToolRequestParams,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;

    debug!(
        function_name = %req.name,
        "Component function invocation started"
    );
    let result = lifecycle_manager
        .invoke_unique_tool(&req.name, &Value::Object(args))
        .await;

    match result {
        Ok(output) => {
            debug!(
                function_name = %req.name,
                component_id = %output.descriptor.key.component_id.as_str(),
                "Component function invocation completed successfully"
            );

            create_component_call_result(
                &output.raw_result,
                output.descriptor.schema.get("outputSchema"),
            )
        }
        Err(e) => {
            error!(
                function_name = %req.name,
                error = %format_error_chain(&e),
                "Component function invocation failed"
            );
            Err(e)
        }
    }
}

fn create_component_call_result(
    raw_result: &str,
    output_schema: Option<&Value>,
) -> Result<CallToolResult> {
    let output = present_tool_output(raw_result, output_schema)?;
    let mut result = CallToolResult::success(vec![ContentBlock::text(output.text)]);
    result.structured_content = output.structured;
    Ok(result)
}

fn normalize_output_schema(schema: &Value) -> Option<Value> {
    if schema.is_null() {
        return None;
    }

    Some(canonicalize_output_schema(schema))
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

    let contents = vec![ContentBlock::text(result_text)];

    Ok(CallToolResult::success(contents))
}

pub(crate) fn extract_args_from_request(
    req: &CallToolRequestParams,
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

    let contents = vec![ContentBlock::text(status_text)];

    Ok(CallToolResult::success(contents))
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

    let contents = vec![ContentBlock::text(status_text)];

    Ok(CallToolResult::success(contents))
}

fn create_package_load_success_result(
    resolved: &wassette::wasm_directory::ResolvedPackage,
    outcome: &ComponentLoadOutcome,
) -> Result<CallToolResult> {
    let status = match outcome.status {
        LoadResult::New => "component loaded successfully",
        LoadResult::Replaced => "component reloaded successfully",
    };
    let receipt = match &outcome.commit.entry {
        wassette::store::StoredEntry::Installed(receipt) => receipt,
        wassette::store::StoredEntry::Retired(_) => {
            anyhow::bail!("Package load unexpectedly returned a retired receipt")
        }
    };
    let status_text = serde_json::to_string(&json!({
        "status": status,
        "id": &outcome.component_id,
        "tools": &outcome.tool_names,
        "package": resolved.package_id.to_string(),
        "wit_identity": &resolved.wit_identity,
        "requested_version": &resolved.requested_version,
        "selected_version": &resolved.selected_version,
        "manifest_digest": &resolved.manifest_digest,
        "storage_key": receipt.storage_key.as_str(),
        "revision": receipt.revision.to_string(),
        "receipt": receipt,
    }))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(
        status_text,
    )]))
}

/// Create error result for component operations
fn create_component_error_result(
    operation_name: &str,
    operation_arg: &str,
    error: &anyhow::Error,
) -> CallToolResult {
    let error_text = serde_json::to_string(&json!({
        "status": "error",
        "message": format!("Failed to {} component: {}", operation_name, format_error_chain(error)),
        "id": operation_arg
    }))
    .unwrap_or_else(|_| {
        format!("{{\"status\":\"error\",\"message\":\"Failed to {operation_name} component\"}}",)
    });

    let contents = vec![ContentBlock::text(error_text)];

    CallToolResult::error(contents)
}

/// Load a component without transport-specific notifications.
#[instrument(skip(lifecycle_manager))]
pub async fn handle_load_component_cli(
    req: &CallToolRequestParams,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'path'"))?;

    info!(path, "Loading component (CLI mode)");

    match lifecycle_manager.load_component(path).await {
        Ok(outcome) => create_load_component_success_result(&outcome),
        Err(e) => {
            let e = e.context(format!("Failed to load component: {path}"));
            error!(error = %format_error_chain(&e), path, "Failed to load component");
            Err(e)
        }
    }
}

/// Unload a component without transport-specific notifications.
#[instrument(skip(lifecycle_manager))]
pub async fn handle_unload_component_cli(
    req: &CallToolRequestParams,
    lifecycle_manager: &LifecycleManager,
) -> Result<CallToolResult> {
    let args = extract_args_from_request(req)?;
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'id' in arguments"))?;

    info!(component_id = %id, "Unloading component (CLI mode)");

    match lifecycle_manager.unload_component(id).await {
        Ok(()) => create_component_success_result("unload", id),
        Err(e) => {
            error!(error = %format_error_chain(&e), "Failed to unload component");
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

    let tool = Tool::new(
        name.to_string(),
        description.to_string(),
        Arc::new(serde_json::from_value(input_schema).unwrap_or_default()),
    );

    Some(match output_schema_arc {
        Some(output_schema) => tool.with_raw_output_schema(output_schema),
        None => tool,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::*;

    async fn manager(root: &Path) -> Result<LifecycleManager> {
        LifecycleManager::builder(root.join("store"))
            .with_secrets_dir(root.join("secrets"))
            .with_eager_loading(false)
            .build()
            .await
    }

    #[tokio::test]
    async fn load_component_rejects_ambiguous_or_invalid_package_selectors() -> Result<()> {
        let root = tempfile::tempdir()?;
        let lifecycle_manager = manager(root.path()).await?;

        let mixed = CallToolRequestParams::new("load-component").with_arguments(
            serde_json::Map::from_iter([
                ("path".to_owned(), json!("file:///tmp/component.wasm")),
                ("package".to_owned(), json!("ghcr.io/owner/component")),
            ]),
        );
        let error = handle_load_component(&mixed, &lifecycle_manager)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("exactly one"));

        let version_without_package = CallToolRequestParams::new("load-component").with_arguments(
            serde_json::Map::from_iter([("version".to_owned(), json!("1.2.3"))]),
        );
        assert!(
            handle_load_component(&version_without_package, &lifecycle_manager)
                .await
                .unwrap_err()
                .to_string()
                .contains("requires a 'package'")
        );

        let malformed_package = CallToolRequestParams::new("load-component").with_arguments(
            serde_json::Map::from_iter([(
                "package".to_owned(),
                json!("oci://ghcr.io/owner/component:latest"),
            )]),
        );
        assert!(
            handle_load_component(&malformed_package, &lifecycle_manager)
                .await
                .unwrap_err()
                .to_string()
                .contains("namespace:package[@version]")
        );
        Ok(())
    }

    async fn install(
        manager: &LifecycleManager,
        root: &Path,
        name: &str,
        interfaces: &[(&str, &str, u32)],
    ) -> Result<()> {
        let mut exports = String::new();
        for (index, (interface, result_type, value)) in interfaces.iter().enumerate() {
            exports.push_str(&format!(
                r#"
                (core module $m{index}
                    (func (export "run") (result i32) i32.const {value}))
                (core instance $i{index} (instantiate $m{index}))
                (func $f{index} (result {result_type})
                    (canon lift (core func $i{index} "run")))
                (instance $e{index} (export "run" (func $f{index})))
                (export "{interface}" (instance $e{index}))
                "#
            ));
        }
        let bytes = wat::parse_str(format!("(component ${name} {exports})"))?;
        let path = root.join(format!("private-{name}.wasm"));
        tokio::fs::write(&path, bytes).await?;
        manager
            .load_component(&format!("file://{}", path.display()))
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn scoped_consumer_preserves_live_cached_and_replaced_tool_responses() -> Result<()> {
        let root = tempfile::tempdir()?;
        let first = manager(root.path()).await?;
        install(&first, root.path(), "alpha", &[("math", "u32", 7)]).await?;
        let live_tools = get_component_tools(&first).await?;
        let cold = manager(root.path()).await?;
        assert_eq!(
            serde_json::to_value(get_component_tools(&cold).await?)?,
            serde_json::to_value(&live_tools)?
        );
        assert!(cold.list_components().await.is_empty());
        cold.populate_registry_from_metadata().await?;
        let request = CallToolRequestParams::new("math_run");
        for manager in [&first, &cold] {
            assert_eq!(
                serde_json::to_value(handle_component_call(&request, manager).await?)?,
                json!({
                    "resultType": "complete",
                    "content": [{"type": "text", "text": "7"}],
                    "structuredContent": {"result": 7},
                    "isError": false
                })
            );
        }
        install(&first, root.path(), "alpha", &[("math", "bool", 1)]).await?;
        assert_eq!(
            serde_json::to_value(handle_component_call(&request, &cold).await?)?,
            json!({
                "resultType": "complete",
                "content": [{"type": "text", "text": "true"}],
                "structuredContent": {"result": true},
                "isError": false
            })
        );
        let tools = get_component_tools(&cold).await?;
        assert_eq!(
            tools[0].output_schema.as_ref().unwrap()["properties"]["result"]["type"],
            "boolean"
        );
        first.unload_component("alpha").await?;
        assert!(get_component_tools(&cold).await?.is_empty());
        assert!(handle_component_call(&request, &cold).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn mcp_lists_but_refuses_ambiguous_component_and_interface_names() -> Result<()> {
        for same_component in [false, true] {
            let root = tempfile::tempdir()?;
            let manager = manager(root.path()).await?;
            if same_component {
                install(
                    &manager,
                    root.path(),
                    "alpha",
                    &[
                        ("example:math/ops@1.0.0-a.b+c", "u32", 7),
                        ("example:math/ops@1.0.0-a+b.c", "bool", 1),
                    ],
                )
                .await?;
            } else {
                install(&manager, root.path(), "alpha", &[("math", "u32", 7)]).await?;
                install(&manager, root.path(), "beta", &[("MATH", "bool", 1)]).await?;
            }
            let tools = get_component_tools(&manager).await?;
            assert_eq!(tools.len(), 2);
            assert_eq!(tools[0].name, tools[1].name);
            let request = CallToolRequestParams::new(tools[0].name.to_string());
            let error = handle_component_call(&request, &manager).await.unwrap_err();
            assert!(matches!(
                error.downcast_ref::<wassette::ToolLookupError>(),
                Some(wassette::ToolLookupError::Ambiguous { .. })
            ));
        }
        Ok(())
    }

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
        let req =
            CallToolRequestParams::new("test-tool").with_arguments(serde_json::Map::from_iter([
                ("path".to_string(), json!("/test/path")),
                ("id".to_string(), json!("test-id")),
            ]));

        let args = extract_args_from_request(&req).unwrap();
        assert_eq!(args.get("path").unwrap(), "/test/path");
        assert_eq!(args.get("id").unwrap(), "test-id");
    }

    #[test]
    fn test_extract_args_from_request_none() {
        let req = CallToolRequestParams::new("test-tool");

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
    fn test_component_call_result_preserves_wire_shape() -> Result<()> {
        let schema = json!({"type": "string"});
        let result = create_component_call_result(r#"{"result":"hello"}"#, Some(&schema))?;
        assert_eq!(
            serde_json::to_value(result)?,
            json!({
                "resultType": "complete",
                "content": [{"type": "text", "text": "hello"}],
                "structuredContent": {"result": "hello"},
                "isError": false
            })
        );
        Ok(())
    }

    #[test]
    fn test_component_call_result_without_schema() -> Result<()> {
        for schema in [None, Some(&Value::Null)] {
            let result = create_component_call_result("plain text", schema)?;
            assert_eq!(
                serde_json::to_value(result)?,
                json!({
                    "resultType": "complete",
                    "content": [{"type": "text", "text": "plain text"}],
                    "isError": false
                })
            );
        }
        Ok(())
    }

    #[test]
    fn test_component_call_result_keeps_guest_errors_as_values() -> Result<()> {
        let schema = json!({
            "oneOf": [
                {"type": "object", "properties": {"ok": {"type": "string"}}},
                {"type": "object", "properties": {"err": {"type": "string"}}}
            ]
        });
        let result =
            create_component_call_result(r#"{"result":{"err":"guest error"}}"#, Some(&schema))?;
        assert_eq!(
            serde_json::to_value(result)?,
            json!({
                "resultType": "complete",
                "content": [{"type": "text", "text": "{\"err\":\"guest error\"}"}],
                "structuredContent": {"result": {"err": "guest error"}},
                "isError": false
            })
        );
        Ok(())
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
    fn test_normalize_output_schema_converts_tuple_array() {
        let legacy = json!({
            "type": "object",
            "properties": {
                "result": {
                    "type": "array",
                    "items": [
                        {"type": "string"},
                        {"type": "number"}
                    ]
                }
            },
            "required": ["result"]
        });

        let normalized = normalize_output_schema(&legacy).unwrap();
        assert_eq!(
            normalized.get("properties").unwrap().get("result").unwrap(),
            &json!({
                "type": "object",
                "properties": {
                    "val0": {"type": "string"},
                    "val1": {"type": "number"}
                },
                "required": ["val0", "val1"]
            })
        );
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
                "result": {
                    "type": "object",
                    "properties": {
                        "temperature": {"type": "number"},
                        "conditions": {"type": "string"}
                    },
                    "required": ["temperature", "conditions"]
                }
            },
            "required": ["result"]
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
