// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::process::Command;
use std::sync::{Arc, Barrier, OnceLock};

use sha2::Digest;

use super::*;

fn directory() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(".store-test-")
        .tempdir_in(std::env::current_dir().unwrap())
        .unwrap()
}

fn wasm(name: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(component $"{name}" (instance $empty) (export "empty" (instance $empty)))"#
    ))
    .unwrap()
}

fn validator(
    bytes: &[u8],
    _: &crate::ArtifactInspection,
    _: Option<&[u8]>,
) -> anyhow::Result<ValidationEvidence> {
    static ENGINE: OnceLock<wasmtime::Engine> = OnceLock::new();
    let engine = ENGINE.get_or_init(wasmtime::Engine::default);
    let component = wasmtime::component::Component::new(engine, bytes)?;
    wasmtime::component::Linker::<()>::new(engine).instantiate_pre(&component)?;
    Ok(ValidationEvidence::OrdinaryPrepared {
        runtime: "test-ordinary".into(),
    })
}

fn source() -> SourceIdentity {
    SourceIdentity::OciRepository("ghcr.io/example/tool".into())
}

fn options(key: &str) -> InstallOptions {
    InstallOptions {
        storage_key: StorageKey::parse(key).unwrap(),
        source: source(),
        origin: OriginEvidence {
            location: "oci://ghcr.io/example/tool:1".into(),
            requested_version: Some("1".into()),
            selected_version: Some("1".into()),
            manifest_digest: Some("sha256:manifest-not-artifact".into()),
            immutable_uri: None,
            generation: None,
        },
        owner: InstallOwner::Explicit,
        intent: InstallIntent::InstallOnly,
        policy: PreparedPolicy::absent(PolicyProvenance::Default),
        observation: None,
    }
}

fn prepare(name: &str, options: InstallOptions) -> PreparedInstall {
    PreparedInstall::prepare(wasm(name), options, validator).unwrap()
}

fn install(store: &ComponentStore, name: &str, options: InstallOptions) -> CommitOutcome {
    let expected = store
        .observe(name, &options.storage_key, &options.source)
        .unwrap();
    store
        .commit_install(prepare(name, options), expected)
        .unwrap()
}

fn receipt(outcome: &CommitOutcome) -> &InstallReceipt {
    match &outcome.entry {
        StoredEntry::Installed(receipt) => receipt,
        _ => panic!("expected installed entry"),
    }
}

fn policy(description: &str, provenance: PolicyProvenance) -> PreparedPolicy {
    PreparedPolicy::parse(
        format!("version: \"1.0\"\ndescription: {description}\npermissions: {{}}\n").into_bytes(),
        provenance,
    )
    .unwrap()
}

fn owner(root_key: &str) -> ManagedLocalSource {
    ManagedLocalSource::new(root_key, "source").unwrap()
}

#[test]
fn install_read_noop_and_reinstall_preserve_semantic_binding() {
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let first = install(&store, "actual:name", options("private-key"));
    let read = store.read("actual:name").unwrap();
    assert_eq!(read.wasm, wasm("actual:name"));
    assert_eq!(read.receipt, *receipt(&first));
    assert!(matches!(
        store.read("private-key"),
        Err(StoreError::NotFound(_))
    ));
    assert!(directory.path().join("private-key.wasm").exists());
    assert_eq!(first.change.as_ref().unwrap().cursor, first.cursor);
    let noop = install(&store, "actual:name", options("private-key"));
    assert!(noop.change.is_none());
    assert_eq!(receipt(&first), receipt(&noop));
    assert_eq!(first.cursor, noop.cursor);
    assert!(store
        .snapshot_if_changed(Some(&first.cursor))
        .unwrap()
        .is_none());
    assert!(store.snapshot_if_changed(None).unwrap().is_some());

    let retired = store
        .remove(
            "actual:name",
            &receipt(&first).revision,
            RemovalAuthority::Explicit,
        )
        .unwrap();
    assert!(!directory.path().join("private-key.wasm").exists());
    assert!(matches!(
        store.read("actual:name"),
        Err(StoreError::NotFound(_))
    ));
    let reinstalled = install(&store, "actual:name", options("private-key"));
    assert_ne!(receipt(&reinstalled).revision, receipt(&first).revision);
    assert_ne!(*reinstalled.entry.revision(), *retired.entry.revision());
}

#[test]
fn invalid_identity_kind_runtime_and_policy_are_never_prepared() {
    assert!(PreparedInstall::prepare(
        wat::parse_str("(component)").unwrap(),
        options("key"),
        validator
    )
    .is_err());
    assert!(PreparedInstall::prepare(
        wat::parse_str("(module)").unwrap(),
        options("key"),
        validator
    )
    .is_err());
    assert!(PreparedInstall::prepare(
        wat::parse_str("(component $name)").unwrap(),
        options("key"),
        validator
    )
    .is_err());
    let mut ambiguous = wasm_encoder::Component::new();
    let mut names = wasm_encoder::ComponentNameSection::new();
    names.component("first");
    names.component("first");
    ambiguous.section(&names);
    assert!(PreparedInstall::prepare(ambiguous.finish(), options("key"), validator).is_err());
    assert!(PreparedInstall::prepare(vec![0, 1, 2], options("key"), validator).is_err());
    assert!(
        PreparedInstall::prepare(wasm("valid"), options("key"), |_, _, _| {
            anyhow::bail!("runtime refused")
        })
        .is_err()
    );
    assert!(
        PreparedInstall::prepare(wasm("valid"), options("key"), |_, _, _| {
            Ok(ValidationEvidence::AcpCompiledAndExportChecked {
                runtime: "wrong-route".into(),
            })
        })
        .is_err()
    );
    assert!(PreparedPolicy::parse(b"[".to_vec(), PolicyProvenance::Bundled).is_err());
    assert!(PreparedPolicy::parse(vec![255], PolicyProvenance::ExplicitAttachment).is_err());
}

