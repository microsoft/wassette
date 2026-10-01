// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Per-store host state shared across every chain stage.
//!
//! One [`Store<HostState>`] per session. Every stage in the chain
//! instantiates into that single store; they share one
//! [`ResourceTable`], one [`WasiCtx`], one outbound bridge channel.
//! This is the prerequisite for shared resources across stages and
//! removes the per-stage actor/mpsc dispatch.
//!
//! Each stage's identity is its index into [`HostState::stages`].
//! Bindgen's `add_to_linker` takes a `fn` pointer host getter — it
//! cannot capture — so we route per-stage context through a stack:
//! [`HostState::stage_stack`] is pushed before invoking a stage's
//! `bindings.wassette_acp_*().call_*(accessor, ...)` and popped after.
//! Host imports read the top of the stack to know which stage is
//! currently executing.

use std::sync::Arc;

use agent_client_protocol::Error as AcpError;
use agent_client_protocol::schema::v1 as schema;
use tokio::sync::{mpsc, oneshot};
use wasmtime::component::ResourceTable;
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};
use wasmtime_wasi_http::WasiHttpCtx;
use wasmtime_wasi_http::p2::{WasiHttpCtxView, WasiHttpView};
use wassette::ToolRef;

use crate::http_policy::HttpPolicyHooks;
use crate::secrets::SecretsRegistry;
use crate::tool_broker::{ActiveToolCalls, ToolBroker};
use crate::{Layer, Provider};

/// Events the wasm-side `client::Host` impl sends out to the bridge
/// task. The bridge is the only side with the JSON-RPC `ConnectionTo`,
/// so the host trait shoves work onto this channel and (for requests)
/// awaits the reply via a per-call oneshot.
pub enum OutboundEvent {
    /// One-way `session/update` notification. The oneshot resolves
    /// once the notification has been forwarded on the wire (or gated
    /// for later flush). The wasm-side `client.update-session` import
    /// awaits this ack so a guest can rely on the editor seeing the
    /// notification before the function returns — preserving the
    /// notification-before-response ordering callers expect when the
    /// import is awaited just before a method return.
    SessionUpdate(
        schema::SessionNotification,
        Option<bool>,
        oneshot::Sender<()>,
    ),
    /// Acknowledged after all preceding outbound events have been forwarded.
    Barrier(oneshot::Sender<()>),
    /// `fs/read_text_file` request that expects a response.
    ReadTextFile(
        schema::ReadTextFileRequest,
        oneshot::Sender<Result<schema::ReadTextFileResponse, AcpError>>,
    ),
    /// `fs/write_text_file` request that expects a response.
    WriteTextFile(
        schema::WriteTextFileRequest,
        oneshot::Sender<Result<schema::WriteTextFileResponse, AcpError>>,
    ),
    /// `session/request_permission` request that expects a response.
    RequestPermission(
        schema::RequestPermissionRequest,
        oneshot::Sender<Result<schema::RequestPermissionResponse, AcpError>>,
    ),
}

/// Which world a stage instantiates as.
#[derive(Copy, Clone, Debug)]
pub enum StageKind {
    Provider,
    Layer,
}

/// Wrapper over the two world bindings. Both expose `wassette_acp_agent`;
/// only `Layer` exposes `wassette_acp_client`.
pub enum Bindings {
    Provider(Provider),
    Layer(Layer),
}

/// Where a stage's outbound `client.*` calls go.
#[derive(Clone)]
pub enum ClientSink {
    /// Top of the chain: client calls go straight to the bridge as
    /// [`OutboundEvent`]s.
    Outbound(mpsc::Sender<OutboundEvent>),
    /// Forward into the upstream layer's exported `client` interface.
    /// Carries the upstream stage's index into [`HostState::stages`].
    Upstream(usize),
}

pub struct StageData {
    pub kind: StageKind,
    pub component_id: String,
    /// Filled after `Provider::instantiate_async` / `Layer::instantiate_async`.
    /// `None` between `HostState` creation and the post-instantiation
    /// write-back in `SessionFactory::instantiate_chain`.
    pub bindings: Option<Arc<Bindings>>,
    pub sink: ClientSink,
    /// Index of the stage one step further down the chain. `None` for
    /// the terminal provider stage.
    pub downstream_idx: Option<usize>,
}

pub struct HostState {
    pub wasi: WasiCtx,
    pub http: WasiHttpCtx,
    /// Outbound-HTTP policy for this chain. `wasi:http` requests bypass
    /// the `WasiCtx` socket permissions entirely, so the allow-list is
    /// enforced here instead (see [`crate::http_policy`]).
    pub http_hooks: HttpPolicyHooks,
    pub table: ResourceTable,
    pub stages: Vec<StageData>,
    /// Stack of stage indices currently inside a host import. The top
    /// is the stage whose component is executing now; host impls read
    /// it via [`Self::current_stage`].
    pub stage_stack: Vec<usize>,
    pub secrets: Arc<SecretsRegistry>,
    /// Downstream `session` resources stashed by [`layer_agent`] host
    /// impls. When a layer's `agent.new-session` import is invoked,
    /// the host calls the downstream stage's exported `new-session`,
    /// receives a [`wasmtime::component::ResourceAny`] tied to the
    /// downstream's resource type, stashes it here keyed by a freshly
    /// minted `u32`, and returns a typed `Resource<Session>` whose
    /// `rep` is that key. Cross-instance resource transfer trips
    /// wasmtime's type-identity check; the indirection sidesteps it.
    pub downstream_sessions: std::collections::HashMap<u32, wasmtime::component::ResourceAny>,
    /// Monotonic counter for keys in [`Self::downstream_sessions`].
    pub next_downstream_rep: u32,
    /// Bound editor ID for callbacks and host tool/generation calls.
    /// Multi-provider chains bind before creation; a single provider binds
    /// once its native ID is known. `None` means no session binding yet.
    pub editor_session_id: Option<String>,
    pub provider_routing: Option<ProviderRouting>,
    /// Whether the host may execute terminal (CLI) commands on behalf of
    /// the guest via the `client.terminal` resource. Driven by the
    /// host-owned `terminal` boolean session config option (default
    /// `false`); toggled per session through `session/set_config_option`
    /// (see [`crate::group`]). When `false` the host refuses to spawn any
    /// process and surfaces that to the guest.
    pub terminal_enabled: bool,
    /// Explicitly exposed ordinary Wassette tools.
    pub tool_broker: Option<Arc<ToolBroker>>,
    /// Editor permission decisions, scoped to this session and exact revision.
    pub tool_decisions: Vec<(ToolRef, bool)>,
    pub(crate) active_tool_calls: ActiveToolCalls,
}

