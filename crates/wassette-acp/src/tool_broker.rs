// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use tokio::sync::{Mutex, Semaphore};
use wasmtime::component::{Accessor, HasSelf};
use wassette::{
    CatalogGeneration, LifecycleManager, PreparedInvocation, ToolDescriptor, ToolInvocationError,
    ToolRef,
};

use crate::state::HostState;
use crate::wassette::acp::prompts::SessionUpdate;
use crate::wassette::acp::tools::{
    PermissionOption, PermissionOptionKind, PermissionOutcome, RequestPermissionRequest,
    ToolCallSnapshot, ToolCallStatus, ToolKind,
};
use crate::wassette::component_tools::tools::{
    Catalog, CatalogResult, ToolDescriptor as WitToolDescriptor, ToolError, ToolResult,
};

const MAX_CONCURRENT_CALLS: usize = 8;

#[derive(Default)]
struct BrokerState {
    generation: u64,
    core_generation: Option<CatalogGeneration>,
    handles: HashMap<String, ToolDescriptor>,
    decisions: Vec<(ToolRef, bool)>,
}

pub struct ToolBroker {
    manager: Arc<LifecycleManager>,
    exposed: HashSet<String>,
    excluded: HashSet<String>,
    state: Mutex<BrokerState>,
    next_handle: AtomicU64,
    next_call: AtomicU64,
    permits: Arc<Semaphore>,
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
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT_CALLS)),
        }
    }

    pub async fn catalog(&self) -> Result<Catalog, ToolError> {
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

        let mut state = self.state.lock().await;
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
                return Ok(self.catalog().await?.generation);
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

    async fn remembered_decision(&self, reference: &ToolRef) -> Option<bool> {
        self.state
            .lock()
            .await
            .decisions
            .iter()
            .find_map(|(known, allowed)| (known == reference).then_some(*allowed))
    }

    async fn remember_decision(&self, reference: ToolRef, allowed: bool) {
        let mut state = self.state.lock().await;
        state.decisions.retain(|(known, _)| known != &reference);
        state.decisions.push((reference, allowed));
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
            .map_err(|error| ToolError::Stale(error.to_string()))?;
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
        let permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| ToolError::Busy)?;
        let (send, receive) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _permit = permit;
            let result = call.invocation.run().await.map_err(map_invocation_error);
            let _ = send.send(result);
        });
        let output = receive
            .await
            .map_err(|_| ToolError::Unavailable("Tool execution task ended".to_owned()))??;
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
        crate::client_impl::notify_session(
            accessor,
            session_id.clone(),
            SessionUpdate::ToolCall(snapshot(ToolCallStatus::Pending, None)),
        )
        .await;
        let allowed = match broker.remembered_decision(&reference).await {
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
                .await
                .map_err(|error| ToolError::Unavailable(error.message))?;
                match permission.outcome {
                    PermissionOutcome::Selected(id) if id == "allow-once" => true,
                    PermissionOutcome::Selected(id) if id == "allow-always" => {
                        broker.remember_decision(reference.clone(), true).await;
                        true
                    }
                    PermissionOutcome::Selected(id) if id == "reject-always" => {
                        broker.remember_decision(reference.clone(), false).await;
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
            return Err(ToolError::PermissionDenied);
        }

        crate::client_impl::notify_session(
            accessor,
            session_id.clone(),
            SessionUpdate::ToolCallUpdate(snapshot(ToolCallStatus::InProgress, None)),
        )
        .await;
        let result = broker.run_call(call).await;
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
        result
    }
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