#[test]
fn acp_evidence_is_bounded_and_requires_its_own_validator() {
    let bytes = wat::parse_str(
        r#"(component $provider
          (instance $agent)
          (export "wassette:acp/agent@0.1.0" (instance $agent)))"#,
    )
    .unwrap();
    assert!(PreparedInstall::prepare(bytes.clone(), options("key"), validator).is_err());
    let prepared = PreparedInstall::prepare(bytes, options("key"), |bytes, inspection, _| {
        assert_eq!(inspection.shape, crate::ArtifactShape::AcpProvider);
        let engine = wasmtime::Engine::default();
        wasmtime::component::Component::new(&engine, bytes)?;
        assert_eq!(inspection.acp_exports, ["wassette:acp/agent@0.1.0"]);
        Ok(ValidationEvidence::AcpCompiledAndExportChecked {
            runtime: "test-acp-export-checked".into(),
        })
    })
    .unwrap();
    assert_eq!(prepared.kind, StoredArtifactKind::AcpProvider);
}

#[test]
fn policy_only_changes_are_revisioned_and_bundles_cannot_override_them() {
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let installed = install(&store, "semantic", options("key"));
    let original_hash = receipt(&installed).artifact_sha256.clone();
    let edited = store
        .update_policy(
            "semantic",
            &receipt(&installed).revision,
            policy("operator", PolicyProvenance::PermissionEdit),
        )
        .unwrap();
    assert_ne!(receipt(&edited).revision, receipt(&installed).revision);
    assert_eq!(receipt(&edited).artifact_sha256, original_hash);
    assert!(edited.change.as_ref().unwrap().policy_changed);
    assert!(!edited.change.as_ref().unwrap().artifact_changed);
    let expected = store
        .observe("semantic", &StorageKey::parse("key").unwrap(), &source())
        .unwrap();
    assert!(matches!(
        store.commit_install(prepare("semantic", options("key")), expected),
        Err(StoreError::Conflict(_))
    ));
    let cleared = store
        .update_policy(
            "semantic",
            &receipt(&edited).revision,
            PreparedPolicy::absent(PolicyProvenance::ExplicitAttachment),
        )
        .unwrap();
    assert!(store.read("semantic").unwrap().policy.is_none());
    assert_ne!(receipt(&cleared).revision, receipt(&edited).revision);
    let expected = store
        .observe("semantic", &StorageKey::parse("key").unwrap(), &source())
        .unwrap();
    assert!(store
        .commit_install(prepare("semantic", options("key")), expected)
        .is_err());
}

#[test]
fn provenance_and_owner_changes_alone_advance_revisions() {
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let first = install(&store, "semantic", options("key"));
    let mut changed = options("key");
    changed.origin.selected_version = Some("2".into());
    let second = install(&store, "semantic", changed);
    assert_ne!(receipt(&first).revision, receipt(&second).revision);
    let change = second.change.unwrap();
    assert!(change.provenance_changed);
    assert!(!change.artifact_changed);
    assert!(!change.policy_changed);
}

#[test]
fn semantic_source_key_and_alias_reservations_survive_removal() {
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let installed = install(&store, "semantic", options("a__b"));
    let other = SourceIdentity::OciRepository("ghcr.io/unrelated/tool".into());
    let check = || {
        for (id, key, source) in [
            ("semantic", "another", source()),
            ("semantic", "a__b", other.clone()),
            ("different", "a__b", source()),
            ("different", "A__B", source()),
            ("different", "a_b", source()),
            ("different", "_a_b_", source()),
        ] {
            assert!(matches!(
                store.observe(id, &StorageKey::parse(key).unwrap(), &source),
                Err(StoreError::Conflict(_))
            ));
        }
    };
    check();
    store
        .remove(
            "semantic",
            &receipt(&installed).revision,
            RemovalAuthority::Explicit,
        )
        .unwrap();
    check();
    assert!(store
        .observe("semantic", &StorageKey::parse("a__b").unwrap(), &source())
        .is_ok());
}

#[test]
fn truncated_secret_aliases_are_reserved() {
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let prefix = "a".repeat(128);
    install(&store, "first", options(&format!("{prefix}x")));
    assert!(store
        .observe(
            "second",
            &StorageKey::parse(&format!("{prefix}y")).unwrap(),
            &source()
        )
        .is_err());
}

#[test]
fn explicit_adoption_blocks_stale_and_fresh_managed_cleanup() {
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let mut managed = options("key");
    managed.source = SourceIdentity::File(directory.path().canonicalize().unwrap().join("source"));
    managed.owner = InstallOwner::ManagedLocalSource(owner("watcher-a"));
    managed.observation = Some(SourceObservation {
        token: "build-1".into(),
        artifact_sha256: digest(&wasm("semantic")),
        sidecar_sha256: Some("source-sidecar-not-effective-policy".into()),
    });
    let first = install(&store, "semantic", managed.clone());
    let mut explicit = managed.clone();
    explicit.owner = InstallOwner::Explicit;
    let adopted = install(&store, "semantic", explicit);
    assert!(adopted.change.as_ref().unwrap().owner_changed);
    for revision in [&receipt(&first).revision, &receipt(&adopted).revision] {
        assert!(store
            .remove(
                "semantic",
                revision,
                RemovalAuthority::Owned(owner("watcher-a"))
            )
            .is_err());
    }
    let expected = store
        .observe("semantic", &managed.storage_key, &managed.source)
        .unwrap();
    assert!(store
        .commit_install(prepare("semantic", managed), expected)
        .is_err());
}

