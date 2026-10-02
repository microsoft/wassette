// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Host-enabled generation uses the existing session broker and UI routes.

use std::sync::Arc;

use wasmtime::component::{Accessor, HasSelf};

use crate::state::HostState;
use crate::tool_broker::ToolBroker;
use crate::wassette::component_generation::builder::{GenerationError, GenerationReport};

mod types {
    wasmtime::component::bindgen!({
        path: "../../wit/component-generation",
        world: "imports",
    });
}

/// ACP routing for the canonical synchronous guest ABI.
///
/// Bindgen's `async | store` on a synchronous WIT function supplies `Access`,
/// not `Accessor`. A synchronous host import drives the store event loop for
/// session binding. Rebuild approvals use the bound editor route: Wasmtime
/// cannot re-enter an active upstream layer during this synchronous guest call.
pub mod builder {
    use wasmtime::component::{Accessor, HasData, Linker};

    pub use super::types::wassette::component_generation::builder::{
        Disposition, GenerationError, GenerationReport,
    };

    pub trait Host {}
    impl Host for crate::state::HostState {}
    impl<H: Host + ?Sized> Host for &mut H {}

    pub trait HostWithStore<T>: HasData + Send {
        fn generate(
            accessor: &Accessor<T, Self>,
            request_json: String,
        ) -> impl Future<Output = Result<GenerationReport, GenerationError>> + Send;
    }

    pub fn add_to_linker<T: Send + 'static, D: HostWithStore<T> + 'static>(
        linker: &mut Linker<T>,
        getter: fn(&mut T) -> D::Data<'_>,
    ) -> wasmtime::Result<()>
    where
        for<'a> D::Data<'a>: Host,
    {
        linker
            .instance("wassette:component-generation/builder@0.1.0")?
            .func_wrap_async("generate", move |store, (request,): (String,)| {
                Box::new(async move {
                    store
                        .run_concurrent(async move |accessor| {
                            let accessor = accessor.with_getter::<D>(getter);
                            (D::generate(&accessor, request).await,)
                        })
                        .await
                })
            })
    }
}

struct Caller {
    session_id: String,
    stage: usize,
    component_id: String,
}

fn caller<T: Send>(
    accessor: &Accessor<T, HasSelf<HostState>>,
) -> Result<(Caller, Arc<ToolBroker>), GenerationError> {
    accessor.with(|mut access| {
        let state = access.get();
        let session_id = state
            .editor_session_id
            .clone()
            .ok_or(GenerationError::SessionNotBound)?;
        let stage = *state
            .stage_stack
            .last()
            .ok_or(GenerationError::SessionNotBound)?;
        let component_id = state
            .stages
            .get(stage)
            .ok_or(GenerationError::SessionNotBound)?
            .component_id
            .clone();
        let broker = state.tool_broker.clone().ok_or(GenerationError::Disabled)?;
        Ok((
            Caller {
                session_id,
                stage,
                component_id,
            },
            broker,
        ))
    })
}

impl<T: Send> crate::wassette::component_generation::builder::HostWithStore<T>
    for HasSelf<HostState>
{
    async fn generate(
        accessor: &Accessor<T, Self>,
        request_json: String,
    ) -> Result<GenerationReport, GenerationError> {
        let (caller, broker) = caller(accessor)?;
        #[cfg(feature = "component-generation")]
        {
            enabled::generate(accessor, caller, broker, request_json).await
        }
        #[cfg(not(feature = "component-generation"))]
        {
            let _ = (
                caller.session_id,
                caller.stage,
                caller.component_id,
                broker,
                request_json,
            );
            Err(GenerationError::Disabled)
        }
    }
}

#[cfg(test)]
mod binding_tests {
    use super::*;
    use crate::state::{ClientSink, StageData, StageKind};

