// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! MCP Server implementation for handling WebAssembly components

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use mcp_server::tools::is_builtin_tool;
use mcp_server::{
    handle_prompts_list, handle_resources_list, handle_tools_call, handle_tools_list,
    LifecycleManager,
};
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, ErrorData, ListPromptsResult,
    ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ProtocolVersion, ServerCapabilities, ServerConfig, ServerNotification, SubscriptionFilter,
    ToolListChangedNotification,
};
use rmcp::service::{RequestContext, RoleServer, SubscriptionContext, SubscriptionSendError};
use rmcp::ServerHandler;
use tokio::sync::{broadcast, Mutex as AsyncMutex};
use wassette::{CatalogGeneration, CatalogRefreshError, CatalogSnapshot};

/// Buffered tool-list changes per subscriber.
///
/// Each event identifies an observed generation. Lagging subscribers resume at
/// the retained generations rather than synthesizing another invalidation.
const TOOL_LIST_CHANGED_CAPACITY: usize = 16;

const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";

fn supports_cache_hints(context: &RequestContext<RoleServer>) -> bool {
    context
        .protocol_version()
        .is_some_and(|version| version >= ProtocolVersion::V_2026_07_28)
}

fn observed_generation(
    snapshot: anyhow::Result<CatalogSnapshot>,
) -> anyhow::Result<CatalogGeneration> {
    match snapshot {
        Ok(snapshot) => Ok(snapshot.generation),
        Err(error) => match error.downcast_ref::<CatalogRefreshError>() {
            Some(refresh) => {
                tracing::warn!(
                    phase = "catalog-refresh",
                    code = "catalog-unavailable",
                    "Catalog refresh published unavailable entries"
                );
                Ok(refresh.report.generation.clone())
            }
            None => Err(error),
        },
    }
}

/// A security-oriented runtime that runs WebAssembly Components via MCP.
#[derive(Clone)]
pub struct McpServer {
    lifecycle_manager: LifecycleManager,
    peer: Arc<Mutex<Option<rmcp::Peer<rmcp::RoleServer>>>>,
    disable_builtin_tools: bool,
    legacy_sessions: bool,
    tool_list_changed: broadcast::Sender<CatalogGeneration>,
    catalog_generation: Arc<AsyncMutex<CatalogGeneration>>,
    #[cfg(feature = "component-generation")]
    generation_jobs: mcp_server::generation::GenerationJobs,
}

impl McpServer {
    /// Creates a new MCP server instance with the given lifecycle manager.
    ///
    /// # Arguments
    /// * `lifecycle_manager` - The lifecycle manager for handling component operations
    /// * `disable_builtin_tools` - Whether to disable built-in tools
    /// * `legacy_sessions` - Whether the pre-`2026-07-28` session lifecycle is served
    ///
    /// The initial catalog establishes the notification baseline without
    /// invalidating it. Unavailable entries still establish a baseline so the
    /// server can accept repairs; errors without a publication fail construction.
    pub async fn new(
        lifecycle_manager: LifecycleManager,
        disable_builtin_tools: bool,
        legacy_sessions: bool,
    ) -> anyhow::Result<Self> {
        let generation = observed_generation(lifecycle_manager.catalog().await)?;
        Ok(Self {
            lifecycle_manager,
            peer: Arc::new(Mutex::new(None)),
            disable_builtin_tools,
            legacy_sessions,
            tool_list_changed: broadcast::channel(TOOL_LIST_CHANGED_CAPACITY).0,
            catalog_generation: Arc::new(AsyncMutex::new(generation)),
            #[cfg(feature = "component-generation")]
            generation_jobs: mcp_server::generation::GenerationJobs::default(),
        })
    }

