// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Wasm chain lifecycle and per-session ownership.
//!
//! Every ACP session owns one [`Session`]: a single
//! [`Store<HostState>`] hosting *all* chain stages (the provider plus
//! any layers). A [`tokio::sync::Mutex`] around the store serialises
//! top-level entry points per session. Narrowly typed access controls are
//! serviced inside the active call without reacquiring that mutex, while
//! concurrency *within* a single
//! `run_concurrent` call is provided by wasmtime's async component
//! model.
//!
//! Calls into the chain head are direct:
//!
//! ```ignore
//! store.lock().await.run_concurrent(async |a| {
//!     a.with(|x| x.get().push_stage(head_idx));
//!     let res = head_bindings.wassette_acp_agent().call_X(a, req).await;
//!     a.with(|x| x.get().pop_stage());
//!     res
//! }).await
//! ```
//!
//! Layers' `agent` imports forward downstream by directly invoking the
//! next stage's bindings on the same store (no mpsc, no oneshot — see
//! [`layer_agent::HostWithStore`] below). Same for client-direction
//! upstream forwarding (see [`crate::client_impl`]).
//!
//! `initialize` first uses a throwaway instance for the wire response;
//! each session's fresh chain receives the same request before any
//! session call. `authenticate` uses a throwaway instance.

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll};

use anyhow::{Context, Result};
use futures_concurrency::future::Race;
use tokio::sync::{mpsc, oneshot, watch};
use wasmtime::component::{
    Component, Destination, HasSelf, Linker, Resource, ResourceAny, ResourceTable, StreamProducer,
    StreamReader, StreamResult, VecBuffer,
};
use wasmtime::{Engine, Store, StoreContextMut};
use wasmtime_wasi_http::WasiHttpCtx;

use crate::http_policy::HttpPolicyHooks;
use crate::install::Resolver;
use crate::sandbox::{ChainSandbox, Sandbox};
use crate::secrets::SecretsRegistry;
use crate::secrets_impl::{add_labeled_secrets_to_linker, check_labeled_secrets, labeled_secrets};
use crate::state::{Bindings, ClientSink, HostState, OutboundEvent, StageData, StageKind};
use crate::tool_broker::ToolBroker;
use crate::wassette::acp::errors::Error;
use crate::wassette::acp::init::{AuthenticateRequest, InitializeRequest, InitializeResponse};
use crate::wassette::acp::prompts::PromptResponse;
use crate::wassette::acp::sessions::{
    LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse,
};
use crate::{Layer, Provider};

// -----------------------------------------------------------------------------
// Factory
// -----------------------------------------------------------------------------

/// One stage in the routing chain: a pre-loaded wasm `Component` plus its
/// semantic identity for routing, and separate receipt-bound private storage.
#[derive(Clone)]
pub struct Stage {
    pub component: Component,
    pub component_id: String,
    pub storage_key: ::wassette::StorageKey,
    pub snapshot: Arc<::wassette::store::ArtifactSnapshot>,
    /// Capabilities this stage is granted, from its Wassette policy (or
    /// `--allow-all`). See [`crate::sandbox`].
    pub sandbox: Sandbox,
}

/// Produces fresh single-store chains on demand. Cheap: instantiation
/// from pre-loaded `Component`s is microseconds per stage.
pub struct SessionFactory {
    engine: Engine,
    /// Terminal provider stages. Every one is the bottom of its own
    /// chain; each session instantiates one chain per provider. Always
    /// non-empty (the CLI discovers installed providers at startup).
    providers: Vec<Stage>,
    /// Layer stages, ordered editor-side → provider-side. Empty means no
    /// layers (legacy single-component behaviour). The same layer stack
    /// wraps every provider.
    layers: Vec<Stage>,
    outbound: mpsc::Sender<OutboundEvent>,
    data_root: PathBuf,
    secrets: Arc<SecretsRegistry>,
    allow_shared_grants: bool,
    /// Resolves component references for the host-side `/install` slash
    /// command against the Wassette component directory.
    resolver: Arc<Resolver>,
    tool_broker: Arc<ToolBroker>,
    build_info: crate::BuildInfo,
    /// Whether the client advertised support for boolean session config
    /// options (`session.configOptions.boolean` in `initialize`). Read
    /// when building `session/new` and `session/load` responses to decide
    /// whether to advertise the host-owned `terminal` toggle (per the ACP
    /// boolean-config-option RFD, agents MUST NOT send `type: "boolean"`
    /// options to clients that didn't opt in). Defaults to `false` until
    /// `initialize` runs.
    boolean_config_supported: std::sync::atomic::AtomicBool,
    load_session_supported: std::sync::atomic::AtomicBool,
    initialize_request: tokio::sync::RwLock<Option<InitializeRequest>>,
    next_editor_session: std::sync::atomic::AtomicU64,
}