    #[tokio::test]
    async fn unbound_is_rejected_and_provider_and_layer_are_equally_disabled() {
        let root = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            wassette::LifecycleManager::builder(root.path().join("store"))
                .with_secrets_dir(root.path().join("secrets"))
                .build()
                .await
                .unwrap(),
        );
        let broker = Arc::new(ToolBroker::new(manager, []));
        let (outbound, _events) = tokio::sync::mpsc::channel(8);
        let mut state = HostState {
            wasi: wasmtime_wasi::WasiCtxBuilder::new().build(),
            http: wasmtime_wasi_http::WasiHttpCtx::new(),
            http_hooks: crate::http_policy::HttpPolicyHooks::new(None),
            table: wasmtime::component::ResourceTable::new(),
            stages: vec![StageData {
                kind: StageKind::Provider,
                component_id: "caller:provider".into(),
                bindings: None,
                sink: ClientSink::Outbound(outbound),
                downstream_idx: None,
            }],
            stage_stack: vec![0],
            secrets: Arc::new(crate::secrets::SecretsRegistry::new(
                root.path().join("secrets"),
            )),
            downstream_sessions: Default::default(),
            next_downstream_rep: 1,
            editor_session_id: None,
            provider_routing: None,
            terminal_enabled: false,
            tool_broker: Some(broker),
            tool_decisions: Vec::new(),
            active_tool_calls: Default::default(),
        };
        let engine = crate::acp_engine().unwrap();
        let mut store = wasmtime::Store::new(&engine, state);
        let result = store
            .run_concurrent(async |accessor| {
                <HasSelf<HostState> as builder::HostWithStore<HostState>>::generate(
                    accessor,
                    "{}".into(),
                )
                .await
            })
            .await
            .unwrap();
        assert!(matches!(result, Err(GenerationError::SessionNotBound)));
        for role in [StageKind::Provider, StageKind::Layer] {
            state = store.into_data();
            state.editor_session_id = Some("bound-session".into());
            state.stages[0].kind = role;
            store = wasmtime::Store::new(&engine, state);
            let result = store
                .run_concurrent(async |accessor| {
                    <HasSelf<HostState> as builder::HostWithStore<HostState>>::generate(
                        accessor,
                        "{}".into(),
                    )
                    .await
                })
                .await
                .unwrap();
            assert!(matches!(result, Err(GenerationError::Disabled)));
        }
    }

    #[test]
    fn acp_dependency_is_the_canonical_generation_package() {
        assert_eq!(
            include_str!("../../../wit/component-generation/builder.wit"),
            include_str!("../../../wit/component-generation/builder.wit"),
        );
    }
}

#[cfg(feature = "component-generation")]
mod enabled {
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use tokio_util::sync::CancellationToken;
    use wassette::generation::{
        BuildError, BuildErrorKind, ComponentKind, GenerationError as CoreError, GenerationOutcome,
        GenerationPermissions, GenerationRequest, GenerationService, GenerationTarget,
        PreparedGeneration,
    };
    use wassette::store::{StoreError, StoredEntry};

    use super::*;
    use crate::tool_broker::{CancelledCallNotice, track_call};
    use crate::wassette::acp::prompts::SessionUpdate;
    use crate::wassette::acp::tools::{
        PermissionOption, PermissionOptionKind, PermissionOutcome, RequestPermissionRequest,
        ToolCallSnapshot, ToolCallStatus, ToolKind,
    };
    use crate::wassette::component_generation::builder::Disposition;
    use crate::wassette::component_tools::tools::ToolError;

    const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
    const MAX_METADATA_BYTES: usize = 512;
    const MAX_DIAGNOSTIC_BYTES: usize = 16 * 1024;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Phase {
        Build,
        Install,
    }

