// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as SyncMutex};

use agent_client_protocol::schema::v1 as schema;
use serde_json::Value;
use tokio::sync::{Mutex, Semaphore, mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use wasmtime::component::{Accessor, HasSelf};
use wassette::{
    CatalogGeneration, CatalogSnapshot, LifecycleManager, PreparedInvocation, ToolDescriptor,
    ToolInvocationError, ToolRef,
};

use crate::state::{ClientSink, HostState, OutboundEvent};
use crate::wassette::acp::prompts::SessionUpdate;
use crate::wassette::acp::tools::{
    PermissionOption, PermissionOptionKind, PermissionOutcome, RequestPermissionRequest,
    ToolCallSnapshot, ToolCallStatus, ToolKind,
};
use crate::wassette::component_tools::tools::{
    Catalog, CatalogResult, ToolDescriptor as WitToolDescriptor, ToolError, ToolResult,
};

const MAX_CONCURRENT_CALLS: usize = 8;

type ExecutionJob = Pin<Box<dyn Future<Output = ()> + Send>>;

pub(crate) struct ToolWorkers {
    permits: Arc<Semaphore>,
    jobs: mpsc::Sender<ExecutionJob>,
    closing: CancellationToken,
    supervisor: Mutex<Option<JoinHandle<()>>>,
}

impl ToolWorkers {
    fn new(limit: usize) -> Self {
        let (jobs, mut incoming) = mpsc::channel::<ExecutionJob>(limit);
        let closing = CancellationToken::new();
        let shutdown = closing.clone();
        let supervisor = tokio::spawn(async move {
            let mut running = JoinSet::new();
            loop {
                while let Some(result) = running.try_join_next() {
                    if let Err(error) = result {
                        tracing::error!(%error, "ACP tool execution task failed");
                    }
                }
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => {
                        incoming.close();
                        break;
                    }
                    job = incoming.recv() => match job {
                        Some(job) => { running.spawn(job); }
                        None => break,
                    },
                    result = running.join_next(), if !running.is_empty() => {
                        if let Some(Err(error)) = result {
                            tracing::error!(%error, "ACP tool execution task failed");
                        }
                    }
                }
            }
            while let Some(job) = incoming.recv().await {
                running.spawn(job);
            }
            while let Some(result) = running.join_next().await {
                if let Err(error) = result {
                    tracing::error!(%error, "ACP tool execution task failed during shutdown");
                }
            }
        });
        Self {
            permits: Arc::new(Semaphore::new(limit)),
            jobs,
            closing,
            supervisor: Mutex::new(Some(supervisor)),
        }
    }

    pub(crate) fn close(&self) {
        self.permits.close();
        self.closing.cancel();
    }

    fn cancellation_token(&self) -> CancellationToken {
        self.closing.child_token()
    }

    pub(crate) async fn shutdown(&self) -> Result<(), ToolError> {
        self.close();
        let mut supervisor = self.supervisor.lock().await;
        if let Some(task) = supervisor.as_mut() {
            let result = task.await;
            *supervisor = None;
            result.map_err(|_| {
                ToolError::Unavailable("Tool supervisor failed during shutdown".into())
            })?;
        }
        Ok(())
    }

    pub(crate) async fn run<F, R>(&self, execution: F) -> Result<R, ToolError>
    where
        F: Future<Output = R> + Send + 'static,
        R: Send + 'static,
    {
        if self.closing.is_cancelled() {
            return Err(ToolError::Unavailable(
                "Tool supervisor is shutting down".into(),
            ));
        }
        let permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|error| match error {
                tokio::sync::TryAcquireError::Closed => {
                    ToolError::Unavailable("Tool supervisor is shutting down".into())
                }
                tokio::sync::TryAcquireError::NoPermits => ToolError::Busy,
            })?;
        let (send, receive) = oneshot::channel();
        self.jobs
            .send(Box::pin(async move {
                let result = execution.await;
                drop(permit);
                let _ = send.send(result);
            }))
            .await
            .map_err(|_| ToolError::Unavailable("Tool supervisor ended".to_string()))?;
        receive
            .await
            .map_err(|_| ToolError::Unavailable("Tool execution task ended".to_string()))
    }
}

struct PendingCall {
    outbound: mpsc::Sender<OutboundEvent>,
    notification: schema::SessionNotification,
    cancelled: Arc<AtomicBool>,
    cancel: CancellationToken,
}

