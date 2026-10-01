// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Opt-in native management adapter for isolated component generation.

use std::borrow::Cow;
use std::fmt;
use std::io::{self, Read, Write};
use std::sync::Arc;

use anyhow::{ensure, Context, Result};
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock, Tool};
use serde_json::{json, Value};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use wassette::generation::{
    BuildError, BuildErrorKind, ComponentKind, GenerationError, GenerationPermissions,
    GenerationRequest, GenerationTarget,
};
use wassette::store::{CommitOutcome, InstallIntent, StoredArtifactKind, StoredEntry};
use wassette::LifecycleManager;

/// Maximum encoded JSON request accepted by the CLI and MCP adapters.
pub const MAX_REQUEST_BYTES: usize = 2 * 1024 * 1024;
const MAX_SOURCE_BYTES: usize = 256 * 1024;
const MAX_WIT_BYTES: usize = 256 * 1024;
const MAX_POLICY_BYTES: usize = 128 * 1024;
const MAX_COMPONENT_NAME_BYTES: usize = 512;
const MAX_WORLD_BYTES: usize = 256;
const MAX_REVISION_BYTES: usize = 256;
const MAX_DIAGNOSTIC_JSON_BYTES: usize = 16 * 1024;
const TRUNCATED: &str = " [truncated]";

/// Owns jobs until the helper has reaped and any accepted transaction has completed.
///
/// Dropping a request cancels its child token, not its job. The transport owner
/// must call `shutdown` before dropping its Tokio runtime.
#[derive(Clone, Default)]
pub struct GenerationJobs {
    tracker: TaskTracker,
    cancel: CancellationToken,
}

impl GenerationJobs {
    /// Cancel precommit work and drain all accepted jobs without aborting transactions.
    pub async fn shutdown(&self) {
        self.cancel.cancel();
        self.tracker.close();
        self.tracker.wait().await;
    }

    /// Run the combined build-and-install operation with operator-issued authority.
    ///
    /// No request field is a permission grant. This native management operation has
    /// no second per-request approval UI: its ceiling comes only from the operator.
    pub async fn build(
        &self,
        manager: &LifecycleManager,
        request: GenerationRequest,
        cancel: CancellationToken,
    ) -> Result<Value> {
        // Register admission before checking shutdown so draining cannot miss
        // a caller between its cancellation check and worker creation.
        let _admission = self.tracker.token();
        let service = manager.generation_service()?;
        let permissions = service.permissions();
        check_permissions(&request, permissions)?;
        validate_request(&request)?;
        if cancel.is_cancelled() || self.cancel.is_cancelled() {
            return Err(GenerationError::Cancelled.into());
        }

        let job_cancel = self.cancel.child_token();
        let manager = manager.clone();
        let worker_cancel = job_cancel.clone();
        let worker = self.tracker.spawn(async move {
            let candidate = service
                .prepare(&manager, request, permissions, worker_cancel.clone())
                .await?;
            let outcome = candidate.install(permissions, worker_cancel).await?;
            let mut report = outcome.report();
            bound_preview_diagnostics(&mut report);
            report["status"] = json!("installed");
            describe_installation(&mut report, &outcome.commit);
            Ok::<_, anyhow::Error>(report)
        });
        finish_job(worker, job_cancel, cancel).await
    }

    /// Handle MCP arguments without logging source, request bodies or compiler diagnostics.
    pub async fn call_tool(
        &self,
        req: CallToolRequestParams,
        manager: &LifecycleManager,
        disable_builtin_tools: bool,
        cancel: CancellationToken,
    ) -> Result<CallToolResult> {
        let result = async {
            ensure!(!disable_builtin_tools, "Built-in tools are disabled");
            let service = manager.generation_service()?;
            require(service.permissions().can_build(), "build")?;
            require(service.permissions().can_install(), "install")?;
            let request = parse_request(Value::Object(req.arguments.unwrap_or_default()))?;
            self.build(manager, request, cancel).await
        }
        .await;
        tool_result(result)
    }
}

fn tool_result(result: Result<Value>) -> Result<CallToolResult> {
    let (report, failed) = match result {
        Ok(report) => (report, false),
        Err(error) => (error_report(&error), true),
    };
    let contents = vec![ContentBlock::text(serde_json::to_string(&report)?)];
    let mut result = if failed {
        CallToolResult::error(contents)
    } else {
        CallToolResult::success(contents)
    };
    result.structured_content = Some(report);
    Ok(result)
}