    impl Phase {
        fn title(self) -> &'static str {
            match self {
                Self::Build => "Build component in isolated VM",
                Self::Install => "Install exact generated component",
            }
        }
    }

    trait Interaction: Sync {
        fn check(&self) -> Result<(), GenerationError>;
        fn approve(
            &self,
            phase: Phase,
            details: Value,
        ) -> impl Future<Output = Result<(), GenerationError>> + Send;
    }

    trait Operation: Sync {
        type Candidate: Send;
        type Output: Send;

        fn prepare(
            &self,
            request: GenerationRequest,
            permissions: GenerationPermissions,
            cancel: CancellationToken,
        ) -> impl Future<Output = Result<Self::Candidate, GenerationError>> + Send;

        fn preview(&self, candidate: &Self::Candidate) -> Value;

        fn install(
            &self,
            candidate: Self::Candidate,
            permissions: GenerationPermissions,
            cancel: CancellationToken,
        ) -> impl Future<Output = Result<Self::Output, GenerationError>> + Send;
    }

    fn parse_request(json: &str) -> Result<GenerationRequest, GenerationError> {
        if json.len() > MAX_REQUEST_BYTES {
            return Err(GenerationError::InvalidRequest(
                "Request exceeds 4 MiB".into(),
            ));
        }
        serde_json::from_str(json).map_err(|_| {
            GenerationError::InvalidRequest("Invalid generation request schema".into())
        })
    }

    fn admit(
        request: &GenerationRequest,
        permissions: GenerationPermissions,
    ) -> Result<(), GenerationError> {
        let valid_metadata = |value: &str| {
            !value.trim().is_empty()
                && value.len() <= MAX_METADATA_BYTES
                && !value.chars().any(char::is_control)
        };
        if !valid_metadata(&request.build.component_name)
            || !valid_metadata(&request.build.world)
            || matches!(&request.target, GenerationTarget::Rebuild { expected_revision }
                if !valid_metadata(expected_revision))
        {
            return Err(GenerationError::InvalidRequest(
                "Component name, world, and revision must be bounded nonempty metadata".into(),
            ));
        }
        if !permissions.can_build()
            || !permissions.can_install()
            || (matches!(request.target, GenerationTarget::Rebuild { .. })
                && !permissions.can_rebuild())
        {
            return Err(GenerationError::PermissionDenied);
        }
        Ok(())
    }

    fn bounded_report(mut report: Value) -> String {
        if let Some(Value::String(diagnostics)) = report.pointer_mut("/preview/diagnostics") {
            truncate_diagnostic(diagnostics);
        }
        report.to_string()
    }

    fn truncate_diagnostic(diagnostics: &mut String) {
        let suffix = "\n[diagnostics truncated]";
        let suffix_bytes: usize = suffix.chars().map(json_character_bytes).sum();
        let mut serialized_bytes = 2;
        let mut end = 0;
        let mut truncated = false;
        for (index, character) in diagnostics.char_indices() {
            serialized_bytes += json_character_bytes(character);
            if serialized_bytes > MAX_DIAGNOSTIC_BYTES {
                truncated = true;
                break;
            }
            if serialized_bytes + suffix_bytes <= MAX_DIAGNOSTIC_BYTES {
                end = index + character.len_utf8();
            }
        }
        if truncated {
            diagnostics.truncate(end);
            diagnostics.push_str(suffix);
        }
    }

    fn build_failure(
        kind: BuildErrorKind,
        diagnostic: Option<&str>,
        already_truncated: bool,
    ) -> GenerationError {
        let message = |fallback: &str| {
            let Some(diagnostic) = diagnostic.filter(|text| !text.is_empty()) else {
                return fallback.to_owned();
            };
            let mut end = diagnostic.len().min(MAX_DIAGNOSTIC_BYTES);
            while !diagnostic.is_char_boundary(end) {
                end -= 1;
            }
            let mut text = diagnostic[..end].to_owned();
            if already_truncated || end < diagnostic.len() {
                text.push_str("\n[diagnostics truncated]");
            }
            truncate_diagnostic(&mut text);
            text
        };
        match kind {
            BuildErrorKind::Cancelled => GenerationError::Cancelled,
            BuildErrorKind::Busy => GenerationError::Busy,
            BuildErrorKind::DeadlineExceeded => {
                GenerationError::BuildFailed("Build deadline exceeded".into())
            }
            BuildErrorKind::InvalidRequest => {
                GenerationError::InvalidRequest(message("Invalid build request"))
            }
            BuildErrorKind::InvalidWit => GenerationError::InvalidRequest(message("Invalid WIT")),
            BuildErrorKind::CompilationFailed => {
                GenerationError::BuildFailed(message("Component compilation failed"))
            }
            BuildErrorKind::InvalidOutput => {
                GenerationError::BuildFailed(message("Invalid generated component"))
            }
            BuildErrorKind::Unavailable | BuildErrorKind::Internal => {
                GenerationError::Unavailable("Builder unavailable".into())
            }
            _ => GenerationError::Unavailable("Unsupported builder failure category".into()),
        }
    }

    fn json_character_bytes(character: char) -> usize {
        match character {
            '"' | '\\' | '\u{0008}' | '\u{000c}' | '\n' | '\r' | '\t' => 2,
            '\u{0000}'..='\u{001f}' => 6,
            _ => character.len_utf8(),
        }
    }

    fn build_details(request: &GenerationRequest) -> Value {
        json!({
            "component_id": request.build.component_name,
            "kind": request.build.kind,
            "world": request.build.world,
            "source_sha256": hex::encode(Sha256::digest(request.build.source.as_bytes())),
            "wit_sha256": hex::encode(Sha256::digest(request.build.wit.as_bytes())),
            "source_bytes": request.build.source.len(),
            "wit_bytes": request.build.wit.len(),
            "target": request.target,
            "scope": "Build only. No installation is authorized by this approval.",
            "rebuild": matches!(request.target, GenerationTarget::Rebuild { .. }),
        })
    }

    async fn phases<I: Interaction, O: Operation>(
        interaction: &I,
        operation: &O,
        request: GenerationRequest,
        ceiling: GenerationPermissions,
        cancel: CancellationToken,
    ) -> Result<O::Output, GenerationError> {
        admit(&request, ceiling)?;
        let rebuild = matches!(request.target, GenerationTarget::Rebuild { .. });
        checkpoint(interaction, &cancel)?;
        if rebuild {
            interaction
                .approve(Phase::Build, build_details(&request))
                .await?;
        }
        checkpoint(interaction, &cancel)?;
        let candidate = operation
            .prepare(
                request,
                GenerationPermissions::new(true, false, rebuild),
                cancel.clone(),
            )
            .await?;
        checkpoint(interaction, &cancel)?;
        if rebuild {
            interaction
                .approve(Phase::Install, operation.preview(&candidate))
                .await?;
        }
        checkpoint(interaction, &cancel)?;
        operation
            .install(
                candidate,
                GenerationPermissions::new(true, true, rebuild),
                cancel,
            )
            .await
    }

    fn checkpoint(
        interaction: &impl Interaction,
        cancel: &CancellationToken,
    ) -> Result<(), GenerationError> {
        if cancel.is_cancelled() {
            return Err(GenerationError::Cancelled);
        }
        interaction.check()
    }

    struct CoreOperation {
        broker: Arc<ToolBroker>,
        service: Arc<GenerationService>,
    }

    impl Operation for CoreOperation {
        type Candidate = PreparedGeneration;
        type Output = GenerationOutcome;

        async fn prepare(
            &self,
            request: GenerationRequest,
            permissions: GenerationPermissions,
            cancel: CancellationToken,
        ) -> Result<Self::Candidate, GenerationError> {
            let service = self.service.clone();
            let manager = self.broker.manager.clone();
            self.broker
                .workers
                .run(async move {
                    service
                        .prepare(&manager, request, permissions, cancel)
                        .await
                        .map_err(core_error)
                })
                .await
                .map_err(broker_error)?
        }

        fn preview(&self, candidate: &Self::Candidate) -> Value {
            let preview = candidate.preview();
            json!({
                "component_id": preview.component_id,
                "kind": preview.kind,
                "wasm_sha256": preview.wasm_sha256,
                "expected_revision": preview.expected_revision,
                "evidence": preview.evidence,
                "scope": "Install this exact output. ACP layers require later explicit selection; \
                          no running chain is changed. Source and compiler diagnostics are omitted.",
            })
        }

        async fn install(
            &self,
            candidate: Self::Candidate,
            permissions: GenerationPermissions,
            cancel: CancellationToken,
        ) -> Result<Self::Output, GenerationError> {
            self.broker
                .workers
                .run(async move {
                    candidate
                        .install(permissions, cancel)
                        .await
                        .map_err(core_error)
                })
                .await
                .map_err(broker_error)?
        }
    }

    struct EditorInteraction<'a, T: Send + 'static> {
        accessor: &'a Accessor<T, HasSelf<HostState>>,
        caller: Caller,
        broker: Arc<ToolBroker>,
        call_id: String,
        cancellation: CancelledCallNotice,
    }

    impl<T: Send + 'static> Interaction for EditorInteraction<'_, T> {
        fn check(&self) -> Result<(), GenerationError> {
            if self.cancellation.is_cancelled() {
                return Err(GenerationError::Cancelled);
            }
            self.accessor.with(|mut access| {
                let state = access.get();
                if state.editor_session_id.as_ref() != Some(&self.caller.session_id)
                    || state.stage_stack.last() != Some(&self.caller.stage)
                    || state
                        .stages
                        .get(self.caller.stage)
                        .is_none_or(|stage| stage.component_id != self.caller.component_id)
                    || !state
                        .tool_broker
                        .as_ref()
                        .is_some_and(|broker| Arc::ptr_eq(broker, &self.broker))
                {
                    return Err(GenerationError::SessionNotBound);
                }
                Ok(())
            })
        }

        async fn approve(&self, phase: Phase, details: Value) -> Result<(), GenerationError> {
            self.check()?;
            let title = if phase == Phase::Build && details["rebuild"] == true {
                "Rebuild existing generated lineage at the specified revision"
            } else {
                phase.title()
            };
            let tool_call = snapshot(
                &self.call_id,
                title,
                ToolCallStatus::Pending,
                Some(
                    json!({
                        "caller": self.caller.component_id,
                        "operation": details,
                    })
                    .to_string(),
                ),
                None,
            );
            let permission = crate::client_impl::request_editor_permission(
                &self.cancellation.outbound,
                RequestPermissionRequest {
                    session_id: self.caller.session_id.clone(),
                    tool_call,
                    options: vec![
                        PermissionOption {
                            id: "allow-once".into(),
                            name: format!("Allow {} once", title.to_lowercase()),
                            kind: PermissionOptionKind::AllowOnce,
                        },
                        PermissionOption {
                            id: "reject-once".into(),
                            name: "Reject".into(),
                            kind: PermissionOptionKind::RejectOnce,
                        },
                    ],
                },
            )
            .await;
            self.check()?;
            permission_result(phase, permission)
        }
    }

    fn permission_result(
        phase: Phase,
        permission: Result<
            crate::wassette::acp::tools::RequestPermissionResponse,
            crate::wassette::acp::errors::Error,
        >,
    ) -> Result<(), GenerationError> {
        match permission {
            Ok(response)
                if matches!(response.outcome,
                PermissionOutcome::Selected(ref id) if id == "allow-once") =>
            {
                Ok(())
            }
            Ok(_) => Err(GenerationError::PermissionDenied),
            Err(error) => {
                tracing::warn!(
                    phase = phase.title(),
                    code = ?error.code,
                    "Generation rebuild editor approval failed"
                );
                Err(GenerationError::Unavailable(format!(
                    "Editor approval unavailable for '{}'; the editor must support \
                     session/request_permission to rebuild a generated component",
                    phase.title(),
                )))
            }
        }
    }

    fn snapshot(
        id: &str,
        title: &str,
        status: ToolCallStatus,
        raw_input: Option<String>,
        raw_output: Option<String>,
    ) -> ToolCallSnapshot {
        ToolCallSnapshot {
            id: id.into(),
            title: title.into(),
            kind: ToolKind::Execute,
            status,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input,
            raw_output,
        }
    }

    pub(super) async fn generate<T: Send>(
        accessor: &Accessor<T, HasSelf<HostState>>,
        caller: Caller,
        broker: Arc<ToolBroker>,
        request_json: String,
    ) -> Result<GenerationReport, GenerationError> {
        let service = broker.manager.generation_service().map_err(core_error)?;
        let request = parse_request(&request_json)?;
        admit(&request, service.permissions())?;
        let call_id = broker.next_call_id();
        let cancellation = track_call(
            accessor,
            caller.session_id.clone(),
            &call_id,
            snapshot(
                &call_id,
                "Generate component",
                ToolCallStatus::Failed,
                None,
                Some(
                    "Generation cancelled. An already accepted commit may still complete; \
                      cancellation does not roll it back."
                        .into(),
                ),
            ),
        )
        .map_err(broker_error)?;
        let interaction = EditorInteraction {
            accessor,
            caller,
            broker: broker.clone(),
            call_id,
            cancellation,
        };
        crate::client_impl::notify_editor_session(
            &interaction.cancellation.outbound,
            interaction.caller.session_id.clone(),
            SessionUpdate::ToolCall(snapshot(
                &interaction.call_id,
                "Generate component",
                ToolCallStatus::Pending,
                None,
                None,
            )),
        )
        .await;
        let operation = CoreOperation {
            broker: broker.clone(),
            service: service.clone(),
        };
        let result = async {
            let outcome = phases(
                &interaction,
                &operation,
                request,
                service.permissions(),
                interaction.cancellation.cancel.clone(),
            )
            .await?;
            let mut report = GenerationReport {
                report_json: bounded_report(outcome.report()),
                disposition: if outcome.preview.kind == ComponentKind::AcpLayer {
                    Disposition::LaterSelectionRequired
                } else {
                    Disposition::Installed
                },
                tool_handles: Vec::new(),
            };
            if outcome.preview.kind == ComponentKind::Tool {
                if interaction.check().is_err() {
                    return Err(GenerationError::Committed(report));
                }
                let StoredEntry::Installed(receipt) = &outcome.commit.entry else {
                    return Err(GenerationError::Committed(report));
                };
                report.disposition = Disposition::CommittedNotExposed;
                let publication = broker
                    .expose_generated(receipt, &interaction.cancellation.cancel)
                    .await;
                return publication_report(report, publication);
            }
            Ok(report)
        }
        .await;
        let (status, output) = match &result {
            Ok(report) if matches!(report.disposition, Disposition::ToolsEligible) => (
                ToolCallStatus::Completed,
                "Component installed with shared-store eligibility, but no callable exports \
                 or session tool handles; receipt returned to caller."
                    .into(),
            ),
            Ok(report) => (
                ToolCallStatus::Completed,
                format!(
                    "Generation completed ({:?}); receipt returned to caller.",
                    report.disposition
                ),
            ),
            Err(GenerationError::Committed(_)) => (
                ToolCallStatus::Failed,
                "Component committed, but requested publication did not complete. \
                 Inspect the returned receipt before retrying."
                    .into(),
            ),
            Err(GenerationError::RecoveryRequired(_)) => (
                ToolCallStatus::Failed,
                "Commit outcome requires store recovery. Inspect the returned operation report \
                 before retrying; cancellation does not establish rollback."
                    .into(),
            ),
            Err(_) => (
                ToolCallStatus::Failed,
                "Generation did not complete.".into(),
            ),
        };
        crate::client_impl::notify_editor_session(
            &interaction.cancellation.outbound,
            interaction.caller.session_id.clone(),
            SessionUpdate::ToolCallUpdate(snapshot(
                &interaction.call_id,
                "Generate component",
                status,
                None,
                Some(output),
            )),
        )
        .await;
        interaction.cancellation.finish();
        result
    }

    fn publication_report(
        mut report: GenerationReport,
        publication: Result<Vec<String>, ToolError>,
    ) -> Result<GenerationReport, GenerationError> {
        report.disposition = Disposition::CommittedNotExposed;
        report.tool_handles.clear();
        match publication {
            Ok(handles) => {
                report.disposition = if handles.is_empty() {
                    Disposition::ToolsEligible
                } else {
                    Disposition::SessionTools
                };
                report.tool_handles = handles;
                Ok(report)
            }
            Err(_) => Err(GenerationError::Committed(report)),
        }
    }

    fn broker_error(error: ToolError) -> GenerationError {
        match error {
            ToolError::Busy => GenerationError::Busy,
            ToolError::Cancelled => GenerationError::Cancelled,
            ToolError::SessionNotBound => GenerationError::SessionNotBound,
            _ => GenerationError::Unavailable("Generation supervisor unavailable".into()),
        }
    }

    fn core_error(error: anyhow::Error) -> GenerationError {
        if let Some(error) = error.downcast_ref::<CoreError>() {
            if let Some(report) = error.recovery_report() {
                return GenerationError::RecoveryRequired(report.to_string());
            }
            if let Some(report) = error.committed_report() {
                return GenerationError::Committed(GenerationReport {
                    report_json: report.to_string(),
                    disposition: Disposition::CommittedNotExposed,
                    tool_handles: Vec::new(),
                });
            }
            return match error {
                CoreError::Disabled => GenerationError::Disabled,
                CoreError::PermissionDenied(_) => GenerationError::PermissionDenied,
                CoreError::Cancelled => GenerationError::Cancelled,
                CoreError::CommitRecoveryRequired { .. } => {
                    unreachable!("the canonical recovery report covers recovery errors")
                }
                CoreError::CommittedButRefreshFailed { .. } => unreachable!(
                    "the canonical committed report covers every postcommit generation error"
                ),
            };
        }
        if matches!(
            error.downcast_ref::<StoreError>(),
            Some(StoreError::Conflict(_))
        ) {
            return GenerationError::Stale("Generated component revision changed".into());
        }
        if let Some(failure) = BuildError::from_error(&error) {
            return build_failure(
                failure.kind(),
                failure.diagnostic(),
                failure.diagnostic_truncated(),
            );
        }
        GenerationError::BuildFailed("Generation or validation failed".into())
    }

    #[cfg(test)]
    mod tests;
}
