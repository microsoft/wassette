// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::collections::BTreeMap;

use super::*;

pub(super) async fn manager() -> (tempfile::TempDir, LifecycleManager) {
    let root = tempfile::tempdir().unwrap();
    let manager = LifecycleManager::builder(root.path().join("components"))
        .with_secrets_dir(root.path().join("secrets"))
        .with_eager_loading(false)
        .build()
        .await
        .unwrap();
    (root, manager)
}

pub(super) fn permissions() -> GenerationPermissions {
    GenerationPermissions::new(true, true, true, true)
}

pub(super) fn request(name: &str, intent: InstallIntent) -> GenerationRequest {
    GenerationRequest {
        build: BuildRequest {
            component_name: name.into(),
            source: "struct Component;".into(),
            wit: "package test:generated; world tool { export run: func() -> s32; }".into(),
            world: "tool".into(),
            kind: ComponentKind::Tool,
        },
        target: GenerationTarget::New,
        intent,
        reinstall_policy: None,
    }
}

pub(super) fn wasm(name: &str, value: i32) -> Vec<u8> {
    let name = serde_json::to_string(name).unwrap();
    wat::parse_str(format!(
        r#"(component ${name}
            (core module $m (func (export "run") (result i32) i32.const {value}))
            (core instance $i (instantiate $m))
            (func (export "run") (result s32) (canon lift (core func $i "run"))))"#
    ))
    .unwrap()
}

fn build_evidence() -> wassette_builder::BuildEvidence {
    serde_json::from_value(serde_json::json!({
        "source_sha256": "11".repeat(32),
        "wit_sha256": "22".repeat(32),
        "wit_dependencies_sha256": "55".repeat(32),
        "initrd_sha256": "33".repeat(32),
        "builder_sha256": "66".repeat(32),
        "profile_id": "rust-std-v1",
        "profile_sha256": "77".repeat(32),
        "compiler_version": "rustc 1.98.1",
        "bindgen_version": "0.62.0",
        "binding_runtime_version": "inline-v1",
        "runtime_version": "hyperlight-unikraft-0.17.0",
        "world": "tool",
        "target_platform": "wasm32-wasip2",
        "host_platform": "aarch64-macos",
        "kind": ComponentKind::Tool,
        "component_name": "fixture"
    }))
    .unwrap()
}

pub(super) async fn candidate(
    manager: &LifecycleManager,
    request: GenerationRequest,
    wasm: Vec<u8>,
) -> PreparedGeneration {
    let selected = select_target(manager, &request).await.unwrap();
    let artifact = BuildArtifact {
        wasm,
        diagnostics: String::new(),
        evidence: build_evidence(),
    };
    let preview = GenerationPreview {
        component_id: request.build.component_name,
        kind: request.build.kind,
        wasm_sha256: hex::encode(Sha256::digest(&artifact.wasm)),
        expected_revision: selected
            .expected
            .entry()
            .map(|entry| entry.revision().to_string()),
        evidence: generation_evidence(&artifact),
        diagnostics: String::new(),
    };
    PreparedGeneration {
        manager: manager.clone(),
        selected,
        artifact,
        preview,
        intent: request.intent,
        permissions: permissions(),
        validator: None,
        is_rebuild: matches!(request.target, GenerationTarget::Rebuild { .. }),
    }
}

fn rebuild(name: &str, revision: &crate::store::EntryRevision) -> GenerationRequest {
    GenerationRequest {
        target: GenerationTarget::Rebuild {
            expected_revision: revision.to_string(),
        },
        ..request(name, InstallIntent::ExposeTools)
    }
}

fn files(root: &std::path::Path) -> BTreeMap<std::ffi::OsString, Vec<u8>> {
    std::fs::read_dir(root)
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.unwrap();
            entry
                .file_type()
                .unwrap()
                .is_file()
                .then(|| (entry.file_name(), std::fs::read(entry.path()).unwrap()))
        })
        .collect()
}

#[tokio::test]
async fn default_manager_has_no_generation_authority() {
    let (_root, manager) = manager().await;
    let error = manager.generation_service().err().unwrap();
    assert!(matches!(
        error.downcast_ref(),
        Some(GenerationError::Disabled)
    ));
    let denied = GenerationPermissions::default();
    assert!(!denied.can_build());
    assert!(!denied.can_install());
    assert!(!denied.can_expose());
    assert!(!denied.can_rebuild());
}