/// Per-chain routing, never shared with another provider's store.
pub struct ProviderRouting {
    pub namespace: String,
    pub active: bool,
    pub commands: Option<crate::wassette::acp::prompts::SessionUpdate>,
    pub initial_updates: Option<Vec<crate::wassette::acp::prompts::SessionUpdate>>,
}

impl ProviderRouting {
    pub fn tool_call_id(&self, id: &str) -> String {
        format!("{}:{id}", self.namespace)
    }

    pub fn update(&mut self, update: &mut crate::wassette::acp::prompts::SessionUpdate) -> bool {
        use crate::wassette::acp::prompts::SessionUpdate;
        match update {
            SessionUpdate::AvailableCommandsUpdate(_) => self.commands = Some(update.clone()),
            SessionUpdate::ToolCall(call) | SessionUpdate::ToolCallUpdate(call) => {
                call.id = self.tool_call_id(&call.id);
            }
            _ => {}
        }
        if let Some(updates) = &mut self.initial_updates {
            if updates.len() < 64 {
                updates.push(update.clone());
            } else {
                tracing::warn!("dropping excess provider creation notification");
            }
            return false;
        }
        self.active
    }
}

impl HostState {
    /// Index of the stage currently executing (top of the stack).
    pub fn current_idx(&self) -> usize {
        *self
            .stage_stack
            .last()
            .expect("stage_stack is empty: a host import fired without a push first")
    }

    pub fn current_stage(&self) -> &StageData {
        let idx = self.current_idx();
        &self.stages[idx]
    }

    pub fn push_stage(&mut self, idx: usize) {
        self.stage_stack.push(idx);
    }

    pub fn pop_stage(&mut self) {
        let _ = self.stage_stack.pop();
    }

    /// Stash a downstream `ResourceAny` returned by the next stage's
    /// `agent.new-session` (or load/resume) so it can be retrieved when
    /// the layer-imported `Resource<Session>` is later dropped or
    /// re-used. Returns the synthetic `u32` rep used as the layer's
    /// resource handle.
    pub fn stash_downstream_session(&mut self, any: wasmtime::component::ResourceAny) -> u32 {
        let rep = self.next_downstream_rep;
        self.next_downstream_rep = self.next_downstream_rep.wrapping_add(1);
        self.downstream_sessions.insert(rep, any);
        rep
    }

    /// Remove and return a stashed downstream session by rep.
    pub fn take_downstream_session(
        &mut self,
        rep: u32,
    ) -> Option<wasmtime::component::ResourceAny> {
        self.downstream_sessions.remove(&rep)
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for HostState {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: &mut self.http_hooks,
        }
    }
}

impl wasmtime_wasi_http::p3::WasiHttpView for HostState {
    fn http(&mut self) -> wasmtime_wasi_http::p3::WasiHttpCtxView<'_> {
        wasmtime_wasi_http::p3::WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: &mut self.http_hooks,
        }
    }
}

impl crate::wassette::acp::errors::Host for HostState {}
impl crate::wassette::acp::sessions::Host for HostState {}
impl crate::wassette::acp::content::Host for HostState {}
impl crate::wassette::acp::terminals::Host for HostState {}
impl crate::wassette::acp::tools::Host for HostState {}
impl crate::wassette::acp::prompts::Host for HostState {}
impl crate::wassette::acp::filesystem::Host for HostState {}
impl crate::wassette::acp::init::Host for HostState {}
impl crate::wassette::component_tools::tools::Host for HostState {}

#[cfg(test)]
mod routing_tests {
    use super::*;
    use crate::wassette::acp::content::{ContentBlock, TextContent};
    use crate::wassette::acp::prompts::SessionUpdate;

    #[test]
    fn creation_updates_are_bounded_and_inactive_updates_are_not_forwarded() {
        let mut route = ProviderRouting {
            namespace: "provider-1".into(),
            active: false,
            commands: None,
            initial_updates: Some(Vec::new()),
        };
        for _ in 0..100 {
            let mut update = SessionUpdate::AgentMessageChunk(ContentBlock::Text(TextContent {
                text: "creation".into(),
            }));
            assert!(!route.update(&mut update));
        }
        assert_eq!(route.initial_updates.take().unwrap().len(), 64);
        let mut commands = SessionUpdate::AvailableCommandsUpdate(Vec::new());
        assert!(!route.update(&mut commands));
        assert!(route.commands.is_some());
        route.active = true;
        assert!(route.update(&mut commands));
        assert_eq!(route.tool_call_id("collision"), "provider-1:collision");
        assert_ne!(
            route.tool_call_id("provider-0:collision"),
            "provider-0:collision"
        );
    }
}
