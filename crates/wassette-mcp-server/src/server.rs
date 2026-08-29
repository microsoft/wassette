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

const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";

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
    legacy_sessions: bool,
    tool_list_changed: broadcast::Sender<()>,
}

impl McpServer {
    /// Creates a new MCP server instance with the given lifecycle manager.
    ///
    /// # Arguments
    /// * `lifecycle_manager` - The lifecycle manager for handling component operations
    /// * `disable_builtin_tools` - Whether to disable built-in tools
    /// * `legacy_sessions` - Whether the pre-`2026-07-28` session lifecycle is served
    pub fn new(
        lifecycle_manager: LifecycleManager,
        disable_builtin_tools: bool,
        legacy_sessions: bool,
    ) -> Self {
        Self {
            lifecycle_manager,
            peer: Arc::new(Mutex::new(None)),
            disable_builtin_tools,
            legacy_sessions,
            tool_list_changed: broadcast::channel(TOOL_LIST_CHANGED_CAPACITY).0,
        }
    }

    /// Whether this request's peer outlives the request that carried it.
    ///
    /// rmcp routes a request through its session layer only when legacy sessions
    /// are enabled *and* the request declares a pre-`2026-07-28` revision. It
    /// injects the client's HTTP parts into every Streamable HTTP context and
    /// never strips `Mcp-Session-Id`, so the header on its own proves nothing: a
    /// stateless request carrying a stale session id would look persistent and
    /// reintroduce exactly the notification leak this guard exists to prevent.
    /// Mirror rmcp's own condition instead of trusting the header alone.
    ///
    /// An unknown protocol version is treated as not persistent. Declining to
    /// track a peer only costs a legacy client a background notification, while
    /// wrongly tracking one injects unsolicited traffic into an ordinary
    /// response, so the uncertain case fails toward the cheaper mistake.
    fn has_persistent_peer(&self, context: &RequestContext<RoleServer>) -> bool {
        let Some(parts) = context.extensions.get::<axum::http::request::Parts>() else {
            // Not an HTTP transport: stdio peers live as long as the process.
            return true;
        };

        self.legacy_sessions
            && parts.headers.contains_key(MCP_SESSION_ID_HEADER)
            && context
                .protocol_version()
                .is_some_and(|version| version < ProtocolVersion::V_2026_07_28)
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

    /// Track a persistent peer used for background notifications.
    ///
    /// rmcp inserts HTTP request parts into every Streamable HTTP context. A
    /// session-routed request has a validated session ID, while a stateless
    /// request does not. Non-HTTP transports such as stdio are persistent.
    fn track_peer(&self, context: &RequestContext<RoleServer>) {
        if !self.has_persistent_peer(context) {
            return;
        }

        let mut peer_guard = self.peer.lock().unwrap();
        let stale = peer_guard
            .as_ref()
            .is_none_or(rmcp::Peer::is_transport_closed);
        if stale {
            *peer_guard = Some(context.peer.clone());
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

        self.track_peer(&ctx);

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
        self.track_peer(&ctx);
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
        self.track_peer(&ctx);
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
        self.track_peer(&ctx);
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
        self.track_peer(&ctx);
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

    use axum::http::Request;
    use rmcp::model::RequestId;
    use rmcp::ServiceExt;
    use serde_json::Value;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

    use super::*;

    const LEGACY_PROTOCOL_VERSION: &str = "2025-06-18";
    const STATELESS_PROTOCOL_VERSION: &str = "2026-07-28";

    fn initialize_request(protocol_version: &str) -> String {
        format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{{\"protocolVersion\":\"{protocol_version}\",\"capabilities\":{{}},\"clientInfo\":{{\"name\":\"peer-lifecycle-test\",\"version\":\"1.0.0\"}}}}}}\n"
        )
    }

    async fn connect_peer(
        server: McpServer,
    ) -> (
        rmcp::Peer<RoleServer>,
        BufReader<DuplexStream>,
        tokio::task::JoinHandle<()>,
    ) {
        connect_peer_with_version(server, LEGACY_PROTOCOL_VERSION).await
    }

    async fn connect_peer_with_version(
        server: McpServer,
        protocol_version: &str,
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
            .write_all(initialize_request(protocol_version).as_bytes())
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

    /// Builds the filesystem example component, which needs one storage grant to run and so
    /// keeps these tests to a single permission.
    fn build_filesystem_component() -> std::path::PathBuf {
        let top_level = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let component_path =
            top_level.join("examples/filesystem-rs/target/wasm32-wasip2/release/filesystem.wasm");

        if !component_path.exists() {
            let status = std::process::Command::new("cargo")
                .current_dir(top_level.join("examples/filesystem-rs"))
                .args(["build", "--release", "--target", "wasm32-wasip2"])
                .status()
                .expect("cargo build of the filesystem example should run");
            assert!(status.success(), "filesystem example should compile");
        }

        assert!(
            component_path.exists(),
            "filesystem component should exist at {}",
            component_path.display()
        );
        component_path
    }

    /// Completes the handshake the way a real client does before it makes requests.
    async fn send_initialized(client: &mut BufReader<DuplexStream>) {
        client
            .get_mut()
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
            .await
            .expect("initialized notification should be written");
        client
            .get_mut()
            .flush()
            .await
            .expect("initialized notification should be flushed");
    }

    /// Issues one `tools/call` and returns how many `tools/list_changed` notifications the
    /// peer received before the response, together with the response itself.    ///
    /// A handler sends its notification on the same transport before returning, so every
    /// notification that belongs to the call has been written by the time the response
    /// arrives. Counting up to the response therefore counts the call's notifications
    /// exactly, without a sleep that would make the test both slow and flaky.
    async fn call_tool_counting_notifications(
        client: &mut BufReader<DuplexStream>,
        request_id: i64,
        tool_name: &str,
        arguments: Value,
    ) -> (usize, Value) {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "tools/call",
            "params": {"name": tool_name, "arguments": arguments},
        });
        client
            .get_mut()
            .write_all(format!("{request}\n").as_bytes())
            .await
            .expect("tools/call request should be written");
        client
            .get_mut()
            .flush()
            .await
            .expect("tools/call request should be flushed");

        let mut notifications = 0;
        loop {
            let mut line = String::new();
            tokio::time::timeout(Duration::from_secs(60), client.read_line(&mut line))
                .await
                .expect("timed out waiting for the tools/call response")
                .expect("server should keep writing to the transport");
            let message: Value = serde_json::from_str(&line).expect("message should be JSON");

            if message["method"] == "notifications/tools/list_changed" {
                notifications += 1;
                continue;
            }
            if message["id"] == request_id {
                return (notifications, message);
            }
        }
    }

    /// Populates a component directory the way an earlier process would have, and returns the
    /// directory, the component id, and a directory the component is allowed to read.
    async fn populated_component_dir() -> (tempfile::TempDir, tempfile::TempDir, String) {
        let component_dir =
            tempfile::tempdir().expect("temporary component directory should exist");
        let work_dir = tempfile::tempdir().expect("temporary work directory should exist");
        let component_path = build_filesystem_component();

        let manager = LifecycleManager::new_unloaded(component_dir.path())
            .await
            .expect("lifecycle manager should be created");
        let outcome = manager
            .load_component(&format!("file://{}", component_path.display()))
            .await
            .expect("filesystem component should load");
        manager
            .grant_permission(
                &outcome.component_id,
                "storage",
                &serde_json::json!({
                    "uri": format!("fs://{}", work_dir.path().display()),
                    "access": ["read"],
                }),
            )
            .await
            .expect("storage permission should be granted");

        (component_dir, work_dir, outcome.component_id)
    }

    /// A tool call can be the first thing to register its component: the tool resolves from
    /// on-disk metadata, and the background restore stays quiet about a component it finds
    /// already loaded. Nobody else is going to announce that registration, so the call that
    /// performed it must, or a client waiting on `tools/list_changed` never re-lists.
    #[tokio::test]
    async fn on_demand_component_registration_announces_the_tool_list_change_once() {
        let (component_dir, work_dir, component_id) = populated_component_dir().await;
        let probe = work_dir.path().join("probe.txt");
        tokio::fs::write(&probe, b"hello")
            .await
            .expect("probe file should be written");

        // A fresh manager over the same directory, with no restore run: exactly the state a
        // just-started server is in when the first `tools/call` arrives.
        let lifecycle_manager = LifecycleManager::new_unloaded(component_dir.path())
            .await
            .expect("lifecycle manager should be created");
        assert!(
            lifecycle_manager.list_components().await.is_empty(),
            "nothing should be registered before the first call"
        );
        let server = McpServer::new(lifecycle_manager, false, true);
        let (_peer, mut client, service) = connect_peer(server.clone()).await;
        send_initialized(&mut client).await;

        let arguments = serde_json::json!({ "path": probe.to_string_lossy() });
        let (notifications, response) =
            call_tool_counting_notifications(&mut client, 2, "file-exists", arguments.clone())
                .await;
        assert!(
            response["error"].is_null() && response["result"]["isError"] != Value::Bool(true),
            "the tool call should succeed: {response}"
        );
        assert_eq!(
            server.lifecycle_manager.list_components().await,
            vec![component_id],
            "the call should have registered the component"
        );
        assert_eq!(
            notifications, 1,
            "registering a component through a tool call changes the tool list exactly once"
        );

        let (notifications, response) =
            call_tool_counting_notifications(&mut client, 3, "file-exists", arguments).await;
        assert!(
            response["error"].is_null() && response["result"]["isError"] != Value::Bool(true),
            "the second tool call should succeed: {response}"
        );
        assert_eq!(
            notifications, 0,
            "a call that found the component already loaded changed nothing to announce"
        );

        drop(client);
        tokio::time::timeout(Duration::from_secs(5), service)
            .await
            .expect("peer should close after its transport is dropped")
            .expect("peer service should shut down cleanly");
    }

    /// The explicit load already announces the change itself, so teaching the tool-call path
    /// to announce must not give it a second voice.
    #[tokio::test]
    async fn explicit_component_load_announces_the_tool_list_change_once() {
        let component_dir =
            tempfile::tempdir().expect("temporary component directory should exist");
        let component_path = build_filesystem_component();
        let lifecycle_manager = LifecycleManager::new_unloaded(component_dir.path())
            .await
            .expect("lifecycle manager should be created");
        let server = McpServer::new(lifecycle_manager, false, true);
        let (_peer, mut client, service) = connect_peer(server.clone()).await;
        send_initialized(&mut client).await;

        let arguments =
            serde_json::json!({ "path": format!("file://{}", component_path.display()) });
        let (notifications, response) =
            call_tool_counting_notifications(&mut client, 2, "load-component", arguments).await;
        assert!(
            response["error"].is_null() && response["result"]["isError"] != Value::Bool(true),
            "the load should succeed: {response}"
        );
        assert_eq!(
            notifications, 1,
            "an explicit load announces the change once, not once per path that could"
        );

        drop(client);
        tokio::time::timeout(Duration::from_secs(5), service)
            .await
            .expect("peer should close after its transport is dropped")
            .expect("peer service should shut down cleanly");
    }

    fn http_request_context(
        peer: rmcp::Peer<RoleServer>,
        request_id: i64,
        session_id: Option<&str>,
    ) -> RequestContext<RoleServer> {
        let mut request = Request::new(());
        if let Some(session_id) = session_id {
            request.headers_mut().insert(
                MCP_SESSION_ID_HEADER,
                session_id
                    .parse()
                    .expect("session ID should be a valid header"),
            );
        }
        let (parts, ()) = request.into_parts();
        let mut context = RequestContext::new(RequestId::Number(request_id), peer);
        context.extensions.insert(parts);
        context
    }

    /// rmcp never strips `Mcp-Session-Id`, and it routes a `2026-07-28` client
    /// statelessly no matter what that header says. Trusting the header alone
    /// would therefore adopt a request-scoped peer and let background loading
    /// inject an unsolicited notification into an ordinary response.
    #[tokio::test]
    async fn stateless_request_with_a_stale_session_header_is_not_tracked() {
        let temp_dir = tempfile::tempdir().expect("temporary component directory should exist");
        let lifecycle_manager = LifecycleManager::new(temp_dir.path())
            .await
            .expect("lifecycle manager should be created");
        let server = McpServer::new(lifecycle_manager, false, true);

        let (peer, client, service) =
            connect_peer_with_version(server.clone(), STATELESS_PROTOCOL_VERSION).await;
        let context = http_request_context(peer, 1, Some("left-over-session"));
        server
            .list_tools(None, context)
            .await
            .expect("stateless tools/list should succeed");

        assert!(
            server.get_peer().is_none(),
            "a stateless request must not be tracked just because it carried a session header"
        );

        drop(client);
        tokio::time::timeout(Duration::from_secs(5), service)
            .await
            .expect("peer should close after its transport is dropped")
            .expect("peer service should shut down cleanly");
    }

    /// With `--legacy-sessions=false` rmcp serves every request statelessly, so
    /// no HTTP peer outlives its request even when a client still sends the
    /// session header it obtained before the flag was flipped.
    #[tokio::test]
    async fn no_http_peer_is_tracked_when_legacy_sessions_are_disabled() {
        let temp_dir = tempfile::tempdir().expect("temporary component directory should exist");
        let lifecycle_manager = LifecycleManager::new(temp_dir.path())
            .await
            .expect("lifecycle manager should be created");
        let server = McpServer::new(lifecycle_manager, false, false);

        let (peer, client, service) = connect_peer(server.clone()).await;
        let context = http_request_context(peer, 1, Some("session-from-before"));
        server
            .list_tools(None, context)
            .await
            .expect("tools/list should succeed");

        assert!(
            server.get_peer().is_none(),
            "no HTTP peer is persistent once the session lifecycle is disabled"
        );

        drop(client);
        tokio::time::timeout(Duration::from_secs(5), service)
            .await
            .expect("peer should close after its transport is dropped")
            .expect("peer service should shut down cleanly");
    }

    #[tokio::test]
    async fn only_session_http_requests_track_their_peer() {
        let temp_dir = tempfile::tempdir().expect("temporary component directory should exist");
        let lifecycle_manager = LifecycleManager::new(temp_dir.path())
            .await
            .expect("lifecycle manager should be created");
        let server = McpServer::new(lifecycle_manager, false, true);

        let (stateless_peer, stateless_client, stateless_service) =
            connect_peer(server.clone()).await;
        let stateless_context = http_request_context(stateless_peer, 1, None);
        server
            .list_tools(None, stateless_context)
            .await
            .expect("stateless tools/list should succeed");
        assert!(
            server.get_peer().is_none(),
            "a request-scoped stateless peer must not be retained"
        );

        let (session_peer, mut session_client, session_service) =
            connect_peer(server.clone()).await;
        let session_context = http_request_context(session_peer, 2, Some("legacy-session"));
        server
            .list_tools(None, session_context)
            .await
            .expect("session tools/list should succeed");
        assert!(
            server.get_peer().is_some(),
            "a legacy session peer should be retained"
        );
        server.publish_tool_list_changed();
        expect_tool_list_changed(&mut session_client).await;

        drop(stateless_client);
        drop(session_client);
        for service in [stateless_service, session_service] {
            tokio::time::timeout(Duration::from_secs(5), service)
                .await
                .expect("peer should close after its transport is dropped")
                .expect("peer service should shut down cleanly");
        }
    }

    /// Startup loading can publish before a peer exists, while a disconnected
    /// persistent peer can remain cached. Neither state should suppress
    /// notifications to subscriptions or the next live peer.
    #[tokio::test]
    async fn publish_tool_list_changed_handles_peer_lifecycle() {
        let temp_dir = tempfile::tempdir().expect("temporary component directory should exist");
        let lifecycle_manager = LifecycleManager::new(temp_dir.path())
            .await
            .expect("lifecycle manager should be created");
        let server = McpServer::new(lifecycle_manager, false, true);
        let mut subscription = server.subscribe_tool_list_changed();

        assert!(server.get_peer().is_none());
        server.publish_tool_list_changed();
        subscription
            .recv()
            .await
            .expect("peerless publication should still reach subscriptions");

        let (first_peer, mut first_client, first_service) = connect_peer(server.clone()).await;
        server.track_peer(&RequestContext::new(
            RequestId::Number(1),
            first_peer.clone(),
        ));
        server.publish_tool_list_changed();
        expect_tool_list_changed(&mut first_client).await;

        drop(first_client);
        tokio::time::timeout(Duration::from_secs(5), first_service)
            .await
            .expect("first peer should close after its transport is dropped")
            .expect("first peer service should shut down cleanly");
        assert!(first_peer.is_transport_closed());

        let (second_peer, mut second_client, second_service) = connect_peer(server.clone()).await;
        server.track_peer(&RequestContext::new(
            RequestId::Number(2),
            second_peer.clone(),
        ));
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