#[tokio::test]
async fn captured_install_is_named_transactional_and_install_only_stays_hidden() {
    let (_root, manager) = manager().await;
    let pending = candidate(
        &manager,
        request("example:generated/tool", InstallIntent::InstallOnly),
        wasm("example:generated/tool", 42),
    )
    .await;
    let name = pending.preview().component_id.clone();
    assert!(manager.component_store().read(&name).is_err());
    let outcome = pending
        .install(permissions(), CancellationToken::new())
        .await
        .unwrap();
    let receipt = outcome.commit.entry.binding();
    assert_eq!(receipt.component_id.as_str(), name);
    assert_ne!(receipt.storage_key.as_str(), name);
    assert!(matches!(receipt.source, SourceIdentity::Generated { .. }));
    assert_eq!(receipt.schema, 2);
    assert_eq!(receipt.artifact_sha256, outcome.preview.wasm_sha256);
    assert!(!receipt.requests_tool_exposure());
    assert!(manager.catalog().await.unwrap().tools.is_empty());
    let cold = LifecycleManager::builder(manager.config.component_dir())
        .with_secrets_dir(manager.config.secrets_dir())
        .with_eager_loading(false)
        .build()
        .await
        .unwrap();
    assert!(cold.catalog().await.unwrap().tools.is_empty());
}

#[tokio::test]
async fn same_lineage_rebuild_invalidates_refs_but_preserves_policy_and_secret_binding() {
    let (_root, manager) = manager().await;
    let first = candidate(
        &manager,
        request("example:generated", InstallIntent::ExposeTools),
        wasm("example:generated", 1),
    )
    .await
    .install(permissions(), CancellationToken::new())
    .await
    .unwrap();
    let first_receipt = first.commit.entry.binding();
    let binding = first_receipt.secret_binding().unwrap();
    manager
        .secrets_manager
        .set_bound_component_secrets(&binding, &[("KEY".into(), "kept".into())])
        .await
        .unwrap();
    let policy = PreparedPolicy::parse(
        b"version: '1.0'\npermissions: {}\n".to_vec(),
        PolicyProvenance::PermissionEdit,
    )
    .unwrap();
    let edited = manager
        .component_store()
        .update_policy("example:generated", first.commit.entry.revision(), policy)
        .unwrap();
    manager.refresh_from_store().await.unwrap();
    let old = manager
        .catalog()
        .await
        .unwrap()
        .tools
        .into_iter()
        .next()
        .unwrap();
    let old_policy = manager
        .component_store()
        .read("example:generated")
        .unwrap()
        .policy;
    let updated = candidate(
        &manager,
        rebuild("example:generated", edited.entry.revision()),
        wasm("example:generated", 2),
    )
    .await
    .install(permissions(), CancellationToken::new())
    .await
    .unwrap();
    assert_ne!(updated.commit.entry.revision(), edited.entry.revision());
    assert_eq!(
        updated.commit.entry.binding().secret_binding().unwrap(),
        binding
    );
    assert_eq!(
        manager
            .component_store()
            .read("example:generated")
            .unwrap()
            .policy,
        old_policy
    );
    assert_eq!(
        manager
            .secrets_manager
            .load_bound_component_secrets(&binding)
            .await
            .unwrap()["KEY"],
        "kept"
    );
    let stale = manager
        .prepare_invocation(&old.reference, &serde_json::json!({}))
        .await
        .err()
        .unwrap();
    assert!(matches!(stale, crate::ToolInvocationError::Stale(_)));
    let called = manager
        .invoke_unique_tool("run", &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&called.raw_result).unwrap(),
        serde_json::json!({"result": 2}),
    );
}

#[tokio::test]
async fn invalid_replacement_preserves_last_good_artifact_policy_and_caches() {
    let (_root, manager) = manager().await;
    let first = candidate(
        &manager,
        request("example:generated", InstallIntent::ExposeTools),
        wasm("example:generated", 1),
    )
    .await
    .install(permissions(), CancellationToken::new())
    .await
    .unwrap();
    let before_call = manager
        .invoke_unique_tool("run", &serde_json::json!({}))
        .await
        .unwrap()
        .raw_result;
    let before = files(manager.config.component_dir());
    let invalid = candidate(
        &manager,
        rebuild("example:generated", first.commit.entry.revision()),
        b"invalid component".to_vec(),
    )
    .await;
    assert!(invalid
        .install(permissions(), CancellationToken::new())
        .await
        .is_err());
    assert_eq!(files(manager.config.component_dir()), before);
    assert_eq!(
        manager
            .invoke_unique_tool("run", &serde_json::json!({}))
            .await
            .unwrap()
            .raw_result,
        before_call
    );
}