#[test]
fn retirement_retains_previous_owner_observation_and_secrets() {
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let mut managed = options("key");
    managed.source = SourceIdentity::File(directory.path().canonicalize().unwrap().join("source"));
    managed.owner = InstallOwner::ManagedLocalSource(owner("watcher"));
    managed.observation = Some(SourceObservation {
        token: "capture-1".into(),
        artifact_sha256: digest(&wasm("semantic")),
        sidecar_sha256: None,
    });
    let first = install(&store, "semantic", managed);
    fs::write(
        directory.path().join("key.secrets.json"),
        b"do-not-read-or-delete",
    )
    .unwrap();
    let removed = store
        .remove(
            "semantic",
            &receipt(&first).revision,
            RemovalAuthority::Owned(owner("watcher")),
        )
        .unwrap();
    let StoredEntry::Retired(retired) = &removed.entry else {
        panic!("not retired")
    };
    assert_eq!(retired.previous, *receipt(&first));
    assert_eq!(retired.reason, RemovalReason::SourceMissing);
    assert!(directory.path().join("key.secrets.json").exists());
    assert_eq!(
        store.snapshot_if_changed(None).unwrap().unwrap().entries,
        [removed.entry]
    );
}

#[test]
fn independent_managers_cas_and_checked_admission() {
    let directory = directory();
    let left = ComponentStore::open(directory.path()).unwrap();
    let right = ComponentStore::open(directory.path()).unwrap();
    let first = install(&left, "semantic", options("key"));
    let stale = right
        .observe("semantic", &StorageKey::parse("key").unwrap(), &source())
        .unwrap();
    let updated = left
        .update_policy(
            "semantic",
            &receipt(&first).revision,
            policy("changed", PolicyProvenance::ExplicitAttachment),
        )
        .unwrap();
    assert!(right
        .commit_install(prepare("semantic", options("key")), stale)
        .is_err());
    assert!(right.checked_read(&first.cursor, None).is_err());
    let scope = right
        .checked_read(
            &updated.cursor,
            Some(("semantic", &receipt(&updated).revision)),
        )
        .unwrap();
    drop(scope);
    assert_eq!(right.read("semantic").unwrap().receipt, *receipt(&updated));
}

#[test]
fn two_writers_have_one_winner_without_mixed_policy() {
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let first = install(&store, "semantic", options("key"));
    let gate = Arc::new(Barrier::new(3));
    let workers: Vec<_> = ["left", "right"]
        .into_iter()
        .map(|name| {
            let store = ComponentStore::open(directory.path()).unwrap();
            let gate = gate.clone();
            let revision = receipt(&first).revision.clone();
            std::thread::spawn(move || {
                let policy = policy(name, PolicyProvenance::PermissionEdit);
                gate.wait();
                store.update_policy("semantic", &revision, policy)
            })
        })
        .collect();
    gate.wait();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let current = store.read("semantic").unwrap();
    assert_eq!(
        current.receipt.policy.sha256,
        current.policy.as_deref().map(digest)
    );
}

#[test]
fn handled_failure_before_decision_rolls_back_every_path() {
    for point in [
        "before-active",
        "after-active",
        "after-publish",
        "before-head",
    ] {
        let directory = directory();
        let store = ComponentStore::open(directory.path()).unwrap();
        let first = install(&store, "semantic", options("key"));
        *store.failpoint.lock().unwrap() = Some((point, false));
        let result = store.update_policy(
            "semantic",
            &receipt(&first).revision,
            policy("new", PolicyProvenance::PermissionEdit),
        );
        assert!(result.is_err(), "{point}");
        assert_eq!(
            store.read("semantic").unwrap().receipt,
            *receipt(&first),
            "{point}"
        );
        assert_eq!(
            store.snapshot_if_changed(None).unwrap().unwrap().cursor,
            first.cursor
        );
        assert!(!directory.path().join(journal::ACTIVE).exists());
    }
}

#[test]
fn failure_after_decision_is_explicit_even_when_recovery_finishes() {
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let first = install(&store, "semantic", options("key"));
    *store.failpoint.lock().unwrap() = Some(("after-head", false));
    let result = store.update_policy(
        "semantic",
        &receipt(&first).revision,
        policy("new", PolicyProvenance::PermissionEdit),
    );
    assert!(matches!(result, Err(StoreError::RecoveryRequired { .. })));
    let current = store.read("semantic").unwrap();
    assert_ne!(current.receipt.revision, receipt(&first).revision);
    assert_eq!(
        current.policy.unwrap(),
        policy("new", PolicyProvenance::PermissionEdit)
            .bytes
            .unwrap()
    );
}

#[test]
fn cancellation_preserves_active_images_for_reader_recovery() {
    for (point, committed) in [
        ("after-active", false),
        ("after-publish", false),
        ("before-head", false),
        ("after-head", true),
    ] {
        let directory = directory();
        let store = ComponentStore::open(directory.path()).unwrap();
        let first = install(&store, "semantic", options("key"));
        *store.failpoint.lock().unwrap() = Some((point, true));
        let result = std::panic::catch_unwind(|| {
            store.update_policy(
                "semantic",
                &receipt(&first).revision,
                policy("new", PolicyProvenance::PermissionEdit),
            )
        });
        assert!(result.is_err());
        assert!(directory.path().join(journal::ACTIVE).exists());
        let current = store.read("semantic").unwrap();
        assert_eq!(
            current.receipt.revision != receipt(&first).revision,
            committed
        );
        assert!(!directory.path().join(journal::ACTIVE).exists());
        assert_eq!(
            fs::read_dir(directory.path().join(TRANSACTIONS))
                .unwrap()
                .count(),
            0
        );
    }
}

