// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! MCP Server implementation for handling WebAssembly components

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use mcp_server::{
    handle_prompts_list, handle_resources_list, handle_tools_call, handle_tools_list,
    LifecycleManager,
};
use rmcp::model::{
    CallToolRequestParam, CallToolResult, ErrorData, ListPromptsResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParam, ServerCapabilities, ServerInfo, ToolsCapability,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::ServerHandler;

/// A security-oriented runtime that runs WebAssembly Components via MCP.
#[derive(Clone)]
pub struct McpServer {
    lifecycle_manager: LifecycleManager,
    peer: Arc<Mutex<Option<rmcp::Peer<rmcp::RoleServer>>>>,
    disable_builtin_tools: bool,
}

impl McpServer {
    /// Creates a new MCP server instance with the given lifecycle manager.
    ///
    /// # Arguments
    /// * `lifecycle_manager` - The lifecycle manager for handling component operations
    /// * `disable_builtin_tools` - Whether to disable built-in tools
    pub fn new(lifecycle_manager: LifecycleManager, disable_builtin_tools: bool) -> Self {
        Self {
            lifecycle_manager,
            peer: Arc::new(Mutex::new(None)),
            disable_builtin_tools,
        }
    }

    /// Store the peer for background notifications (called on first request)
    fn store_peer_if_empty(&self, peer: rmcp::Peer<rmcp::RoleServer>) {
        let mut peer_guard = self.peer.lock().unwrap();
        if peer_guard.is_none() {
            *peer_guard = Some(peer);
        }
    }

    /// Get a clone of the stored peer if available
    pub fn get_peer(&self) -> Option<rmcp::Peer<rmcp::RoleServer>> {
        self.peer.lock().unwrap().clone()
    }
}

#[allow(refining_impl_trait_reachable)]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability {
                    list_changed: Some(true),
                }),
                ..Default::default()
            },
            instructions: Some(
                r#"This server runs tools in sandboxed WebAssembly environments with no default access to host resources.

Key points:
- Tools must be loaded before use: "Load component from oci://registry/tool:version" or "file:///path/to/tool.wasm"
- When the server starts, it will load all tools present in the component directory.
- You can list loaded tools with 'list-components' tool.
- Each tool only accesses resources explicitly granted by a policy file (filesystem paths, network domains, etc.)
- You MUST never modify the policy file directly, use tools to grant permissions instead.
- Tools needs permission for that resource
- If access is denied, suggest alternatives within allowed permissions or propose to grant permission"#.to_string(),
            ),
            ..Default::default()
        }
    }

    fn call_tool<'a>(
        &'a self,
        params: CallToolRequestParam,
        ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<CallToolResult, ErrorData>> + Send + 'a>> {
        let peer_clone = ctx.peer.clone();

        // Store peer on first request
        self.store_peer_if_empty(peer_clone.clone());

        let disable_builtin_tools = self.disable_builtin_tools;
        Box::pin(async move {
            let result = handle_tools_call(
                params,
                &self.lifecycle_manager,
                peer_clone,
                disable_builtin_tools,
            )
            .await;
            match result {
                Ok(value) => serde_json::from_value(value).map_err(|e| {
                    ErrorData::parse_error(format!("Failed to parse result: {e}"), None)
                }),
                Err(err) => Err(ErrorData::parse_error(err.to_string(), None)),
            }
        })
    }

    fn list_tools<'a>(
        &'a self,
        _params: Option<PaginatedRequestParam>,
        ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<ListToolsResult, ErrorData>> + Send + 'a>> {
        // Store peer on first request
        self.store_peer_if_empty(ctx.peer.clone());

        let disable_builtin_tools = self.disable_builtin_tools;
        Box::pin(async move {
            let result = handle_tools_list(&self.lifecycle_manager, disable_builtin_tools).await;
            match result {
                Ok(value) => serde_json::from_value(value).map_err(|e| {
                    ErrorData::parse_error(format!("Failed to parse result: {e}"), None)
                }),
                Err(err) => Err(ErrorData::parse_error(err.to_string(), None)),
            }
        })
    }

    fn list_prompts<'a>(
        &'a self,
        _params: Option<PaginatedRequestParam>,
        ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<ListPromptsResult, ErrorData>> + Send + 'a>> {
        // Store peer on first request
        self.store_peer_if_empty(ctx.peer.clone());

        Box::pin(async move {
            let result = handle_prompts_list(serde_json::Value::Null).await;
            match result {
                Ok(value) => serde_json::from_value(value).map_err(|e| {
                    ErrorData::parse_error(format!("Failed to parse result: {e}"), None)
                }),
                Err(err) => Err(ErrorData::parse_error(err.to_string(), None)),
            }
        })
    }

    fn list_resources<'a>(
        &'a self,
        _params: Option<PaginatedRequestParam>,
        ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<ListResourcesResult, ErrorData>> + Send + 'a>> {
        // Store peer on first request
        self.store_peer_if_empty(ctx.peer.clone());

        Box::pin(async move {
            let result = handle_resources_list(serde_json::Value::Null).await;
            match result {
                Ok(value) => serde_json::from_value(value).map_err(|e| {
                    ErrorData::parse_error(format!("Failed to parse result: {e}"), None)
                }),
                Err(err) => Err(ErrorData::parse_error(err.to_string(), None)),
            }
        })
    }
}