#[tokio::test]
async fn install_expose_and_rebuild_permissions_are_independent() {
    let (_root, manager) = manager().await;
    let pending = candidate(
        &manager,
        request("example:generated", InstallIntent::ExposeTools),
        wasm("example:generated", 1),
    )
    .await;
    let error = pending
        .install(
            GenerationPermissions::new(true, true, false, false),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(GenerationError::PermissionDenied("expose"))
    ));
    assert!(manager.component_store().read("example:generated").is_err());
    let pending = candidate(
        &manager,
        request("example:generated", InstallIntent::InstallOnly),
        wasm("example:generated", 1),
    )
    .await;
    let error = pending
        .install(
            GenerationPermissions::new(true, false, true, true),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(GenerationError::PermissionDenied("install"))
    ));
    assert!(manager.component_store().read("example:generated").is_err());
}

#[tokio::test]
async fn cancellation_and_pending_permission_revision_race_do_not_commit() {
    let (_root, manager) = manager().await;
    let pending = candidate(
        &manager,
        request("example:generated", InstallIntent::InstallOnly),
        wasm("example:generated", 1),
    )
    .await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = pending.install(permissions(), cancel).await.unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(GenerationError::Cancelled)
    ));
    assert!(manager.component_store().read("example:generated").is_err());

    let first = candidate(
        &manager,
        request("example:generated", InstallIntent::ExposeTools),
        wasm("example:generated", 1),
    )
    .await
    .install(permissions(), CancellationToken::new())
    .await
    .unwrap();
    let pending = candidate(
        &manager,
        rebuild("example:generated", first.commit.entry.revision()),
        wasm("example:generated", 2),
    )
    .await;
    let changed = manager
        .component_store()
        .update_policy(
            "example:generated",
            first.commit.entry.revision(),
            PreparedPolicy::absent(PolicyProvenance::PermissionEdit),
        )
        .unwrap();
    assert!(pending
        .install(permissions(), CancellationToken::new())
        .await
        .is_err());
    assert_eq!(
        manager
            .component_store()
            .read("example:generated")
            .unwrap()
            .receipt
            .revision,
        *changed.entry.revision()
    );
}

#[tokio::test]
async fn another_lineage_cannot_take_over_even_after_retirement() {
    let (_root, manager) = manager().await;
    let first = candidate(
        &manager,
        request("example:generated", InstallIntent::InstallOnly),
        wasm("example:generated", 1),
    )
    .await
    .install(permissions(), CancellationToken::new())
    .await
    .unwrap();
    let new_source = request("example:generated", InstallIntent::InstallOnly);
    assert!(select_target(&manager, &new_source).await.is_err());
    let retired = manager
        .component_store()
        .remove(
            "example:generated",
            first.commit.entry.revision(),
            crate::store::RemovalAuthority::Explicit,
        )
        .unwrap();
    assert!(select_target(&manager, &new_source).await.is_err());
    assert!(select_target(
        &manager,
        &rebuild("example:generated", first.commit.entry.revision())
    )
    .await
    .is_err());
    let restored = candidate(
        &manager,
        rebuild("example:generated", retired.entry.revision()),
        wasm("example:generated", 2),
    )
    .await
    .install(permissions(), CancellationToken::new())
    .await
    .unwrap();
    assert_eq!(
        restored.commit.entry.binding().source,
        first.commit.entry.binding().source
    );
}

#[tokio::test]
async fn unchanged_rebuild_still_hydrates_a_cold_manager() {
    let (_root, manager) = manager().await;
    let first = candidate(
        &manager,
        request("example:generated", InstallIntent::ExposeTools),
        wasm("example:generated", 42),
    )
    .await
    .install(permissions(), CancellationToken::new())
    .await
    .unwrap();
    let cold = LifecycleManager::builder(manager.config.component_dir())
        .with_secrets_dir(manager.config.secrets_dir())
        .with_eager_loading(false)
        .build()
        .await
        .unwrap();
    let same = candidate(
        &cold,
        rebuild("example:generated", first.commit.entry.revision()),
        wasm("example:generated", 42),
    )
    .await
    .install(permissions(), CancellationToken::new())
    .await
    .unwrap();
    assert!(same.commit.change.is_none());
    assert_eq!(same.commit.cursor, first.commit.cursor);
    assert_eq!(cold.catalog().await.unwrap().tools.len(), 1);
    let output = cold
        .invoke_unique_tool("run", &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output.raw_result).unwrap(),
        serde_json::json!({"result": 42})
    );
}