#[test]
fn crash_child() {
    let Some(root) = std::env::var_os("WASSETTE_STORE_CHILD_ROOT") else {
        return;
    };
    let store = ComponentStore::open(root).unwrap();
    let snapshot = store.read("semantic").unwrap();
    store
        .update_policy(
            "semantic",
            &snapshot.receipt.revision,
            policy("subprocess", PolicyProvenance::PermissionEdit),
        )
        .unwrap();
    panic!("child did not exit at failpoint");
}

#[test]
fn subprocess_death_before_and_after_head_recovers_durably() {
    for (point, committed) in [("after-publish", false), ("after-head", true)] {
        let directory = directory();
        let store = ComponentStore::open(directory.path()).unwrap();
        let first = install(&store, "semantic", options("key"));
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "store::tests::crash_child", "--nocapture"])
            .env("WASSETTE_STORE_CHILD_ROOT", directory.path())
            .env("WASSETTE_STORE_CRASH_POINT", point)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(91));
        let reopened = ComponentStore::open(directory.path()).unwrap();
        let current = reopened.read("semantic").unwrap();
        assert_eq!(
            current.receipt.revision != receipt(&first).revision,
            committed
        );
        assert_eq!(
            ComponentStore::open(directory.path())
                .unwrap()
                .read("semantic")
                .unwrap()
                .receipt,
            current.receipt
        );
    }
}

#[test]
fn native_cache_is_revision_bound_and_never_removes_wasm() {
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let first = install(&store, "semantic", options("key"));
    let cache = || PreparedCache {
        artifact_sha256: receipt(&first).artifact_sha256.clone(),
        engine: "engine-1".into(),
        schema: "schema-1".into(),
        metadata: serde_json::json!({"tools": []}),
        native: b"test trusted serialization".to_vec(),
    };
    store
        .publish_cache("semantic", &receipt(&first).revision, cache())
        .unwrap();
    assert!(store
        .snapshot_if_changed(Some(&first.cursor))
        .unwrap()
        .is_none());
    assert!(store
        .read_cache("semantic", &receipt(&first).revision, "other", "schema-1")
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .read_cache(
                "semantic",
                &receipt(&first).revision,
                "engine-1",
                "schema-1"
            )
            .unwrap()
            .unwrap()
            .native,
        cache().native
    );
    let changed = store
        .update_policy(
            "semantic",
            &receipt(&first).revision,
            policy("new", PolicyProvenance::PermissionEdit),
        )
        .unwrap();
    assert!(store
        .publish_cache("semantic", &receipt(&first).revision, cache())
        .is_err());
    assert!(directory.path().join("key.wasm").exists());
    assert!(!directory.path().join("key.cwasm").exists());
    assert!(store
        .read_cache(
            "semantic",
            &receipt(&changed).revision,
            "engine-1",
            "schema-1"
        )
        .unwrap()
        .is_none());
    store
        .remove(
            "semantic",
            &receipt(&changed).revision,
            RemovalAuthority::Explicit,
        )
        .unwrap();
    assert!(store
        .publish_cache("semantic", &receipt(&changed).revision, cache())
        .is_err());
    assert!(!directory.path().join("key.wasm").exists());
}

#[test]
fn protected_legacy_inventory_never_infers_identity_or_source() {
    let directory = directory();
    fs::write(directory.path().join("named.wasm"), wasm("embedded")).unwrap();
    fs::write(
        directory.path().join("unnamed.wasm"),
        wat::parse_str("(component)").unwrap(),
    )
    .unwrap();
    fs::write(directory.path().join("orphan.policy.yaml"), b"[").unwrap();
    let store = ComponentStore::open(directory.path()).unwrap();
    let snapshot = store.snapshot_if_changed(None).unwrap().unwrap();
    assert_eq!(snapshot.protected.len(), 3);
    assert!(snapshot.entries.is_empty());
    let named = snapshot
        .protected
        .iter()
        .find(|entry| entry.physical_key == "named")
        .unwrap();
    assert_eq!(named.component_id.as_ref().unwrap().as_str(), "embedded");
    let unnamed = snapshot
        .protected
        .iter()
        .find(|entry| entry.physical_key == "unnamed")
        .unwrap();
    assert!(unnamed.component_id.is_none());
    assert!(unnamed
        .diagnostic
        .as_ref()
        .unwrap()
        .contains("missing root"));
    for (name, key) in [
        ("embedded", "new"),
        ("new", "_named_"),
        ("new", "unnamed"),
        ("new", "orphan"),
    ] {
        assert!(store
            .observe(name, &StorageKey::parse(key).unwrap(), &source())
            .is_err());
    }
    assert!(!directory.path().join("named.install.json").exists());
}

#[test]
fn operator_edits_and_corrupt_receipts_fail_closed() {
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let first = install(&store, "semantic", options("key"));
    fs::write(directory.path().join("key.policy.yaml"), b"[").unwrap();
    assert!(matches!(
        store.read("semantic"),
        Err(StoreError::Integrity(_))
    ));
    assert!(store
        .remove(
            "semantic",
            &receipt(&first).revision,
            RemovalAuthority::Explicit
        )
        .is_err());
    assert!(directory.path().join("key.wasm").exists());
    fs::remove_file(directory.path().join("key.policy.yaml")).unwrap();
    let path = directory.path().join("key.install.json");
    let original = fs::read(&path).unwrap();
    let mut record: serde_json::Value = serde_json::from_slice(&original).unwrap();
    record["Installed"]["schema"] = 999.into();
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(store.snapshot_if_changed(None).is_err());
}