async fn finish_job<T>(
    mut worker: JoinHandle<Result<T>>,
    job_cancel: CancellationToken,
    request_cancel: CancellationToken,
) -> Result<T> {
    let _cancel_on_drop = job_cancel.clone().drop_guard();
    tokio::select! {
        result = &mut worker => result.context("generation job failed")?,
        _ = request_cancel.cancelled() => {
            job_cancel.cancel();
            worker.await.context("generation job failed")?
        }
    }
}

/// Whether the configured host permits this combined management operation.
pub fn is_available(manager: &LifecycleManager) -> bool {
    manager
        .generation_service()
        .is_ok_and(|service| combined_operation_allowed(service.permissions()))
}

fn combined_operation_allowed(permissions: GenerationPermissions) -> bool {
    permissions.can_build() && permissions.can_install()
}

fn require(allowed: bool, operation: &'static str) -> Result<()> {
    if allowed {
        Ok(())
    } else {
        Err(GenerationError::PermissionDenied(operation).into())
    }
}

fn check_permissions(
    request: &GenerationRequest,
    permissions: GenerationPermissions,
) -> Result<()> {
    require(permissions.can_build(), "build")?;
    require(permissions.can_install(), "install")?;
    if matches!(request.target, GenerationTarget::Rebuild { .. }) {
        require(permissions.can_rebuild(), "rebuild")?;
    }
    if request.intent == InstallIntent::ExposeTools {
        require(permissions.can_expose(), "expose")?;
    }
    Ok(())
}

/// Read at most the adapter input limit plus one sentinel byte.
pub fn read_request(reader: impl Read) -> Result<GenerationRequest> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_REQUEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .context("reading generation request")?;
    ensure!(
        bytes.len() <= MAX_REQUEST_BYTES,
        "generation request exceeds 2 MiB"
    );
    let value = serde_json::from_slice(&bytes).context("invalid generation request JSON")?;
    parse_request(value)
}

/// Parse only inline build inputs, never host profiles, paths, flags or ownership.
pub fn parse_request(value: Value) -> Result<GenerationRequest> {
    serde_json::to_writer(SizeLimit(MAX_REQUEST_BYTES), &value)
        .context("generation request exceeds 2 MiB")?;
    let request: GenerationRequest =
        serde_json::from_value(value).context("invalid generation request")?;
    validate_request(&request)?;
    Ok(request)
}

struct SizeLimit(usize);

impl Write for SizeLimit {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.0 {
            return Err(io::Error::other("generation request size limit"));
        }
        self.0 -= bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn validate_request(request: &GenerationRequest) -> Result<()> {
    for (value, max, label) in [
        (
            request.build.component_name.as_str(),
            MAX_COMPONENT_NAME_BYTES,
            "component name",
        ),
        (request.build.world.as_str(), MAX_WORLD_BYTES, "world"),
        (request.build.source.as_str(), MAX_SOURCE_BYTES, "source"),
        (request.build.wit.as_str(), MAX_WIT_BYTES, "WIT"),
    ] {
        ensure!(
            !value.is_empty() && value.len() <= max,
            "{label} exceeds input limits or is empty"
        );
    }
    if let GenerationTarget::Rebuild { expected_revision } = &request.target {
        ensure!(
            !expected_revision.is_empty() && expected_revision.len() <= MAX_REVISION_BYTES,
            "expected revision exceeds input limits or is empty"
        );
    }
    ensure!(
        request
            .reinstall_policy
            .as_ref()
            .is_none_or(|policy| policy.len() <= MAX_POLICY_BYTES),
        "reinstall policy exceeds input limits"
    );
    ensure!(
        matches!(
            request.intent,
            InstallIntent::InstallOnly | InstallIntent::ExposeTools
        ),
        "generation cannot select or activate ACP components"
    );
    ensure!(
        request.build.kind != ComponentKind::AcpLayer
            || request.intent == InstallIntent::InstallOnly,
        "ACP layers must use InstallOnly and require later explicit selection"
    );
    Ok(())
}

fn describe_installation(report: &mut Value, commit: &CommitOutcome) {
    if let StoredEntry::Installed(receipt) = &commit.entry {
        report["exposure"] = json!(if receipt.requests_tool_exposure() {
            "ordinary-tools-requested"
        } else {
            "not-exposed"
        });
        let layer = receipt.kind == StoredArtifactKind::AcpLayer;
        report["requires_selection"] = json!(layer);
        if layer {
            report["selection_note"] = json!("Installed only; select this ACP layer in a later ACP session. No active layer was changed.");
        }
    }
}

struct DiagnosticText {
    text: String,
    remaining: usize,
}

impl fmt::Write for DiagnosticText {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        for character in value.chars() {
            // Budget the JSON string representation, including control escapes,
            // rather than assuming an input byte becomes one output byte.
            let encoded_len = match character {
                '"' | '\\' | '\x08' | '\x0c' | '\n' | '\r' | '\t' => 2,
                '\0'..='\x1f' => 6,
                _ => character.len_utf8(),
            };
            if encoded_len > self.remaining {
                return Err(fmt::Error);
            }
            self.remaining -= encoded_len;
            self.text.push(character);
        }
        Ok(())
    }
}

