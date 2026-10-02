// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;

struct Editor {
    approved: Mutex<Vec<(Phase, Value)>>,
    reject: Option<Phase>,
    cancel_on_allow: Option<Phase>,
    cancel: CancellationToken,
    bound: AtomicBool,
}

impl Editor {
    fn new(cancel: CancellationToken) -> Self {
        Self {
            approved: Mutex::new(Vec::new()),
            reject: None,
            cancel_on_allow: None,
            cancel,
            bound: AtomicBool::new(true),
        }
    }
}

impl Interaction for Editor {
    fn check(&self) -> Result<(), GenerationError> {
        if self.bound.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(GenerationError::SessionNotBound)
        }
    }

    async fn approve(&self, phase: Phase, details: Value) -> Result<(), GenerationError> {
        self.approved.lock().unwrap().push((phase, details));
        if self.cancel_on_allow == Some(phase) {
            self.cancel.cancel();
        }
        if self.reject == Some(phase) {
            Err(GenerationError::PermissionDenied)
        } else {
            Ok(())
        }
    }
}

#[derive(Default)]
struct Builder {
    builds: AtomicUsize,
    installs: AtomicUsize,
    stale_commit: bool,
}

impl Operation for Builder {
    type Candidate = Value;
    type Output = GenerationPermissions;

    async fn prepare(
        &self,
        _: GenerationRequest,
        permissions: GenerationPermissions,
        _: CancellationToken,
    ) -> Result<Value, GenerationError> {
        assert!(permissions.can_build());
        assert!(!permissions.can_install());
        self.builds.fetch_add(1, Ordering::Relaxed);
        Ok(json!({"wasm_sha256": "actual-finalized-hash", "expected_revision": "exact-revision"}))
    }

    fn preview(&self, candidate: &Value) -> Value {
        candidate.clone()
    }

    async fn install(
        &self,
        _: Value,
        permissions: GenerationPermissions,
        _: CancellationToken,
    ) -> Result<GenerationPermissions, GenerationError> {
        assert!(permissions.can_install());
        if self.stale_commit {
            return Err(GenerationError::Stale(
                "revision changed during permission".into(),
            ));
        }
        self.installs.fetch_add(1, Ordering::Relaxed);
        Ok(permissions)
    }
}

fn request() -> GenerationRequest {
    parse_request(
        &json!({
            "build": {
                "component_name": "example:generated",
                "source": "secret source body",
                "wit": "package example:generated; world tool {}",
                "world": "tool",
                "kind": ComponentKind::Tool,
            },
            "target": {"mode": "new"},
        })
        .to_string(),
    )
    .unwrap()
}

fn ceiling() -> GenerationPermissions {
    GenerationPermissions::new(true, true, true)
}

#[tokio::test]
async fn every_phase_denial_stops_before_the_next_side_effect() {
    for (reject, builds) in [(Phase::Build, 0), (Phase::Install, 1)] {
        let cancel = CancellationToken::new();
        let mut editor = Editor::new(cancel.clone());
        editor.reject = Some(reject);
        let builder = Builder::default();
        assert!(matches!(
            phases(&editor, &builder, request(), ceiling(), cancel).await,
            Err(GenerationError::PermissionDenied)
        ));
        assert_eq!(builder.builds.load(Ordering::Relaxed), builds);
        assert_eq!(builder.installs.load(Ordering::Relaxed), 0);
    }
}

#[tokio::test]
async fn late_allow_after_cancellation_never_builds_or_installs() {
    for (phase, builds) in [(Phase::Build, 0), (Phase::Install, 1)] {
        let cancel = CancellationToken::new();
        let mut editor = Editor::new(cancel.clone());
        editor.cancel_on_allow = Some(phase);
        let builder = Builder::default();
        assert!(matches!(
            phases(&editor, &builder, request(), ceiling(), cancel.clone()).await,
            Err(GenerationError::Cancelled)
        ));
        assert_eq!(builder.builds.load(Ordering::Relaxed), builds);
        assert_eq!(builder.installs.load(Ordering::Relaxed), 0);
        let count = editor.approved.lock().unwrap().len();
        assert!(matches!(
            phases(&editor, &builder, request(), ceiling(), cancel).await,
            Err(GenerationError::Cancelled)
        ));
        assert_eq!(editor.approved.lock().unwrap().len(), count);
    }
}