impl SessionFactory {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        engine: Engine,
        providers: Vec<Stage>,
        layers: Vec<Stage>,
        outbound: mpsc::Sender<OutboundEvent>,
        data_root: PathBuf,
        secrets: Arc<SecretsRegistry>,
        resolver: Arc<Resolver>,
        tool_broker: Arc<ToolBroker>,
        build_info: crate::BuildInfo,
    ) -> Self {
        assert!(!providers.is_empty(), "SessionFactory needs >= 1 provider");
        Self {
            engine,
            providers,
            layers,
            outbound,
            data_root,
            secrets,
            resolver,
            tool_broker,
            build_info,
            allow_shared_grants: false,
            boolean_config_supported: std::sync::atomic::AtomicBool::new(false),
            load_session_supported: std::sync::atomic::AtomicBool::new(false),
            initialize_request: tokio::sync::RwLock::new(None),
            next_editor_session: std::sync::atomic::AtomicU64::new(1),
        }
    }

    /// Mount the provider's persistent data for layers only with explicit opt-in.
    pub fn with_shared_provider_data(mut self, allow_shared_grants: bool) -> Self {
        self.allow_shared_grants = allow_shared_grants;
        self
    }

    pub fn dynamic_tool_exposure_allowed(&self) -> bool {
        self.layers.is_empty() || self.allow_shared_grants
    }

    pub fn build_info(&self) -> crate::BuildInfo {
        self.build_info
    }

    /// Resolver for the Wassette component directory, used by the
    /// host-side `/install` slash command.
    pub fn resolver(&self) -> &Arc<Resolver> {
        &self.resolver
    }

    /// Whether the client advertised support for boolean session config
    /// options during `initialize`. Gates the host-owned `terminal`
    /// toggle.
    pub fn boolean_config_supported(&self) -> bool {
        self.boolean_config_supported
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Whether the builder image enables guest component generation
    /// (build and install). Used only to tell the Copilot provider whether
    /// to advertise its model-facing build tool; the host still checks
    /// every request.
    pub fn generation_available(&self) -> bool {
        self.tool_broker.generation_available()
    }

    /// Record whether the client advertised `session.configOptions.boolean`.
    /// Called once from the `initialize` handler; sessions created
    /// afterwards read it back via [`Self::boolean_config_supported`].
    pub fn set_boolean_config_supported(&self, supported: bool) {
        self.boolean_config_supported
            .store(supported, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn load_session_supported(&self) -> bool {
        self.load_session_supported
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set_load_session_supported(&self, supported: bool) {
        self.load_session_supported
            .store(supported, std::sync::atomic::Ordering::Relaxed);
    }

    /// Remember the successful connection initialization for fresh session chains.
    pub async fn set_initialize_request(&self, req: InitializeRequest) {
        *self.initialize_request.write().await = Some(req);
    }

    /// Build a session with no `/data` preopen for single-provider authentication.
    pub async fn instantiate(&self) -> Result<Session> {
        self.instantiate_chain(&self.providers[0], None).await
    }

    pub fn is_multi_provider(&self) -> bool {
        self.providers.len() > 1
    }

    pub fn allocate_editor_session_id(&self) -> String {
        let id = self
            .next_editor_session
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("wassette-{id}")
    }

    /// Initialize every configured chain; only advertise connection-wide
    /// capabilities that all providers can actually honour.
    pub async fn initialize(
        &self,
        req: InitializeRequest,
    ) -> Result<Result<InitializeResponse, Error>> {
        let mut responses = Vec::with_capacity(self.providers.len());
        for (index, provider) in self.providers.iter().enumerate() {
            let session = self.instantiate_chain(provider, None).await?;
            if self.is_multi_provider() {
                session.configure_provider_route(index).await;
            }
            let response = match session.call_initialize(req.clone()).await? {
                Ok(response) => response,
                Err(error) => return Ok(Err(error)),
            };
            responses.push(response);
        }
        let mut response = responses.remove(0);
        for other in responses {
            anyhow::ensure!(
                other.protocol_version == response.protocol_version,
                "providers negotiated different ACP protocol versions"
            );
            response.agent_capabilities.mcp_capabilities.http &=
                other.agent_capabilities.mcp_capabilities.http;
            response.agent_capabilities.mcp_capabilities.sse &=
                other.agent_capabilities.mcp_capabilities.sse;
        }
        if self.is_multi_provider() {
            // A group has multiple independent persisted histories, not one
            // provider-native ID that can safely be passed to every provider.
            response.agent_capabilities.load_session = false;
            response.auth_methods.clear();
            response.agent_info = Some(crate::wassette::acp::init::ImplementationInfo {
                name: "wassette-acp".to_string(),
                title: Some(format!(
                    "Wassette ({})",
                    self.providers
                        .iter()
                        .map(|p| p.component_id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
                version: env!("CARGO_PKG_VERSION").to_string(),
            });
        }
        Ok(Ok(response))
    }

    /// Component id of the first provider. Used by the bridge to label
    /// session modes on the legacy (non-config-option) path.
    pub fn component_id(&self) -> &str {
        &self.providers[0].component_id
    }

    /// Shared wasmtime [`Engine`].
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub(crate) fn close_tool_jobs(&self) {
        self.tool_broker.workers.close();
    }

    pub(crate) async fn drain_tool_jobs(&self) -> Result<()> {
        self.tool_broker
            .workers
            .shutdown()
            .await
            .map_err(|_| anyhow::anyhow!("ACP tool supervisor failed during shutdown"))
    }

    pub async fn flush_outbound(&self) -> Result<()> {
        let (ack, wait) = tokio::sync::oneshot::channel();
        self.outbound
            .send(crate::state::OutboundEvent::Barrier(ack))
            .await
            .context("sending outbound barrier")?;
        wait.await.context("waiting for outbound barrier")?;
        Ok(())
    }

    /// Build one chain per loaded provider, each with `/data` preopened
    /// to a project-scoped subdir, returning them paired with their
    /// provider component id (load order preserved). The caller groups
    /// these into a single ACP session (see [`crate::group`]).
    pub async fn instantiate_group_for_project(
        &self,
        cwd: &std::path::Path,
        editor_session_id: Option<&str>,
    ) -> Result<Vec<(String, Session)>> {
        let project_id = project_id_from_cwd(cwd);
        let project_dir = self.data_root.join(&project_id);
        update_project_meta(&project_dir, cwd);
        let initialize_request = self
            .initialize_request
            .read()
            .await
            .clone()
            .context("session requested before initialize")?;
        let mut out = Vec::with_capacity(self.providers.len());
        for (index, provider) in self.providers.iter().enumerate() {
            let session = self
                .instantiate_chain(provider, Some(&project_dir))
                .await
                .with_context(|| format!("instantiating provider `{}`", provider.component_id))?;
            if let Some(id) = editor_session_id {
                session.set_editor_session_id(id.to_string()).await;
            }
            if self.is_multi_provider() {
                session.configure_provider_route(index).await;
            }
            session
                .call_initialize(initialize_request.clone())
                .await
                .map_err(|e| {
                    anyhow::anyhow!("initializing provider `{}`: {e:#}", provider.component_id)
                })?
                .map_err(|e| {
                    anyhow::anyhow!("initializing provider `{}`: {e:?}", provider.component_id)
                })?;
            out.push((provider.component_id.clone(), session));
        }
        Ok(out)
    }

    /// Build the chain for one provider into a single shared store.
    ///
    /// Wiring (indices into `HostState::stages`):
    /// - `0` = provider (bottom).
    /// - `1..=layers.len()` = layers, bottom-up (`stages.last()` is the
    ///   chain head — the outermost layer or the provider when no
    ///   layers are configured).
    ///
    /// The provider stage's `downstream_idx = None`; each layer's
    /// `downstream_idx = Some(i - 1)`. Sinks: the outermost stage's
    /// sink is `Outbound`; every other stage's sink is
    /// `Upstream(i + 1)` (one step closer to the editor).
    async fn instantiate_chain(
        &self,
        provider: &Stage,
        project_dir: Option<&std::path::Path>,
    ) -> Result<Session> {
        let stage_count = self.layers.len() + 1;
        let head_idx = stage_count - 1;

        // Pre-allocate stage metadata with bindings=None.
        let mut stages: Vec<StageData> = Vec::with_capacity(stage_count);
        // Stage 0: provider.
        stages.push(StageData {
            kind: StageKind::Provider,
            component_id: provider.component_id.clone(),
            bindings: None,
            sink: if self.layers.is_empty() {
                ClientSink::Outbound(self.outbound.clone())
            } else {
                ClientSink::Upstream(1)
            },
            downstream_idx: None,
        });
        // Layers, bottom-up.
        for (i, layer) in self.layers.iter().rev().enumerate() {
            let idx = i + 1;
            let sink = if idx == stage_count - 1 {
                ClientSink::Outbound(self.outbound.clone())
            } else {
                ClientSink::Upstream(idx + 1)
            };
            stages.push(StageData {
                kind: StageKind::Layer,
                component_id: layer.component_id.clone(),
                bindings: None,
                sink,
                downstream_idx: Some(idx - 1),
            });
        }

        // A store has one `WasiCtx`, and a chain is one store, so the
        // stages' policy grants are unioned. Do not mount the provider's
        // persistent data into a non-opted-in chain where layers could read it.
        let provider_data = crate::data::stage_data_dir(
            project_dir,
            &provider.storage_key,
            &provider.snapshot.receipt.secret_binding()?,
            self.layers.is_empty() || self.allow_shared_grants,
        )
        .await?;
        let mut chain_sandbox = ChainSandbox::default();
        chain_sandbox.merge(&provider.sandbox);
        for layer in &self.layers {
            chain_sandbox.merge(&layer.sandbox);
        }
        let wasi = chain_sandbox
            .build_ctx(provider_data.as_deref())
            .with_context(|| {
                format!("building the sandbox for chain `{}`", provider.component_id)
            })?;

        let state = HostState {
            wasi,
            http: WasiHttpCtx::new(),
            http_hooks: HttpPolicyHooks::new(chain_sandbox.http_allowlist()),
            table: ResourceTable::new(),
            stages,
            stage_stack: Vec::with_capacity(4),
            secrets: self.secrets.clone(),
            downstream_sessions: std::collections::HashMap::new(),
            next_downstream_rep: 1,
            editor_session_id: None,
            provider_routing: None,
            terminal_enabled: false,
            tool_broker: Some(Arc::new(self.tool_broker.session_view())),
            tool_decisions: Vec::new(),
            active_tool_calls: Default::default(),
        };
        let mut store = Store::new(&self.engine, state);

        // Shared linker for every stage. Crucially, this gives every
        // component instance the *same* resource type identity for
        // `wassette:acp/agent/session`, so a `ResourceAny` returned from
        // the provider's `new-session` export can be lifted into the
        // layer's imported `session` slot via `try_into_resource`
        // without a "resource type mismatch" trap.
        let mut linker: Linker<HostState> = Linker::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
        wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)?;
        wasmtime_wasi_http::p3::add_to_linker(&mut linker)?;
        // `Layer::add_to_linker` is a superset of `Provider::add_to_linker`
        // (both worlds now import the same set since the provider also
        // imports `agent` for the `session` resource's destructor), so
        // calling it once suffices for either component shape.
        Layer::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |s| s)?;

        // Labeled `wasmcloud:secrets/secret` imports are named by each
        // component, so define them per chain once every one is known to
        // be backed by a stored secret.
        let mut labels = Vec::new();
        for stage in std::iter::once(provider).chain(&self.layers) {
            let stage_labels = labeled_secrets(&self.engine, &stage.component);
            check_labeled_secrets(&self.secrets, &stage.component_id, &stage_labels).await?;
            labels.extend(stage_labels);
        }
        add_labeled_secrets_to_linker(&mut linker, &labels)?;

        // Instantiate each stage's component against the shared linker.
        for idx in 0..stage_count {
            let kind = store.data().stages[idx].kind;
            let component = if idx == 0 {
                &provider.component
            } else {
                // `stages[1]` is the bottom-most layer (last in
                // `self.layers`, since we iterated `rev()` above).
                &self.layers[self.layers.len() - idx].component
            };
            let bindings = match kind {
                StageKind::Provider => Bindings::Provider(
                    Provider::instantiate_async(&mut store, component, &linker).await?,
                ),
                StageKind::Layer => {
                    Bindings::Layer(Layer::instantiate_async(&mut store, component, &linker).await?)
                }
            };
            store.data_mut().stages[idx].bindings = Some(Arc::new(bindings));
        }

        debug_assert_chain_wiring(store.data());

        let (cancel_tx, _cancel_rx) = watch::channel(false);
        Ok(Session::new(store, head_idx, cancel_tx))
    }
}

/// Sanity-check the chain wiring after `instantiate_chain` populates
/// stages. Catches indexing typos at build time instead of as confusing
/// test failures later.
fn debug_assert_chain_wiring(state: &HostState) {
    if !cfg!(debug_assertions) {
        return;
    }
    let mut outbound_count = 0;
    for (i, s) in state.stages.iter().enumerate() {
        assert!(s.bindings.is_some(), "stage {i} bindings missing");
        match &s.sink {
            ClientSink::Outbound(_) => outbound_count += 1,
            ClientSink::Upstream(parent) => assert!(
                *parent < state.stages.len(),
                "stage {i} upstream points out of range"
            ),
        }
        match s.kind {
            StageKind::Provider => assert!(s.downstream_idx.is_none()),
            StageKind::Layer => {
                let d = s.downstream_idx.expect("layer downstream_idx");
                assert!(d < state.stages.len());
            }
        }
    }
    assert_eq!(
        outbound_count, 1,
        "exactly one stage must have Outbound sink"
    );
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct ProjectMeta {
    cwd: String,
    first_seen: Option<String>,
    last_used: Option<String>,
}

fn update_project_meta(project_dir: &std::path::Path, cwd: &std::path::Path) {
    let meta_path = project_dir.join("meta.json");
    let canon = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| format!("{}", d.as_secs()));
    let mut meta: ProjectMeta = std::fs::read(&meta_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if meta.first_seen.is_none() {
        meta.first_seen = now.clone();
    }
    meta.last_used = now;
    meta.cwd = canon.to_string_lossy().into_owned();
    if let Ok(bytes) = serde_json::to_vec_pretty(&meta)
        && let Err(e) = std::fs::write(&meta_path, bytes)
    {
        tracing::debug!(path = %meta_path.display(), error = %e, "failed to write project meta");
    }
}

fn project_id_from_cwd(cwd: &std::path::Path) -> String {
    use std::hash::{Hash, Hasher};
    let canon = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    canon.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

// -----------------------------------------------------------------------------
// Session — direct-linked chain
// -----------------------------------------------------------------------------

/// Outcome of a `prompt` call (matches the previous actor's API so the
/// bridge handler is unchanged).
pub enum PromptOutcome {
    Done(PromptResponse),
    Cancelled,
    Wit(Error),
    Trap(wasmtime::Error),
}

pub enum SetModeOutcome {
    Done,
    Wit(Error),
    Trap(wasmtime::Error),
}

pub enum SetConfigOptionOutcome {
    Done(Vec<crate::wassette::acp::sessions::SessionConfigOption>),
    Wit(Error),
    Trap(wasmtime::Error),
}

/// One ACP session: a shared store hosting every chain stage, plus the
/// head index and a cancel watch. Cheap to clone (it's an `Arc`).
#[derive(Clone)]
pub struct Session {
    inner: Arc<SessionInner>,
}

#[derive(Clone, Copy)]
pub(crate) enum LiveConfigOption {
    AutoApprove,
    Terminal,
}

impl LiveConfigOption {
    fn id(self) -> &'static str {
        match self {
            Self::AutoApprove => "allow-all",
            Self::Terminal => crate::group::TERMINAL_CONFIG_ID,
        }
    }
}

enum LiveControl {
    Config {
        option: LiveConfigOption,
        enabled: bool,
        reply: oneshot::Sender<SetConfigOptionOutcome>,
    },
    Terminal {
        enabled: bool,
        reply: oneshot::Sender<()>,
    },
}

struct SessionInner {
    /// `tokio::sync::Mutex` so top-level entry points serialise per
    /// session without blocking the runtime worker thread.
    store: tokio::sync::Mutex<Store<HostState>>,
    head_idx: usize,
    cancel: watch::Sender<bool>,
    active_tool_calls: crate::tool_broker::ActiveToolCalls,
    /// Owned chain-head `session` resource handle, populated after
    /// `new-session` / `load-session` / `resume-session`. Held inside
    /// the `Store`, so it (and any downstream resources transitively
    /// owned by guest-side wrappers like `LayerSession.downstream`)
    /// are cleaned up when this `SessionInner` drops: `Store::drop`
    /// walks the resource table firing every wasm-side destructor.
    head_session: Mutex<Option<ResourceAny>>,
    live_config_supported: bool,
    controls: mpsc::Sender<LiveControl>,
    incoming_controls: tokio::sync::Mutex<mpsc::Receiver<LiveControl>>,
}

impl Session {
    fn new(store: Store<HostState>, head_idx: usize, cancel: watch::Sender<bool>) -> Self {
        let active_tool_calls = store.data().active_tool_calls.clone();
        let live_config_supported = store.data().stages.len() == 1;
        let (controls, incoming_controls) = mpsc::channel(8);
        Self {
            inner: Arc::new(SessionInner {
                store: tokio::sync::Mutex::new(store),
                head_idx,
                cancel,
                active_tool_calls,
                head_session: Mutex::new(None),
                live_config_supported,
                controls,
                incoming_controls: tokio::sync::Mutex::new(incoming_controls),
            }),
        }
    }

    pub fn cancel(&self) {
        self.inner.active_tool_calls.cancel_all();
        self.inner.cancel.send_replace(true);
    }

    /// Clear cancellation when the bridge accepts a new prompt, before it spawns the call.
    pub fn prepare_prompt(&self) {
        self.inner.cancel.send_replace(false);
    }

    /// Bind editor callbacks and host tool/generation calls to this session,
    /// overriding provider-local string IDs at the outer chain boundary.
    pub async fn set_editor_session_id(&self, id: String) {
        let mut store = self.inner.store.lock().await;
        store.data_mut().editor_session_id = Some(id);
    }

    async fn configure_provider_route(&self, index: usize) {
        self.inner.store.lock().await.data_mut().provider_routing =
            Some(crate::state::ProviderRouting {
                namespace: format!("provider-{index}"),
                active: false,
                commands: None,
                initial_updates: Some(Vec::new()),
            });
    }

    pub async fn set_active(&self, active: bool) {
        let mut store = self.inner.store.lock().await;
        let state = store.data_mut();
        let Some(routing) = &mut state.provider_routing else {
            return;
        };
        let changed = routing.active != active;
        routing.active = active;
        let updates = if let Some(initial) = routing.initial_updates.take() {
            initial
        } else if changed {
            vec![routing.commands.clone().unwrap_or_else(|| {
                crate::wassette::acp::prompts::SessionUpdate::AvailableCommandsUpdate(Vec::new())
            })]
        } else {
            Vec::new()
        };
        if active
            && let Some(id) = &state.editor_session_id
            && let ClientSink::Outbound(outbound) = &state.stages[self.inner.head_idx].sink
        {
            for update in updates {
                crate::client_impl::notify_editor_session(outbound, id.clone(), update).await;
            }
        }
    }

    /// Enable or disable host-side terminal (CLI) execution for this
    /// chain. Set from the host-owned `terminal` boolean config option
    /// (see [`crate::group`]); read by the `client.terminal` host impl
    /// which refuses to spawn processes while `false`. Defaults to
    /// `false` at instantiation.
    pub async fn set_terminal_enabled(&self, enabled: bool) -> wasmtime::Result<()> {
        let (reply, response) = oneshot::channel();
        self.inner
            .controls
            .send(LiveControl::Terminal { enabled, reply })
            .await
            .map_err(|_| wasmtime::Error::msg("session control channel closed"))?;
        self.await_control(response).await
    }

    pub(crate) fn live_config_supported(&self) -> bool {
        self.inner.live_config_supported
    }

    pub(crate) async fn set_live_config_option(
        &self,
        option: LiveConfigOption,
        enabled: bool,
    ) -> SetConfigOptionOutcome {
        if !self.live_config_supported() {
            return SetConfigOptionOutcome::Trap(wasmtime::Error::msg(
                "live provider configuration requires a single-stage session",
            ));
        }
        let (reply, response) = oneshot::channel();
        if self
            .inner
            .controls
            .send(LiveControl::Config {
                option,
                enabled,
                reply,
            })
            .await
            .is_err()
        {
            return SetConfigOptionOutcome::Trap(wasmtime::Error::msg(
                "session control channel closed",
            ));
        }
        match self.await_control(response).await {
            Ok(outcome) => outcome,
            Err(error) => SetConfigOptionOutcome::Trap(error),
        }
    }

    async fn await_control<R: Send>(
        &self,
        mut response: oneshot::Receiver<R>,
    ) -> wasmtime::Result<R> {
        // An active call drives the mailbox. Otherwise, acquire the store and
        // drive it here; racing acquisition closes the idle/active handoff.
        tokio::select! {
            result = &mut response => {
                result.map_err(|_| wasmtime::Error::msg("session control interrupted during application"))
            }
            result = self.run_head(|_| Box::pin(async {})) => {
                result?;
                response.await.map_err(|_| wasmtime::Error::msg("session control interrupted during application"))
            }
        }
    }

    pub async fn tool_broker(&self) -> Option<Arc<ToolBroker>> {
        self.inner.store.lock().await.data().tool_broker.clone()
    }

    /// Run `body` inside `store.run_concurrent`, pushing/popping the
    /// chain-head stage idx around it.
    async fn run_head<F, R>(&self, body: F) -> wasmtime::Result<R>
    where
        F: for<'a> FnOnce(
                &'a wasmtime::component::Accessor<HostState, HasSelf<HostState>>,
            )
                -> std::pin::Pin<Box<dyn std::future::Future<Output = R> + Send + 'a>>
            + Send,
        R: Send,
    {
        let head_idx = self.inner.head_idx;
        let mut store = self.inner.store.lock().await;
        let mut controls = self.inner.incoming_controls.lock().await;
        store
            .run_concurrent(async move |a| {
                a.with(|mut x| x.get().push_stage(head_idx));
                let body = body(a);
                tokio::pin!(body);
                let r = loop {
                    tokio::select! {
                        biased;
                        Some(control) = controls.recv() => {
                            match control {
                                LiveControl::Terminal { enabled, reply } => {
                                    a.with(|mut x| x.get().terminal_enabled = enabled);
                                    let _ = reply.send(());
                                }
                                LiveControl::Config { option, enabled, reply } => {
                                    let session = *self.inner.head_session.lock().unwrap();
                                    let outcome = match session {
                                        Some(session) => config_outcome(
                                            call_config_option(
                                                a,
                                                head_idx,
                                                session,
                                                option.id().to_string(),
                                                if enabled { "on" } else { "off" }.to_string(),
                                            ).await,
                                        ),
                                        None => SetConfigOptionOutcome::Trap(wasmtime::Error::msg(
                                            "set-config-option called before new-session",
                                        )),
                                    };
                                    let _ = reply.send(outcome);
                                }
                            }
                        }
                        result = &mut body => break result,
                    }
                };
                a.with(|mut x| x.get().pop_stage());
                r
            })
            .await
    }

    pub async fn call_initialize(
        &self,
        req: InitializeRequest,
    ) -> wasmtime::Result<Result<InitializeResponse, Error>> {
        let head_idx = self.inner.head_idx;
        self.run_head(|a| {
            Box::pin(async move {
                let bindings = a
                    .with(|mut x| x.get().stages[head_idx].bindings.clone())
                    .expect("head bindings filled");
                match &*bindings {
                    Bindings::Provider(b) => b.wassette_acp_agent().call_initialize(a, req).await,
                    Bindings::Layer(b) => b.wassette_acp_agent().call_initialize(a, req).await,
                }
            })
        })
        .await?
    }

    pub async fn call_authenticate(
        &self,
        req: AuthenticateRequest,
    ) -> wasmtime::Result<Result<(), Error>> {
        let head_idx = self.inner.head_idx;
        self.run_head(|a| {
            Box::pin(async move {
                let bindings = a
                    .with(|mut x| x.get().stages[head_idx].bindings.clone())
                    .expect("head bindings filled");
                match &*bindings {
                    Bindings::Provider(b) => b.wassette_acp_agent().call_authenticate(a, req).await,
                    Bindings::Layer(b) => b.wassette_acp_agent().call_authenticate(a, req).await,
                }
            })
        })
        .await?
    }

    pub async fn call_new_session(
        &self,
        req: NewSessionRequest,
    ) -> wasmtime::Result<Result<NewSessionResponse, Error>> {
        let head_idx = self.inner.head_idx;
        let inner = self.inner.clone();
        self.run_head(|a| {
            Box::pin(async move {
                let bindings = a
                    .with(|mut x| x.get().stages[head_idx].bindings.clone())
                    .expect("head bindings filled");
                let raw = match &*bindings {
                    Bindings::Provider(b) => b.wassette_acp_agent().call_new_session(a, req).await,
                    Bindings::Layer(b) => b.wassette_acp_agent().call_new_session(a, req).await,
                };
                raw.map(|r| {
                    r.map(|(resource, resp)| {
                        *inner.head_session.lock().unwrap() = Some(resource);
                        resp
                    })
                })
            })
        })
        .await?
    }

    pub async fn call_load_session(
        &self,
        req: LoadSessionRequest,
    ) -> wasmtime::Result<Result<LoadSessionResponse, Error>> {
        let head_idx = self.inner.head_idx;
        let inner = self.inner.clone();
        self.run_head(|a| {
            Box::pin(async move {
                let bindings = a
                    .with(|mut x| x.get().stages[head_idx].bindings.clone())
                    .expect("head bindings filled");
                let raw = match &*bindings {
                    Bindings::Provider(b) => b.wassette_acp_agent().call_load_session(a, req).await,
                    Bindings::Layer(b) => b.wassette_acp_agent().call_load_session(a, req).await,
                };
                raw.map(|r| {
                    r.map(|(resource, resp)| {
                        *inner.head_session.lock().unwrap() = Some(resource);
                        resp
                    })
                })
            })
        })
        .await?
    }

    pub async fn set_mode(
        &self,
        mode_id: crate::wassette::acp::sessions::SessionModeId,
    ) -> SetModeOutcome {
        let head_idx = self.inner.head_idx;
        let head_session = match *self.inner.head_session.lock().unwrap() {
            Some(any) => any,
            None => {
                return SetModeOutcome::Trap(wasmtime::Error::msg(
                    "set-mode called before new-session",
                ));
            }
        };
        let res = self
            .run_head(|a| {
                Box::pin(async move {
                    let bindings = a
                        .with(|mut x| x.get().stages[head_idx].bindings.clone())
                        .expect("head bindings filled");
                    match &*bindings {
                        Bindings::Provider(b) => {
                            b.wassette_acp_agent()
                                .session()
                                .call_set_mode(a, head_session, mode_id)
                                .await
                        }
                        Bindings::Layer(b) => {
                            b.wassette_acp_agent()
                                .session()
                                .call_set_mode(a, head_session, mode_id)
                                .await
                        }
                    }
                })
            })
            .await;
        match res {
            Err(e) => SetModeOutcome::Trap(e),
            Ok(Err(e)) => SetModeOutcome::Trap(e),
            Ok(Ok(Err(e))) => SetModeOutcome::Wit(e),
            Ok(Ok(Ok(()))) => SetModeOutcome::Done,
        }
    }

    pub async fn set_config_option(
        &self,
        config_id: crate::wassette::acp::sessions::SessionConfigId,
        value: crate::wassette::acp::sessions::SessionConfigValueId,
    ) -> SetConfigOptionOutcome {
        let head_idx = self.inner.head_idx;
        let head_session = match *self.inner.head_session.lock().unwrap() {
            Some(any) => any,
            None => {
                return SetConfigOptionOutcome::Trap(wasmtime::Error::msg(
                    "set-config-option called before new-session",
                ));
            }
        };
        let res = self
            .run_head(|a| {
                Box::pin(async move {
                    call_config_option(a, head_idx, head_session, config_id, value).await
                })
            })
            .await;
        match res {
            Ok(result) => config_outcome(result),
            Err(error) => SetConfigOptionOutcome::Trap(error),
        }
    }
    /// Dropping the prompt future on cancel releases the store lock and
    /// wasmtime cancels any in-flight component tasks.
    ///
    /// All `session-update`s flow over `client.notify-session`
    /// (see [`crate::client_impl`]); this call only returns the
    /// terminal `prompt-response`.
    pub async fn prompt(
        &self,
        _session_id: String,
        prompt: Vec<crate::wassette::acp::content::ContentBlock>,
    ) -> PromptOutcome {
        let mut cancel_rx = self.inner.cancel.subscribe();
        if *cancel_rx.borrow() {
            return PromptOutcome::Cancelled;
        }

        let head_idx = self.inner.head_idx;
        let head_session = match *self.inner.head_session.lock().unwrap() {
            Some(any) => any,
            None => {
                return PromptOutcome::Trap(wasmtime::Error::msg(
                    "prompt called before new-session",
                ));
            }
        };
        let this = self.clone();
        let prompt_arm = async move {
            let res = this
                .run_head(|a| {
                    Box::pin(async move {
                        let bindings = a
                            .with(|mut x| x.get().stages[head_idx].bindings.clone())
                            .expect("head bindings filled");
                        let resp = match &*bindings {
                            Bindings::Provider(b) => {
                                b.wassette_acp_agent()
                                    .session()
                                    .call_prompt(a, head_session, prompt)
                                    .await
                            }
                            Bindings::Layer(b) => {
                                b.wassette_acp_agent()
                                    .session()
                                    .call_prompt(a, head_session, prompt)
                                    .await
                            }
                        };
                        Ok::<_, wasmtime::Error>(resp)
                    })
                })
                .await;
            match res {
                Err(e) => PromptOutcome::Trap(e),
                Ok(Err(e)) => PromptOutcome::Trap(e),
                Ok(Ok(Err(e))) => PromptOutcome::Trap(e),
                Ok(Ok(Ok(Err(e)))) => PromptOutcome::Wit(e),
                Ok(Ok(Ok(Ok(r)))) => PromptOutcome::Done(r),
            }
        };
        let cancel_arm = async move {
            let _ = cancel_rx.changed().await;
            PromptOutcome::Cancelled
        };
        (cancel_arm, prompt_arm).race().await
    }
}

async fn call_config_option(
    accessor: &wasmtime::component::Accessor<HostState, HasSelf<HostState>>,
    head_idx: usize,
    session: ResourceAny,
    config_id: crate::wassette::acp::sessions::SessionConfigId,
    value: crate::wassette::acp::sessions::SessionConfigValueId,
) -> wasmtime::Result<Result<Vec<crate::wassette::acp::sessions::SessionConfigOption>, Error>> {
    let bindings = accessor
        .with(|mut x| x.get().stages[head_idx].bindings.clone())
        .expect("head bindings filled");
    match &*bindings {
        Bindings::Provider(b) => {
            b.wassette_acp_agent()
                .session()
                .call_set_config_option(accessor, session, config_id, value)
                .await
        }
        Bindings::Layer(b) => {
            b.wassette_acp_agent()
                .session()
                .call_set_config_option(accessor, session, config_id, value)
                .await
        }
    }
}

fn config_outcome(
    result: wasmtime::Result<
        Result<Vec<crate::wassette::acp::sessions::SessionConfigOption>, Error>,
    >,
) -> SetConfigOptionOutcome {
    match result {
        Err(error) => SetConfigOptionOutcome::Trap(error),
        Ok(Err(error)) => SetConfigOptionOutcome::Wit(error),
        Ok(Ok(options)) => SetConfigOptionOutcome::Done(options),
    }
}

// -----------------------------------------------------------------------------
// Registry
// -----------------------------------------------------------------------------

pub struct SessionRegistry {
    // KNOWN LIMITATION: entries are inserted by `session/new` and
    // `session/load` handlers but never removed — the upstream ACP
    // protocol at this version has no `session/close` notification
    // from the editor, and we removed our WIT `close-session` in
    // favor of the now-resource-based lifecycle. As a result the map
    // grows monotonically for the lifetime of the host process.
    // Each entry holds a [`SessionGroup`] (one `Store` per loaded
    // provider, a few `tokio::sync` primitives) which is bounded but
    // not negligible for hosts churning many sessions. Real fix:
    // either route a host-side timeout / explicit `/close` command
    // through here, or wait for ACP to add an editor-driven close
    // signal.
    sessions: Mutex<HashMap<String, crate::group::SessionGroup>>,
    creating: Mutex<HashMap<String, tokio_util::sync::CancellationToken>>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            creating: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, crate::group::SessionGroup>> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn insert(
        &self,
        id: String,
        group: crate::group::SessionGroup,
    ) -> std::result::Result<(), String> {
        let mut sessions = self.lock();
        match sessions.entry(id) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                Err(format!("session id `{}` is already active", entry.key()))
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(group);
                Ok(())
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<crate::group::SessionGroup> {
        self.lock().get(id).cloned()
    }

    pub fn begin_creation(self: &Arc<Self>, id: String) -> SessionCreation {
        let cancellation = tokio_util::sync::CancellationToken::new();
        self.creating
            .lock()
            .unwrap()
            .insert(id.clone(), cancellation.clone());
        SessionCreation {
            registry: self.clone(),
            id,
            cancellation,
        }
    }

    pub fn cancel(&self, id: &str) {
        if let Some(cancellation) = self.creating.lock().unwrap().get(id) {
            cancellation.cancel();
        }
        if let Some(group) = self.get(id) {
            group.cancel();
        }
    }

    pub fn clear(&self) {
        for (_, cancellation) in self.creating.lock().unwrap().drain() {
            cancellation.cancel();
        }
        let mut sessions = self.lock();
        for group in sessions.values() {
            group.cancel();
        }
        sessions.clear();
    }

    pub fn remove(&self, id: &str) -> Option<crate::group::SessionGroup> {
        self.lock().remove(id)
    }
}

pub struct SessionCreation {
    registry: Arc<SessionRegistry>,
    id: String,
    pub cancellation: tokio_util::sync::CancellationToken,
}

impl Drop for SessionCreation {
    fn drop(&mut self) {
        self.registry.creating.lock().unwrap().remove(&self.id);
    }
}

// -----------------------------------------------------------------------------
// agent::Host (layer's imported `agent`) — forwards to downstream stage
// -----------------------------------------------------------------------------
//
// A layer's `agent` import lands on this trait. The impl reads the
// currently executing stage's `downstream_idx` and calls the downstream
// stage's exported `agent` directly on the shared store — no mpsc, no
// oneshot. Push/pop the stage stack around the call so the linker's
// host getter returns the correct stage for any nested host imports.

use crate::wassette::acp::sessions::{
    ListSessionsRequest, ListSessionsResponse, ResumeSessionRequest, ResumeSessionResponse,
};
use crate::{layer_agent, translate};

/// Stash a downstream `ResourceAny` (returned from the next stage's
/// exported `agent.session`) in `HostState` and mint a fresh typed
/// `Resource<layer_agent::Session>` whose `rep` is the stash key.
///
/// Cross-instance resource transfer trips wasmtime's resource
/// type-identity check — even with a shared linker, the provider's
/// export-side `Session` type identity differs from the layer's
/// import-side `Session`. The indirection here keeps the downstream
/// resource alive (held by the host) while presenting the layer with
/// a freshly-typed handle owned by its own instance.
fn stash_layer_session(
    accessor: &wasmtime::component::Accessor<impl Send, HasSelf<HostState>>,
    any: ResourceAny,
) -> wasmtime::component::Resource<layer_agent::Session> {
    let rep = accessor.with(|mut a| a.get().stash_downstream_session(any));
    wasmtime::component::Resource::new_own(rep)
}

fn no_downstream<T>(method: &'static str) -> Result<T, Error> {
    Err(no_downstream_err(method))
}

fn no_downstream_err(method: &'static str) -> Error {
    translate::internal_error(&format!(
        "layer called `agent.{method}` but no downstream is configured"
    ))
}

fn flatten_downstream<T>(
    method: &'static str,
    res: wasmtime::Result<Result<T, Error>>,
) -> Result<T, Error> {
    match res {
        Ok(inner) => inner,
        Err(trap) => Err(translate::internal_error(&format!(
            "downstream `{method}` trapped: {trap:#}"
        ))),
    }
}

/// Resolve the downstream stage's bindings + idx for a layer's import.
/// Returns `None` if there's no downstream (host bug — providers don't
/// import `agent`).
fn downstream(
    accessor: &wasmtime::component::Accessor<impl Send, HasSelf<HostState>>,
) -> Option<(usize, Arc<Bindings>)> {
    accessor.with(|mut a| {
        let state = a.get();
        let stage = state.current_stage();
        let idx = stage.downstream_idx?;
        let bindings = state.stages[idx]
            .bindings
            .clone()
            .expect("downstream bindings filled");
        Some((idx, bindings))
    })
}

async fn downstream_call<R, F, Fut>(
    accessor: &wasmtime::component::Accessor<impl Send, HasSelf<HostState>>,
    idx: usize,
    f: F,
) -> R
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = R>,
{
    accessor.with(|mut a| a.get().push_stage(idx));
    let r = f().await;
    accessor.with(|mut a| a.get().pop_stage());
    r
}

impl layer_agent::Host for HostState {}

/// Host-side `drop` for the layer-imported `agent.session` resource.
///
/// Removes the bookkeeping entry from [`HostState::downstream_sessions`].
/// The stashed `ResourceAny` is then Rust-dropped; the downstream stage's
/// wasm-side `Drop for ProviderSession`/`PlanSession` fires when the
/// containing `Store` tears down (i.e. when the host's [`Session`] drops).
/// In-flight cleanup mid-session isn't possible from here because the
/// trait method has no `Accessor` to invoke `ResourceAny::resource_drop_async`
/// on, but the per-session window matches the chain's lifetime anyway:
/// the layer's own `Session` resource (and its `downstream` field) only
/// drop when the chain head drops.
impl layer_agent::HostSession for HostState {
    async fn drop(
        &mut self,
        rep: wasmtime::component::Resource<layer_agent::Session>,
    ) -> wasmtime::Result<()> {
        let _ = self.take_downstream_session(rep.rep());
        Ok(())
    }
}

/// Host-owned payload for a `client.terminal` resource.
///
/// When the session's host-owned `terminal` boolean config option is enabled,
/// the host actually spawns the requested command as a local child process;
/// the guest reaches it through the `client.terminal` resource (`output()`
/// streams the combined stdout/stderr, `wait_for_exit()` resolves with the exit
/// status). When terminal tools are disabled, or the spawn fails, the entry
/// records that and both methods surface it to the guest.
pub enum HostTerminalEntry {
    /// Terminal execution is disabled by host configuration.
    Disabled,
    /// The command could not be spawned; carries the OS error text.
    SpawnFailed(String),
    /// A live child process.
    Running(TerminalProcess),
}

/// Live handle to a spawned terminal command.
pub struct TerminalProcess {
    /// Combined stdout+stderr byte stream, chunked. Taken by the first
    /// `output()` call (the stream is consumed once).
    output_rx: Option<mpsc::Receiver<Vec<u8>>>,
    /// Resolves to `Some(_)` once the process has exited. `None` until
    /// then.
    exit_rx: watch::Receiver<Option<ExitInfo>>,
    /// Background pump task; aborted on drop so a dropped resource kills
    /// the process (via `kill_on_drop`) — matching the WIT "drop = kill".
    pump: tokio::task::AbortHandle,
}

impl Drop for TerminalProcess {
    fn drop(&mut self) {
        // Aborting drops the task's `Child`, whose `kill_on_drop(true)`
        // terminates the process. A no-op if the process already exited.
        self.pump.abort();
    }
}

/// Owned, cloneable snapshot of a process exit status. Kept separate from
/// the generated `TerminalExitStatus` so it can be stored in a `watch`
/// channel without depending on the WIT type deriving `Clone`.
#[derive(Clone)]
struct ExitInfo {
    code: Option<i32>,
    signal: Option<String>,
}

/// [`StreamProducer`] that forwards chunks pumped from a child process's
/// combined output channel to the guest's `stream<u8>` read end.
struct TerminalOutputProducer {
    rx: mpsc::Receiver<Vec<u8>>,
}

const TERMINAL_OUTPUT_CHANNEL_CHUNKS: usize = 8;
const MAX_TERMINAL_OUTPUT_BYTES: u64 = 1024 * 1024;

impl<D: 'static> StreamProducer<D> for TerminalOutputProducer {
    type Item = u8;
    type Buffer = VecBuffer<u8>;

    fn poll_produce<'a>(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        _store: StoreContextMut<'a, D>,
        mut destination: Destination<'a, u8, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let this = self.get_mut();
        match this.rx.poll_recv(cx) {
            // A chunk of output: hand it to the reader. Anything beyond
            // the reader's immediate capacity is retained by the runtime
            // and delivered on the next read.
            Poll::Ready(Some(chunk)) => {
                if !chunk.is_empty() {
                    destination.set_buffer(chunk.into());
                }
                Poll::Ready(Ok(StreamResult::Completed))
            }
            // Channel closed: the process exited and all output flushed.
            Poll::Ready(None) => Poll::Ready(Ok(StreamResult::Dropped)),
            Poll::Pending => {
                if finish {
                    Poll::Ready(Ok(StreamResult::Cancelled))
                } else {
                    Poll::Pending
                }
            }
        }
    }
}

/// Spawn `req`'s command as a child process, wiring its combined output
/// into an mpsc channel and its exit status into a `watch` channel. A
/// background task pumps both.
fn spawn_terminal(
    req: &crate::wassette::acp::terminals::CreateTerminalRequest,
) -> std::io::Result<TerminalProcess> {
    use std::process::Stdio;

    let mut cmd = tokio::process::Command::new(&req.command);
    cmd.args(&req.args);
    // Editor-supplied env vars are added on top of the inherited
    // environment, matching ACP terminal semantics.
    for ev in &req.env {
        cmd.env(&ev.name, &ev.value);
    }
    if let Some(cwd) = &req.cwd {
        cmd.current_dir(cwd);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>(TERMINAL_OUTPUT_CHANNEL_CHUNKS);
    let (exit_tx, exit_rx) = watch::channel::<Option<ExitInfo>>(None);
    let limit = req
        .output_byte_limit
        .unwrap_or(MAX_TERMINAL_OUTPUT_BYTES)
        .min(MAX_TERMINAL_OUTPUT_BYTES);

    let handle = tokio::spawn(pump_terminal(child, stdout, stderr, out_tx, exit_tx, limit));

    Ok(TerminalProcess {
        output_rx: Some(out_rx),
        exit_rx,
        pump: handle.abort_handle(),
    })
}

/// Drive a spawned child: drain stdout+stderr concurrently into
/// `out_tx`, wait for exit, then publish the exit status on `exit_tx`.
async fn pump_terminal(
    mut child: tokio::process::Child,
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
    out_tx: mpsc::Sender<Vec<u8>>,
    exit_tx: watch::Sender<Option<ExitInfo>>,
    limit: u64,
) {
    use std::sync::atomic::AtomicU64;

    let counter = Arc::new(AtomicU64::new(0));
    let r1 = spawn_reader(stdout, out_tx.clone(), limit, counter.clone());
    let r2 = spawn_reader(stderr, out_tx, limit, counter);

    let status = child.wait().await;
    // Ensure all output is flushed to the channel before we report exit.
    let _ = r1.await;
    let _ = r2.await;

    let info = match status {
        Ok(st) => exit_info_from_status(st),
        Err(e) => ExitInfo {
            code: None,
            signal: Some(format!("wait-error: {e}")),
        },
    };
    let _ = exit_tx.send(Some(info));
}

/// Spawn a task that reads `reader` to EOF, forwarding chunks to `tx`.
/// Honors `limit` as an upper bound on total forwarded bytes across both
/// streams (excess is dropped — an end-truncation simplification of the
/// WIT's start-truncation semantics).
fn spawn_reader<R>(
    reader: Option<R>,
    tx: mpsc::Sender<Vec<u8>>,
    limit: u64,
    counter: Arc<std::sync::atomic::AtomicU64>,
) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    use std::sync::atomic::Ordering;

    use tokio::io::AsyncReadExt;

    tokio::spawn(async move {
        let Some(mut reader) = reader else { return };
        let mut buf = vec![0u8; 8192];
        loop {
            let n = match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let prev = counter.fetch_add(n as u64, Ordering::Relaxed);
            if prev >= limit {
                continue;
            }
            let end = n.min((limit - prev) as usize);
            if tx.send(buf[..end].to_vec()).await.is_err() {
                break;
            }
        }
    })
}

/// Convert a process exit status into an [`ExitInfo`]. On Unix a
/// signal-terminated process reports the signal name; otherwise the
/// numeric exit code.
fn exit_info_from_status(status: std::process::ExitStatus) -> ExitInfo {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return ExitInfo {
                code: None,
                signal: Some(signal_name(sig)),
            };
        }
    }
    ExitInfo {
        code: status.code(),
        signal: None,
    }
}

/// Map a Unix signal number to its conventional name, falling back to
/// `SIG<n>` for anything not in the common set.
#[cfg(unix)]
fn signal_name(sig: i32) -> String {
    let name = match sig {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        6 => "SIGABRT",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        _ => return format!("SIG{sig}"),
    };
    name.to_string()
}

impl crate::wassette::acp::client::HostTerminal for HostState {
    async fn new(
        &mut self,
        req: crate::wassette::acp::terminals::CreateTerminalRequest,
    ) -> Resource<crate::wassette::acp::client::Terminal> {
        let entry = if !self.terminal_enabled {
            tracing::warn!(
                command = %req.command,
                "terminal tools are disabled; refusing to spawn (enable the `terminal` session config option)",
            );
            HostTerminalEntry::Disabled
        } else {
            match spawn_terminal(&req) {
                Ok(proc) => {
                    tracing::info!(
                        command = %req.command,
                        arg_count = req.args.len(),
                        cwd = ?req.cwd,
                        "spawned terminal command",
                    );
                    HostTerminalEntry::Running(proc)
                }
                Err(e) => {
                    tracing::warn!(
                        command = %req.command,
                        error = %e,
                        "failed to spawn terminal command",
                    );
                    HostTerminalEntry::SpawnFailed(e.to_string())
                }
            }
        };
        // Allocate a slot in the per-store resource table. The rep we
        // mint here is the table-side rep, retagged under the WIT
        // `Terminal` resource type (same pattern as
        // [`crate::secrets_impl::StoreHost::get`]).
        let handle = self
            .table
            .push(entry)
            .expect("resource table push for client.terminal");
        Resource::new_own(handle.rep())
    }

    async fn drop(
        &mut self,
        rep: Resource<crate::wassette::acp::client::Terminal>,
    ) -> wasmtime::Result<()> {
        let entry: Resource<HostTerminalEntry> = Resource::new_own(rep.rep());
        // Dropping the entry runs `TerminalProcess::drop`, killing any
        // still-running child.
        let _ = self.table.delete(entry);
        Ok(())
    }
}

// The generated `*WithStore` trait methods return `impl Future + Send`; an
// `async fn` in a trait impl cannot restate that bound, so the desugared form
// stays.
#[allow(clippy::manual_async_fn)]
impl<T: Send> crate::wassette::acp::client::HostTerminalWithStore<T> for HasSelf<HostState> {
    fn output(
        accessor: &wasmtime::component::Accessor<T, Self>,
        self_: Resource<crate::wassette::acp::client::Terminal>,
    ) -> impl ::core::future::Future<Output = StreamReader<u8>> + Send {
        async move {
            // Take the output receiver out of the entry (a stream is
            // consumed once); `None` for disabled/failed terminals or a
            // second `output()` call.
            let rx = accessor.with(|mut a| {
                let key: Resource<HostTerminalEntry> = Resource::new_own(self_.rep());
                match a.get().table.get_mut(&key) {
                    Ok(HostTerminalEntry::Running(proc)) => proc.output_rx.take(),
                    _ => None,
                }
            });
            accessor
                .with(|mut a| match rx {
                    Some(rx) => StreamReader::new(&mut a, TerminalOutputProducer { rx }),
                    None => StreamReader::new(&mut a, std::iter::empty::<u8>()),
                })
                .expect("terminal output stream construction")
        }
    }

    fn wait_for_exit(
        accessor: &wasmtime::component::Accessor<T, Self>,
        self_: Resource<crate::wassette::acp::client::Terminal>,
    ) -> impl ::core::future::Future<
        Output = Result<crate::wassette::acp::terminals::TerminalExitStatus, Error>,
    > + Send {
        async move {
            enum Waiter {
                Wait(watch::Receiver<Option<ExitInfo>>),
                Disabled,
                Failed(String),
                Missing,
            }
            let waiter = accessor.with(|mut a| {
                let key: Resource<HostTerminalEntry> = Resource::new_own(self_.rep());
                match a.get().table.get(&key) {
                    Ok(HostTerminalEntry::Running(proc)) => Waiter::Wait(proc.exit_rx.clone()),
                    Ok(HostTerminalEntry::Disabled) => Waiter::Disabled,
                    Ok(HostTerminalEntry::SpawnFailed(msg)) => Waiter::Failed(msg.clone()),
                    Err(_) => Waiter::Missing,
                }
            });
            match waiter {
                Waiter::Disabled => Err(translate::internal_error(
                    "terminal tools are disabled; enable the `terminal` session config option",
                )),
                Waiter::Failed(msg) => Err(translate::internal_error(&format!(
                    "failed to start terminal command: {msg}"
                ))),
                Waiter::Missing => Err(translate::internal_error("terminal resource not found")),
                Waiter::Wait(mut rx) => loop {
                    if let Some(info) = rx.borrow_and_update().clone() {
                        return Ok(crate::wassette::acp::terminals::TerminalExitStatus {
                            exit_code: info.code,
                            signal: info.signal,
                        });
                    }
                    if rx.changed().await.is_err() {
                        return Err(translate::internal_error(
                            "terminal process ended without reporting an exit status",
                        ));
                    }
                },
            }
        }
    }
}

impl crate::wassette::acp::tools::HostToolCall for HostState {
    async fn new(
        &mut self,
        _initial: crate::wassette::acp::tools::ToolCallInit,
    ) -> wasmtime::component::Resource<crate::wassette::acp::tools::ToolCall> {
        unimplemented!("phase 4: tools::HostToolCall::new")
    }

    async fn drop(
        &mut self,
        _rep: wasmtime::component::Resource<crate::wassette::acp::tools::ToolCall>,
    ) -> wasmtime::Result<()> {
        Ok(())
    }
}

// The generated `*WithStore` trait methods return `impl Future + Send`; an
// `async fn` in a trait impl cannot restate that bound, so the desugared form
// stays.
#[allow(clippy::manual_async_fn)]
impl<T: Send> crate::wassette::acp::tools::HostToolCallWithStore<T> for HasSelf<HostState> {
    fn update(
        _accessor: &wasmtime::component::Accessor<T, Self>,
        _self_: wasmtime::component::Resource<crate::wassette::acp::tools::ToolCall>,
        _patch: crate::wassette::acp::tools::ToolCallPatch,
    ) -> impl ::core::future::Future<Output = ()> + Send {
        async move { unimplemented!("phase 4: tools::HostToolCallWithStore::update") }
    }
}

impl<T: Send> layer_agent::HostWithStore<T> for HasSelf<HostState> {
    fn initialize(
        accessor: &wasmtime::component::Accessor<T, Self>,
        req: InitializeRequest,
    ) -> impl ::core::future::Future<Output = Result<InitializeResponse, Error>> + Send {
        let ds = downstream(accessor);
        async move {
            let Some((idx, bindings)) = ds else {
                return no_downstream("initialize");
            };
            let res = downstream_call(accessor, idx, || async {
                match &*bindings {
                    Bindings::Provider(b) => {
                        b.wassette_acp_agent().call_initialize(accessor, req).await
                    }
                    Bindings::Layer(b) => {
                        b.wassette_acp_agent().call_initialize(accessor, req).await
                    }
                }
            })
            .await;
            flatten_downstream("initialize", res)
        }
    }

    fn authenticate(
        accessor: &wasmtime::component::Accessor<T, Self>,
        req: AuthenticateRequest,
    ) -> impl ::core::future::Future<Output = Result<(), Error>> + Send {
        let ds = downstream(accessor);
        async move {
            let Some((idx, bindings)) = ds else {
                return no_downstream("authenticate");
            };
            let res = downstream_call(accessor, idx, || async {
                match &*bindings {
                    Bindings::Provider(b) => {
                        b.wassette_acp_agent()
                            .call_authenticate(accessor, req)
                            .await
                    }
                    Bindings::Layer(b) => {
                        b.wassette_acp_agent()
                            .call_authenticate(accessor, req)
                            .await
                    }
                }
            })
            .await;
            flatten_downstream("authenticate", res)
        }
    }

    fn new_session(
        accessor: &wasmtime::component::Accessor<T, Self>,
        req: NewSessionRequest,
    ) -> impl ::core::future::Future<
        Output = Result<
            (
                wasmtime::component::Resource<layer_agent::Session>,
                NewSessionResponse,
            ),
            Error,
        >,
    > + Send {
        let ds = downstream(accessor);
        async move {
            let Some((idx, bindings)) = ds else {
                return Err(no_downstream_err("new-session"));
            };
            let res: wasmtime::Result<Result<(ResourceAny, NewSessionResponse), Error>> =
                downstream_call(accessor, idx, || async {
                    match &*bindings {
                        Bindings::Provider(b) => {
                            b.wassette_acp_agent().call_new_session(accessor, req).await
                        }
                        Bindings::Layer(b) => {
                            b.wassette_acp_agent().call_new_session(accessor, req).await
                        }
                    }
                })
                .await;
            let (any, resp) = flatten_downstream("new-session", res)?;
            let resource = stash_layer_session(accessor, any);
            Ok((resource, resp))
        }
    }

    fn load_session(
        accessor: &wasmtime::component::Accessor<T, Self>,
        req: LoadSessionRequest,
    ) -> impl ::core::future::Future<
        Output = Result<
            (
                wasmtime::component::Resource<layer_agent::Session>,
                LoadSessionResponse,
            ),
            Error,
        >,
    > + Send {
        let ds = downstream(accessor);
        async move {
            let Some((idx, bindings)) = ds else {
                return Err(no_downstream_err("load-session"));
            };
            let res: wasmtime::Result<Result<(ResourceAny, LoadSessionResponse), Error>> =
                downstream_call(accessor, idx, || async {
                    match &*bindings {
                        Bindings::Provider(b) => {
                            b.wassette_acp_agent()
                                .call_load_session(accessor, req)
                                .await
                        }
                        Bindings::Layer(b) => {
                            b.wassette_acp_agent()
                                .call_load_session(accessor, req)
                                .await
                        }
                    }
                })
                .await;
            let (any, resp) = flatten_downstream("load-session", res)?;
            let resource = stash_layer_session(accessor, any);
            Ok((resource, resp))
        }
    }

    fn list_sessions(
        accessor: &wasmtime::component::Accessor<T, Self>,
        req: ListSessionsRequest,
    ) -> impl ::core::future::Future<Output = Result<ListSessionsResponse, Error>> + Send {
        let ds = downstream(accessor);
        async move {
            let Some((idx, bindings)) = ds else {
                return no_downstream("list-sessions");
            };
            let res = downstream_call(accessor, idx, || async {
                match &*bindings {
                    Bindings::Provider(b) => {
                        b.wassette_acp_agent()
                            .call_list_sessions(accessor, req)
                            .await
                    }
                    Bindings::Layer(b) => {
                        b.wassette_acp_agent()
                            .call_list_sessions(accessor, req)
                            .await
                    }
                }
            })
            .await;
            flatten_downstream("list-sessions", res)
        }
    }

    fn resume_session(
        accessor: &wasmtime::component::Accessor<T, Self>,
        req: ResumeSessionRequest,
    ) -> impl ::core::future::Future<
        Output = Result<
            (
                wasmtime::component::Resource<layer_agent::Session>,
                ResumeSessionResponse,
            ),
            Error,
        >,
    > + Send {
        let ds = downstream(accessor);
        async move {
            let Some((idx, bindings)) = ds else {
                return Err(no_downstream_err("resume-session"));
            };
            let res: wasmtime::Result<Result<(ResourceAny, ResumeSessionResponse), Error>> =
                downstream_call(accessor, idx, || async {
                    match &*bindings {
                        Bindings::Provider(b) => {
                            b.wassette_acp_agent()
                                .call_resume_session(accessor, req)
                                .await
                        }
                        Bindings::Layer(b) => {
                            b.wassette_acp_agent()
                                .call_resume_session(accessor, req)
                                .await
                        }
                    }
                })
                .await;
            let (any, resp) = flatten_downstream("resume-session", res)?;
            let resource = stash_layer_session(accessor, any);
            Ok((resource, resp))
        }
    }
}

/// Host-side glue for the layer-imported `agent.session` resource.
/// `prompt`, `set-mode`, and `select-model` all forward to the
/// downstream stashed session resource. `prompt` returns the terminal
/// `prompt-response` directly; intermediate session updates flow over
/// `client.notify-session`.
impl<T: Send> layer_agent::HostSessionWithStore<T> for HasSelf<HostState> {
    fn prompt(
        accessor: &wasmtime::component::Accessor<T, Self>,
        self_: wasmtime::component::Resource<layer_agent::Session>,
        prompt: Vec<crate::wassette::acp::content::ContentBlock>,
    ) -> impl ::core::future::Future<Output = Result<PromptResponse, Error>> + Send {
        let downstream = accessor.with(|mut a| {
            let state = a.get();
            let stage = state.current_stage();
            let idx = stage.downstream_idx?;
            let bindings = state.stages[idx].bindings.clone()?;
            let any = state.downstream_sessions.get(&self_.rep()).copied()?;
            Some((idx, bindings, any))
        });
        async move {
            let Some((idx, bindings, session_any)) = downstream else {
                return Err(translate::internal_error(
                    "layer called `session.prompt` but no downstream session is mapped",
                ));
            };
            let res = downstream_call(accessor, idx, || async {
                match &*bindings {
                    Bindings::Provider(b) => {
                        b.wassette_acp_agent()
                            .session()
                            .call_prompt(accessor, session_any, prompt)
                            .await
                    }
                    Bindings::Layer(b) => {
                        b.wassette_acp_agent()
                            .session()
                            .call_prompt(accessor, session_any, prompt)
                            .await
                    }
                }
            })
            .await;
            flatten_downstream("session.prompt", res)
        }
    }

    fn set_mode(
        accessor: &wasmtime::component::Accessor<T, Self>,
        self_: wasmtime::component::Resource<layer_agent::Session>,
        mode_id: crate::wassette::acp::sessions::SessionModeId,
    ) -> impl ::core::future::Future<Output = Result<(), Error>> + Send {
        let downstream = accessor.with(|mut a| {
            let state = a.get();
            let stage = state.current_stage();
            let idx = stage.downstream_idx?;
            let bindings = state.stages[idx].bindings.clone()?;
            let any = state.downstream_sessions.get(&self_.rep()).copied()?;
            Some((idx, bindings, any))
        });
        async move {
            let Some((idx, bindings, any)) = downstream else {
                return Err(translate::internal_error(
                    "layer called `session.set-mode` but no downstream session is mapped",
                ));
            };
            let res = downstream_call(accessor, idx, || async {
                match &*bindings {
                    Bindings::Provider(b) => {
                        b.wassette_acp_agent()
                            .session()
                            .call_set_mode(accessor, any, mode_id)
                            .await
                    }
                    Bindings::Layer(b) => {
                        b.wassette_acp_agent()
                            .session()
                            .call_set_mode(accessor, any, mode_id)
                            .await
                    }
                }
            })
            .await;
            flatten_downstream("session.set-mode", res)
        }
    }

    fn select_model(
        accessor: &wasmtime::component::Accessor<T, Self>,
        self_: wasmtime::component::Resource<layer_agent::Session>,
        model_id: crate::wassette::acp::sessions::SessionModelId,
    ) -> impl ::core::future::Future<Output = Result<(), Error>> + Send {
        let downstream = accessor.with(|mut a| {
            let state = a.get();
            let stage = state.current_stage();
            let idx = stage.downstream_idx?;
            let bindings = state.stages[idx].bindings.clone()?;
            let any = state.downstream_sessions.get(&self_.rep()).copied()?;
            Some((idx, bindings, any))
        });
        async move {
            let Some((idx, bindings, any)) = downstream else {
                return Err(translate::internal_error(
                    "layer called `session.select-model` but no downstream session is mapped",
                ));
            };
            let res = downstream_call(accessor, idx, || async {
                match &*bindings {
                    Bindings::Provider(b) => {
                        b.wassette_acp_agent()
                            .session()
                            .call_select_model(accessor, any, model_id)
                            .await
                    }
                    Bindings::Layer(b) => {
                        b.wassette_acp_agent()
                            .session()
                            .call_select_model(accessor, any, model_id)
                            .await
                    }
                }
            })
            .await;
            flatten_downstream("session.select-model", res)
        }
    }

    fn set_config_option(
        accessor: &wasmtime::component::Accessor<T, Self>,
        self_: wasmtime::component::Resource<layer_agent::Session>,
        config_id: crate::wassette::acp::sessions::SessionConfigId,
        value: crate::wassette::acp::sessions::SessionConfigValueId,
    ) -> impl ::core::future::Future<
        Output = Result<Vec<crate::wassette::acp::sessions::SessionConfigOption>, Error>,
    > + Send {
        let downstream = accessor.with(|mut a| {
            let state = a.get();
            let stage = state.current_stage();
            let idx = stage.downstream_idx?;
            let bindings = state.stages[idx].bindings.clone()?;
            let any = state.downstream_sessions.get(&self_.rep()).copied()?;
            Some((idx, bindings, any))
        });
        async move {
            let Some((idx, bindings, any)) = downstream else {
                return Err(translate::internal_error(
                    "layer called `session.set-config-option` but no downstream session is mapped",
                ));
            };
            let res = downstream_call(accessor, idx, || async {
                match &*bindings {
                    Bindings::Provider(b) => {
                        b.wassette_acp_agent()
                            .session()
                            .call_set_config_option(accessor, any, config_id, value)
                            .await
                    }
                    Bindings::Layer(b) => {
                        b.wassette_acp_agent()
                            .session()
                            .call_set_config_option(accessor, any, config_id, value)
                            .await
                    }
                }
            })
            .await;
            flatten_downstream("session.set-config-option", res)
        }
    }
}

#[cfg(test)]
mod terminal_tests {
    use super::*;

    /// Build a minimal `create-terminal-request` for `command args...`
    /// with no env, no cwd, and no output limit.
    fn make_request(
        command: &str,
        args: &[&str],
    ) -> crate::wassette::acp::terminals::CreateTerminalRequest {
        crate::wassette::acp::terminals::CreateTerminalRequest {
            session_id: "test-session".to_string(),
            command: command.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: Vec::new(),
            cwd: None,
            output_byte_limit: None,
        }
    }

    fn test_host_state() -> HostState {
        let mut wasi = wasmtime_wasi::WasiCtxBuilder::new();
        HostState {
            wasi: wasi.build(),
            http: WasiHttpCtx::new(),
            http_hooks: HttpPolicyHooks::new(None),
            table: ResourceTable::new(),
            stages: Vec::new(),
            stage_stack: Vec::new(),
            secrets: Arc::new(SecretsRegistry::new("terminal-test")),
            downstream_sessions: std::collections::HashMap::new(),
            next_downstream_rep: 1,
            editor_session_id: None,
            provider_routing: None,
            terminal_enabled: false,
            tool_broker: None,
            tool_decisions: Vec::new(),
            active_tool_calls: Default::default(),
        }
    }

    #[tokio::test]
    async fn cancellation_after_prompt_acceptance_survives_until_poll() {
        let engine = Engine::default();
        let (cancel, _) = watch::channel(false);
        let session = Session::new(Store::new(&engine, test_host_state()), 0, cancel);
        session.prepare_prompt();
        let prompt = session.prompt("test-session".to_string(), Vec::new());
        session.cancel();
        assert!(matches!(prompt.await, PromptOutcome::Cancelled));
        session.prepare_prompt();
        assert!(
            !*session.inner.cancel.borrow(),
            "next turn must start uncancelled"
        );
    }

    #[tokio::test]
    async fn live_terminal_control_completes_inside_an_active_call() {
        let engine = crate::acp_engine().unwrap();
        let (cancel, _) = watch::channel(false);
        let session = Session::new(Store::new(&engine, test_host_state()), 0, cancel);
        let running = session.clone();
        let (entered, ready) = oneshot::channel();
        let task = tokio::spawn(async move {
            running
                .run_head(|_| {
                    Box::pin(async {
                        entered.send(()).unwrap();
                        std::future::pending::<()>().await;
                    })
                })
                .await
        });
        ready.await.unwrap();
        for enabled in [true, false, true] {
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                session.set_terminal_enabled(enabled),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(!task.is_finished(), "control must not end the active call");
        }
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(session.inner.store.lock().await.data().terminal_enabled);
    }

    #[tokio::test]
    async fn live_controls_survive_active_to_idle_handoff() {
        let engine = crate::acp_engine().unwrap();
        for cancelled in [false, true] {
            for _ in 0..16 {
                let (cancel, _) = watch::channel(false);
                let session = Session::new(Store::new(&engine, test_host_state()), 0, cancel);
                let running = session.clone();
                let (entered, ready) = oneshot::channel();
                let (finish, finished) = oneshot::channel();
                let task = tokio::spawn(async move {
                    running
                        .run_head(|_| {
                            Box::pin(async {
                                entered.send(()).unwrap();
                                finished.await.unwrap();
                            })
                        })
                        .await
                });
                ready.await.unwrap();
                let changing = session.clone();
                let setter = tokio::spawn(async move { changing.set_terminal_enabled(true).await });
                if cancelled {
                    task.abort();
                } else {
                    finish.send(()).unwrap();
                }
                tokio::time::timeout(std::time::Duration::from_secs(2), setter)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                if cancelled {
                    assert!(task.await.unwrap_err().is_cancelled());
                } else {
                    task.await.unwrap().unwrap();
                }
                assert!(session.inner.store.lock().await.data().terminal_enabled);
            }
        }
    }

    #[tokio::test]
    async fn live_configuration_reservations_exclude_backend_changes() {
        let engine = crate::acp_engine().unwrap();
        let (cancel, _) = watch::channel(false);
        let session = Session::new(Store::new(&engine, test_host_state()), 0, cancel);
        let group = crate::group::SessionGroup::new(
            "editor".to_string(),
            vec![crate::group::ProviderSession {
                component_id: "test:provider".to_string(),
                session,
                options: None,
            }],
            true,
        )
        .unwrap();
        let prompt = group.begin_operation().unwrap();
        let first = group.begin_configuration("terminal").unwrap();
        let second = group.begin_configuration("terminal").unwrap();
        assert!(first.live && second.live);
        assert!(group.begin_configuration("model").is_err());
        assert!(group.begin_configuration("mode").is_err());
        assert!(group.begin_configuration("allow-all").is_err());
        drop(prompt);
        assert!(group.begin_configuration("model").is_err());
        drop((first, second));
        assert!(!group.begin_configuration("model").unwrap().live);
    }

    /// Spawn `req`, drain its combined output to EOF, then wait for and
    /// return the collected bytes alongside the resolved exit info.
    async fn run(
        req: &crate::wassette::acp::terminals::CreateTerminalRequest,
    ) -> (Vec<u8>, ExitInfo) {
        let mut proc = spawn_terminal(req).expect("spawn");
        let mut rx = proc.output_rx.take().expect("output stream present");
        let mut out = Vec::new();
        while let Some(chunk) = rx.recv().await {
            out.extend_from_slice(&chunk);
        }
        let mut exit_rx = proc.exit_rx.clone();
        let info = loop {
            if let Some(info) = exit_rx.borrow_and_update().clone() {
                break info;
            }
            exit_rx.changed().await.expect("exit status channel open");
        };
        (out, info)
    }

    #[tokio::test]
    async fn echo_captures_stdout_and_zero_exit() {
        let (out, info) = run(&make_request("echo", &["hello", "world"])).await;
        assert_eq!(String::from_utf8_lossy(&out).trim_end(), "hello world");
        assert_eq!(info.code, Some(0));
        assert_eq!(info.signal, None);
    }

    #[tokio::test]
    async fn terminal_config_toggle_controls_every_provider_spawn() {
        use crate::wassette::acp::sessions::{
            ComponentSource, SessionConfigOption, SessionConfigOptionCategory,
            SessionConfigSelectOption, SessionConfigSelectOptions,
        };
        let options = || {
            Some(vec![SessionConfigOption {
                id: "model".into(),
                name: "Model".into(),
                description: None,
                category: Some(SessionConfigOptionCategory::Model),
                current_value: "default".into(),
                options: SessionConfigSelectOptions::Ungrouped(vec![SessionConfigSelectOption {
                    value: "default".into(),
                    name: "Default".into(),
                    description: None,
                }]),
                provided_by: ComponentSource {
                    component_id: "test".into(),
                },
            }])
        };
        let engine = Engine::default();
        let (primary_cancel, _) = watch::channel(false);
        let primary = Session::new(Store::new(&engine, test_host_state()), 0, primary_cancel);
        let (secondary_cancel, _) = watch::channel(false);
        let secondary = Session::new(Store::new(&engine, test_host_state()), 0, secondary_cancel);
        let group = crate::group::SessionGroup::new(
            "test-session".to_string(),
            vec![
                crate::group::ProviderSession {
                    component_id: "local:first-provider".to_string(),
                    session: primary.clone(),
                    options: options(),
                },
                crate::group::ProviderSession {
                    component_id: "local:second-provider".to_string(),
                    session: secondary.clone(),
                    options: options(),
                },
            ],
            true,
        )
        .unwrap();

        assert_eq!(group.terminal_option(), Some(false));
        let disabled = {
            let mut store = primary.inner.store.lock().await;
            <HostState as crate::wassette::acp::client::HostTerminal>::new(
                store.data_mut(),
                make_request("echo", &["must-not-run"]),
            )
            .await
        };
        {
            let mut store = primary.inner.store.lock().await;
            let key: Resource<HostTerminalEntry> = Resource::new_own(disabled.rep());
            assert!(matches!(
                store.data().table.get(&key),
                Ok(HostTerminalEntry::Disabled)
            ));
            store
                .data_mut()
                .table
                .delete(key)
                .expect("delete disabled terminal resource");
        }

        group.set_terminal_enabled(true).await.unwrap();
        assert_eq!(group.terminal_option(), Some(true));
        let (enabled, mut output_rx, mut exit_rx) = {
            assert!(primary.inner.store.lock().await.data().terminal_enabled);
            let mut store = secondary.inner.store.lock().await;
            assert!(store.data().terminal_enabled);
            let terminal = <HostState as crate::wassette::acp::client::HostTerminal>::new(
                store.data_mut(),
                make_request("echo", &["terminal-enabled"]),
            )
            .await;
            let key: Resource<HostTerminalEntry> = Resource::new_own(terminal.rep());
            let entry = store
                .data_mut()
                .table
                .get_mut(&key)
                .expect("enabled terminal resource");
            let HostTerminalEntry::Running(proc) = entry else {
                panic!("enabled terminal config did not spawn a process");
            };
            (
                terminal,
                proc.output_rx.take().expect("terminal output stream"),
                proc.exit_rx.clone(),
            )
        };

        let mut output = Vec::new();
        while let Some(chunk) = output_rx.recv().await {
            output.extend_from_slice(&chunk);
        }
        let exit = loop {
            if let Some(exit) = exit_rx.borrow_and_update().clone() {
                break exit;
            }
            exit_rx.changed().await.expect("exit status channel open");
        };
        assert_eq!(
            String::from_utf8_lossy(&output).trim_end(),
            "terminal-enabled"
        );
        assert_eq!(exit.code, Some(0));
        assert_eq!(exit.signal, None);

        {
            let mut store = secondary.inner.store.lock().await;
            let key: Resource<HostTerminalEntry> = Resource::new_own(enabled.rep());
            store
                .data_mut()
                .table
                .delete(key)
                .expect("delete enabled terminal resource");
        }

        group.set_terminal_enabled(false).await.unwrap();
        assert_eq!(group.terminal_option(), Some(false));
        assert!(!primary.inner.store.lock().await.data().terminal_enabled);
        assert!(!secondary.inner.store.lock().await.data().terminal_enabled);
    }

    #[test]
    fn creation_registrations_cancel_and_clean_up() {
        let registry = Arc::new(SessionRegistry::new());
        let creation = registry.begin_creation("pending".into());
        assert_eq!(registry.creating.lock().unwrap().len(), 1);
        registry.cancel("pending");
        assert!(creation.cancellation.is_cancelled());
        drop(creation);
        assert!(registry.creating.lock().unwrap().is_empty());
        let next = registry.begin_creation("next".into());
        registry.clear();
        assert!(next.cancellation.is_cancelled());
        assert!(registry.creating.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn registry_rejects_duplicate_ids_without_replacing_a_session() {
        let engine = Engine::default();
        let (cancel, _) = watch::channel(false);
        let session = Session::new(Store::new(&engine, test_host_state()), 0, cancel);
        let group = crate::group::SessionGroup::new(
            "same-id".into(),
            vec![crate::group::ProviderSession {
                component_id: "local:echo".into(),
                session,
                options: None,
            }],
            true,
        )
        .unwrap();
        group.set_terminal_enabled(true).await.unwrap();
        let registry = SessionRegistry::new();
        registry.insert("same-id".into(), group.clone()).unwrap();
        assert_eq!(
            registry.insert("same-id".into(), group).unwrap_err(),
            "session id `same-id` is already active"
        );
        assert_eq!(
            registry.get("same-id").unwrap().terminal_option(),
            Some(true)
        );
    }

    #[tokio::test]
    async fn terminal_spawn_log_excludes_arguments() {
        let log = tempfile::NamedTempFile::new().unwrap();
        let writer = log.reopen().unwrap();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || writer.try_clone().unwrap())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let mut state = test_host_state();
        state.terminal_enabled = true;
        let terminal = <HostState as crate::wassette::acp::client::HostTerminal>::new(
            &mut state,
            make_request("echo", &["credential-from-argv"]),
        )
        .await;
        <HostState as crate::wassette::acp::client::HostTerminal>::drop(&mut state, terminal)
            .await
            .unwrap();
        let output = std::fs::read_to_string(log.path()).unwrap();
        assert!(output.contains("spawned terminal command"), "{output}");
        assert!(output.contains("arg_count=1"), "{output}");
        assert!(!output.contains("credential-from-argv"), "{output}");
    }

    #[tokio::test]
    async fn nonzero_exit_code_is_reported() {
        let (_out, info) = run(&make_request("sh", &["-c", "exit 7"])).await;
        assert_eq!(info.code, Some(7));
        assert_eq!(info.signal, None);
    }

    #[tokio::test]
    async fn stderr_is_merged_into_output() {
        let (out, info) = run(&make_request("sh", &["-c", "echo out; echo err 1>&2"])).await;
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("out"), "stdout missing from {text:?}");
        assert!(text.contains("err"), "stderr missing from {text:?}");
        assert_eq!(info.code, Some(0));
    }

    #[tokio::test]
    async fn missing_command_fails_to_spawn() {
        let req = make_request("definitely-not-a-real-command-xyz", &[]);
        assert!(spawn_terminal(&req).is_err());
    }

    #[tokio::test]
    async fn output_byte_limit_truncates_excess() {
        let mut req = make_request("sh", &["-c", "printf abcdefghij"]);
        req.output_byte_limit = Some(4);
        let (out, _info) = run(&req).await;
        assert!(out.len() <= 4, "expected <= 4 bytes, got {}", out.len());
    }

    #[tokio::test]
    async fn host_output_cap_applies_without_guest_limit() {
        let (out, info) = run(&make_request("sh", &["-c", "head -c 2097152 /dev/zero"])).await;
        assert_eq!(out.len() as u64, MAX_TERMINAL_OUTPUT_BYTES);
        assert_eq!(info.code, Some(0));
    }

    #[tokio::test]
    async fn output_channel_applies_backpressure_to_reader() {
        use tokio::io::AsyncReadExt;

        let (tx, mut rx) = mpsc::channel(1);
        let task = spawn_reader(
            Some(tokio::io::repeat(b'x').take(8192 * 3)),
            tx,
            8192 * 3,
            Arc::new(std::sync::atomic::AtomicU64::new(0)),
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while rx.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::task::yield_now().await;
        assert_eq!(rx.len(), 1);
        assert!(!task.is_finished(), "reader must wait for channel capacity");
        let mut received = 0;
        while let Some(chunk) = rx.recv().await {
            received += chunk.len();
        }
        task.await.unwrap();
        assert_eq!(received, 8192 * 3);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn signal_termination_is_reported() {
        let (_out, info) = run(&make_request("sh", &["-c", "kill -TERM $$"])).await;
        assert_eq!(info.signal.as_deref(), Some("SIGTERM"));
        assert_eq!(info.code, None);
    }

    #[cfg(unix)]
    #[test]
    fn signal_name_maps_common_and_falls_back() {
        assert_eq!(signal_name(2), "SIGINT");
        assert_eq!(signal_name(9), "SIGKILL");
        assert_eq!(signal_name(15), "SIGTERM");
        assert_eq!(signal_name(99), "SIG99");
    }
}