impl PendingCall {
    fn cancel(self) {
        self.cancelled.store(true, Ordering::Release);
        self.cancel.cancel();
        let (ack, _) = oneshot::channel();
        if let Err(error) =
            self.outbound
                .try_send(OutboundEvent::SessionUpdate(self.notification, None, ack))
        {
            tracing::warn!(%error, "Could not deliver cancelled ACP tool-call update");
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct ActiveToolCalls(Arc<SyncMutex<HashMap<String, PendingCall>>>);

impl ActiveToolCalls {
    pub(crate) fn cancel_all(&self) {
        let calls = std::mem::take(&mut *self.0.lock().unwrap());
        for call in calls.into_values() {
            call.cancel();
        }
    }
}

pub(crate) struct CancelledCallNotice {
    calls: ActiveToolCalls,
    id: String,
    cancelled: Arc<AtomicBool>,
    pub(crate) cancel: CancellationToken,
    #[cfg(feature = "component-generation")]
    pub(crate) outbound: mpsc::Sender<OutboundEvent>,
}

impl CancelledCallNotice {
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire) || self.cancel.is_cancelled()
    }

    pub(crate) fn finish(&self) {
        self.calls.0.lock().unwrap().remove(&self.id);
    }
}

impl Drop for CancelledCallNotice {
    fn drop(&mut self) {
        if let Some(call) = self.calls.0.lock().unwrap().remove(&self.id) {
            call.cancel();
        }
    }
}

#[derive(Clone, Default)]
struct BrokerState {
    generation: u64,
    core_generation: Option<CatalogGeneration>,
    handles: HashMap<String, ToolDescriptor>,
    #[cfg(feature = "component-generation")]
    generated: HashMap<String, wassette::store::InstallReceipt>,
}

pub struct ToolBroker {
    pub(crate) manager: Arc<LifecycleManager>,
    exposed: HashSet<String>,
    excluded: HashSet<String>,
    state: Arc<Mutex<Arc<BrokerState>>>,
    next_handle: Arc<AtomicU64>,
    next_call: Arc<AtomicU64>,
    pub(crate) workers: Arc<ToolWorkers>,
    view_changed: tokio::sync::Notify,
    #[cfg(all(test, feature = "component-generation"))]
    after_generated_read: SyncMutex<Option<PublicationPause>>,
    #[cfg(all(test, feature = "component-generation"))]
    before_generated_publish: SyncMutex<Option<PublicationPause>>,
}

#[cfg(all(test, feature = "component-generation"))]
type PublicationPause = (oneshot::Sender<()>, oneshot::Receiver<()>);

#[cfg(all(test, feature = "component-generation"))]
async fn pause_publication(hook: &SyncMutex<Option<PublicationPause>>) {
    let pause = hook.lock().unwrap().take();
    if let Some((ready, resume)) = pause {
        ready.send(()).unwrap();
        resume.await.unwrap();
    }
}

pub struct PreparedToolCall {
    pub id: String,
    pub descriptor: ToolDescriptor,
    invocation: PreparedInvocation,
}

impl ToolBroker {
    pub fn new(
        manager: Arc<LifecycleManager>,
        exposed: impl IntoIterator<Item = String>,
        excluded: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            manager,
            exposed: exposed.into_iter().collect(),
            excluded: excluded.into_iter().collect(),
            state: Arc::new(Mutex::new(Arc::new(BrokerState::default()))),
            next_handle: Arc::new(AtomicU64::new(1)),
            next_call: Arc::new(AtomicU64::new(1)),
            workers: Arc::new(ToolWorkers::new(MAX_CONCURRENT_CALLS)),
            view_changed: tokio::sync::Notify::new(),
            #[cfg(all(test, feature = "component-generation"))]
            after_generated_read: SyncMutex::new(None),
            #[cfg(all(test, feature = "component-generation"))]
            before_generated_publish: SyncMutex::new(None),
        }
    }

    /// Fresh session visibility with the same manager and bounded job supervisor.
    pub(crate) fn session_view(&self) -> Self {
        Self {
            manager: self.manager.clone(),
            exposed: self.exposed.clone(),
            excluded: self.excluded.clone(),
            state: Arc::new(Mutex::new(Arc::new(BrokerState::default()))),
            next_handle: self.next_handle.clone(),
            next_call: self.next_call.clone(),
            workers: self.workers.clone(),
            view_changed: tokio::sync::Notify::new(),
            #[cfg(all(test, feature = "component-generation"))]
            after_generated_read: SyncMutex::new(None),
            #[cfg(all(test, feature = "component-generation"))]
            before_generated_publish: SyncMutex::new(None),
        }
    }

    /// Whether the operator profile lets guests build and install components.
    /// Exposure and rebuild remain separately checked per request.
    pub(crate) fn generation_available(&self) -> bool {
        #[cfg(feature = "component-generation")]
        {
            self.manager.generation_service().is_ok_and(|service| {
                let permissions = service.permissions();
                permissions.can_build() && permissions.can_install()
            })
        }
        #[cfg(not(feature = "component-generation"))]
        {
            false
        }
    }