fn bounded_diagnostic(arguments: fmt::Arguments<'_>) -> String {
    let mut output = DiagnosticText {
        text: String::new(),
        remaining: MAX_DIAGNOSTIC_JSON_BYTES - 2 - TRUNCATED.len(),
    };
    if fmt::write(&mut output, arguments).is_err() {
        output.text.push_str(TRUNCATED);
    }
    output.text
}

fn bound_preview_diagnostics(report: &mut Value) {
    if let Some(Value::String(diagnostics)) = report.pointer_mut("/preview/diagnostics") {
        *diagnostics = bounded_diagnostic(format_args!("{diagnostics}"));
    }
}

fn builder_error_report(kind: BuildErrorKind, diagnostic: Option<&str>) -> Value {
    let mut report = json!({
        "status": "failed",
        "phase": "build",
        "code": "builder-error",
        "build_error_kind": kind,
        "error": "Component build did not complete successfully.",
    });
    if matches!(
        kind,
        BuildErrorKind::InvalidRequest
            | BuildErrorKind::InvalidWit
            | BuildErrorKind::CompilationFailed
            | BuildErrorKind::InvalidOutput
    ) {
        if let Some(diagnostic) = diagnostic {
            report["diagnostic"] = json!(bounded_diagnostic(format_args!("{diagnostic}")));
        }
    }
    report
}

/// Render typed postcommit failures with the authoritative receipt, never rollback fiction.
pub fn error_report(error: &anyhow::Error) -> Value {
    let generation_error = error.downcast_ref::<GenerationError>();
    if generation_error.is_none() {
        if let Some(error) = BuildError::from_error(error) {
            return builder_error_report(error.kind(), error.diagnostic());
        }
    }
    let message = bounded_diagnostic(format_args!("{error:#}"));
    match generation_error {
        Some(error @ GenerationError::CommitRecoveryRequired { .. }) => {
            let mut report = error.recovery_report().expect("commit recovery report");
            report["phase"] = json!("commit-recovery");
            report["code"] = json!("recovery-required");
            report["error"] = json!(message);
            report["next_step"] = json!(
                "Inspect and recover the existing store operation before continuing; do not retry as a new generation."
            );
            report
        }
        Some(error @ GenerationError::CommittedButRefreshFailed { commit, .. }) => {
            let mut report = error.committed_report().expect("postcommit report");
            report["status"] = json!("committed-refresh-failed");
            report["code"] = json!("committed-refresh-failed");
            report["error"] = json!(message);
            report["next_step"] = json!(
                "The receipt is committed. Refresh the catalog; do not retry as a new generation."
            );
            describe_installation(&mut report, commit);
            report
        }
        error => json!({
            "status": "failed",
            "code": match error {
                Some(GenerationError::Disabled) => "disabled",
                Some(GenerationError::PermissionDenied(_)) => "permission-denied",
                Some(GenerationError::Cancelled) => "cancelled",
                _ => "generation-failed",
            },
            "error": message,
        }),
    }
}

