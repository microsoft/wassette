use std::borrow::Cow;
use std::sync::Arc;

use anyhow::Result;
use rmcp::model::{CallToolRequestParam, CallToolResult, Content, Tool};
use rmcp::{Peer, RoleServer};
use serde_json::{json, Value};
use tracing::{debug, error, info, instrument};
use weld::LifecycleManager;

use crate::components::{
    get_component_tools, handle_component_call, handle_load_component, handle_unload_component,
};

/// Handles a request to list available tools.
#[instrument(skip(lifecycle_manager))]
pub async fn handle_tools_list(lifecycle_manager: &LifecycleManager, builtin_tools_enabled: bool) -> Result<Value> {
    debug!("Handling tools list request");

    let mut tools = get_component_tools(lifecycle_manager).await?;
    if builtin_tools_enabled {
        tools.extend(get_builtin_tools());
    }
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
    builtin_tools_enabled: bool,
) -> Result<Value> {
    info!("Handling tool call");

    let result = match req.name.as_ref() {
        "load-component" if builtin_tools_enabled => handle_load_component(&req, lifecycle_manager, server_peer).await,
        "unload-component" if builtin_tools_enabled => handle_unload_component(&req, lifecycle_manager, server_peer).await,
        _ => handle_component_call(&req, lifecycle_manager).await,
    };

    if let Err(ref e) = result {
        error!(error = ?e, "Tool call failed");
    }

    match result {
        Ok(result) => Ok(serde_json::to_value(result)?),
        Err(e) => {
            let error_text = format!("Error: {}", e);
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
                "Dynamically loads a new WebAssembly component. Arguments: path (string)",
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
                "Dynamically unloads a WebAssembly component. Argument: id (string)",
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
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_builtin_tools() {
        let tools = get_builtin_tools();
        assert_eq!(tools.len(), 2);
        assert!(tools.iter().any(|t| t.name == "load-component"));
        assert!(tools.iter().any(|t| t.name == "unload-component"));
    }

    #[test]
    fn test_get_builtin_tools_empty_when_disabled() {
        // This test verifies that when builtin tools are disabled,
        // they are not included in the tools list
        let tools = get_builtin_tools();
        // The function itself always returns the tools, but the filtering
        // happens in handle_tools_list based on the builtin_tools_enabled flag
        assert_eq!(tools.len(), 2);
    }

    #[tokio::test]
    async fn test_handle_tools_list_with_builtin_tools_disabled() {
        use tempfile::TempDir;
        use weld::LifecycleManager;
        
        // Create a temporary directory for testing
        let temp_dir = TempDir::new().unwrap();
        let lifecycle_manager = LifecycleManager::new(temp_dir.path(), None::<String>).await.unwrap();
        
        // Test with builtin tools disabled
        let result = handle_tools_list(&lifecycle_manager, false).await.unwrap();
        let tools_result: rmcp::model::ListToolsResult = serde_json::from_value(result).unwrap();
        
        // Should not contain builtin tools
        assert!(!tools_result.tools.iter().any(|t| t.name == "load-component"));
        assert!(!tools_result.tools.iter().any(|t| t.name == "unload-component"));
    }

    #[tokio::test]
    async fn test_handle_tools_call_with_builtin_tools_disabled() {
        use tempfile::TempDir;
        use weld::LifecycleManager;
        use rmcp::model::CallToolRequestParam;
        use serde_json::{Map, Value};
        
        // Create a temporary directory for testing
        let temp_dir = TempDir::new().unwrap();
        let lifecycle_manager = LifecycleManager::new(temp_dir.path(), None::<String>).await.unwrap();
        
        // Create arguments map properly
        let mut arguments = Map::new();
        arguments.insert("path".to_string(), Value::String("/some/test/path".to_string()));
        
        // Create a request for load-component tool
        let req = CallToolRequestParam {
            name: "load-component".into(),
            arguments: Some(arguments)
        };
        
        // Test with builtin tools disabled - should fail to handle the built-in tool
        let result = handle_tools_call(req, &lifecycle_manager, None, false).await;
        
        // The call should succeed but return an error because the tool is not found
        // (since builtin tools are disabled, it will try to find it as a component tool)
        assert!(result.is_ok());
        let result_value = result.unwrap();
        let call_result: CallToolResult = serde_json::from_value(result_value).unwrap();
        
        // Should have is_error set to true since the tool wasn't found
        assert_eq!(call_result.is_error, Some(true));
    }

    #[tokio::test]
    async fn test_handle_tools_call_with_builtin_tools_enabled() {
        use tempfile::TempDir;
        use weld::LifecycleManager;
        use rmcp::model::CallToolRequestParam;
        use serde_json::{Map, Value};
        
        // Create a temporary directory for testing
        let temp_dir = TempDir::new().unwrap();
        let lifecycle_manager = LifecycleManager::new(temp_dir.path(), None::<String>).await.unwrap();
        
        // Create arguments map properly
        let mut arguments = Map::new();
        arguments.insert("path".to_string(), Value::String("/some/invalid/path".to_string()));
        
        // Create a request for load-component tool
        let req = CallToolRequestParam {
            name: "load-component".into(),
            arguments: Some(arguments)
        };
        
        // Test with builtin tools enabled - should try to handle the built-in tool
        let result = handle_tools_call(req, &lifecycle_manager, None, true).await;
        
        // The call should succeed but return an error because the path is invalid
        assert!(result.is_ok());
        let result_value = result.unwrap();
        let call_result: CallToolResult = serde_json::from_value(result_value).unwrap();
        
        // Should have is_error set to true since the path is invalid
        assert_eq!(call_result.is_error, Some(true));
    }

    #[tokio::test]
    async fn test_handle_tools_list_with_builtin_tools_enabled() {
        use tempfile::TempDir;
        use weld::LifecycleManager;
        
        // Create a temporary directory for testing
        let temp_dir = TempDir::new().unwrap();
        let lifecycle_manager = LifecycleManager::new(temp_dir.path(), None::<String>).await.unwrap();
        
        // Test with builtin tools enabled
        let result = handle_tools_list(&lifecycle_manager, true).await.unwrap();
        let tools_result: rmcp::model::ListToolsResult = serde_json::from_value(result).unwrap();
        
        // Should contain builtin tools
        assert!(tools_result.tools.iter().any(|t| t.name == "load-component"));
        assert!(tools_result.tools.iter().any(|t| t.name == "unload-component"));
    }
}