    pub(crate) fn next_call_id(&self) -> String {
        format!(
            "wassette-tool-{}",
            self.next_call.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// Add only this call's exact committed generated tool revision to this view.
    #[cfg(feature = "component-generation")]
    pub(crate) async fn expose_generated(
        &self,
        receipt: &wassette::store::InstallReceipt,
        cancel: &CancellationToken,
    ) -> Result<Vec<String>, ToolError> {
        if !receipt.requests_tool_exposure()
            || !matches!(
                receipt.source,
                wassette::store::SourceIdentity::Generated { .. }
            )
            || self.excluded.contains(receipt.component_id.as_str())
        {
            return Err(ToolError::PolicyDenied(
                "Only an approved generated ordinary tool may enter this session".into(),
            ));
        }
        for _ in 0..8 {
            if cancel.is_cancelled() {
                return Err(ToolError::Cancelled);
            }
            let store = self.manager.component_store().clone();
            let expected = receipt.clone();
            let cursor = tokio::task::spawn_blocking(move || {
                let current = store
                    .read(expected.component_id.as_str())
                    .map_err(Self::exposure_store_error)?;
                if current.receipt != expected {
                    return Err(ToolError::Stale(
                        "Generated receipt is no longer current".into(),
                    ));
                }
                Ok(current.cursor)
            })
            .await
            .map_err(|_| ToolError::Unavailable("Exposure verification worker ended".into()))??;
            #[cfg(test)]
            pause_publication(&self.after_generated_read).await;
            let snapshot = self
                .manager
                .catalog()
                .await
                .map_err(|_| ToolError::Unavailable("Tool catalog unavailable".into()))?;
            let matching: Vec<_> = snapshot
                .tools
                .iter()
                .filter(|tool| tool.reference.key().component_id == receipt.component_id)
                .collect();
            if matching
                .iter()
                .any(|tool| tool.reference.revision() != &receipt.revision)
            {
                return Err(ToolError::Stale(
                    "Generated tool revision is no longer callable".into(),
                ));
            }
            let observed = self.state.lock().await.clone();
            let mut next = (*observed).clone();
            next.generated
                .insert(receipt.component_id.as_str().to_owned(), receipt.clone());
            next.core_generation = None;
            self.update_catalog_state(&mut next, snapshot);
            let handles = next
                .handles
                .iter()
                .filter(|(_, tool)| {
                    tool.reference.key().component_id == receipt.component_id
                        && tool.reference.revision() == &receipt.revision
                })
                .map(|(handle, _)| handle.clone())
                .collect::<Vec<_>>();
            let next = Arc::new(next);
            let state = self.state.clone();
            let store = self.manager.component_store().clone();
            let expected = receipt.clone();
            let cancelled = cancel.clone();
            #[cfg(test)]
            pause_publication(&self.before_generated_publish).await;
            let published = tokio::task::spawn_blocking(move || {
                let _scope = match store.checked_read(
                    &cursor,
                    Some((expected.component_id.as_str(), &expected.revision)),
                ) {
                    Ok(scope) => scope,
                    Err(wassette::store::StoreError::Conflict(_)) => return Ok(false),
                    Err(error) => return Err(Self::exposure_store_error(error)),
                };
                let Ok(mut live) = state.try_lock() else {
                    return Ok(false);
                };
                if !Arc::ptr_eq(&live, &observed) {
                    return Ok(false);
                }
                if cancelled.is_cancelled() {
                    return Err(ToolError::Cancelled);
                }
                *live = next;
                Ok(true)
            })
            .await
            .map_err(|_| ToolError::Unavailable("Session exposure worker ended".into()))??;
            if published {
                self.view_changed.notify_waiters();
                return Ok(handles);
            }
            tokio::task::yield_now().await;
        }
        Err(ToolError::Unavailable(
            "Store or session view kept changing during exposure".into(),
        ))
    }

    pub async fn catalog(&self) -> Result<Catalog, ToolError> {
        let mut state = self.state.lock().await;
        let snapshot = self
            .manager
            .catalog()
            .await
            .map_err(|error| ToolError::Unavailable(error.to_string()))?;
        if state.core_generation.as_ref() == Some(&snapshot.generation) {
            return Ok(catalog_from_state(&state));
        }
        let mut next = (**state).clone();
        self.update_catalog_state(&mut next, snapshot);
        *state = Arc::new(next);
        Ok(catalog_from_state(&state))
    }

    #[cfg(feature = "component-generation")]
    fn exposure_store_error(error: wassette::store::StoreError) -> ToolError {
        match error {
            wassette::store::StoreError::NotFound(_) | wassette::store::StoreError::Conflict(_) => {
                ToolError::Stale("Generated receipt is no longer current".into())
            }
            _ => ToolError::Unavailable(
                "Cannot verify generated receipt for session exposure".into(),
            ),
        }
    }

    fn update_catalog_state(&self, state: &mut BrokerState, snapshot: CatalogSnapshot) {
        let visible: Vec<_> = snapshot
            .tools
            .into_iter()
            .filter(|tool| {
                (self.exposed.contains(tool.tool.key.component_id.as_str())
                    || session_generated_visible(state, tool))
                    && !self.excluded.contains(tool.tool.key.component_id.as_str())
            })
            .collect();

        if state.core_generation.as_ref() != Some(&snapshot.generation) {
            let unchanged = state.handles.len() == visible.len()
                && visible.iter().all(|descriptor| {
                    state
                        .handles
                        .values()
                        .any(|old| old.reference == descriptor.reference)
                });
            state.core_generation = Some(snapshot.generation);
            if unchanged {
                return;
            }
            let previous = std::mem::take(&mut state.handles);
            state.handles = visible
                .into_iter()
                .map(|descriptor| {
                    let handle = previous
                        .iter()
                        .find_map(|(handle, old)| {
                            (old.reference == descriptor.reference).then(|| handle.clone())
                        })
                        .unwrap_or_else(|| {
                            format!("tool-{}", self.next_handle.fetch_add(1, Ordering::Relaxed))
                        });
                    (handle, descriptor)
                })
                .collect();
            state.generation = state.generation.wrapping_add(1).max(1);
        }
    }

    pub async fn wait_for_change(&self, after: u64) -> Result<u64, ToolError> {
        loop {
            let changed = self.view_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let (generation, core_generation) = {
                let state = self.state.lock().await;
                (state.generation, state.core_generation.clone())
            };
            if generation != after {
                return Ok(generation);
            }
            let Some(core_generation) = core_generation else {
                self.catalog().await?;
                continue;
            };
            tokio::select! {
                result = self.manager.wait_changed(&core_generation) => {
                    result.map_err(|error| ToolError::Unavailable(error.to_string()))?;
                }
                _ = &mut changed => {}
            }
            let current = self.catalog().await?.generation;
            if current != after {
                return Ok(current);
            }
        }
    }

    pub async fn reference(&self, handle: &str) -> Result<ToolRef, ToolError> {
        let _ = self.catalog().await?;
        self.state
            .lock()
            .await
            .handles
            .get(handle)
            .map(|descriptor| descriptor.reference.clone())
            .ok_or_else(|| ToolError::Stale(format!("Unknown or stale tool handle `{handle}`")))
    }

    pub async fn reference_by_name(&self, name: &str) -> Result<ToolRef, ToolError> {
        let _ = self.catalog().await?;
        let state = self.state.lock().await;
        let matches: Vec<_> = state
            .handles
            .values()
            .filter(|descriptor| descriptor.tool.schema["name"].as_str() == Some(name))
            .collect();
        match matches.as_slice() {
            [] => Err(ToolError::NotFound(name.to_owned())),
            [descriptor] => Ok(descriptor.reference.clone()),
            descriptors => Err(ToolError::Ambiguous(
                descriptors
                    .iter()
                    .map(|descriptor| descriptor.tool.key.component_id.as_str().to_owned())
                    .collect(),
            )),
        }
    }

    pub async fn prepare_call(
        &self,
        reference: ToolRef,
        arguments_json: &str,
    ) -> Result<PreparedToolCall, ToolError> {
        let arguments: Value = serde_json::from_str(arguments_json)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let descriptor = self
            .manager
            .describe_tool(&reference)
            .await
            .map_err(|error| match error.downcast::<ToolInvocationError>() {
                Ok(error) => map_invocation_error(error),
                Err(error) => ToolError::Unavailable(error.to_string()),
            })?;
        let prepared = self
            .manager
            .prepare_invocation(&reference, &arguments)
            .await
            .map_err(map_invocation_error)?;
        Ok(PreparedToolCall {
            id: self.next_call_id(),
            descriptor,
            invocation: prepared,
        })
    }

    pub async fn run_call(&self, call: PreparedToolCall) -> Result<ToolResult, ToolError> {
        let call_id = call.id.clone();
        let output = self
            .workers
            .run(async move { call.invocation.run().await.map_err(map_invocation_error) })
            .await??;
        let presentation = wassette::tool_result::present_tool_output(
            &output.raw_result,
            output.descriptor.tool.schema.get("outputSchema"),
        )
        .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        Ok(ToolResult {
            tool_call_id: call_id,
            text: presentation.text,
            structured: presentation
                .structured
                .map(|value| serde_json::to_string(&value))
                .transpose()
                .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?,
        })
    }

    async fn execute_with_permission<T: Send>(
        accessor: &Accessor<T, HasSelf<HostState>>,
        broker: Arc<ToolBroker>,
        reference: ToolRef,
        arguments_json: String,
    ) -> Result<ToolResult, ToolError> {
        let session_id = accessor
            .with(|mut access| access.get().editor_session_id.clone())
            .ok_or(ToolError::SessionNotBound)?;
        let call = broker.prepare_call(reference, &arguments_json).await?;
        let reference = call.descriptor.reference.clone();
        let title = format!(
            "Run {}",
            call.descriptor.tool.schema["name"]
                .as_str()
                .unwrap_or("Wassette tool")
        );
        let call_id = call.id.clone();
        let snapshot = |status, raw_output| ToolCallSnapshot {
            id: call_id.clone(),
            title: title.clone(),
            kind: ToolKind::Execute,
            status,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: Some(arguments_json.clone()),
            raw_output,
        };
        let mut cancelled_snapshot = snapshot(
            ToolCallStatus::Failed,
            Some("Tool call cancelled; execution may still be finishing.".to_string()),
        );
        // Cancellation bypasses the chain, so apply the same namespace as
        // the normal notification and permission callbacks at the boundary.
        accessor.with(|mut access| {
            if let Some(route) = &access.get().provider_routing {
                cancelled_snapshot.id = route.tool_call_id(&cancelled_snapshot.id);
            }
        });
        let cancellation = track_call(accessor, session_id.clone(), &call_id, cancelled_snapshot)?;
        crate::client_impl::notify_session(
            accessor,
            session_id.clone(),
            SessionUpdate::ToolCall(snapshot(ToolCallStatus::Pending, None)),
        )
        .await;
        let remembered = accessor.with(|mut access| {
            access
                .get()
                .tool_decisions
                .iter()
                .find_map(|(known, allowed)| (known == &reference).then_some(*allowed))
        });
        let allowed = match remembered {
            Some(allowed) => allowed,
            None => {
                let permission = crate::client_impl::request_permission(
                    accessor,
                    RequestPermissionRequest {
                        session_id: session_id.clone(),
                        tool_call: snapshot(ToolCallStatus::Pending, None),
                        options: permission_options(),
                    },
                )
                .await;
                if cancellation.is_cancelled() {
                    return Err(ToolError::Cancelled);
                }
                let permission = match permission {
                    Ok(permission) => permission,
                    Err(error) => {
                        crate::client_impl::notify_session(
                            accessor,
                            session_id,
                            SessionUpdate::ToolCallUpdate(snapshot(
                                ToolCallStatus::Failed,
                                Some(error.message.clone()),
                            )),
                        )
                        .await;
                        cancellation.finish();
                        return Err(ToolError::Unavailable(error.message));
                    }
                };
                match permission.outcome {
                    PermissionOutcome::Selected(id) if id == "allow-once" => true,
                    PermissionOutcome::Selected(id) if id == "allow-always" => {
                        remember_decision(accessor, reference.clone(), true);
                        true
                    }
                    PermissionOutcome::Selected(id) if id == "reject-always" => {
                        remember_decision(accessor, reference.clone(), false);
                        false
                    }

                    _ => false,
                }
            }
        };
        if !allowed {
            crate::client_impl::notify_session(
                accessor,
                session_id,
                SessionUpdate::ToolCallUpdate(snapshot(ToolCallStatus::Failed, None)),
            )
            .await;
            cancellation.finish();
            return Err(ToolError::PermissionDenied);
        }

        crate::client_impl::notify_session(
            accessor,
            session_id.clone(),
            SessionUpdate::ToolCallUpdate(snapshot(ToolCallStatus::InProgress, None)),
        )
        .await;
        if cancellation.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let result = broker.run_call(call).await;
        if cancellation.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let (status, raw_output) = match &result {
            Ok(output) => (ToolCallStatus::Completed, Some(output.text.clone())),
            Err(error) => (ToolCallStatus::Failed, Some(format!("{error:?}"))),
        };
        crate::client_impl::notify_session(
            accessor,
            session_id,
            SessionUpdate::ToolCallUpdate(snapshot(status, raw_output)),
        )
        .await;
        cancellation.finish();
        result
    }
}

#[cfg(feature = "component-generation")]
fn session_generated_visible(state: &BrokerState, tool: &ToolDescriptor) -> bool {
    state
        .generated
        .get(tool.reference.key().component_id.as_str())
        .is_some_and(|receipt| &receipt.revision == tool.reference.revision())
}

#[cfg(not(feature = "component-generation"))]
fn session_generated_visible(_: &BrokerState, _: &ToolDescriptor) -> bool {
    false
}

pub(crate) fn track_call<T: Send>(
    accessor: &Accessor<T, HasSelf<HostState>>,
    session_id: String,
    id: &str,
    cancelled_snapshot: ToolCallSnapshot,
) -> Result<CancelledCallNotice, ToolError> {
    let (outbound, active_calls, cancel) = accessor.with(|mut access| {
        let state = access.get();
        let outbound = state
            .stages
            .iter()
            .find_map(|stage| match &stage.sink {
                ClientSink::Outbound(outbound) => Some(outbound.clone()),
                ClientSink::Upstream(_) => None,
            })
            .ok_or_else(|| ToolError::Unavailable("No editor route for tool updates".into()))?;
        let cancel = state
            .tool_broker
            .as_ref()
            .ok_or_else(|| ToolError::Unavailable("Tool broker unavailable".into()))?
            .workers
            .cancellation_token();
        Ok::<_, ToolError>((outbound, state.active_tool_calls.clone(), cancel))
    })?;
    // Store callback cancellation must still reach the editor without polling a layer.
    let notification = crate::translate::session_update_wit_to_schema(
        session_id,
        SessionUpdate::ToolCallUpdate(cancelled_snapshot),
    )
    .ok_or_else(|| ToolError::Unavailable("Could not encode cancellation update".into()))?;
    let cancelled = Arc::new(AtomicBool::new(false));
    active_calls.0.lock().unwrap().insert(
        id.to_owned(),
        PendingCall {
            outbound: outbound.clone(),
            notification,
            cancelled: cancelled.clone(),
            cancel: cancel.clone(),
        },
    );
    Ok(CancelledCallNotice {
        calls: active_calls,
        id: id.to_owned(),
        cancelled,
        cancel,
        #[cfg(feature = "component-generation")]
        outbound,
    })
}

fn remember_decision<T: Send>(
    accessor: &Accessor<T, HasSelf<HostState>>,
    reference: ToolRef,
    allowed: bool,
) {
    accessor.with(|mut access| {
        let decisions = &mut access.get().tool_decisions;
        decisions.retain(|(known, _)| known != &reference);
        decisions.push((reference, allowed));
    });
}

fn catalog_from_state(state: &BrokerState) -> Catalog {
    Catalog {
        generation: state.generation,
        tools: state
            .handles
            .iter()
            .map(|(handle, descriptor)| descriptor_to_wit(handle, descriptor))
            .collect(),
    }
}

fn permission_options() -> Vec<PermissionOption> {
    vec![
        PermissionOption {
            id: "allow-once".to_owned(),
            name: "Allow once".to_owned(),
            kind: PermissionOptionKind::AllowOnce,
        },
        PermissionOption {
            id: "allow-always".to_owned(),
            name: "Always allow this revision".to_owned(),
            kind: PermissionOptionKind::AllowAlways,
        },
        PermissionOption {
            id: "reject-once".to_owned(),
            name: "Reject".to_owned(),
            kind: PermissionOptionKind::RejectOnce,
        },
        PermissionOption {
            id: "reject-always".to_owned(),
            name: "Always reject this revision".to_owned(),
            kind: PermissionOptionKind::RejectAlways,
        },
    ]
}

fn descriptor_to_wit(handle: &str, descriptor: &ToolDescriptor) -> WitToolDescriptor {
    let schema = &descriptor.tool.schema;
    let export = &descriptor.tool.key.export;
    WitToolDescriptor {
        handle: handle.to_owned(),
        name: schema["name"].as_str().unwrap_or_default().to_owned(),
        component_id: descriptor.tool.key.component_id.as_str().to_owned(),
        export_name: format!(
            "{}{}{}",
            export
                .package_name
                .as_deref()
                .map(|package| format!("{package}/"))
                .unwrap_or_default(),
            export
                .interface_name
                .as_deref()
                .map(|interface| format!("{interface}."))
                .unwrap_or_default(),
            export.function_name
        ),
        description: schema["description"].as_str().map(str::to_owned),
        input_schema: serde_json::to_string(&schema["inputSchema"])
            .unwrap_or_else(|_| "{}".to_owned()),
        output_schema: schema
            .get("outputSchema")
            .filter(|value| !value.is_null())
            .and_then(|value| serde_json::to_string(value).ok()),
    }
}

fn map_invocation_error(error: ToolInvocationError) -> ToolError {
    match error {
        ToolInvocationError::NotFound(error) => ToolError::NotFound(error.to_string()),
        ToolInvocationError::Stale(error) => ToolError::Stale(error.to_string()),
        ToolInvocationError::InvalidArguments(error) => {
            ToolError::InvalidArguments(error.to_string())
        }
        ToolInvocationError::PolicyDenied(error) => ToolError::PolicyDenied(error.to_string()),
        ToolInvocationError::ExecutionFailed(error) => {
            ToolError::ExecutionFailed(error.to_string())
        }
        ToolInvocationError::Unavailable(error) => ToolError::Unavailable(error.to_string()),
    }
}

impl<T: Send> crate::wassette::component_tools::tools::HostWithStore<T> for HasSelf<HostState> {
    fn list_tools(
        accessor: &Accessor<T, Self>,
        known_generation: Option<u64>,
    ) -> impl Future<Output = Result<CatalogResult, ToolError>> + Send {
        let broker = accessor.with(|mut access| access.get().tool_broker.clone());
        async move {
            let broker = broker
                .ok_or_else(|| ToolError::Unavailable("Tool broker unavailable".to_owned()))?;
            let catalog = broker.catalog().await?;
            Ok(if known_generation == Some(catalog.generation) {
                CatalogResult::Unchanged(catalog.generation)
            } else {
                CatalogResult::Changed(catalog)
            })
        }
    }

    fn wait_for_change(
        accessor: &Accessor<T, Self>,
        after: u64,
    ) -> impl Future<Output = Result<u64, ToolError>> + Send {
        let broker = accessor.with(|mut access| access.get().tool_broker.clone());
        async move {
            broker
                .ok_or_else(|| ToolError::Unavailable("Tool broker unavailable".to_owned()))?
                .wait_for_change(after)
                .await
        }
    }

    fn call_tool(
        accessor: &Accessor<T, Self>,
        handle: String,
        arguments_json: String,
    ) -> impl Future<Output = Result<ToolResult, ToolError>> + Send {
        let (broker, bound, provider) = accessor.with(|mut access| {
            let state = access.get();
            (
                state.tool_broker.clone(),
                state.editor_session_id.is_some(),
                matches!(
                    state.current_stage().kind,
                    crate::state::StageKind::Provider
                ),
            )
        });
        async move {
            if !bound {
                return Err(ToolError::SessionNotBound);
            }
            let broker = broker
                .ok_or_else(|| ToolError::Unavailable("Tool broker unavailable".to_owned()))?;
            if !provider {
                return Err(ToolError::PolicyDenied(
                    "ACP layers cannot call Wassette tools".to_owned(),
                ));
            }
            let reference = broker.reference(&handle).await?;
            ToolBroker::execute_with_permission(accessor, broker, reference, arguments_json).await
        }
    }

    fn call_tool_by_name(
        accessor: &Accessor<T, Self>,
        name: String,
        arguments_json: String,
    ) -> impl Future<Output = Result<ToolResult, ToolError>> + Send {
        let (broker, bound, provider) = accessor.with(|mut access| {
            let state = access.get();
            (
                state.tool_broker.clone(),
                state.editor_session_id.is_some(),
                matches!(
                    state.current_stage().kind,
                    crate::state::StageKind::Provider
                ),
            )
        });
        async move {
            if !bound {
                return Err(ToolError::SessionNotBound);
            }
            let broker = broker
                .ok_or_else(|| ToolError::Unavailable("Tool broker unavailable".to_owned()))?;
            if !provider {
                return Err(ToolError::PolicyDenied(
                    "ACP layers cannot call Wassette tools".to_owned(),
                ));
            }
            let reference = broker.reference_by_name(&name).await?;
            ToolBroker::execute_with_permission(accessor, broker, reference, arguments_json).await
        }
    }
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::*;

    async fn install_test_tool(
        manager: &LifecycleManager,
        root: &std::path::Path,
        name: &str,
        key: &str,
    ) {
        let bytes = wat::parse_str(format!(
            r#"(component ${name}
            (core module $m (func (export "run") (result i32) i32.const 1))
            (core instance $i (instantiate $m))
            (func (export "run") (result u32) (canon lift (core func $i "run"))))"#
        ))
        .unwrap();
        let path = root.join(format!("{key}.wasm"));
        std::fs::write(&path, bytes).unwrap();
        manager
            .load_component(&format!("file://{}", path.display()))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn catalog_filters_exposure_preserves_ambiguity_and_invalidates_handles() {
        let root = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            LifecycleManager::builder(root.path().join("store"))
                .with_secrets_dir(root.path().join("secrets"))
                .build()
                .await
                .unwrap(),
        );
        let broker = ToolBroker::new(
            manager.clone(),
            ["semantic:one".into(), "semantic:two".into()],
            ["semantic:two".into()],
        );
        let empty = broker.catalog().await.unwrap();
        install_test_tool(&manager, root.path(), "hidden", "hidden-file").await;
        assert_eq!(broker.catalog().await.unwrap().generation, empty.generation);
        install_test_tool(&manager, root.path(), "semantic:one", "first-file").await;
        let visible = broker.catalog().await.unwrap();
        assert_eq!(visible.tools.len(), 1);
        assert_eq!(visible.tools[0].component_id, "semantic:one");
        assert_ne!(visible.generation, empty.generation);
        install_test_tool(&manager, root.path(), "semantic:two", "second-file").await;
        assert_eq!(
            broker.catalog().await.unwrap().generation,
            visible.generation
        );
        let ambiguous = ToolBroker::new(
            manager.clone(),
            ["semantic:one".into(), "semantic:two".into()],
            [],
        );
        assert!(
            matches!(ambiguous.reference_by_name("run").await, Err(ToolError::Ambiguous(ids)) if ids.len() == 2)
        );
        let handle = &visible.tools[0].handle;
        let reference = broker.reference(handle).await.unwrap();
        let prepared = broker.prepare_call(reference, "{}").await.unwrap();
        manager.unload_component("semantic:one").await.unwrap();
        assert!(matches!(
            broker.reference(handle).await,
            Err(ToolError::Stale(_))
        ));
        assert!(matches!(
            broker.run_call(prepared).await,
            Err(ToolError::Stale(_))
        ));
    }

    #[tokio::test]
    async fn initial_empty_catalog_wait_does_not_report_a_change() {
        let root = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            LifecycleManager::builder(root.path().join("store"))
                .with_secrets_dir(root.path().join("secrets"))
                .build()
                .await
                .unwrap(),
        );
        let broker = ToolBroker::new(manager, [], []);
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                broker.wait_for_change(0)
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn cancelled_waiter_retains_permit_and_supervised_job() {
        let workers = Arc::new(ToolWorkers::new(1));
        let (started, started_rx) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let (finished, finished_rx) = oneshot::channel();
        let worker = workers.clone();
        let waiter = tokio::spawn(async move {
            worker
                .run(async move {
                    started.send(()).unwrap();
                    released.await.unwrap();
                    finished.send(()).unwrap();
                })
                .await
        });
        started_rx.await.unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(matches!(workers.run(async {}).await, Err(ToolError::Busy)));
        drop(workers);
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), finished_rx)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn completed_jobs_release_permits() {
        let workers = ToolWorkers::new(1);
        assert_eq!(workers.run(async { 42 }).await.unwrap(), 42);
        assert_eq!(workers.run(async { 43 }).await.unwrap(), 43);
    }

