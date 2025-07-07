use std::time::Duration;
use std::future::Future;
use std::pin::Pin;

use rmcp::service::{RequestContext, RoleServer};
use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParam, CallToolResult, ErrorData, ListPromptsResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParamInner, ServerInfo,
};

/// A wrapper around McpServer that adds keep-alive functionality for HTTP transport
/// to prevent SSE connection timeouts after 10-15 minutes.
#[derive(Clone)]
pub struct KeepAliveServer {
    inner: crate::McpServer,
    last_activity: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl KeepAliveServer {
    pub fn new(inner: crate::McpServer) -> Self {
        let server = Self {
            inner,
            last_activity: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            )),
        };
        
        // Start the keep-alive task
        server.start_keepalive_task();
        
        server
    }
    
    fn update_activity(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        self.last_activity.store(now, std::sync::atomic::Ordering::Relaxed);
    }
    
    fn start_keepalive_task(&self) {
        let inner_clone = self.inner.clone();
        let last_activity = self.last_activity.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(300)); // 5 minutes
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            
            loop {
                interval.tick().await;
                
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                
                let last = last_activity.load(std::sync::atomic::Ordering::Relaxed);
                
                // If it's been more than 8 minutes since last activity, send a keep-alive
                if now - last > 480 {
                    if let Some(peer) = inner_clone.get_peer() {
                        // Send a minimal notification to keep the connection alive
                        let notification = serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "notifications/ping"
                        });
                        
                        if let Ok(notification_msg) = serde_json::from_value(notification) {
                            let _ = peer.send_notification(notification_msg).await;
                            tracing::debug!("Sent keep-alive ping to maintain SSE connection");
                        }
                    }
                    
                    // Update activity time to prevent constant pinging
                    last_activity.store(now, std::sync::atomic::Ordering::Relaxed);
                }
            }
        });
    }
}

#[allow(refining_impl_trait)]
impl ServerHandler for KeepAliveServer {
    fn get_info(&self) -> ServerInfo {
        self.update_activity();
        self.inner.get_info()
    }

    fn call_tool<'a>(
        &'a self,
        params: CallToolRequestParam,
        ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<CallToolResult, ErrorData>> + Send + 'a>> {
        self.update_activity();
        self.inner.call_tool(params, ctx)
    }

    fn list_tools<'a>(
        &'a self,
        params: Option<PaginatedRequestParamInner>,
        ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<ListToolsResult, ErrorData>> + Send + 'a>> {
        self.update_activity();
        self.inner.list_tools(params, ctx)
    }

    fn list_prompts<'a>(
        &'a self,
        params: Option<PaginatedRequestParamInner>,
        ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<ListPromptsResult, ErrorData>> + Send + 'a>> {
        self.update_activity();
        self.inner.list_prompts(params, ctx)
    }

    fn list_resources<'a>(
        &'a self,
        params: Option<PaginatedRequestParamInner>,
        ctx: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<ListResourcesResult, ErrorData>> + Send + 'a>> {
        self.update_activity();
        self.inner.list_resources(params, ctx)
    }

    fn get_peer(&self) -> Option<rmcp::service::Peer<RoleServer>> {
        self.inner.get_peer()
    }

    fn set_peer(&mut self, peer: rmcp::service::Peer<RoleServer>) {
        self.update_activity();
        self.inner.set_peer(peer);
    }
}