/// Schema for the opt-in combined management operation.
pub fn tool() -> Tool {
    Tool::new_with_raw(
        Cow::Borrowed("build-component"),
        Some(Cow::Borrowed(
            "Build inline Rust/WIT in the operator-configured isolated helper, validate and install. InstallOnly is the default; ordinary-tool exposure and rebuild require separate operator grants. ACP layers are installed only and require later selection. No profile, host paths, compiler flags, or new policy grants are accepted.",
        )),
        Arc::new(serde_json::from_value(json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["build"],
            "properties": {
                "build": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["component_name", "source", "wit", "world", "kind"],
                    "properties": {
                        "component_name": {"type": "string", "minLength": 1, "maxLength": MAX_COMPONENT_NAME_BYTES},
                        "source": {"type": "string", "minLength": 1, "maxLength": MAX_SOURCE_BYTES},
                        "wit": {"type": "string", "minLength": 1, "maxLength": MAX_WIT_BYTES},
                        "world": {"type": "string", "minLength": 1, "maxLength": MAX_WORLD_BYTES},
                        "kind": {"type": "string", "enum": [ComponentKind::Tool, ComponentKind::AcpLayer]}
                    }
                },
                "target": {
                    "oneOf": [
                        {
                            "type": "object", "additionalProperties": false,
                            "required": ["mode"], "properties": {"mode": {"const": "new"}}
                        },
                        {
                            "type": "object", "additionalProperties": false,
                            "required": ["mode", "expected_revision"],
                            "properties": {
                                "mode": {"const": "rebuild"},
                                "expected_revision": {"type": "string", "minLength": 1, "maxLength": MAX_REVISION_BYTES}
                            }
                        }
                    ],
                    "default": {"mode": "new"}
                },
                "intent": {"type": "string", "enum": ["InstallOnly", "ExposeTools"], "default": "InstallOnly"},
                "reinstall_policy": {"type": "string", "maxLength": MAX_POLICY_BYTES}
            }
        })).expect("static generation schema")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_value() -> Value {
        json!({
            "build": {
                "component_name": "generated-example",
                "source": "struct Component;",
                "wit": "package example:tool; world tool {}",
                "world": "tool",
                "kind": "Tool"
            }
        })
    }

    #[test]
    fn defaults_are_install_only_and_new() {
        let request = parse_request(request_value()).unwrap();
        assert_eq!(request.intent, InstallIntent::InstallOnly);
        assert!(matches!(request.target, GenerationTarget::New));
    }

    #[test]
    fn name_and_world_keep_exact_spelling_at_builder_input_boundaries() {
        for name_bytes in [129, 511, 512] {
            let mut value = request_value();
            let name = format!("Mixed-{}", "n".repeat(name_bytes - 6));
            let world = format!("World-{}", "w".repeat(250));
            value["build"]["component_name"] = json!(name);
            value["build"]["world"] = json!(world);
            let parsed = parse_request(value).unwrap();
            assert_eq!(parsed.build.component_name, name);
            assert_eq!(parsed.build.world, world);
        }
        for (field, maximum) in [("component_name", 512), ("world", 256)] {
            let mut value = request_value();
            value["build"][field] = json!("x".repeat(maximum + 1));
            assert!(parse_request(value).is_err(), "{field} byte limit");
        }

        let mut value = request_value();
        let name = "é".repeat(256);
        value["build"]["component_name"] = json!(name);
        assert_eq!(
            parse_request(value.clone()).unwrap().build.component_name,
            name
        );
        value["build"]["component_name"] = json!("é".repeat(257));
        assert!(parse_request(value).is_err());
    }

    #[test]
    fn combined_tool_requires_explicit_build_and_install_but_not_exposure() {
        for build in [false, true] {
            for install in [false, true] {
                let permissions = GenerationPermissions::new(build, install, false, false);
                assert_eq!(combined_operation_allowed(permissions), build && install);
            }
        }
    }

    #[tokio::test]
    async fn absent_service_and_disabled_builtins_never_build_or_mutate() {
        let dir = tempfile::tempdir_in(".").unwrap();
        let manager = LifecycleManager::builder(dir.path())
            .with_eager_loading(false)
            .build()
            .await
            .unwrap();
        let before = manager
            .component_store()
            .snapshot_if_changed(None)
            .unwrap()
            .unwrap();
        assert!(!is_available(&manager));
        let listing = crate::tools::handle_tools_list(&manager, false)
            .await
            .unwrap();
        assert!(!listing["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "build-component"));
        assert!(listing["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "load-component"));
        let hidden = crate::tools::handle_tools_list(&manager, true)
            .await
            .unwrap();
        assert!(hidden["tools"].as_array().unwrap().is_empty());
        for disabled in [false, true] {
            let req = CallToolRequestParams::new("build-component")
                .with_arguments(request_value().as_object().unwrap().clone());
            let response = crate::tools::handle_tools_call(req, &manager, disabled)
                .await
                .unwrap();
            let response: CallToolResult = serde_json::from_value(response).unwrap();
            assert_eq!(response.is_error, Some(true));
            let report = response.structured_content.unwrap();
            if disabled {
                assert!(report["error"]
                    .as_str()
                    .unwrap()
                    .contains("Built-in tools are disabled"));
            } else {
                assert_eq!(report["code"], "disabled");
            }
        }
        assert!(manager
            .component_store()
            .snapshot_if_changed(Some(&before.cursor))
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn cancelling_a_request_waits_for_the_owned_worker_to_reap() {
        let jobs = GenerationJobs::default();
        let job_cancel = jobs.cancel.child_token();
        let worker_cancel = job_cancel.clone();
        let (reaped, reaping) = tokio::sync::oneshot::channel();
        let worker = jobs.tracker.spawn(async move {
            worker_cancel.cancelled().await;
            reaping.await.unwrap();
            Ok(42)
        });
        let request_cancel = CancellationToken::new();
        let response = finish_job(worker, job_cancel, request_cancel.clone());
        tokio::pin!(response);
        request_cancel.cancel();
        assert!(futures::poll!(&mut response).is_pending());
        assert_eq!(jobs.tracker.len(), 1);
        reaped.send(()).unwrap();
        assert_eq!(response.await.unwrap(), 42);
        jobs.shutdown().await;
        assert!(jobs.tracker.is_empty());
    }

    #[tokio::test]
    async fn dropping_a_request_cancels_without_aborting_the_owned_job() {
        let jobs = GenerationJobs::default();
        let job_cancel = jobs.cancel.child_token();
        let worker_cancel = job_cancel.clone();
        let (reaped, reaping) = tokio::sync::oneshot::channel();
        let worker = jobs.tracker.spawn(async move {
            worker_cancel.cancelled().await;
            reaping.await.unwrap();
            Ok(())
        });
        let mut response = Box::pin(finish_job(
            worker,
            job_cancel.clone(),
            CancellationToken::new(),
        ));
        assert!(futures::poll!(&mut response).is_pending());
        drop(response);
        assert!(job_cancel.is_cancelled());
        assert_eq!(jobs.tracker.len(), 1);
        reaped.send(()).unwrap();
        jobs.shutdown().await;
        assert!(jobs.tracker.is_empty());
    }

    #[test]
    fn request_cannot_choose_profile_paths_flags_or_authority() {
        for key in [
            "generation_config",
            "builder",
            "allow_build",
            "permissions",
            "source_identity",
            "storage_key",
        ] {
            let mut value = request_value();
            value[key] = json!("untrusted");
            assert!(parse_request(value).is_err(), "{key}");
        }
        for key in [
            "helper_path",
            "helper_sha256",
            "initrd_path",
            "initrd_sha256",
            "staging_root",
            "wit_dependencies",
            "limits",
            "profile",
            "flags",
            "source_path",
            "cargo",
        ] {
            let mut value = request_value();
            value["build"][key] = json!("untrusted");
            assert!(parse_request(value).is_err(), "{key}");
        }
        let mut value = request_value();
        value["target"] = json!({"mode": "new", "source": "untrusted"});
        assert!(parse_request(value).is_err());
    }

    #[test]
    fn input_limits_apply_to_bytes_and_encoded_json() {
        let mut value = request_value();
        value["build"]["source"] = json!("é".repeat(MAX_SOURCE_BYTES / 2 + 1));
        assert!(parse_request(value).is_err());
        let mut value = request_value();
        value["build"]["wit"] = json!("w".repeat(MAX_WIT_BYTES + 1));
        assert!(parse_request(value).is_err());
        let bytes = vec![b' '; MAX_REQUEST_BYTES + 1];
        assert!(read_request(bytes.as_slice()).is_err());
        assert!(read_request(b"{".as_slice()).is_err());
        let bytes = serde_json::to_vec(&request_value()).unwrap();
        assert!(read_request(bytes.as_slice()).is_ok());
        let mut value = request_value();
        value["reinstall_policy"] = json!("p".repeat(MAX_POLICY_BYTES + 1));
        assert!(parse_request(value).is_err());
        let mut value = request_value();
        value["target"] =
            json!({"mode": "rebuild", "expected_revision": "r".repeat(MAX_REVISION_BYTES + 1)});
        assert!(parse_request(value).is_err());
        let mut value = request_value();
        value["unknown"] = json!("x".repeat(MAX_REQUEST_BYTES));
        assert!(parse_request(value).is_err());
    }

    #[test]
    fn reader_stops_at_the_sentinel_without_reading_the_remaining_input() {
        let bytes = vec![b' '; MAX_REQUEST_BYTES * 2];
        let mut reader = io::Cursor::new(bytes);
        assert!(read_request(&mut reader).is_err());
        assert_eq!(reader.position(), MAX_REQUEST_BYTES as u64 + 1);
    }

    #[test]
    fn encoded_request_cap_is_independent_of_decoded_source_and_wit_caps() {
        let mut value = request_value();
        value["build"]["source"] = json!("s".repeat(MAX_SOURCE_BYTES));
        value["build"]["wit"] = json!("w".repeat(MAX_WIT_BYTES));
        assert!(parse_request(value.clone()).is_ok());
        value["build"]["source"] = json!("\0".repeat(MAX_SOURCE_BYTES));
        value["build"]["wit"] = json!("\0".repeat(MAX_WIT_BYTES));
        let error = parse_request(value)
            .err()
            .expect("escaped request exceeds transport cap");
        assert_eq!(error.to_string(), "generation request exceeds 2 MiB");
    }

    #[test]
    fn permission_distinctions_are_checked_before_build() {
        let mut request = parse_request(request_value()).unwrap();
        for (permissions, operation) in [
            (GenerationPermissions::default(), "build"),
            (
                GenerationPermissions::new(true, false, true, true),
                "install",
            ),
        ] {
            let error = check_permissions(&request, permissions).unwrap_err();
            assert!(
                matches!(error.downcast_ref::<GenerationError>(), Some(GenerationError::PermissionDenied(actual)) if *actual == operation)
            );
        }
        let install_only = GenerationPermissions::new(true, true, false, false);
        assert!(check_permissions(&request, install_only).is_ok());
        request.intent = InstallIntent::ExposeTools;
        assert!(matches!(
            check_permissions(&request, install_only)
                .unwrap_err()
                .downcast_ref::<GenerationError>(),
            Some(GenerationError::PermissionDenied("expose"))
        ));
        request.intent = InstallIntent::InstallOnly;
        request.target = GenerationTarget::Rebuild {
            expected_revision: "opaque".into(),
        };
        assert!(matches!(
            check_permissions(&request, install_only)
                .unwrap_err()
                .downcast_ref::<GenerationError>(),
            Some(GenerationError::PermissionDenied("rebuild"))
        ));
    }

    #[test]
    fn layers_cannot_request_exposure_or_activation_and_providers_are_rejected() {
        let mut value = request_value();
        value["build"]["kind"] = json!("AcpLayer");
        assert!(parse_request(value.clone()).is_ok());
        for intent in ["ExposeTools", "AcpSelection"] {
            value["intent"] = json!(intent);
            assert!(parse_request(value.clone()).is_err());
        }
        value["intent"] = json!("InstallOnly");
        value["build"]["kind"] = json!("AcpProvider");
        assert!(parse_request(value).is_err());
    }

    #[test]
    fn schema_has_limits_and_no_operator_inputs() {
        let schema = serde_json::to_value(tool().input_schema).unwrap();
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["build"]["additionalProperties"], false);
        assert_eq!(
            schema["properties"]["build"]["properties"]["source"]["maxLength"],
            MAX_SOURCE_BYTES
        );
        assert_eq!(
            schema["properties"]["build"]["properties"]["component_name"]["maxLength"],
            512
        );
        assert_eq!(
            schema["properties"]["build"]["properties"]["world"]["maxLength"],
            256
        );
        assert!(schema["properties"].get("generation_config").is_none());
        assert_eq!(
            schema["properties"]["build"]["properties"]["kind"]["enum"],
            json!([ComponentKind::Tool, ComponentKind::AcpLayer])
        );
    }

    #[test]
    fn error_text_is_bounded_without_parsing_rendered_messages() {
        let error = anyhow::anyhow!("component generation cancelled before commit");
        assert_eq!(error_report(&error)["code"], "generation-failed");
        assert_eq!(
            error_report(&GenerationError::Cancelled.into())["code"],
            "cancelled"
        );
        let report = error_report(&anyhow::anyhow!(
            "\0\u{001f}\n\\\"é".repeat(MAX_DIAGNOSTIC_JSON_BYTES)
        ));
        assert!(serde_json::to_vec(&report["error"]).unwrap().len() <= MAX_DIAGNOSTIC_JSON_BYTES);
        assert!(report["error"].as_str().unwrap().ends_with(TRUNCATED));
    }

    #[test]
    fn unknown_field_and_parser_errors_cannot_expand_serialized_diagnostics() {
        let mut value = request_value();
        value["build"]["\0\\\"".repeat(MAX_SOURCE_BYTES / 3)] = json!("private input");
        let error = parse_request(value).err().expect("unknown field rejected");
        let report = error_report(&error);
        assert!(serde_json::to_vec(&report["error"]).unwrap().len() <= MAX_DIAGNOSTIC_JSON_BYTES);

        let error = anyhow::anyhow!(
            "WIT parser rejected source: {}",
            "\0\\\"".repeat(MAX_WIT_BYTES)
        );
        let report = error_report(&error);
        assert!(serde_json::to_vec(&report["error"]).unwrap().len() <= MAX_DIAGNOSTIC_JSON_BYTES);
    }

    #[test]
    fn successful_preview_diagnostics_are_bounded_without_changing_the_receipt() {
        let commit = committed_fixture(StoredArtifactKind::Tool, InstallIntent::InstallOnly);
        let expected = serde_json::to_value(&commit).unwrap();
        let mut report = json!({
            "commit": commit,
            "preview": {"diagnostics": "\0\\\"é".repeat(MAX_SOURCE_BYTES)},
        });
        bound_preview_diagnostics(&mut report);
        assert_eq!(report["commit"], expected);
        assert!(
            serde_json::to_vec(&report["preview"]["diagnostics"])
                .unwrap()
                .len()
                <= MAX_DIAGNOSTIC_JSON_BYTES
        );
    }

    fn committed_fixture(kind: StoredArtifactKind, intent: InstallIntent) -> CommitOutcome {
        let entry = serde_json::from_value(json!({
            "Installed": {
                "schema": 2,
                "component_id": "generated-example",
                "storage_key": "generated_private",
                "source": {"Generated": {"id": "0123456789abcdef0123456789abcdef"}},
                "origin": {"location": "generated://0123456789abcdef0123456789abcdef"},
                "owner": "Explicit",
                "artifact_sha256": "a".repeat(64),
                "kind": kind,
                "validation": {"OrdinaryPrepared": {"runtime": "unit-test"}},
                "policy": {
                    "sha256": null,
                    "provenance": "Default",
                    "metadata": null,
                    "metadata_sha256": "b".repeat(64)
                },
                "revision": {"epoch": "test", "sequence": 4},
                "intent": intent,
                "observation": null
            }
        }))
        .unwrap();
        CommitOutcome {
            entry,
            cursor: serde_json::from_value(json!({"epoch": "test", "sequence": 4})).unwrap(),
            change: None,
        }
    }

    #[test]
    fn postcommit_error_preserves_the_exact_receipt_and_noop_status() {
        let commit = committed_fixture(StoredArtifactKind::Tool, InstallIntent::InstallOnly);
        let expected = serde_json::to_value(&commit).unwrap();
        let error = anyhow::Error::from(GenerationError::CommittedButRefreshFailed {
            commit: Box::new(commit),
            source: anyhow::anyhow!("refresh unavailable"),
        })
        .context("outer adapter context");
        let report = error_report(&error);
        assert_eq!(report["status"], "committed-refresh-failed");
        assert_eq!(report["commit"], expected);
        assert_eq!(report["revision"], "test:4");
        assert!(report["refresh"].is_null());
        assert_eq!(report["exposure"], "not-exposed");
        assert!(report["next_step"]
            .as_str()
            .unwrap()
            .contains("do not retry"));
    }

    #[test]
    fn layer_status_requires_later_selection_and_never_claims_tool_exposure() {
        let commit = committed_fixture(StoredArtifactKind::AcpLayer, InstallIntent::InstallOnly);
        let mut report = json!({});
        describe_installation(&mut report, &commit);
        assert_eq!(report["requires_selection"], true);
        assert_eq!(report["exposure"], "not-exposed");
        assert!(report["selection_note"]
            .as_str()
            .unwrap()
            .contains("No active layer was changed"));
    }

    #[test]
    fn recovery_results_preserve_canonical_status_and_operation_as_mcp_failures() {
        for committed in [false, true] {
            let error = GenerationError::CommitRecoveryRequired {
                operation: "existing-operation-42".into(),
                observed_commit: committed.then(|| {
                    Box::new(committed_fixture(
                        StoredArtifactKind::Tool,
                        InstallIntent::InstallOnly,
                    ))
                }),
                observation_error: (!committed).then(|| {
                    wassette::store::StoreError::Integrity("observation interrupted".into())
                }),
                source: wassette::store::StoreError::RecoveryRequired {
                    operation: "existing-operation-42".into(),
                    detail: "journal requires recovery".into(),
                },
            };
            let canonical = error.recovery_report().unwrap();
            let response =
                tool_result(Err(anyhow::Error::from(error).context("adapter context"))).unwrap();
            assert_eq!(response.is_error, Some(true));
            let report = response.structured_content.as_ref().unwrap();
            assert_eq!(
                report["status"],
                if committed {
                    "committed-recovery-required"
                } else {
                    "commit-unknown"
                }
            );
            for field in ["status", "operation", "commit", "refresh"] {
                assert_eq!(report[field], canonical[field], "{field}");
            }
            assert_eq!(report["phase"], "commit-recovery");
            assert_eq!(report["code"], "recovery-required");
            assert!(report["next_step"]
                .as_str()
                .unwrap()
                .contains("do not retry"));
            let text = response.content[0].as_text().unwrap();
            let text_report: Value = serde_json::from_str(&text.text).unwrap();
            assert_eq!(&text_report, report);
            let serialized = serde_json::to_value(response).unwrap();
            let roundtrip: CallToolResult = serde_json::from_value(serialized).unwrap();
            assert_eq!(roundtrip.is_error, Some(true));
            assert_eq!(
                roundtrip.structured_content.unwrap()["commit"],
                canonical["commit"]
            );
        }
    }

    #[test]
    fn unknown_commit_diagnostics_stay_bounded_without_inventing_a_receipt() {
        let error = GenerationError::CommitRecoveryRequired {
            operation: "existing-operation-43".into(),
            observed_commit: None,
            observation_error: None,
            source: wassette::store::StoreError::RecoveryRequired {
                operation: "existing-operation-43".into(),
                detail: "\0\\\"é".repeat(MAX_SOURCE_BYTES),
            },
        };
        let report = error_report(&error.into());
        assert_eq!(report["status"], "commit-unknown");
        assert_eq!(report["operation"], "existing-operation-43");
        assert!(report["commit"].is_null());
        assert!(report["refresh"].is_null());
        assert!(serde_json::to_vec(&report["error"]).unwrap().len() <= MAX_DIAGNOSTIC_JSON_BYTES);
    }

    #[test]
    fn typed_request_and_compiler_diagnostics_are_returned_with_encoded_bounds() {
        for (kind, serialized_kind) in [
            (BuildErrorKind::InvalidRequest, "invalid_request"),
            (BuildErrorKind::InvalidWit, "invalid_wit"),
            (BuildErrorKind::CompilationFailed, "compilation_failed"),
            (BuildErrorKind::InvalidOutput, "invalid_output"),
        ] {
            let report = builder_error_report(kind, Some("safe actionable diagnostic"));
            assert_eq!(report["status"], "failed");
            assert_eq!(report["phase"], "build");
            assert_eq!(report["code"], "builder-error");
            assert_eq!(report["build_error_kind"], serialized_kind);
            assert_eq!(report["diagnostic"], "safe actionable diagnostic");
            assert!(builder_error_report(kind, None).get("diagnostic").is_none());

            let text = "\0\\\"é".repeat(MAX_SOURCE_BYTES);
            let bounded = builder_error_report(kind, Some(&text));
            assert!(
                serde_json::to_vec(&bounded["diagnostic"]).unwrap().len()
                    <= MAX_DIAGNOSTIC_JSON_BYTES
            );
            assert!(bounded["diagnostic"].as_str().unwrap().ends_with(TRUNCATED));
        }
    }

    #[test]
    fn unavailable_internal_and_control_failures_never_disclose_diagnostics() {
        for (kind, serialized_kind) in [
            (BuildErrorKind::Unavailable, "unavailable"),
            (BuildErrorKind::Internal, "internal"),
            (BuildErrorKind::Cancelled, "cancelled"),
            (BuildErrorKind::Busy, "busy"),
            (BuildErrorKind::DeadlineExceeded, "deadline_exceeded"),
        ] {
            let report = builder_error_report(kind, Some("private host/config details"));
            assert_eq!(report["status"], "failed");
            assert_eq!(report["build_error_kind"], serialized_kind);
            assert!(report.get("diagnostic").is_none());
            assert!(!serde_json::to_string(&report)
                .unwrap()
                .contains("private host/config"));
        }
    }

    fn unavailable_builder_error() -> anyhow::Error {
        use wassette::generation::{BuildLimits, Builder, BuilderConfig};

        Builder::new(
            BuilderConfig {
                helper_path: Default::default(),
                helper_sha256: String::new(),
                initrd_path: Default::default(),
                initrd_sha256: String::new(),
                staging_root: Default::default(),
                wit_dependencies: Vec::new(),
                rust_crates: Vec::new(),
            },
            BuildLimits {
                source_bytes: 0,
                ..Default::default()
            },
        )
        .err()
        .expect("invalid limits fail without opening files or starting a helper")
    }

    #[test]
    fn real_builder_error_kind_survives_context_without_disclosing_operator_details() {
        let error = unavailable_builder_error().context("private operator configuration marker");
        let typed = BuildError::from_error(&error).unwrap();
        assert_eq!(typed.kind(), BuildErrorKind::Unavailable);
        assert!(typed.diagnostic().is_none());
        let response = tool_result(Err(error)).unwrap();
        assert_eq!(response.is_error, Some(true));
        let report = response.structured_content.unwrap();
        assert_eq!(report["build_error_kind"], "unavailable");
        assert!(report.get("diagnostic").is_none());
        assert!(!serde_json::to_string(&report)
            .unwrap()
            .contains("private operator"));
    }

    #[test]
    fn committed_failure_takes_precedence_over_a_builder_error_in_its_source_chain() {
        let error = GenerationError::CommittedButRefreshFailed {
            commit: Box::new(committed_fixture(
                StoredArtifactKind::Tool,
                InstallIntent::InstallOnly,
            )),
            source: unavailable_builder_error(),
        };
        let response = tool_result(Err(error.into())).unwrap();
        assert_eq!(response.is_error, Some(true));
        let report = response.structured_content.unwrap();
        assert_eq!(report["status"], "committed-refresh-failed");
        assert!(report["commit"].is_object());
        assert!(report.get("build_error_kind").is_none());
        assert!(report.get("diagnostic").is_none());
    }
}
