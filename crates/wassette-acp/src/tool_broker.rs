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
use wasmtime::component::{Accessor, HasSelf};
use wassette::{
    CatalogGeneration, LifecycleManager, PreparedInvocation, ToolDescriptor, ToolInvocationError,
    ToolRef,
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

struct ToolWorkers {
    permits: Arc<Semaphore>,
    jobs: mpsc::Sender<ExecutionJob>,
    _supervisor: JoinHandle<()>,
}

impl ToolWorkers {
    fn new(limit: usize) -> Self {
        let (jobs, mut incoming) = mpsc::channel::<ExecutionJob>(limit);
        let supervisor = tokio::spawn(async move {
            let mut running = JoinSet::new();
            loop {
                while let Some(result) = running.try_join_next() {
                    if let Err(error) = result {
                        tracing::error!(%error, "ACP tool execution task failed");
                    }
                }
                tokio::select! {
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
            while let Some(result) = running.join_next().await {
                if let Err(error) = result {
                    tracing::error!(%error, "ACP tool execution task failed during shutdown");
                }
            }
        });
        Self {
            permits: Arc::new(Semaphore::new(limit)),
            jobs,
            _supervisor: supervisor,
        }
    }

    async fn run<F, R>(&self, execution: F) -> Result<R, ToolError>
    where
        F: Future<Output = R> + Send + 'static,
        R: Send + 'static,
    {
        let permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| ToolError::Busy)?;
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
}

impl PendingCall {
    fn cancel(self) {
        self.cancelled.store(true, Ordering::Release);
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

struct CancelledCallNotice {
    calls: ActiveToolCalls,
    id: String,
    cancelled: Arc<AtomicBool>,
}

impl CancelledCallNotice {
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn finish(&self) {
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

#[derive(Default)]
struct BrokerState {
    generation: u64,
    core_generation: Option<CatalogGeneration>,
    handles: HashMap<String, ToolDescriptor>,
}

pub struct ToolBroker {
    manager: Arc<LifecycleManager>,
    exposed: HashSet<String>,
    excluded: HashSet<String>,
    state: Mutex<BrokerState>,
    next_handle: AtomicU64,
    next_call: AtomicU64,
    workers: ToolWorkers,
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
            state: Mutex::new(BrokerState::default()),
            next_handle: AtomicU64::new(1),
            next_call: AtomicU64::new(1),
            workers: ToolWorkers::new(MAX_CONCURRENT_CALLS),
        }
    }

    pub async fn catalog(&self) -> Result<Catalog, ToolError> {
        let mut state = self.state.lock().await;
        let snapshot = self
            .manager
            .catalog()
            .await
            .map_err(|error| ToolError::Unavailable(error.to_string()))?;
        let visible: Vec<_> = snapshot
            .tools
            .into_iter()
            .filter(|tool| {
                self.exposed.contains(tool.tool.key.component_id.as_str())
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
                return Ok(catalog_from_state(&state));
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
        Ok(catalog_from_state(&state))
    }

    pub async fn wait_for_change(&self, after: u64) -> Result<u64, ToolError> {
        loop {
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
            self.manager
                .wait_changed(&core_generation)
                .await
                .map_err(|error| ToolError::Unavailable(error.to_string()))?;
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
            id: format!(
                "wassette-tool-{}",
                self.next_call.fetch_add(1, Ordering::Relaxed)
            ),
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
        let (outbound, active_calls) = accessor.with(|mut access| {
            let state = access.get();
            let outbound = state
                .stages
                .iter()
                .find_map(|stage| match &stage.sink {
                    ClientSink::Outbound(outbound) => Some(outbound.clone()),
                    ClientSink::Upstream(_) => None,
                })
                .ok_or_else(|| {
                    ToolError::Unavailable("No editor route for tool updates".to_string())
                })?;
            Ok::<_, ToolError>((outbound, state.active_tool_calls.clone()))
        })?;
        // Cancellation can stop polling the store callback. Its final notice
        // must reach the bound editor without awaiting an upstream Wasm layer.
        let cancelled = Arc::new(AtomicBool::new(false));
        let notification = crate::translate::session_update_wit_to_schema(
            session_id.clone(),
            SessionUpdate::ToolCallUpdate(snapshot(
                ToolCallStatus::Failed,
                Some("Tool call cancelled; execution may still be finishing.".to_string()),
            )),
        )
        .ok_or_else(|| {
            ToolError::Unavailable("Could not encode tool cancellation update".to_string())
        })?;
        active_calls.0.lock().unwrap().insert(
            call_id.clone(),
            PendingCall {
                outbound,
                notification,
                cancelled: cancelled.clone(),
            },
        );
        let cancellation = CancelledCallNotice {
            calls: active_calls,
            id: call_id.clone(),
            cancelled,
        };
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
}