#[test]
fn lock_file_is_permanent_and_epoch_survives_reopen() {
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let first = store.snapshot_if_changed(None).unwrap().unwrap();
    let lock_metadata = fs::metadata(directory.path().join(".store.lock")).unwrap();
    install(&store, "semantic", options("key"));
    let reopened = ComponentStore::open(directory.path()).unwrap();
    let second = reopened.snapshot_if_changed(None).unwrap().unwrap();
    assert_eq!(first.cursor.epoch, second.cursor.epoch);
    assert!(second.cursor.sequence > first.cursor.sequence);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            lock_metadata.ino(),
            fs::metadata(directory.path().join(".store.lock"))
                .unwrap()
                .ino()
        );
    }
}

fn live_images(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut images = BTreeMap::new();
    for name in SUFFIXES
        .iter()
        .map(|suffix| format!("key{suffix}"))
        .chain([journal::HEAD.to_owned()])
    {
        let path = root.join(&name);
        if path.exists() {
            images.insert(name, fs::read(path).unwrap());
        }
    }
    images
}

#[test]
fn every_partial_replacement_and_removal_restores_the_complete_bundle() {
    for point in [
        "after-file-0",
        "after-file-1",
        "after-file-2",
        "after-file-3",
        "after-file-4",
        "after-file-5",
    ] {
        for remove in [false, true] {
            let directory = directory();
            let store = ComponentStore::open(directory.path()).unwrap();
            let mut original = options("key");
            original.policy = policy("original", PolicyProvenance::Bundled);
            let installed = install(&store, "semantic", original);
            store
                .publish_cache(
                    "semantic",
                    &receipt(&installed).revision,
                    PreparedCache {
                        artifact_sha256: receipt(&installed).artifact_sha256.clone(),
                        engine: "engine".into(),
                        schema: "schema".into(),
                        metadata: serde_json::json!({"original": true}),
                        native: b"old-cache".to_vec(),
                    },
                )
                .unwrap();
            let before = live_images(directory.path());
            *store.failpoint.lock().unwrap() = Some((point, false));
            let result = if remove {
                store.remove(
                    "semantic",
                    &receipt(&installed).revision,
                    RemovalAuthority::Explicit,
                )
            } else {
                let bytes = wat::parse_str(
                    r#"(component $semantic
                       (instance $empty)
                       (export "different" (instance $empty)))"#,
                )
                .unwrap();
                let mut replacement = options("key");
                replacement.policy = policy("replacement", PolicyProvenance::Bundled);
                let expected = store
                    .observe("semantic", &replacement.storage_key, &replacement.source)
                    .unwrap();
                let prepared = PreparedInstall::prepare(bytes, replacement, validator).unwrap();
                assert_ne!(
                    prepared.artifact_sha256(),
                    receipt(&installed).artifact_sha256
                );
                store.commit_install(prepared, expected)
            };
            assert!(result.is_err(), "{point}, remove={remove}");
            assert_eq!(
                live_images(directory.path()),
                before,
                "{point}, remove={remove}"
            );
            assert_eq!(
                store.read("semantic").unwrap().receipt,
                *receipt(&installed)
            );
        }
    }
}

#[test]
fn interrupted_new_install_rolls_back_to_genuine_absence() {
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let before = store.snapshot_if_changed(None).unwrap().unwrap();
    let expected = store
        .observe("semantic", &StorageKey::parse("key").unwrap(), &source())
        .unwrap();
    *store.failpoint.lock().unwrap() = Some(("before-head", true));
    assert!(std::panic::catch_unwind(|| {
        store.commit_install(prepare("semantic", options("key")), expected)
    })
    .is_err());
    let recovered = store.snapshot_if_changed(None).unwrap().unwrap();
    assert_eq!(before.cursor, recovered.cursor);
    assert!(recovered.entries.is_empty());
    assert!(recovered.protected.is_empty());
    assert!(!directory.path().join("key.wasm").exists());
    install(&store, "semantic", options("key"));
}

#[test]
fn cache_recovery_uses_head_operation_even_without_cursor_change() {
    for (point, expected_native) in [
        ("before-head", b"old".as_slice()),
        ("after-head", b"new".as_slice()),
    ] {
        let directory = directory();
        let store = ComponentStore::open(directory.path()).unwrap();
        let installed = install(&store, "semantic", options("key"));
        let cache = |bytes: &[u8]| PreparedCache {
            artifact_sha256: receipt(&installed).artifact_sha256.clone(),
            engine: "engine".into(),
            schema: "schema".into(),
            metadata: serde_json::json!({}),
            native: bytes.to_vec(),
        };
        store
            .publish_cache("semantic", &receipt(&installed).revision, cache(b"old"))
            .unwrap();
        *store.failpoint.lock().unwrap() = Some((point, true));
        assert!(std::panic::catch_unwind(|| {
            store.publish_cache("semantic", &receipt(&installed).revision, cache(b"new"))
        })
        .is_err());
        let cached = store
            .read_cache(
                "semantic",
                &receipt(&installed).revision,
                "engine",
                "schema",
            )
            .unwrap()
            .unwrap();
        assert_eq!(cached.native, expected_native);
        assert!(store
            .snapshot_if_changed(Some(&installed.cursor))
            .unwrap()
            .is_none());
        assert!(directory.path().join("key.wasm").exists());
    }
}