    #[tokio::test]
    async fn shutdown_cancels_precommit_work_and_waits_for_reaping() {
        let workers = Arc::new(ToolWorkers::new(1));
        let cancel = workers.cancellation_token();
        let (started, ready) = oneshot::channel();
        let (cancelled, observed_cancel) = oneshot::channel();
        let (release, reaped) = oneshot::channel();
        let worker = workers.clone();
        let waiter = tokio::spawn(async move {
            worker
                .run(async move {
                    started.send(()).unwrap();
                    cancel.cancelled().await;
                    cancelled.send(()).unwrap();
                    reaped.await.unwrap();
                })
                .await
        });
        ready.await.unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        workers.close();
        let drain = workers.clone();
        let shutdown = tokio::spawn(async move { drain.shutdown().await });
        observed_cancel.await.unwrap();
        assert!(!shutdown.is_finished());
        assert_eq!(workers.permits.available_permits(), 0);
        assert!(matches!(
            workers.run(async {}).await,
            Err(ToolError::Unavailable(_))
        ));
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), shutdown)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(workers.permits.available_permits(), 1);
        workers.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_drains_accepted_jobs_even_after_both_waiters_are_cancelled() {
        let workers = Arc::new(ToolWorkers::new(1));
        let (accepted, ready) = oneshot::channel();
        let (release, complete) = oneshot::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let committed = finished.clone();
        let worker = workers.clone();
        let waiter = tokio::spawn(async move {
            worker
                .run(async move {
                    accepted.send(()).unwrap();
                    complete.await.unwrap();
                    committed.store(true, Ordering::Release);
                })
                .await
        });
        ready.await.unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        workers.close();
        let drain = workers.clone();
        let interrupted_shutdown = tokio::spawn(async move { drain.shutdown().await });
        tokio::task::yield_now().await;
        assert!(!finished.load(Ordering::Acquire));
        assert!(!interrupted_shutdown.is_finished());
        interrupted_shutdown.abort();
        assert!(interrupted_shutdown.await.unwrap_err().is_cancelled());
        assert_eq!(workers.permits.available_permits(), 0);
        let drain = workers.clone();
        let shutdown = tokio::spawn(async move { drain.shutdown().await });
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), shutdown)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(finished.load(Ordering::Acquire));
        assert_eq!(workers.permits.available_permits(), 1);
    }

    #[test]
    fn invocation_errors_map_without_string_classification() {
        let cases = [
            (
                ToolInvocationError::NotFound(anyhow!("missing")),
                "ToolError::NotFound(\"missing\")",
            ),
            (
                ToolInvocationError::Stale(anyhow!("changed")),
                "ToolError::Stale(\"changed\")",
            ),
            (
                ToolInvocationError::InvalidArguments(anyhow!("bad json")),
                "ToolError::InvalidArguments(\"bad json\")",
            ),
            (
                ToolInvocationError::PolicyDenied(anyhow!("denied")),
                "ToolError::PolicyDenied(\"denied\")",
            ),
            (
                ToolInvocationError::ExecutionFailed(anyhow!("trap")),
                "ToolError::ExecutionFailed(\"trap\")",
            ),
            (
                ToolInvocationError::Unavailable(anyhow!("offline")),
                "ToolError::Unavailable(\"offline\")",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(format!("{:?}", map_invocation_error(input)), expected);
        }
    }

    #[cfg(feature = "component-generation")]
    mod generation_tests {
        use wassette::store::{
            GenerationEvidence, InstallIntent, InstallOptions, InstallOwner, OriginEvidence,
            PolicyProvenance, PreparedInstall, PreparedPolicy, SourceIdentity, ValidationEvidence,
        };

        use super::*;

        async fn generated_tool(
            manager: &LifecycleManager,
            intent: InstallIntent,
            value: u32,
        ) -> wassette::store::InstallReceipt {
            let wasm = wat::parse_str(format!(
                r#"(component $example:generated
                (core module $m (func (export "run") (result i32) i32.const {value}))
                (core instance $i (instantiate $m))
                (func (export "run") (result u32) (canon lift (core func $i "run"))))"#
            ))
            .unwrap();
            generated_artifact(manager, intent, wasm).await
        }

        async fn generated_artifact(
            manager: &LifecycleManager,
            intent: InstallIntent,
            wasm: Vec<u8>,
        ) -> wassette::store::InstallReceipt {
            let source = SourceIdentity::Generated { id: "1".repeat(32) };
            let key = wassette::StorageKey::parse("generated_fixture").unwrap();
            let store = manager.component_store();
            let expected = store.observe("example:generated", &key, &source).unwrap();
            let prepared = PreparedInstall::prepare(
                wasm,
                InstallOptions {
                    storage_key: key,
                    source,
                    origin: OriginEvidence {
                        location: format!("generated://{}", "1".repeat(32)),
                        requested_version: None,
                        selected_version: None,
                        manifest_digest: None,
                        immutable_uri: None,
                        generation: Some(GenerationEvidence {
                            source_sha256: "a".repeat(64),
                            wit_sha256: "b".repeat(64),
                            wit_dependencies_sha256: "b".repeat(64),
                            builder_initrd_sha256: "c".repeat(64),
                            builder_helper_sha256: "c".repeat(64),
                            builder_manifest_digest: None,
                            profile: "test".into(),
                            profile_sha256: "d".repeat(64),
                            compiler: "test".into(),
                            bindgen: "test".into(),
                            binding_runtime: "test".into(),
                            vm_runtime: "test".into(),
                            world: "tool".into(),
                            target: "wasm32-wasip2".into(),
                            host_platform: "test".into(),
                        }),
                    },
                    owner: InstallOwner::Explicit,
                    intent,
                    policy: PreparedPolicy::absent(PolicyProvenance::Default),
                    observation: None,
                },
                |_, _, _| {
                    Ok(ValidationEvidence::OrdinaryPrepared {
                        runtime: "test".into(),
                    })
                },
            )
            .unwrap();
            let outcome = store.commit_install(prepared, expected).unwrap();
            manager.refresh_from_store().await.unwrap();
            outcome.entry.binding().clone()
        }

        #[tokio::test]
        async fn zero_callable_exports_are_valid_but_still_require_current_receipt_admission() {
            let root = tempfile::tempdir().unwrap();
            let manager = Arc::new(
                LifecycleManager::builder(root.path().join("store"))
                    .with_secrets_dir(root.path().join("secrets"))
                    .build()
                    .await
                    .unwrap(),
            );
            let wasm = wat::parse_str(
                r#"(component $example:generated
                    (instance $empty)
                    (export "example:empty/api@1.0.0" (instance $empty)))"#,
            )
            .unwrap();
            let receipt = generated_artifact(&manager, InstallIntent::ExposeTools, wasm).await;
            let broker = ToolBroker::new(manager.clone(), [], []);
            assert!(
                broker
                    .expose_generated(&receipt, &CancellationToken::new())
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert!(broker.catalog().await.unwrap().tools.is_empty());
            broker.workers.shutdown().await.unwrap();

            let broker = Arc::new(ToolBroker::new(manager.clone(), [], []));
            let (ready, paused) = oneshot::channel();
            let (resume, resumed) = oneshot::channel();
            *broker.after_generated_read.lock().unwrap() = Some((ready, resumed));
            let publisher = broker.clone();
            let publication = tokio::spawn(async move {
                publisher
                    .expose_generated(&receipt, &CancellationToken::new())
                    .await
            });
            paused.await.unwrap();
            manager.unload_component("example:generated").await.unwrap();
            resume.send(()).unwrap();
            assert!(matches!(
                publication.await.unwrap(),
                Err(ToolError::Stale(_))
            ));
            assert!(broker.state.lock().await.generated.is_empty());
            broker.workers.shutdown().await.unwrap();
        }

        #[tokio::test]
        async fn committed_tools_require_explicit_session_exposure_and_revision_invalidation() {
            let root = tempfile::tempdir().unwrap();
            let manager = Arc::new(
                LifecycleManager::builder(root.path().join("store"))
                    .with_secrets_dir(root.path().join("secrets"))
                    .build()
                    .await
                    .unwrap(),
            );
            let connection = ToolBroker::new(manager.clone(), [], []);
            let first = connection.session_view();
            let second = connection.session_view();
            let empty = first.catalog().await.unwrap();
            let receipt = generated_tool(&manager, InstallIntent::ExposeTools, 1).await;
            assert!(first.catalog().await.unwrap().tools.is_empty());
            assert!(second.catalog().await.unwrap().tools.is_empty());
            let handles = first
                .expose_generated(&receipt, &CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(handles.len(), 1);
            assert_eq!(first.catalog().await.unwrap().tools[0].handle, handles[0]);
            assert_ne!(first.catalog().await.unwrap().generation, empty.generation);
            assert!(second.catalog().await.unwrap().tools.is_empty());
            assert!(connection.catalog().await.unwrap().tools.is_empty());
            let replacement = generated_tool(&manager, InstallIntent::ExposeTools, 2).await;
            assert!(matches!(
                first.reference(&handles[0]).await,
                Err(ToolError::Stale(_))
            ));
            assert!(first.catalog().await.unwrap().tools.is_empty());
            assert!(matches!(
                first
                    .expose_generated(&receipt, &CancellationToken::new())
                    .await,
                Err(ToolError::Stale(_))
            ));
            let fresh = first
                .expose_generated(&replacement, &CancellationToken::new())
                .await
                .unwrap();
            assert_ne!(fresh, handles);
            assert!(second.catalog().await.unwrap().tools.is_empty());
        }

        #[tokio::test]
        async fn cancelled_and_install_only_results_cannot_enter_session_view() {
            let root = tempfile::tempdir().unwrap();
            let manager = Arc::new(
                LifecycleManager::builder(root.path().join("store"))
                    .with_secrets_dir(root.path().join("secrets"))
                    .build()
                    .await
                    .unwrap(),
            );
            let broker = ToolBroker::new(manager.clone(), [], []);
            let receipt = generated_tool(&manager, InstallIntent::InstallOnly, 1).await;
            assert!(matches!(
                broker
                    .expose_generated(&receipt, &CancellationToken::new())
                    .await,
                Err(ToolError::PolicyDenied(_))
            ));
            let receipt = generated_tool(&manager, InstallIntent::ExposeTools, 1).await;
            let cancelled = CancellationToken::new();
            cancelled.cancel();
            assert!(matches!(
                broker.expose_generated(&receipt, &cancelled).await,
                Err(ToolError::Cancelled)
            ));
            assert!(broker.catalog().await.unwrap().tools.is_empty());
            let mut wrong_source = receipt.clone();
            wrong_source.source = SourceIdentity::Generated { id: "2".repeat(32) };
            assert!(matches!(
                broker
                    .expose_generated(&wrong_source, &CancellationToken::new())
                    .await,
                Err(ToolError::Stale(_))
            ));
            let mut not_generated = receipt;
            not_generated.source = SourceIdentity::File(root.path().join("same-name.wasm"));
            assert!(matches!(
                broker
                    .expose_generated(&not_generated, &CancellationToken::new())
                    .await,
                Err(ToolError::PolicyDenied(_))
            ));
            assert!(broker.catalog().await.unwrap().tools.is_empty());
        }

        #[tokio::test]
        async fn generated_publication_rejects_removal_and_replacement_at_both_boundaries() {
            for before_publish in [false, true] {
                for replace in [false, true] {
                    let root = tempfile::tempdir().unwrap();
                    let manager = Arc::new(
                        LifecycleManager::builder(root.path().join("store"))
                            .with_secrets_dir(root.path().join("secrets"))
                            .build()
                            .await
                            .unwrap(),
                    );
                    let broker = Arc::new(ToolBroker::new(manager.clone(), [], []));
                    let receipt = generated_tool(&manager, InstallIntent::ExposeTools, 1).await;
                    let (ready, paused) = oneshot::channel();
                    let (resume, resumed) = oneshot::channel();
                    let hook = if before_publish {
                        &broker.before_generated_publish
                    } else {
                        &broker.after_generated_read
                    };
                    *hook.lock().unwrap() = Some((ready, resumed));
                    let publisher = broker.clone();
                    let publication = tokio::spawn(async move {
                        publisher
                            .expose_generated(&receipt, &CancellationToken::new())
                            .await
                    });
                    paused.await.unwrap();
                    if replace {
                        generated_tool(&manager, InstallIntent::ExposeTools, 2).await;
                    } else {
                        manager.unload_component("example:generated").await.unwrap();
                    }
                    resume.send(()).unwrap();
                    let result =
                        tokio::time::timeout(std::time::Duration::from_secs(5), publication)
                            .await
                            .unwrap()
                            .unwrap();
                    assert!(matches!(result, Err(ToolError::Stale(_))));
                    assert!(broker.state.lock().await.generated.is_empty());
                    assert!(broker.catalog().await.unwrap().tools.is_empty());
                    broker.workers.shutdown().await.unwrap();
                }
            }
        }

        #[tokio::test]
        async fn cancellation_at_final_publication_keeps_the_session_view_private() {
            let root = tempfile::tempdir().unwrap();
            let manager = Arc::new(
                LifecycleManager::builder(root.path().join("store"))
                    .with_secrets_dir(root.path().join("secrets"))
                    .build()
                    .await
                    .unwrap(),
            );
            let broker = Arc::new(ToolBroker::new(manager.clone(), [], []));
            let receipt = generated_tool(&manager, InstallIntent::ExposeTools, 1).await;
            let (ready, paused) = oneshot::channel();
            let (resume, resumed) = oneshot::channel();
            *broker.before_generated_publish.lock().unwrap() = Some((ready, resumed));
            let cancel = broker.workers.cancellation_token();
            let publisher = broker.clone();
            let publication =
                tokio::spawn(async move { publisher.expose_generated(&receipt, &cancel).await });
            paused.await.unwrap();
            broker.workers.close();
            resume.send(()).unwrap();
            assert!(matches!(
                publication.await.unwrap(),
                Err(ToolError::Cancelled)
            ));
            assert!(broker.state.lock().await.generated.is_empty());
            assert!(broker.catalog().await.unwrap().tools.is_empty());
            broker.workers.shutdown().await.unwrap();
        }
    }
}
