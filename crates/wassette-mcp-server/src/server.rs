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
    CallToolRequestParams, CallToolResponse, ErrorData, ListPromptsResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo,
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

    /// Track the peer used for background notifications (called on every request).
    ///
    /// Under a stateless request (protocol revision 2026-07-28 and later) the
    /// peer is scoped to that single request and its transport closes as soon
    /// as the response is written. Keeping the first peer forever would let one
    /// such request permanently silence notifications for every later client,
    /// so a peer is only adopted when there is no live one already.
    fn track_peer(&self, peer: rmcp::Peer<rmcp::RoleServer>) {
        let mut peer_guard = self.peer.lock().unwrap();
        let stale = peer_guard
            .as_ref()
            .is_none_or(rmcp::Peer::is_transport_closed);
        if stale {
            *peer_guard = Some(peer);
        }
    }

    /// Get a clone of the stored peer if it is still usable.
    ///
    /// A peer whose transport has closed is dropped rather than returned, so a
    /// dead peer never masks a live one that arrives later.
    pub fn get_peer(&self) -> Option<rmcp::Peer<rmcp::RoleServer>> {
        let mut peer_guard = self.peer.lock().unwrap();
        if peer_guard
            .as_ref()
            .is_some_and(rmcp::Peer::is_transport_closed)
        {
            *peer_guard = None;
        }
        peer_guard.clone()
    }
}

#[allow(refining_impl_trait_reachable)]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .build();
        info.instructions = Some(
            r#"This server runs tools in sandboxed WebAssembly environments with no default access to host resources.

Key points:
- Tools must be loaded before use: "Load component from oci://registry/tool:version" or "file:///path/to/tool.wasm"
- When the server starts, it will load all tools present in the component directory.
- You can list loaded tools with 'list-components' tool.
- Each tool only accesses resources explicitly granted by a policy file (filesystem paths, network domains, etc.)
- You MUST never modify the policy file directly, use tools to grant permissions instead.
- Tools needs permission for that resource
- If access is denied, suggest alternatives within allowed permissions or propose to grant permission"#.to_string(),
        );
        info
    }

    fn call_tool<'a>(
        &'a self,
        params: CallToolRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<CallToolResponse, ErrorData>> + Send + 'a>> {
        let peer_clone = ctx.peer.clone();

        // Track the peer for background notifications
        self.track_peer(peer_clone.clone());

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
                Ok(value) => serde_json::from_value(value)
                    .map(CallToolResponse::Complete)
                    .map_err(|e| {
                        ErrorData::parse_error(format!("Failed to parse result: {e}"), None)
                    }),
                Err(err) => Err(ErrorData::parse_error(err.to_string(), None)),
            }
        })
    }

    fn list_tools<'a>(
        &'a self,
        _params: Option<PaginatedRequestParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<ListToolsResult, ErrorData>> + Send + 'a>> {
        // Track the peer for background notifications
        self.track_peer(ctx.peer.clone());

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
        _params: Option<PaginatedRequestParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<ListPromptsResult, ErrorData>> + Send + 'a>> {
        // Track the peer for background notifications
        self.track_peer(ctx.peer.clone());

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
        _params: Option<PaginatedRequestParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<ListResourcesResult, ErrorData>> + Send + 'a>> {
        // Track the peer for background notifications
        self.track_peer(ctx.peer.clone());

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