    /// Retain the generation job owner until transport shutdown and helper reaping complete.
    #[cfg(feature = "component-generation")]
    pub fn generation_jobs(&self) -> mcp_server::generation::GenerationJobs {
        self.generation_jobs.clone()
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

    /// Forward in-memory catalog publications until this future is dropped.
    ///
    /// The transport owner must retain and cancel the task running this future.
    /// Waiting does not poll or refresh the shared store.
    pub async fn watch_catalog_changes(&self) {
        loop {
            let previous = self.catalog_generation.lock().await.clone();
            let generation = match self.lifecycle_manager.wait_changed(&previous).await {
                Ok(generation) => generation,
                Err(_) => {
                    tracing::error!(
                        phase = "catalog-subscription",
                        code = "subscription-failed",
                        "Catalog subscription failed"
                    );
                    return;
                }
            };
            let mut published = self.catalog_generation.lock().await;
            // A direct request may have published a newer generation while the
            // waiter was waking. Never republish its now-stale observation.
            if *published != previous {
                continue;
            }
            self.publish_catalog_generation(&mut published, generation, None)
                .await;
        }
    }

    async fn observe_catalog(
        &self,
        published: &mut CatalogGeneration,
        request_peer: Option<&rmcp::Peer<RoleServer>>,
    ) {
        match observed_generation(self.lifecycle_manager.catalog().await) {
            Ok(generation) => {
                self.publish_catalog_generation(published, generation, request_peer)
                    .await;
            }
            Err(_) => tracing::warn!(
                phase = "catalog-observation",
                code = "observation-failed",
                "Failed to observe catalog generation"
            ),
        }
    }

    async fn publish_catalog_generation(
        &self,
        published: &mut CatalogGeneration,
        generation: CatalogGeneration,
        request_peer: Option<&rmcp::Peer<RoleServer>>,
    ) {
        if *published == generation {
            return;
        }
        *published = generation.clone();
        let _ = self.tool_list_changed.send(generation);

        if let Some(peer) = request_peer {
            if let Err(error) = peer.notify_tool_list_changed().await {
                tracing::warn!("Failed to notify requesting peer of catalog change: {error}");
            }
        }
        if let Some(peer) = self.get_peer() {
            // Clones of a legacy peer share their handshake allocation. Compare
            // its identity, not client metadata (distinct clients can match).
            let already_notified = request_peer.is_some_and(|request_peer| {
                peer.peer_info()
                    .zip(request_peer.peer_info())
                    .is_some_and(|(stored, requesting)| Arc::ptr_eq(&stored, &requesting))
            });
            if !already_notified {
                if let Err(error) = peer.notify_tool_list_changed().await {
                    tracing::warn!("Failed to notify persistent peer of catalog change: {error}");
                }
            }
        }
    }

    /// Subscribe to tool-list changes for one `subscriptions/listen` stream.
    pub fn subscribe_tool_list_changed(&self) -> broadcast::Receiver<CatalogGeneration> {
        self.tool_list_changed.subscribe()
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
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::default();
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
        Box::pin(async move {
            #[cfg(feature = "component-generation")]
            if params.name == "build-component" {
                // No catalog mutex is held while the VM runs. The existing observer
                // and subscription share the same deduplication baseline after commit.
                let result = self
                    .generation_jobs
                    .call_tool(
                        params,
                        &self.lifecycle_manager,
                        disable_builtin_tools,
                        ctx.ct,
                    )
                    .await;
                let mut published = self.catalog_generation.lock().await;
                self.observe_catalog(&mut published, Some(&peer_clone))
                    .await;
                return result
                    .map(CallToolResponse::Complete)
                    .map_err(|error| ErrorData::internal_error(error.to_string(), None));
            }
            let mut published = self.catalog_generation.lock().await;
            self.observe_catalog(&mut published, None).await;
            // Lifecycle mutations run in built-in dispatch. Keep their
            // publication before the response, but do not serialize arbitrary
            // guest execution behind the notification mutex.
            let published = is_builtin_tool(params.name.as_ref()).then_some(published);
            let result =
                handle_tools_call(params, &self.lifecycle_manager, disable_builtin_tools).await;
            let mut published = match published {
                Some(published) => published,
                None => self.catalog_generation.lock().await,
            };
            self.observe_catalog(&mut published, Some(&peer_clone))
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
        self.track_peer(&ctx);
        let supports_cache_hints = supports_cache_hints(&ctx);

        let disable_builtin_tools = self.disable_builtin_tools;
        Box::pin(async move {
            let mut published = self.catalog_generation.lock().await;
            let result = handle_tools_list(&self.lifecycle_manager, disable_builtin_tools).await;
            self.observe_catalog(&mut published, None).await;
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
                            Ok(_) => {}
                            Err(broadcast::error::RecvError::Lagged(_)) => continue,
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
    use std::path::Path;
    use std::time::Duration;

    use axum::http::Request;
    use oci_client::client::{ClientConfig, ClientProtocol, Config, ImageLayer};
    use rmcp::model::{ClientCapabilities, Implementation, RequestId, RequestMetaObject};
    use rmcp::ServiceExt;
    use serde_json::Value;
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream};
    use tokio::net::TcpListener;
    use tokio::task::JoinSet;
    use wassette::wasm_directory::WasmDirectoryClient;

    use super::*;

    const LEGACY_PROTOCOL_VERSION: &str = "2025-06-18";

    fn test_root() -> tempfile::TempDir {
        tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap()
    }

    async fn component_uri(root: &Path, value: u32) -> String {
        let source_dir = root.join("source");
        tokio::fs::create_dir_all(&source_dir).await.unwrap();
        let path = source_dir.join("catalog-fixture.wasm");
        let bytes = wat::parse_str(format!(
            r#"(component $catalog-fixture
                (core module $m
                    (func (export "run") (result i32) i32.const {value}))
                (core instance $i (instantiate $m))
                (func $run (result u32) (canon lift (core func $i "run")))
                (export "run" (func $run)))"#
        ))
        .unwrap();
        tokio::fs::write(&path, bytes).await.unwrap();
        format!("file://{}", path.display())
    }

    async fn package_fixture(root: &Path) -> (String, String, String, tokio::task::JoinHandle<()>) {
        let component_uri = component_uri(root, 7).await;
        let bytes = tokio::fs::read(component_uri.strip_prefix("file://").unwrap())
            .await
            .unwrap();
        let layer = ImageLayer::new(bytes, oci_wasm::WASM_LAYER_MEDIA_TYPE.into(), None);
        let config = Config::new(
            serde_json::to_vec(&serde_json::json!({
                "created": "1970-01-01T00:00:00Z",
                "architecture": oci_wasm::WASM_ARCHITECTURE,
                "os": oci_wasm::COMPONENT_OS,
                "layerDigests": [layer.sha256_digest()],
                "component": {"exports": ["run"], "imports": [], "target": null}
            }))
            .unwrap(),
            oci_wasm::WASM_MANIFEST_CONFIG_MEDIA_TYPE.into(),
            None,
        );
        let mut manifest = oci_client::manifest::OciImageManifest::build(
            std::slice::from_ref(&layer),
            &config,
            None,
        );
        manifest.media_type = Some(oci_wasm::WASM_MANIFEST_MEDIA_TYPE.into());
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(&manifest_bytes)));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let registry = listener.local_addr().unwrap().to_string();
        let package = format!("{registry}/owner/catalog-fixture");
        let api_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api_url = format!("http://{}", api_listener.local_addr().unwrap());
        let detail = serde_json::to_vec(&serde_json::json!({
            "registry": registry,
            "repository": "owner/catalog-fixture",
            "kind": "component",
            "versions": [{"tag": "1.2.3", "digest": digest}]
        }))
        .unwrap();
        let routes = std::collections::HashMap::from([
            (
                "/v2/".to_owned(),
                ("application/json".to_owned(), b"{}".to_vec()),
            ),
            (
                format!("/v2/owner/catalog-fixture/manifests/{digest}"),
                (
                    oci_wasm::WASM_MANIFEST_MEDIA_TYPE.to_owned(),
                    manifest_bytes,
                ),
            ),
            (
                format!("/v2/owner/catalog-fixture/blobs/{}", manifest.config.digest),
                (config.media_type, config.data.to_vec()),
            ),
            (
                format!("/v2/owner/catalog-fixture/blobs/{}", layer.sha256_digest()),
                (layer.media_type, layer.data.to_vec()),
            ),
        ]);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, _) = accepted.unwrap();
                        let routes = routes.clone();
                        tokio::spawn(async move {
                            reply_fixture(socket, |path| routes.get(path).cloned()).await;
                        });
                    }
                    accepted = api_listener.accept() => {
                        let (socket, _) = accepted.unwrap();
                        let detail = detail.clone();
                        tokio::spawn(async move {
                            reply_fixture(socket, |_| Some(("application/json".to_owned(), detail.clone()))).await;
                        });
                    }
                }
            }
        });
        (api_url, package, digest, task)
    }

    async fn reply_fixture(
        mut socket: tokio::net::TcpStream,
        route: impl Fn(&str) -> Option<(String, Vec<u8>)>,
    ) {
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let count = socket.read(&mut buffer).await.unwrap();
            assert!(count > 0 && request.len() + count < 8192);
            request.extend_from_slice(&buffer[..count]);
        }
        let request = String::from_utf8(request).unwrap();
        let mut words = request.split_whitespace();
        let method = words.next().unwrap();
        let path = words.next().unwrap();
        let (media_type, body) = route(path).unwrap_or_else(|| panic!("unexpected route: {path}"));
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: {media_type}\r\n\
             Docker-Content-Digest: sha256:{}\r\nConnection: close\r\n\r\n",
            body.len(),
            hex::encode(Sha256::digest(&body))
        );
        socket.write_all(headers.as_bytes()).await.unwrap();
        if method != "HEAD" {
            socket.write_all(&body).await.unwrap();
        }
    }

    #[test]
    fn registry_get_and_mcp_package_load_resolve_wit_selectors() {
        use wassette::wasm_directory::fixtures::{FixturePackage, WasmDirectoryFixture};

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let root = test_root();
        let fixture = runtime.block_on(async {
            let one = component_uri(&root.path().join("one"), 1).await;
            let two = component_uri(&root.path().join("two"), 2).await;
            let read = |uri: &str| std::fs::read(uri.strip_prefix("file://").unwrap()).unwrap();
            WasmDirectoryFixture::start(vec![FixturePackage::new(
                "owner/catalog-fixture",
                Some("demo:catalog-fixture"),
                [("1.0.0", read(&one)), ("1.2.3", read(&two))],
            )])
            .await
            .unwrap()
        });
        let api_url = fixture.api_url.to_string();
        temp_env::with_vars(
            [("WASSETTE_WASM_DIRECTORY_URL", Some(api_url.as_str()))],
            || {
                runtime.block_on(async {
                    let manager = LifecycleManager::builder(root.path().join("store"))
                        .with_secrets_dir(root.path().join("secrets"))
                        .with_eager_loading(false)
                        .with_oci_client(fixture.oci_client())
                        .build()
                        .await
                        .unwrap();
                    let directory = fixture.directory().unwrap();
                    let package = fixture.package_id("owner/catalog-fixture");

                    let installed = crate::install_registry_package(
                        &manager,
                        &directory,
                        "demo:catalog-fixture",
                        None,
                    )
                    .await
                    .unwrap();
                    assert_eq!(installed["package"], package);
                    assert_eq!(installed["wit_identity"], "demo:catalog-fixture");
                    assert_eq!(installed["selected_version"], "1.2.3");
                    assert_eq!(
                        installed["manifest_digest"],
                        fixture.digest("owner/catalog-fixture", "1.2.3")
                    );
                    assert_eq!(installed["component_id"], package);
                    assert!(manager.catalog().await.unwrap().tools.is_empty());

                    let request = CallToolRequestParams::new("load-component").with_arguments(
                        serde_json::Map::from_iter([(
                            "package".to_owned(),
                            serde_json::json!("demo:catalog-fixture@1.0.0"),
                        )]),
                    );
                    let response = handle_tools_call(request, &manager, false).await.unwrap();
                    let result: Value =
                        serde_json::from_str(response["content"][0]["text"].as_str().unwrap())
                            .unwrap();
                    assert_eq!(result["id"], package, "{result}");
                    assert_eq!(result["package"], package);
                    assert_eq!(result["selected_version"], "1.0.0");
                    assert_eq!(
                        result["manifest_digest"],
                        fixture.digest("owner/catalog-fixture", "1.0.0")
                    );
                    assert_eq!(manager.catalog().await.unwrap().tools.len(), 1);

                    let missing =
                        crate::install_registry_package(&manager, &directory, "demo:absent", None)
                            .await
                            .unwrap_err();
                    assert!(
                        format!("{missing:#}").contains("No wasm.directory component package"),
                        "{missing:#}"
                    );
                })
            },
        );
    }

    #[test]
    fn registry_get_then_mcp_package_load_exposes_one_catalog_generation() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let root = test_root();
        let (api_url, package, digest, fixture) = runtime.block_on(package_fixture(root.path()));
        temp_env::with_vars(
            [("WASSETTE_WASM_DIRECTORY_URL", Some(api_url.as_str()))],
            || {
                runtime.block_on(async {
                    let manager = LifecycleManager::builder(root.path().join("store"))
                        .with_secrets_dir(root.path().join("secrets"))
                        .with_eager_loading(false)
                        .with_oci_client(oci_client::Client::new(ClientConfig {
                            protocol: ClientProtocol::Http,
                            ..Default::default()
                        }))
                        .build()
                        .await
                        .unwrap();
                    let server = McpServer::new(manager.clone(), false, true).await.unwrap();
                    let mut receiver = server.subscribe_tool_list_changed();
                    let mut tasks = watch_catalog(&server);
                    let directory = WasmDirectoryClient::from_environment().unwrap();
                    let installed = crate::install_registry_package(
                        &manager,
                        &directory,
                        &package,
                        Some("1.2.3"),
                    )
                    .await
                    .unwrap();
                    assert_eq!(installed["status"], "installed");
                    assert_eq!(installed["package"], package);
                    assert_eq!(installed["selected_version"], "1.2.3");
                    assert_eq!(installed["manifest_digest"], digest);
                    assert_eq!(installed["component_id"], package);
                    assert_eq!(installed["storage_key"], "local_catalog-fixture");
                    assert_eq!(installed["receipt"]["intent"], "InstallOnly");
                    assert_eq!(installed["receipt"]["origin"]["selected_version"], "1.2.3");
                    assert!(installed["revision"].is_string());
                    assert!(!installed["change"].is_null());
                    assert!(server
                        .lifecycle_manager
                        .catalog()
                        .await
                        .unwrap()
                        .tools
                        .is_empty());
                    expect_no_subscription_change(&mut receiver).await;

                    let (_peer, mut client, service) = connect_peer(server.clone()).await;
                    let request = serde_json::json!({
                        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                        "params": {"name": "load-component", "arguments": {
                            "package": package, "version": "1.2.3"
                        }}
                    });
                    client
                        .get_mut()
                        .write_all(format!("{request}\n").as_bytes())
                        .await
                        .unwrap();
                    client.get_mut().flush().await.unwrap();
                    expect_tool_list_changed(&mut client).await;
                    expect_subscription_change(&mut receiver).await;
                    let mut response = String::new();
                    tokio::time::timeout(Duration::from_secs(20), client.read_line(&mut response))
                        .await
                        .unwrap()
                        .unwrap();
                    let response: Value = serde_json::from_str(&response).unwrap();
                    assert_eq!(response["id"], 2, "{response}");
                    let result: Value = serde_json::from_str(
                        response["result"]["content"][0]["text"].as_str().unwrap(),
                    )
                    .unwrap();
                    assert_eq!(result["id"], package);
                    assert_eq!(result["package"], package);
                    assert_eq!(result["selected_version"], "1.2.3");
                    assert_eq!(result["manifest_digest"], digest);
                    assert_eq!(result["storage_key"], "local_catalog-fixture");
                    assert_eq!(result["receipt"]["intent"], "ExposeTools");
                    assert_eq!(result["receipt"]["origin"]["selected_version"], "1.2.3");
                    let catalog = manager.catalog().await.unwrap();
                    assert_eq!(catalog.tools.len(), 1);
                    assert_eq!(catalog.tools[0].tool.key.component_id.as_str(), package);
                    expect_no_subscription_change(&mut receiver).await;
                    let mut extra = String::new();
                    assert!(
                        tokio::time::timeout(
                            Duration::from_millis(100),
                            client.read_line(&mut extra),
                        )
                        .await
                        .is_err(),
                        "duplicate peer notification: {extra}"
                    );

                    let list = serde_json::json!({
                        "jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {}
                    });
                    client
                        .get_mut()
                        .write_all(format!("{list}\n").as_bytes())
                        .await
                        .unwrap();
                    client.get_mut().flush().await.unwrap();
                    let mut list_response = String::new();
                    tokio::time::timeout(
                        Duration::from_secs(20),
                        client.read_line(&mut list_response),
                    )
                    .await
                    .unwrap()
                    .unwrap();
                    let list_response: Value = serde_json::from_str(&list_response).unwrap();
                    assert!(list_response["result"]["tools"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|tool| tool["name"] == "run"));

                    let request = serde_json::json!({
                        "jsonrpc": "2.0", "id": 4, "method": "tools/call",
                        "params": {"name": "run", "arguments": {}}
                    });
                    client
                        .get_mut()
                        .write_all(format!("{request}\n").as_bytes())
                        .await
                        .unwrap();
                    client.get_mut().flush().await.unwrap();
                    let mut invocation_response = String::new();
                    tokio::time::timeout(
                        Duration::from_secs(20),
                        client.read_line(&mut invocation_response),
                    )
                    .await
                    .unwrap()
                    .unwrap();
                    let response: Value = serde_json::from_str(&invocation_response).unwrap();
                    assert_eq!(response["id"], 4, "{response}");
                    assert_eq!(response["result"]["isError"], false, "{response}");
                    assert_eq!(response["result"]["content"][0]["text"], "7");
                    assert_eq!(
                        response["result"]["structuredContent"],
                        serde_json::json!({"result": 7})
                    );
                    tasks.shutdown().await;
                    drop(client);
                    tokio::time::timeout(Duration::from_secs(5), service)
                        .await
                        .unwrap()
                        .unwrap();
                });
            },
        );
        fixture.abort();
    }

    async fn publish_component(server: &McpServer, root: &Path, value: u32) {
        let uri = component_uri(root, value).await;
        server.lifecycle_manager.load_component(&uri).await.unwrap();
        let mut published = server.catalog_generation.lock().await;
        server.observe_catalog(&mut published, None).await;
    }

    fn watch_catalog(server: &McpServer) -> JoinSet<()> {
        let mut tasks = JoinSet::new();
        let server = server.clone();
        tasks.spawn(async move { server.watch_catalog_changes().await });
        tasks
    }

    async fn expect_subscription_change(receiver: &mut broadcast::Receiver<CatalogGeneration>) {
        tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .expect("catalog publication should wake subscribers")
            .expect("subscription should remain open");
    }

    async fn expect_no_subscription_change(receiver: &mut broadcast::Receiver<CatalogGeneration>) {
        assert!(
            tokio::time::timeout(Duration::from_millis(100), receiver.recv())
                .await
                .is_err(),
            "an unchanged catalog must not be invalidated again"
        );
    }

    #[cfg(feature = "component-generation")]
    #[tokio::test]
    async fn rejected_generation_does_not_publish_catalog_notifications() {
        let temp_dir = test_root();
        let manager = LifecycleManager::builder(temp_dir.path())
            .with_eager_loading(false)
            .build()
            .await
            .unwrap();
        let server = McpServer::new(manager, false, true).await.unwrap();
        let mut subscription = server.subscribe_tool_list_changed();
        let mut watcher = watch_catalog(&server);
        let (peer, client, service) = connect_peer(server.clone()).await;
        let response = server
            .call_tool(
                CallToolRequestParams::new("build-component"),
                RequestContext::new(RequestId::Number(2), peer),
            )
            .await
            .unwrap();
        match response {
            CallToolResponse::Complete(result) => {
                assert_eq!(result.is_error, Some(true));
                assert_eq!(result.structured_content.unwrap()["code"], "disabled");
            }
            _ => panic!("generation returned a non-complete tool result"),
        }
        expect_no_subscription_change(&mut subscription).await;
        drop(client);
        service.await.unwrap();
        server.generation_jobs().shutdown().await;
        watcher.shutdown().await;
    }

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
            .write_all(initialize_request(LEGACY_PROTOCOL_VERSION).as_bytes())
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
        assert_eq!(
            response["result"]["protocolVersion"], LEGACY_PROTOCOL_VERSION,
            "test peer should negotiate the requested legacy version"
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
        let temp_dir = test_root();
        let lifecycle_manager = LifecycleManager::new(temp_dir.path())
            .await
            .expect("lifecycle manager should be created");
        let server = McpServer::new(lifecycle_manager, false, true)
            .await
            .unwrap();

        let (peer, client, service) = connect_peer(server.clone()).await;
        let mut context = http_request_context(peer, 1, Some("left-over-session"));
        // Stateless requests declare their version in _meta, not initialize.
        // It must take precedence over the peer's legacy handshake state.
        context.meta = RequestMetaObject::with_client_context(
            ProtocolVersion::V_2026_07_28,
            Implementation::new("peer-lifecycle-test", "1.0.0"),
            ClientCapabilities::default(),
        );
        assert_eq!(
            context.protocol_version(),
            Some(ProtocolVersion::V_2026_07_28)
        );
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
        let temp_dir = test_root();
        let lifecycle_manager = LifecycleManager::new(temp_dir.path())
            .await
            .expect("lifecycle manager should be created");
        let server = McpServer::new(lifecycle_manager, false, false)
            .await
            .unwrap();

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
        let temp_dir = test_root();
        let lifecycle_manager = LifecycleManager::new(temp_dir.path())
            .await
            .expect("lifecycle manager should be created");
        let server = McpServer::new(lifecycle_manager, false, true)
            .await
            .unwrap();

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
        publish_component(&server, temp_dir.path(), 7).await;
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
    async fn catalog_publication_handles_peer_lifecycle() {
        let temp_dir = test_root();
        let lifecycle_manager = LifecycleManager::new(temp_dir.path())
            .await
            .expect("lifecycle manager should be created");
        let server = McpServer::new(lifecycle_manager, false, true)
            .await
            .unwrap();
        let mut subscription = server.subscribe_tool_list_changed();

        assert!(server.get_peer().is_none());
        publish_component(&server, temp_dir.path(), 7).await;
        subscription
            .recv()
            .await
            .expect("peerless publication should still reach subscriptions");

        let (first_peer, mut first_client, first_service) = connect_peer(server.clone()).await;
        server.track_peer(&RequestContext::new(
            RequestId::Number(1),
            first_peer.clone(),
        ));
        publish_component(&server, temp_dir.path(), 8).await;
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
        publish_component(&server, temp_dir.path(), 9).await;
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

    #[tokio::test]
    async fn catalog_waiter_notifies_once_per_generation_and_not_for_runtime_restore() {
        let root = test_root();
        let manager = LifecycleManager::new(root.path().join("store"))
            .await
            .unwrap();
        let server = McpServer::new(manager.clone(), false, true).await.unwrap();
        let mut receiver = server.subscribe_tool_list_changed();
        let mut tasks = watch_catalog(&server);

        let uri = component_uri(root.path(), 7).await;
        manager.load_component(&uri).await.unwrap();
        expect_subscription_change(&mut receiver).await;
        let initial = manager.catalog().await.unwrap();

        assert!(!manager.refresh_from_store().await.unwrap().changed);
        assert!(!manager.resnapshot_from_store().await.unwrap().changed);
        manager
            .load_existing_components_async(None, None::<fn()>)
            .await
            .unwrap();
        assert_eq!(
            manager.catalog().await.unwrap().generation,
            initial.generation
        );
        expect_no_subscription_change(&mut receiver).await;

        let uri = component_uri(root.path(), 8).await;
        manager.load_component(&uri).await.unwrap();
        expect_subscription_change(&mut receiver).await;
        let replaced = manager.catalog().await.unwrap();
        assert_ne!(replaced.generation, initial.generation);
        assert_eq!(replaced.tools.len(), initial.tools.len());
        assert_eq!(
            replaced.tools[0].tool.schema, initial.tools[0].tool.schema,
            "a revision change must invalidate even when the tool schema is unchanged"
        );
        expect_no_subscription_change(&mut receiver).await;

        manager.unload_component("catalog-fixture").await.unwrap();
        expect_subscription_change(&mut receiver).await;
        expect_no_subscription_change(&mut receiver).await;
        tasks.shutdown().await;
    }

    #[tokio::test]
    async fn refresh_errors_retain_their_published_generation() {
        let root = test_root();
        let manager = LifecycleManager::new(root.path().join("store"))
            .await
            .unwrap();
        let mut report = manager.refresh_from_store().await.unwrap();
        let generation = report.generation.clone();
        report.diagnostics.push(wassette::CatalogDiagnostic {
            component_id: None,
            message: "Unavailable test entry".to_string(),
        });
        let error = anyhow::Error::new(CatalogRefreshError { report });
        assert_eq!(
            observed_generation(Err(error)).unwrap(),
            generation,
            "unavailability can be published even when refreshing fails"
        );
        assert!(observed_generation(Err(anyhow::anyhow!("store could not be read"))).is_err());
    }

    #[tokio::test]
    async fn catalog_waiter_requires_explicit_external_refresh_and_stops_with_owner() {
        let root = test_root();
        let store = root.path().join("store");
        let manager = LifecycleManager::new(&store).await.unwrap();
        let server = McpServer::new(manager.clone(), false, true).await.unwrap();
        let mut receiver = server.subscribe_tool_list_changed();
        let mut tasks = watch_catalog(&server);
        let writer = LifecycleManager::new(&store).await.unwrap();
        let uri = component_uri(root.path(), 7).await;
        writer.load_component(&uri).await.unwrap();

        expect_no_subscription_change(&mut receiver).await;
        assert!(manager.refresh_from_store().await.unwrap().changed);
        expect_subscription_change(&mut receiver).await;
        assert!(!manager.refresh_from_store().await.unwrap().changed);
        expect_no_subscription_change(&mut receiver).await;

        let uri = component_uri(root.path(), 8).await;
        writer.load_component(&uri).await.unwrap();
        assert!(manager.resnapshot_from_store().await.unwrap().changed);
        expect_subscription_change(&mut receiver).await;
        assert!(!manager.resnapshot_from_store().await.unwrap().changed);
        expect_no_subscription_change(&mut receiver).await;

        tasks.shutdown().await;
        assert!(tasks.is_empty());
        manager.unload_component("catalog-fixture").await.unwrap();
        expect_no_subscription_change(&mut receiver).await;
    }

    #[tokio::test]
    async fn cold_runtime_restore_does_not_invalidate_an_unchanged_catalog() {
        let root = test_root();
        let store = root.path().join("store");
        let writer = LifecycleManager::new(&store).await.unwrap();
        let uri = component_uri(root.path(), 7).await;
        writer.load_component(&uri).await.unwrap();
        let manager = LifecycleManager::builder(&store)
            .with_secrets_dir(root.path().join("secrets"))
            .with_eager_loading(false)
            .build()
            .await
            .unwrap();
        let server = McpServer::new(manager.clone(), false, true).await.unwrap();
        let mut receiver = server.subscribe_tool_list_changed();
        let mut tasks = watch_catalog(&server);
        assert!(manager.list_components().await.is_empty());
        manager
            .load_existing_components_async(None, None::<fn()>)
            .await
            .unwrap();
        expect_no_subscription_change(&mut receiver).await;
        tasks.shutdown().await;
    }

    #[tokio::test]
    async fn direct_publication_does_not_replay_a_waiters_stale_generation() {
        let root = test_root();
        let manager = LifecycleManager::new(root.path().join("store"))
            .await
            .unwrap();
        let server = McpServer::new(manager.clone(), false, true).await.unwrap();
        let mut receiver = server.subscribe_tool_list_changed();
        let mut tasks = watch_catalog(&server);
        tokio::task::yield_now().await;

        let mut published = server.catalog_generation.lock().await;
        for value in [7, 8] {
            let uri = component_uri(root.path(), value).await;
            manager.load_component(&uri).await.unwrap();
        }
        server.observe_catalog(&mut published, None).await;
        drop(published);

        expect_subscription_change(&mut receiver).await;
        expect_no_subscription_change(&mut receiver).await;
        tasks.shutdown().await;
    }

    #[tokio::test]
    async fn direct_mutations_notify_before_response_without_waiter_duplicates() {
        let root = test_root();
        let manager = LifecycleManager::new(root.path().join("store"))
            .await
            .unwrap();
        let server = McpServer::new(manager, false, true).await.unwrap();
        let mut receiver = server.subscribe_tool_list_changed();
        let mut tasks = watch_catalog(&server);
        let (_peer, mut client, service) = connect_peer(server.clone()).await;
        let uri = component_uri(root.path(), 7).await;
        let missing_uri = format!("file://{}", root.path().join("missing.wasm").display());

        for (id, name, arguments, changed, is_error) in [
            (
                2,
                "load-component",
                serde_json::json!({"path": uri}),
                true,
                false,
            ),
            (
                3,
                "load-component",
                serde_json::json!({"path": missing_uri}),
                false,
                true,
            ),
            (4, "list-components", serde_json::json!({}), false, false),
            (
                5,
                "unload-component",
                serde_json::json!({"id": "catalog-fixture"}),
                true,
                false,
            ),
        ] {
            let request = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {"name": name, "arguments": arguments},
            });
            client
                .get_mut()
                .write_all(format!("{request}\n").as_bytes())
                .await
                .unwrap();
            client.get_mut().flush().await.unwrap();
            if changed {
                expect_tool_list_changed(&mut client).await;
                expect_subscription_change(&mut receiver).await;
            }
            let mut response = String::new();
            tokio::time::timeout(Duration::from_secs(5), client.read_line(&mut response))
                .await
                .unwrap()
                .unwrap();
            let response: Value = serde_json::from_str(&response).unwrap();
            assert_eq!(response["id"], id, "{response}");
            assert_eq!(response["result"]["isError"], is_error, "{response}");
            expect_no_subscription_change(&mut receiver).await;
            let mut extra = String::new();
            assert!(
                tokio::time::timeout(Duration::from_millis(100), client.read_line(&mut extra))
                    .await
                    .is_err(),
                "one catalog generation must produce only one peer notification: {extra}"
            );
        }

        tasks.shutdown().await;
        drop(client);
        tokio::time::timeout(Duration::from_secs(5), service)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn stateless_mutation_notifies_the_caller_and_the_persistent_peer_once() {
        let root = test_root();
        let manager = LifecycleManager::new(root.path().join("store"))
            .await
            .unwrap();
        let server = McpServer::new(manager, false, true).await.unwrap();
        let mut receiver = server.subscribe_tool_list_changed();
        let mut tasks = watch_catalog(&server);
        let (persistent_peer, mut persistent_client, persistent_service) =
            connect_peer(server.clone()).await;
        server.track_peer(&RequestContext::new(
            RequestId::Number(1),
            persistent_peer.clone(),
        ));
        let (request_peer, mut request_client, request_service) =
            connect_peer(server.clone()).await;
        let context = http_request_context(request_peer, 2, None);
        let uri = component_uri(root.path(), 7).await;
        let result = server
            .call_tool(
                CallToolRequestParams::new("load-component").with_arguments(
                    serde_json::Map::from_iter([("path".to_string(), serde_json::json!(uri))]),
                ),
                context,
            )
            .await
            .unwrap();
        let CallToolResponse::Complete(result) = result else {
            panic!("loading a component should complete immediately");
        };
        assert_eq!(result.is_error, Some(false));
        expect_tool_list_changed(&mut request_client).await;
        expect_tool_list_changed(&mut persistent_client).await;
        expect_subscription_change(&mut receiver).await;
        expect_no_subscription_change(&mut receiver).await;
        assert!(Arc::ptr_eq(
            &server.get_peer().unwrap().peer_info().unwrap(),
            &persistent_peer.peer_info().unwrap(),
        ));

        tasks.shutdown().await;
        drop(request_client);
        drop(persistent_client);
        for service in [request_service, persistent_service] {
            tokio::time::timeout(Duration::from_secs(5), service)
                .await
                .unwrap()
                .unwrap();
        }
    }
}