#[test]
fn corrupt_recovery_image_is_explicit_and_not_cleaned_up() {
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let installed = install(&store, "semantic", options("key"));
    *store.failpoint.lock().unwrap() = Some(("after-file-1", true));
    assert!(std::panic::catch_unwind(|| {
        store.remove(
            "semantic",
            &receipt(&installed).revision,
            RemovalAuthority::Explicit,
        )
    })
    .is_err());
    let operation: String = journal::read_json(&directory.path().join(journal::ACTIVE)).unwrap();
    let stage = directory.path().join(TRANSACTIONS).join(operation);
    fs::write(stage.join("old-1"), b"damaged recovery artifact").unwrap();
    assert!(matches!(
        store.read("semantic"),
        Err(StoreError::RecoveryRequired { .. })
    ));
    assert!(stage.exists());
    assert!(directory.path().join(journal::ACTIVE).exists());
}

#[test]
fn immutable_handles_keep_old_artifact_and_policy_after_replacement() {
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let installed = install(&store, "semantic", options("key"));
    let mut pinned = store.capture(Some("semantic"), None).unwrap();
    let updated = store
        .update_policy(
            "semantic",
            &receipt(&installed).revision,
            policy("new", PolicyProvenance::PermissionEdit),
        )
        .unwrap();
    let (wasm, policy) = pinned.verify(receipt(&installed)).unwrap();
    assert_eq!(wasm, self::wasm("semantic"));
    assert!(policy.is_none());
    assert_ne!(receipt(&installed).revision, receipt(&updated).revision);
}

#[test]
fn source_sidecar_observation_does_not_replace_effective_policy() {
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let mut original = options("key");
    original.policy = policy("operator", PolicyProvenance::ExplicitAttachment);
    let first = install(&store, "semantic", original.clone());
    original.observation = Some(SourceObservation {
        token: "sidecar-changed".into(),
        artifact_sha256: receipt(&first).artifact_sha256.clone(),
        sidecar_sha256: Some(digest(b"incoming sidecar")),
    });
    let second = install(&store, "semantic", original);
    assert_ne!(receipt(&first).revision, receipt(&second).revision);
    assert_eq!(receipt(&first).policy, receipt(&second).policy);
    assert!(second.change.as_ref().unwrap().provenance_changed);
    assert!(!second.change.as_ref().unwrap().policy_changed);
}

#[test]
fn origin_and_source_urls_never_persist_raw_credentials_or_queries() {
    for location in [
        "https://user:secret@example.com/component.wasm",
        "https://example.com/component.wasm?token=secret",
        "https://example.com/component.wasm#secret",
        "oci://user:secret@example.com/team/component:latest",
    ] {
        let mut input = options("key");
        input.origin.location = location.into();
        assert!(PreparedInstall::prepare(wasm("semantic"), input, validator).is_err());
        let mut input = options("key");
        input.origin.immutable_uri = Some(location.into());
        assert!(PreparedInstall::prepare(wasm("semantic"), input, validator).is_err());
    }
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let identity = |token: &str| SourceIdentity::Https {
        location: "https://example.com/component.wasm".into(),
        request_sha256: digest(
            format!("https://example.com/component.wasm?token={token}").as_bytes(),
        ),
    };
    let mut input = options("key");
    input.source = identity("secret-a");
    input.origin.location = "https://example.com/component.wasm".into();
    install(&store, "semantic", input);
    let record = fs::read_to_string(directory.path().join("key.install.json")).unwrap();
    assert!(!record.contains("secret-a"));
    assert!(!record.contains("token="));
    assert!(store
        .observe(
            "semantic",
            &StorageKey::parse("key").unwrap(),
            &identity("secret-b")
        )
        .is_err());
    let restored: SourceIdentity =
        serde_json::from_str(&serde_json::to_string(&identity("secret-a")).unwrap()).unwrap();
    assert_eq!(restored, identity("secret-a"));
}

#[test]
fn default_absence_is_distinct_from_bundle_and_explicit_clearing() {
    assert!(!PolicyProvenance::Default.protected());
    assert!(!PolicyProvenance::Bundled.protected());
    assert!(PolicyProvenance::ExplicitAttachment.protected());
    assert!(PolicyProvenance::PermissionEdit.protected());
    assert!(PreparedPolicy::parse(
        b"version: '1.0'\npermissions: {}".to_vec(),
        PolicyProvenance::Default,
    )
    .is_err());
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let initial = install(&store, "semantic", options("key"));
    assert_eq!(
        receipt(&initial).policy.provenance,
        PolicyProvenance::Default
    );
    let mut bundled = options("key");
    bundled.policy = policy("bundle", PolicyProvenance::Bundled);
    let installed = install(&store, "semantic", bundled);
    assert_eq!(
        receipt(&installed).policy.provenance,
        PolicyProvenance::Bundled
    );
    let cleared = store
        .update_policy(
            "semantic",
            &receipt(&installed).revision,
            PreparedPolicy::absent(PolicyProvenance::ExplicitAttachment),
        )
        .unwrap();
    assert_eq!(
        receipt(&cleared).policy.provenance,
        PolicyProvenance::ExplicitAttachment
    );
    let expected = store
        .observe("semantic", &StorageKey::parse("key").unwrap(), &source())
        .unwrap();
    assert!(store
        .commit_install(prepare("semantic", options("key")), expected)
        .is_err());
}