#[tokio::test]
async fn generation_uses_only_build_and_install_approvals() {
    let cancel = CancellationToken::new();
    let editor = Editor::new(cancel.clone());
    let builder = Builder::default();
    let permissions = phases(&editor, &builder, request(), ceiling(), cancel)
        .await
        .unwrap();
    assert!(permissions.can_install());
    assert_eq!(builder.installs.load(Ordering::Relaxed), 1);
    let prompts = editor.approved.lock().unwrap();
    assert_eq!(prompts.len(), 2);
    assert_eq!(prompts[0].0, Phase::Build);
    assert!(!prompts[0].1.to_string().contains("secret source body"));
    assert_eq!(prompts[1].1["wasm_sha256"], "actual-finalized-hash");
}

#[tokio::test]
async fn unbound_call_and_disabled_ceiling_never_prompt_or_build() {
    let cancel = CancellationToken::new();
    let editor = Editor::new(cancel.clone());
    let builder = Builder::default();
    editor.bound.store(false, Ordering::Release);
    assert!(matches!(
        phases(&editor, &builder, request(), ceiling(), cancel.clone()).await,
        Err(GenerationError::SessionNotBound)
    ));
    editor.bound.store(true, Ordering::Release);
    assert!(matches!(
        phases(
            &editor,
            &builder,
            request(),
            GenerationPermissions::default(),
            cancel
        )
        .await,
        Err(GenerationError::PermissionDenied)
    ));
    assert!(editor.approved.lock().unwrap().is_empty());
    assert_eq!(builder.builds.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn rebuild_requires_separate_operator_authority_and_ui_disclosure() {
    let cancel = CancellationToken::new();
    let editor = Editor::new(cancel.clone());
    let builder = Builder::default();
    let mut input = request();
    input.target = GenerationTarget::Rebuild {
        expected_revision: "bound-revision".into(),
    };
    assert!(matches!(
        phases(
            &editor,
            &builder,
            input.clone(),
            GenerationPermissions::new(true, true, false),
            cancel.clone()
        )
        .await,
        Err(GenerationError::PermissionDenied)
    ));
    assert!(editor.approved.lock().unwrap().is_empty());
    let permissions = phases(&editor, &builder, input, ceiling(), cancel)
        .await
        .unwrap();
    assert!(permissions.can_rebuild());
    let prompts = editor.approved.lock().unwrap();
    assert_eq!(
        prompts[0].1["target"]["expected_revision"],
        "bound-revision"
    );
    assert_eq!(prompts[0].1["rebuild"], true);
}

#[tokio::test]
async fn stale_revision_during_permission_is_not_reported_as_success() {
    let cancel = CancellationToken::new();
    let editor = Editor::new(cancel.clone());
    let builder = Builder {
        stale_commit: true,
        ..Builder::default()
    };
    assert!(matches!(
        phases(&editor, &builder, request(), ceiling(), cancel).await,
        Err(GenerationError::Stale(_))
    ));
    assert_eq!(builder.installs.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn layer_generation_needs_only_build_and_install_approvals() {
    let cancel = CancellationToken::new();
    let editor = Editor::new(cancel.clone());
    let builder = Builder::default();
    let mut layer = request();
    layer.build.kind = ComponentKind::AcpLayer;
    let permissions = phases(&editor, &builder, layer, ceiling(), cancel)
        .await
        .unwrap();
    assert!(permissions.can_install());
}

#[test]
fn bounded_requests_cannot_supply_operator_config_or_permissions() {
    assert!(matches!(
        parse_request(&" ".repeat(MAX_REQUEST_BYTES + 1)),
        Err(GenerationError::InvalidRequest(_))
    ));
    let input = serde_json::to_value(request()).unwrap();
    for key in ["builder_config", "source_id", "permissions", "helper_path"] {
        let mut input = input.clone();
        input[key] = json!(true);
        assert!(matches!(
            parse_request(&input.to_string()),
            Err(GenerationError::InvalidRequest(_))
        ));
    }

    let mut input = input;
    input["build"]["kind"] = json!("acp-provider");
    assert!(matches!(
        parse_request(&input.to_string()),
        Err(GenerationError::InvalidRequest(_))
    ));
}

#[test]
fn malformed_input_and_host_errors_never_echo_source_or_unbounded_details() {
    let secret = "PRIVATE_SOURCE_SENTINEL";
    let payload = secret.repeat(100_000);
    let invalid = parse_request(&format!(r#"{{"build": "{payload}"}}"#))
        .err()
        .unwrap();
    let errors = [
        invalid,
        core_error(anyhow::anyhow!(payload.clone())),
        broker_error(ToolError::Unavailable(payload)),
        core_error(StoreError::Conflict(secret.into()).into()),
    ];
    for error in errors {
        let text = format!("{error:?}");
        assert!(!text.contains(secret));
        assert!(
            text.len() < 256,
            "adapter errors must have bounded fixed descriptions"
        );
    }
}

#[test]
fn typed_builder_failures_disclose_only_authorized_guest_diagnostics() {
    for (kind, diagnostic) in [
        (
            BuildErrorKind::InvalidRequest,
            "Rust source exceeds its budget",
        ),
        (BuildErrorKind::InvalidWit, "expected a type at WIT:1:20"),
        (
            BuildErrorKind::CompilationFailed,
            "error[E0308]: mismatched types",
        ),
        (
            BuildErrorKind::InvalidOutput,
            "exported function type differs from WIT",
        ),
    ] {
        let error = build_failure(kind, Some(diagnostic), false);
        let message = match error {
            GenerationError::InvalidRequest(message) | GenerationError::BuildFailed(message) => {
                message
            }
            _ => panic!("guest diagnostics must retain their typed failure classification"),
        };
        assert_eq!(message, diagnostic);
    }
    for kind in [
        BuildErrorKind::Unavailable,
        BuildErrorKind::Internal,
        BuildErrorKind::Cancelled,
        BuildErrorKind::DeadlineExceeded,
    ] {
        let error = build_failure(kind, Some("PRIVATE_HOST_CONTEXT"), false);
        assert!(!format!("{error:?}").contains("PRIVATE_HOST_CONTEXT"));
    }
    assert!(matches!(
        build_failure(BuildErrorKind::Cancelled, None, false),
        GenerationError::Cancelled,
    ));
}

#[test]
fn typed_guest_failure_diagnostics_keep_json_bounds_and_truncation_evidence() {
    for diagnostic in [
        "\0".repeat(100_000),
        "\"\\\n".repeat(100_000),
        "\u{1f980}".repeat(100_000),
    ] {
        let GenerationError::BuildFailed(message) =
            build_failure(BuildErrorKind::CompilationFailed, Some(&diagnostic), false)
        else {
            panic!("expected a typed compilation failure")
        };
        assert!(serde_json::to_string(&message).unwrap().len() <= MAX_DIAGNOSTIC_BYTES);
        assert!(message.ends_with("[diagnostics truncated]"));
    }
    let GenerationError::InvalidRequest(message) = build_failure(
        BuildErrorKind::InvalidWit,
        Some("bounded WIT diagnostic"),
        true,
    ) else {
        panic!("expected an invalid WIT request")
    };
    assert_eq!(message, "bounded WIT diagnostic\n[diagnostics truncated]");
}

#[test]
fn diagnostic_json_is_bounded_without_reformatting_the_receipt() {
    for text in ["\u{1f980}", "\u{0000}", "\n\r\t\"\\", "a"] {
        let report = json!({
            "commit": { "exact": "canonical receipt", "revision": "revision" },
            "preview": { "diagnostics": text.repeat(MAX_DIAGNOSTIC_BYTES) },
        });
        let presented: Value = serde_json::from_str(&bounded_report(report.clone())).unwrap();
        assert_eq!(presented["commit"], report["commit"]);
        let diagnostics = presented["preview"]["diagnostics"].as_str().unwrap();
        assert!(serde_json::to_string(diagnostics).unwrap().len() <= MAX_DIAGNOSTIC_BYTES);
        assert!(diagnostics.ends_with("[diagnostics truncated]"));
    }
    let short = json!({ "preview": { "diagnostics": "short diagnostic" } });
    assert_eq!(
        serde_json::from_str::<Value>(&bounded_report(short.clone())).unwrap(),
        short
    );
    let exact = json!({ "preview": { "diagnostics": "a".repeat(MAX_DIAGNOSTIC_BYTES - 2) } });
    assert_eq!(
        serde_json::from_str::<Value>(&bounded_report(exact.clone())).unwrap(),
        exact
    );
}

#[tokio::test]
async fn oversized_metadata_never_enters_permission_ui() {
    let cancel = CancellationToken::new();
    let editor = Editor::new(cancel.clone());
    let builder = Builder::default();
    let mut input = request();
    input.build.component_name = "x".repeat(MAX_METADATA_BYTES + 1);
    assert!(matches!(
        phases(&editor, &builder, input, ceiling(), cancel).await,
        Err(GenerationError::InvalidRequest(_))
    ));
    assert!(editor.approved.lock().unwrap().is_empty());
    assert_eq!(builder.builds.load(Ordering::Relaxed), 0);
}

#[test]
fn stale_publication_preserves_the_actual_commit_without_claiming_exposure() {
    for publication in [
        Err(ToolError::Stale("revision changed".into())),
        Err(ToolError::Unavailable("catalog unavailable".into())),
    ] {
        let canonical = r#"{"commit":{"entry":{"revision":"committed-revision"}}}"#;
        let report = GenerationReport {
            report_json: canonical.into(),
            disposition: Disposition::Installed,
            tool_handles: Vec::new(),
        };
        let result = publication_report(report, publication);
        let Err(GenerationError::Committed(report)) = result else {
            panic!("a failed publication must retain the durable commit");
        };
        assert_eq!(report.report_json, canonical);
        assert!(matches!(
            report.disposition,
            Disposition::CommittedNotExposed
        ));
        assert!(report.tool_handles.is_empty());
    }
}

#[test]
fn current_zero_export_artifact_is_installed_without_claiming_session_tools() {
    let canonical = r#"{"commit":{"entry":{"revision":"committed-revision"}}}"#;
    let report = GenerationReport {
        report_json: canonical.into(),
        disposition: Disposition::Installed,
        tool_handles: Vec::new(),
    };
    let report = publication_report(report, Ok(Vec::new())).unwrap();
    assert_eq!(report.report_json, canonical);
    assert!(matches!(report.disposition, Disposition::ToolsEligible));
    assert!(report.tool_handles.is_empty());
}

#[tokio::test]
async fn recovery_reports_preserve_known_and_unknown_outcomes_without_diagnostics() {
    let root = tempfile::tempdir_in(".").unwrap();
    let manager = wassette::LifecycleManager::builder(root.path().join("store"))
        .with_secrets_dir(root.path().join("secrets"))
        .build()
        .await
        .unwrap();
    let wasm = wat::parse_str(
        r#"(component $recovery:tool
            (core module $m (func (export "run") (result i32) i32.const 1))
            (core instance $i (instantiate $m))
            (func (export "run") (result u32) (canon lift (core func $i "run"))))"#,
    )
    .unwrap();
    let path = root.path().join("tool.wasm");
    std::fs::write(&path, wasm).unwrap();
    let commit = manager
        .load_component(&format!(
            "file://{}",
            path.canonicalize().unwrap().display()
        ))
        .await
        .unwrap()
        .commit;
    let actual_operation = commit.change.as_ref().unwrap().operation.clone();
    let secret = "PRIVATE_RECOVERY_DIAGNOSTIC";
    for known in [false, true] {
        let operation = if known {
            actual_operation.clone()
        } else {
            "unobserved-operation".into()
        };
        let error = CoreError::CommitRecoveryRequired {
            operation: operation.clone(),
            observed_commit: known.then(|| Box::new(commit.clone())),
            observation_error: Some(StoreError::Integrity(secret.repeat(100_000))),
            source: StoreError::RecoveryRequired {
                operation: operation.clone(),
                detail: secret.repeat(100_000),
            },
        };
        let canonical = error.recovery_report().unwrap();
        assert!(error.committed_report().is_none());
        let GenerationError::RecoveryRequired(report) = core_error(error.into()) else {
            panic!("recovery must not become an ordinary failure or an assumed commit");
        };
        assert!(!report.contains(secret));
        assert!(report.len() < MAX_DIAGNOSTIC_BYTES);
        let report: Value = serde_json::from_str(&report).unwrap();
        assert_eq!(report, canonical);
        assert_eq!(report["operation"], operation);
        assert!(report["refresh"].is_null());
        if known {
            assert_eq!(report["status"], "committed-recovery-required");
            assert_eq!(report["commit"], serde_json::to_value(&commit).unwrap());
        } else {
            assert_eq!(report["status"], "commit-unknown");
            assert!(report["commit"].is_null());
        }
    }
}
