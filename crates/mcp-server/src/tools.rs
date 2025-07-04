use std::borrow::Cow;
use std::sync::Arc;

use anyhow::Result;
use rmcp::model::{CallToolRequestParam, CallToolResult, Content, Tool};
use rmcp::{Peer, RoleServer};
use serde_json::{json, Value};
use tracing::{debug, error, info, instrument};
use weld::LifecycleManager;

use crate::components::{
    get_component_tools, handle_component_call, handle_list_components, handle_load_component,
    handle_unload_component,
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
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_builtin_tools() {
        let tools = get_builtin_tools();
        assert_eq!(tools.len(), 3);
        assert!(tools.iter().any(|t| t.name == "load-component"));
        assert!(tools.iter().any(|t| t.name == "unload-component"));
        assert!(tools.iter().any(|t| t.name == "list-components"));
    }
}
