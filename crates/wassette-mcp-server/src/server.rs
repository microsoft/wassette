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
    CacheScope, CallToolRequestParams, CallToolResponse, ErrorData, ListPromptsResult,
    ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ProtocolVersion, ServerCapabilities, ServerInfo, ServerNotification, SubscriptionFilter,
    ToolListChangedNotification,
};
use rmcp::service::{RequestContext, RoleServer, SubscriptionContext, SubscriptionSendError};
use rmcp::ServerHandler;
use tokio::sync::broadcast;

/// Buffered tool-list changes per subscriber.
///
/// Every change carries the same "go re-read the list" payload, so a subscriber
/// that falls behind only ever needs one more notification to catch up.
const TOOL_LIST_CHANGED_CAPACITY: usize = 16;

/// Built-in tools that change which tools the server exposes.
///
/// Calling one of these has to reach `subscriptions/listen` streams, which
/// belong to clients other than the one making the call.
const TOOL_LIST_MUTATING_TOOLS: [&str; 2] = ["load-component", "unload-component"];

fn supports_cache_hints(context: &RequestContext<RoleServer>) -> bool {
    context
        .protocol_version()
        .is_some_and(|version| version >= ProtocolVersion::V_2026_07_28)
}

/// A security-oriented runtime that runs WebAssembly Components via MCP.
#[derive(Clone)]
pub struct McpServer {
    lifecycle_manager: LifecycleManager,
    peer: Arc<Mutex<Option<rmcp::Peer<rmcp::RoleServer>>>>,
    disable_builtin_tools: bool,
    tool_list_changed: broadcast::Sender<()>,
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
            tool_list_changed: broadcast::channel(TOOL_LIST_CHANGED_CAPACITY).0,
        }
    }

    /// Announce that the tool list changed to every client shape.
    ///
    /// A stateless client (protocol revision 2026-07-28 and later) has no
    /// long-lived peer, so it learns about changes by holding open a
    /// `subscriptions/listen` stream; a legacy client is told through its
    /// session peer. Callers should not have to know which one is attached, so
    /// this feeds both.
    pub fn publish_tool_list_changed(&self) {
        // An error here only means nobody is subscribed right now.
        let _ = self.tool_list_changed.send(());

        if let Some(peer) = self.get_peer() {
            tokio::spawn(async move {
                if let Err(e) = peer.notify_tool_list_changed().await {
                    tracing::warn!("Failed to notify tool list changed: {}", e);
                }
            });
        }
    }

    /// Subscribe to tool-list changes for one `subscriptions/listen` stream.
    pub fn subscribe_tool_list_changed(&self) -> broadcast::Receiver<()> {
        self.tool_list_changed.subscribe()
    }

    /// Announce a tool-list change to subscription streams only.
    ///
    /// Used after a built-in tool mutated the tool list, where the calling
    /// peer has already been notified by the tool handler itself.
    fn broadcast_tool_list_changed(&self) {
        let _ = self.tool_list_changed.send(());
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
        let mutates_tool_list =
            !disable_builtin_tools && TOOL_LIST_MUTATING_TOOLS.contains(&params.name.as_ref());
        Box::pin(async move {
            let result = handle_tools_call(
                params,
                &self.lifecycle_manager,
                peer_clone,
                disable_builtin_tools,
            )
            .await;
            // A failing tool is reported as `Ok` carrying `isError: true`, not as
            // `Err`, so testing `is_ok` alone would announce a load or unload that
            // never changed the tool list.
            let tool_list_mutated = mutates_tool_list
                && result.as_ref().is_ok_and(|value| {
                    value.get("isError") != Some(&serde_json::Value::Bool(true))
                });
            if tool_list_mutated {
                // The tool handler already told the calling peer; subscription
                // streams belong to other clients and still need telling.
                self.broadcast_tool_list_changed();
            }
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
        let supports_cache_hints = supports_cache_hints(&ctx);

        let disable_builtin_tools = self.disable_builtin_tools;
        Box::pin(async move {
            let result = handle_tools_list(&self.lifecycle_manager, disable_builtin_tools).await;
            match result {
                Ok(value) => {
                    let mut result: ListToolsResult =
                        serde_json::from_value(value).map_err(|e| {
                            ErrorData::parse_error(format!("Failed to parse result: {e}"), None)
                        })?;
                    if supports_cache_hints {
                        result.ttl_ms.get_or_insert(0);
                        result.cache_scope.get_or_insert(CacheScope::Public);
                    }
                    Ok(result)
                }
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
        let supports_cache_hints = supports_cache_hints(&ctx);

        Box::pin(async move {
            let result = handle_prompts_list(serde_json::Value::Null).await;
            match result {
                Ok(value) => {
                    let mut result: ListPromptsResult =
                        serde_json::from_value(value).map_err(|e| {
                            ErrorData::parse_error(format!("Failed to parse result: {e}"), None)
                        })?;
                    if supports_cache_hints {
                        result.ttl_ms.get_or_insert(0);
                        result.cache_scope.get_or_insert(CacheScope::Public);
                    }
                    Ok(result)
                }
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
        let supports_cache_hints = supports_cache_hints(&ctx);

        Box::pin(async move {
            let result = handle_resources_list().await;
            match result {
                Ok(value) => {
                    let mut result: ListResourcesResult =
                        serde_json::from_value(value).map_err(|e| {
                            ErrorData::parse_error(format!("Failed to parse result: {e}"), None)
                        })?;
                    if supports_cache_hints {
                        result.ttl_ms.get_or_insert(0);
                        result.cache_scope.get_or_insert(CacheScope::Public);
                    }
                    Ok(result)
                }
                Err(err) => Err(ErrorData::parse_error(err.to_string(), None)),
            }
        })
    }

    fn list_resource_templates<'a>(
        &'a self,
        _params: Option<PaginatedRequestParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<ListResourceTemplatesResult, ErrorData>> + Send + 'a>>
    {
        self.track_peer(ctx.peer.clone());
        let supports_cache_hints = supports_cache_hints(&ctx);

        Box::pin(async move {
            let mut result = ListResourceTemplatesResult::default();
            if supports_cache_hints {
                result.ttl_ms = Some(0);
                result.cache_scope = Some(CacheScope::Public);
            }
            Ok(result)
        })
    }

    fn accepted_subscription_filter(
        &self,
        _requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        // rmcp intersects this with the client's request and with the
        // capabilities from `get_info`, which already advertise tool list
        // changes. Returning `None` would leave `subscriptions/listen`
        // unimplemented, which is what left a stateless client unable to hear
        // about a newly loaded component.
        Some(SubscriptionFilter::builder().tools_list_changed().build())
    }

    fn listen<'a>(
        &'a self,
        context: SubscriptionContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), ErrorData>> + Send + 'a>> {
        let mut receiver = self.subscribe_tool_list_changed();
        Box::pin(async move {
            loop {
                tokio::select! {
                    _ = context.cancelled() => return Ok(()),
                    changed = receiver.recv() => {
                        match changed {
                            Ok(()) => {}
                            // The stream only ever says "re-read the list", so a
                            // subscriber that fell behind is caught up by one
                            // notification.
                            Err(broadcast::error::RecvError::Lagged(_)) => {}
                            Err(broadcast::error::RecvError::Closed) => return Ok(()),
                        }

                        let notification = ServerNotification::ToolListChangedNotification(
                            ToolListChangedNotification::default(),
                        );
                        match context.sink().send(notification).await {
                            Ok(()) => {}
                            Err(SubscriptionSendError::SubscriptionClosed) => return Ok(()),
                            Err(e) => {
                                tracing::warn!("Failed to send tool list changed to subscription: {}", e);
                            }
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rmcp::ServiceExt;
    use serde_json::Value;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

    use super::*;

    const INITIALIZE_REQUEST: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"peer-lifecycle-test","version":"1.0.0"}}}
                        "#;

    async fn connect_peer(
        server: McpServer,
    ) -> (
        rmcp::Peer<RoleServer>,
        BufReader<DuplexStream>,
        tokio::task::JoinHandle<()>,
    ) {
        let (server_transport, client_transport) = tokio::io::duplex(4096);
        let (peer_sender, peer_receiver) = tokio::sync::oneshot::channel();
        let service_task = tokio::spawn(async move {
            let service = server
                .serve(server_transport)
                .await
                .expect("server should accept the test peer");
            peer_sender
                .send(service.peer().clone())
                .expect("test should still be waiting for the peer");
            let _ = service.waiting().await;
        });

        let mut client = BufReader::new(client_transport);
        client
            .get_mut()
            .write_all(INITIALIZE_REQUEST.as_bytes())
            .await
            .expect("initialize request should be written");
        client
            .get_mut()
            .flush()
            .await
            .expect("initialize request should be flushed");

        let mut response = String::new();
        client
            .read_line(&mut response)
            .await
            .expect("initialize response should be read");
        let response: Value =
            serde_json::from_str(&response).expect("initialize response should be JSON");
        assert!(
            response.get("error").is_none(),
            "initialize failed: {response}"
        );

        let peer = peer_receiver
            .await
            .expect("server should expose its connected peer");
        (peer, client, service_task)
    }

    async fn expect_tool_list_changed(client: &mut BufReader<DuplexStream>) {
        let mut notification = String::new();
        tokio::time::timeout(Duration::from_secs(5), client.read_line(&mut notification))
            .await
            .expect("timed out waiting for tools/list_changed")
            .expect("tools/list_changed should be read");
        let notification: Value =
            serde_json::from_str(&notification).expect("notification should be JSON");
        assert_eq!(
            notification["method"], "notifications/tools/list_changed",
            "connected peer should receive the tool-list change: {notification}"
        );
    }

    /// Startup loading can publish before a peer exists, while a later stateless
    /// request can leave a dead peer behind. This prevents either state from
    /// suppressing notifications to subscriptions or the next live peer.
    #[tokio::test]
    async fn publish_tool_list_changed_handles_peer_lifecycle() {
        let temp_dir = tempfile::tempdir().expect("temporary component directory should exist");
        let lifecycle_manager = LifecycleManager::new(temp_dir.path())
            .await
            .expect("lifecycle manager should be created");
        let server = McpServer::new(lifecycle_manager, false);
        let mut subscription = server.subscribe_tool_list_changed();

        assert!(server.get_peer().is_none());
        server.publish_tool_list_changed();
        subscription
            .recv()
            .await
            .expect("peerless publication should still reach subscriptions");

        let (first_peer, mut first_client, first_service) = connect_peer(server.clone()).await;
        server.track_peer(first_peer.clone());
        server.publish_tool_list_changed();
        expect_tool_list_changed(&mut first_client).await;

        drop(first_client);
        tokio::time::timeout(Duration::from_secs(5), first_service)
            .await
            .expect("first peer should close after its transport is dropped")
            .expect("first peer service should shut down cleanly");
        assert!(first_peer.is_transport_closed());

        let (second_peer, mut second_client, second_service) = connect_peer(server.clone()).await;
        server.track_peer(second_peer.clone());
        assert!(
            server
                .get_peer()
                .is_some_and(|peer| !peer.is_transport_closed()),
            "a live peer should replace the stale peer"
        );
        server.publish_tool_list_changed();
        expect_tool_list_changed(&mut second_client).await;

        drop(second_client);
        tokio::time::timeout(Duration::from_secs(5), second_service)
            .await
            .expect("second peer should close after its transport is dropped")
            .expect("second peer service should shut down cleanly");
        assert!(second_peer.is_transport_closed());
        assert!(
            server.get_peer().is_none(),
            "get_peer should remove a peer whose transport has closed"
        );
    }
}