#[tokio::test]
async fn refresh_failure_reports_the_actual_committed_receipt() {
    let (_root, manager) = manager().await;
    let old = candidate(
        &manager,
        request("example:other", InstallIntent::ExposeTools),
        wasm("example:other", 1),
    )
    .await
    .install(permissions(), CancellationToken::new())
    .await
    .unwrap();
    let cold = LifecycleManager::builder(manager.config.component_dir())
        .with_secrets_dir(manager.config.secrets_dir())
        .with_eager_loading(false)
        .build()
        .await
        .unwrap();
    let pending = candidate(
        &cold,
        request("example:generated", InstallIntent::InstallOnly),
        wasm("example:generated", 42),
    )
    .await;
    let other_path = manager
        .config
        .component_dir()
        .join(format!("{}.wasm", old.commit.entry.storage_key().as_str()));
    std::fs::write(other_path, b"externally corrupted unrelated artifact").unwrap();
    let error = pending
        .install(permissions(), CancellationToken::new())
        .await
        .unwrap_err();
    let Some(GenerationError::CommittedButRefreshFailed { commit, .. }) = error.downcast_ref()
    else {
        panic!("expected a receipt-bearing refresh error, got {error:#}");
    };
    let stored = manager.component_store().read("example:generated").unwrap();
    assert_eq!(&stored.receipt, commit.entry.binding());
    assert_eq!(stored.wasm, wasm("example:generated", 42));
    assert!(!stored.receipt.requests_tool_exposure());
}

#[test]
fn request_cannot_supply_authority_or_lineage() {
    let base =
        serde_json::to_value(request("example:generated", InstallIntent::InstallOnly)).unwrap();
    for field in [
        "permissions",
        "builder",
        "source_identity",
        "storage_key",
        "helper_path",
        "caller",
        "prepared_handle",
    ] {
        let mut value = base.clone();
        value[field] = serde_json::json!("not host authority");
        assert!(serde_json::from_value::<GenerationRequest>(value).is_err());
    }
    let mut value = base;
    value["target"] = serde_json::json!({"mode":"new", "id":"ab".repeat(16)});
    assert!(serde_json::from_value::<GenerationRequest>(value).is_err());
}

#[tokio::test]
async fn postdecision_failure_preserves_operation_and_observed_receipt() {
    let (_root, manager) = manager().await;
    let pending = candidate(
        &manager,
        request("example:generated", InstallIntent::InstallOnly),
        wasm("example:generated", 42),
    )
    .await;
    manager
        .component_store()
        .fail_next_commit_for_test("after-head");
    let error = pending
        .install(permissions(), CancellationToken::new())
        .await
        .unwrap_err();
    let Some(
        error @ GenerationError::CommitRecoveryRequired {
            operation,
            observed_commit,
            observation_error,
            ..
        },
    ) = error.downcast_ref::<GenerationError>()
    else {
        panic!("postdecision failure lost its store outcome: {error:#}");
    };
    assert!(!operation.is_empty());
    assert!(observation_error.is_none());
    let observed = observed_commit
        .as_ref()
        .expect("exact committed operation was observed");
    let current = manager.component_store().read("example:generated").unwrap();
    assert_eq!(&current.receipt, observed.entry.binding());
    let report = error.recovery_report().unwrap();
    assert_eq!(report["status"], "committed-recovery-required");
    assert_eq!(report["operation"], *operation);
    assert!(!report["commit"].is_null());
    assert!(report["refresh"].is_null());
}

#[tokio::test]
async fn recovery_never_infers_commit_from_matching_artifact_bytes() {
    let (_root, manager) = manager().await;
    let installed = candidate(
        &manager,
        request("example:generated", InstallIntent::InstallOnly),
        wasm("example:generated", 42),
    )
    .await
    .install(permissions(), CancellationToken::new())
    .await
    .unwrap();
    assert!(observe_generated_commit(
        manager.component_store(),
        "another-operation",
        "example:generated",
        &installed.commit.entry.binding().artifact_sha256,
    )
    .unwrap()
    .is_none());
    let error = GenerationError::CommitRecoveryRequired {
        operation: "another-operation".into(),
        observed_commit: None,
        observation_error: None,
        source: StoreError::RecoveryRequired {
            operation: "another-operation".into(),
            detail: "outcome could not be established".into(),
        },
    };
    let report = error.recovery_report().unwrap();
    assert_eq!(report["status"], "commit-unknown");
    assert!(report["commit"].is_null());
    assert!(error.committed_report().is_none());
}

#[test]
fn operator_configuration_is_bounded_before_deserialization() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("generation.json");
    std::fs::write(&path, vec![b' '; MAX_CONFIG_BYTES as usize + 1]).unwrap();
    assert!(GenerationConfig::read(&path)
        .err()
        .unwrap()
        .to_string()
        .contains("size limit"));
}