#[test]
fn structured_managed_ownership_has_no_delimiter_aliases() {
    let first = ManagedLocalSource::new("a/b", "c").unwrap();
    let second = ManagedLocalSource::new("a", "b/c").unwrap();
    assert_ne!(first, second);
    assert_ne!(
        serde_json::to_vec(&first).unwrap(),
        serde_json::to_vec(&second).unwrap()
    );
    for path in ["", ".", "../source", "/source", "a/./b", "a//b", "a/"] {
        assert!(ManagedLocalSource::new("root", path).is_err(), "{path}");
    }
    assert!(ManagedLocalSource::new("", "source").is_err());
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let mut input = options("key");
    input.source = SourceIdentity::File(directory.path().canonicalize().unwrap().join("source"));
    input.owner = InstallOwner::ManagedLocalSource(first.clone());
    let installed = install(&store, "semantic", input);
    assert!(store
        .remove(
            "semantic",
            &receipt(&installed).revision,
            RemovalAuthority::Owned(second)
        )
        .is_err());
    let retired = store
        .remove(
            "semantic",
            &receipt(&installed).revision,
            RemovalAuthority::Owned(first.clone()),
        )
        .unwrap();
    assert_eq!(
        retired.entry.binding().owner,
        InstallOwner::ManagedLocalSource(first)
    );
}

#[test]
fn exact_policy_attachment_metadata_is_bound_to_receipt_and_revision() {
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let first_metadata = PolicyMetadata {
        source_uri: "https://example.com/policy.yaml".into(),
        attached_at: Some(100),
    };
    let mut input = options("key");
    input.policy = policy("attached", PolicyProvenance::ExplicitAttachment)
        .with_metadata(Some(first_metadata.clone()))
        .unwrap();
    let initial = install(&store, "semantic", input);
    let snapshot = store.read("semantic").unwrap();
    assert_eq!(
        snapshot.receipt.policy.metadata,
        Some(first_metadata.clone())
    );
    let bytes = fs::read(directory.path().join("key.policy.meta.json")).unwrap();
    assert_eq!(digest(&bytes), snapshot.receipt.policy.metadata_sha256);
    assert_eq!(
        serde_json::from_slice::<PolicyMetadata>(&bytes).unwrap(),
        first_metadata
    );
    let next_metadata = PolicyMetadata {
        source_uri: "file:///policies/operator.yaml".into(),
        attached_at: Some(200),
    };
    let updated_policy = PreparedPolicy::parse(
        snapshot.policy.unwrap(),
        PolicyProvenance::ExplicitAttachment,
    )
    .unwrap()
    .with_metadata(Some(next_metadata.clone()))
    .unwrap();
    let updated = store
        .update_policy("semantic", &snapshot.receipt.revision, updated_policy)
        .unwrap();
    assert_ne!(receipt(&initial).revision, receipt(&updated).revision);
    assert_eq!(
        receipt(&initial).policy.sha256,
        receipt(&updated).policy.sha256
    );
    assert_ne!(
        receipt(&initial).policy.metadata_sha256,
        receipt(&updated).policy.metadata_sha256
    );
    assert_eq!(
        store.read("semantic").unwrap().receipt.policy.metadata,
        Some(next_metadata)
    );
    assert!(updated.change.as_ref().unwrap().policy_changed);
    let mut tampered = fs::read(directory.path().join("key.policy.meta.json")).unwrap();
    tampered.push(b' ');
    fs::write(directory.path().join("key.policy.meta.json"), tampered).unwrap();
    assert!(matches!(
        store.read("semantic"),
        Err(StoreError::Integrity(_))
    ));
}

#[test]
fn policy_attachment_evidence_rejects_credential_bearing_urls() {
    for source_uri in [
        "https://user:secret@example.com/policy.yaml",
        "https://example.com/policy.yaml?token=secret",
    ] {
        let result = policy("attached", PolicyProvenance::ExplicitAttachment).with_metadata(Some(
            PolicyMetadata {
                source_uri: source_uri.into(),
                attached_at: None,
            },
        ));
        assert!(result.is_err());
    }
}

#[test]
fn checked_scope_releases_lock_on_drop_and_rejects_stale_entries() {
    let directory = directory();
    let store = ComponentStore::open(directory.path()).unwrap();
    let installed = install(&store, "semantic", options("key"));
    let scope = store
        .checked_read(
            &installed.cursor,
            Some(("semantic", &receipt(&installed).revision)),
        )
        .unwrap();
    let competing_writer = journal::open_lock(directory.path()).unwrap();
    assert!(competing_writer.try_lock().is_err());
    drop(scope);
    competing_writer.try_lock().unwrap();
    drop(competing_writer);

    let changed = store
        .update_policy(
            "semantic",
            &receipt(&installed).revision,
            policy("edited", PolicyProvenance::PermissionEdit),
        )
        .unwrap();
    assert!(store
        .checked_read(
            &changed.cursor,
            Some(("semantic", &receipt(&installed).revision))
        )
        .is_err());
    let writer_after_error = journal::open_lock(directory.path()).unwrap();
    writer_after_error.try_lock().unwrap();
}

fn generated_options(lineage: &str) -> InstallOptions {
    let mut input = options("generated_private_key");
    input.source = SourceIdentity::Generated { id: lineage.into() };
    input.origin = OriginEvidence {
        location: format!("generated://{lineage}"),
        requested_version: None,
        selected_version: None,
        manifest_digest: None,
        immutable_uri: None,
        generation: Some(GenerationEvidence {
            source_sha256: "11".repeat(32),
            wit_sha256: "22".repeat(32),
            wit_dependencies_sha256: "55".repeat(32),
            builder_initrd_sha256: "33".repeat(32),
            builder_helper_sha256: "66".repeat(32),
            builder_manifest_digest: None,
            profile: "rust-std-v1".into(),
            profile_sha256: "77".repeat(32),
            compiler: "rustc 1.98.1".into(),
            bindgen: "0.62.0".into(),
            binding_runtime: "inline-v1".into(),
            vm_runtime: "hyperlight-unikraft-0.17.0".into(),
            world: "tool".into(),
            target: "wasm32-wasip2".into(),
            host_platform: "aarch64-macos".into(),
        }),
    };
    input
}

#[test]
fn generated_receipt_roundtrips_and_preserves_noop_retirement_and_cas() {
    let root = directory();
    let store = ComponentStore::open(root.path()).unwrap();
    let input = generated_options(&"ab".repeat(16));
    let first = install(&store, "actual:generated/name", input.clone());
    assert_eq!(receipt(&first).schema, 2);
    assert_eq!(
        store.read("actual:generated/name").unwrap().receipt,
        *receipt(&first)
    );
    let unchanged = install(&store, "actual:generated/name", input.clone());
    assert!(unchanged.change.is_none());
    assert_eq!(first.cursor, unchanged.cursor);

    let expected = store
        .observe("actual:generated/name", &input.storage_key, &input.source)
        .unwrap();
    let changed = store
        .update_policy(
            "actual:generated/name",
            &receipt(&first).revision,
            PreparedPolicy::absent(PolicyProvenance::PermissionEdit),
        )
        .unwrap();
    assert!(matches!(
        store.commit_install(prepare("actual:generated/name", input.clone()), expected),
        Err(StoreError::Conflict(_))
    ));
    let retired = store
        .remove(
            "actual:generated/name",
            changed.entry.revision(),
            RemovalAuthority::Explicit,
        )
        .unwrap();
    let mut reinstall = input;
    reinstall.policy = PreparedPolicy::absent(PolicyProvenance::PermissionEdit);
    let installed = install(&store, "actual:generated/name", reinstall);
    assert_ne!(installed.entry.revision(), retired.entry.revision());
    assert_eq!(
        receipt(&first).secret_binding().unwrap(),
        receipt(&installed).secret_binding().unwrap()
    );
    assert!(store
        .observe(
            "actual:generated/name",
            &StorageKey::parse("another_key").unwrap(),
            &SourceIdentity::Generated {
                id: "cd".repeat(16)
            },
        )
        .is_err());
}

#[test]
fn generated_evidence_is_not_a_package_or_identity_override() {
    let good = generated_options(&"ab".repeat(16));
    let mut wrong_location = good.clone();
    wrong_location.origin.location = "generated://someone-else".into();
    let mut package = good.clone();
    package.origin.manifest_digest = Some(format!("sha256:{}", "44".repeat(32)));
    let mut file = good.clone();
    file.source = SourceIdentity::File(std::path::absolute("unrelated.wasm").unwrap());
    let mut absent = good.clone();
    absent.origin.generation = None;
    let mut invalid_id = good.clone();
    invalid_id.source = SourceIdentity::Generated {
        id: "caller-chosen-label".into(),
    };
    let mut huge = good;
    huge.origin.generation.as_mut().unwrap().world = "x".repeat(513);
    for input in [wrong_location, package, file, absent, invalid_id, huge] {
        assert!(PreparedInstall::prepare(wasm("generated"), input, validator).is_err());
    }
}

#[test]
fn generation_receipt_schema_and_binding_tampering_fail_closed() {
    let root = directory();
    let store = ComponentStore::open(root.path()).unwrap();
    let installed = install(&store, "generated", generated_options(&"ab".repeat(16)));
    let record_path = root.path().join("generated_private_key.install.json");
    let bytes = std::fs::read(&record_path).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    // Use the actual stored enum representation instead of a parallel receipt format.
    let current = value.get_mut("Installed").unwrap();
    current["schema"] = serde_json::json!(1);
    std::fs::write(&record_path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(store.read("generated").is_err());
    std::fs::write(&record_path, &bytes).unwrap();
    assert_eq!(
        store.read("generated").unwrap().receipt,
        *receipt(&installed)
    );
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let current = value.get_mut("Installed").unwrap();
    current["origin"]
        .as_object_mut()
        .unwrap()
        .remove("generation");
    std::fs::write(&record_path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(store.read("generated").is_err());
}

#[test]
fn existing_source_serialization_and_secret_binding_are_unchanged() {
    let old_source = source();
    let encoding = serde_json::to_string(&old_source).unwrap();
    assert_eq!(encoding, r#"{"OciRepository":"ghcr.io/example/tool"}"#);
    let expected_binding = hex::encode(sha2::Sha256::digest(encoding.as_bytes()));
    assert_eq!(
        crate::store_support::source_binding_key(&old_source).unwrap(),
        expected_binding
    );
    let origin = options("key").origin;
    let encoded = serde_json::to_value(&origin).unwrap();
    assert!(encoded.get("generation").is_none());
    assert_eq!(
        serde_json::from_value::<OriginEvidence>(encoded).unwrap(),
        origin
    );

    #[derive(serde::Deserialize)]
    enum LegacySource {
        OciRepository(String),
    }
    let old: LegacySource = serde_json::from_str(&encoding).unwrap();
    let LegacySource::OciRepository(repository) = old;
    assert_eq!(repository, "ghcr.io/example/tool");
    assert!(serde_json::from_value::<LegacySource>(
        serde_json::to_value(SourceIdentity::Generated {
            id: "ab".repeat(16)
        },)
        .unwrap()
    )
    .is_err());
}
