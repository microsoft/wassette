// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::time::Duration;

use tempfile::TempDir;

use super::*;

const ID: &str = "declared-replacement-name";
const KEY: &str = "safe-replacement";
const WAIT: Duration = Duration::from_secs(30);
const POLICY: &str = r#"
version: "1.0"
permissions:
  network:
    allow:
      - host: "retained.example.invalid"
  environment:
    allow:
      - key: "REPLACEMENT_CONFIG"
"#;

fn storage_key() -> StorageKey {
    StorageKey::parse(KEY).expect("portable fixture key")
}

fn test_dir() -> Result<TempDir> {
    Ok(tempfile::Builder::new()
        .prefix(".safe-replacement-")
        .tempdir_in(std::env::current_dir()?)?)
}

fn component(value: u32) -> Result<Vec<u8>> {
    Ok(wat::parse_str(format!(
        r#"(component $declared-replacement-name
            (core module $m
                (func (export "run") (result i32) i32.const {value}))
            (core instance $i (instantiate $m))
            (func (export "run") (result u32)
                (canon lift (core func $i "run")))
        )"#
    ))?)
}

fn unsupported_import_component() -> Result<Vec<u8>> {
    Ok(wat::parse_str(
        r#"(component $declared-replacement-name
            (import "replacement:missing/host@1.0.0"
                (instance (export "value" (func (result u32)))))
            (core module $m
                (func (export "run") (result i32) i32.const 2))
            (core instance $i (instantiate $m))
            (func (export "run") (result u32)
                (canon lift (core func $i "run")))
        )"#,
    )?)
}

fn file_uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

struct Fixture {
    manager: LifecycleManager,
    source: TempDir,
    _components: TempDir,
    _secrets: TempDir,
}

impl Fixture {
    async fn new() -> Result<Self> {
        Self::with_http_client(None).await
    }

    async fn with_http_client(client: Option<reqwest::Client>) -> Result<Self> {
        let components = test_dir()?;
        let secrets = test_dir()?;
        let source = test_dir()?;
        let mut builder = LifecycleManager::builder(components.path())
            .with_secrets_dir(secrets.path())
            .with_environment_var("REPLACEMENT_CONFIG", "configured-test-value")
            .with_eager_loading(false);
        if let Some(client) = client {
            builder = builder.with_http_client(client);
        }
        Ok(Self {
            manager: builder.build().await?,
            source,
            _components: components,
            _secrets: secrets,
        })
    }

    async fn source(&self, bytes: &[u8]) -> Result<PathBuf> {
        let path = self.source.path().join(format!("{KEY}.wasm"));
        tokio::fs::write(&path, bytes).await?;
        Ok(path)
    }

    async fn load(&self, value: u32) -> Result<ComponentLoadOutcome> {
        let source = self.source(&component(value)?).await?;
        self.manager.load_component(&file_uri(&source)).await
    }

    async fn load_downloaded(
        &self,
        value: u32,
        policy: Option<&[u8]>,
    ) -> Result<ComponentLoadOutcome> {
        self.manager
            .load_component_resource(downloaded(&component(value)?, policy).await?)
            .await
    }

    async fn attach_policy(&self) -> Result<()> {
        let path = self.source.path().join("explicit-policy.yaml");
        tokio::fs::write(&path, POLICY).await?;
        self.manager.attach_policy(ID, &file_uri(&path)).await
    }
}

async fn downloaded(bytes: &[u8], policy: Option<&[u8]>) -> Result<DownloadedResource> {
    let directory = test_dir()?;
    let path = directory.path().join(format!("{KEY}.wasm"));
    tokio::fs::write(&path, bytes).await?;
    if let Some(policy) = policy {
        tokio::fs::write(directory.path().join(format!("{KEY}.policy.yaml")), policy).await?;
    }
    Ok(DownloadedResource::Temp((directory, path)))
}

async fn assert_call(manager: &LifecycleManager, value: u32) -> Result<()> {
    let result = manager.execute_component_call(ID, "run", "{}").await?;
    assert_eq!(
        serde_json::from_str::<Value>(&result)?,
        serde_json::json!({ "result": value })
    );
    Ok(())
}

struct InstalledState {
    files: Vec<(PathBuf, Option<Vec<u8>>)>,
    schema: Value,
    instance: ComponentInstance,
    receipt: store::InstallReceipt,
}

impl InstalledState {
    async fn capture(manager: &LifecycleManager) -> Result<Self> {
        let mut files = Vec::new();
        for path in [
            manager.component_path(KEY),
            manager.storage.policy_path(&storage_key()),
            manager.storage.policy_metadata_path(&storage_key()),
            manager.storage.metadata_path(&storage_key()),
            manager.component_precompiled_path(KEY),
            manager.component_root().join(format!("{KEY}.install.json")),
        ] {
            files.push((path.clone(), loader::read_optional_file(&path).await?));
        }
        Ok(Self {
            files,
            schema: manager
                .get_component_schema(ID)
                .await
                .context("installed component has no schema")?,
            instance: manager
                .get_component(ID)
                .await
                .context("missing runtime instance")?,
            receipt: manager.store_snapshot(ID).await?.receipt,
        })
    }

    async fn assert_pinned_unchanged(&self, manager: &LifecycleManager) -> Result<()> {
        for (path, expected) in &self.files {
            assert!(
                loader::read_optional_file(path).await? == *expected,
                "replacement changed {}",
                path.display()
            );
        }
        let current = manager
            .get_component(ID)
            .await
            .context("missing pinned instance")?;
        assert!(Arc::ptr_eq(&self.instance.component, &current.component));
        assert!(Arc::ptr_eq(
            &self.instance.policy_template,
            &current.policy_template
        ));
        assert_eq!(current.revision.as_ref(), Some(&self.receipt.revision));
        assert_eq!(current.effective_policy, self.instance.effective_policy);
        Ok(())
    }

    async fn assert_unchanged(&self, manager: &LifecycleManager) -> Result<()> {
        self.assert_pinned_unchanged(manager).await?;
        assert_eq!(manager.store_snapshot(ID).await?.receipt, self.receipt);
        assert_eq!(
            manager.get_component_schema(ID).await.as_ref(),
            Some(&self.schema)
        );
        assert_eq!(manager.get_component_id_for_tool("run").await?, ID);
        assert_call(manager, 1).await
    }
}

#[tokio::test]
async fn safe_replacement_invalid_binary_preserves_installed_state() -> Result<()> {
    let fixture = Fixture::new().await?;
    fixture.load(1).await?;
    fixture.attach_policy().await?;
    let before = InstalledState::capture(&fixture.manager).await?;
    let source = fixture.source(b"not a WebAssembly component").await?;

    assert!(fixture
        .manager
        .load_component(&file_uri(&source))
        .await
        .is_err());

    before.assert_unchanged(&fixture.manager).await
}

#[tokio::test]
async fn safe_replacement_unsupported_import_does_not_reuse_old_native_cache() -> Result<()> {
    let fixture = Fixture::new().await?;
    fixture.load(1).await?;
    fixture.attach_policy().await?;
    let before = InstalledState::capture(&fixture.manager).await?;
    let candidate = unsupported_import_component()?;
    assert_eq!(
        inspect_artifact(&candidate)?.shape,
        ArtifactShape::ToolCandidate
    );
    let compiled = Component::new(fixture.manager.runtime.as_ref(), &candidate)?;
    assert!(fixture.manager.runtime.instantiate_pre(&compiled).is_err());
    let source = fixture.source(&candidate).await?;

    assert!(fixture
        .manager
        .load_component(&file_uri(&source))
        .await
        .is_err());

    before.assert_unchanged(&fixture.manager).await
}

#[tokio::test]
async fn safe_replacement_invalid_bundled_policy_preserves_installed_state() -> Result<()> {
    let overflowing_memory =
        b"version: '1.0'\npermissions:\n  resources:\n    memory: 18446744073709551615\n";
    // Legacy memory is accepted by the parser; converting MB to bytes must fail in preparation.
    policy::PolicyParser::parse_bytes(overflowing_memory)?;

    for policy in [
        b"version: [".as_slice(),
        b"\xff".as_slice(),
        overflowing_memory.as_slice(),
    ] {
        let fixture = Fixture::new().await?;
        fixture.load_downloaded(1, None).await?;
        let before = InstalledState::capture(&fixture.manager).await?;
        let resource = downloaded(&component(2)?, Some(policy)).await?;

        assert!(fixture
            .manager
            .load_component_resource(resource)
            .await
            .is_err());

        before.assert_unchanged(&fixture.manager).await?;
    }
    Ok(())
}

#[tokio::test]
async fn safe_replacement_unreadable_bundled_policy_preserves_installed_state() -> Result<()> {
    let fixture = Fixture::new().await?;
    fixture.load_downloaded(1, None).await?;
    fixture.attach_policy().await?;
    let before = InstalledState::capture(&fixture.manager).await?;
    let resource = downloaded(&component(2)?, None).await?;
    let policy_path = resource
        .as_ref()
        .with_file_name(format!("{KEY}.policy.yaml"));
    tokio::fs::create_dir(&policy_path).await?;
    tokio::fs::write(policy_path.join("blocker"), b"not a policy file").await?;

    assert!(fixture
        .manager
        .load_component_resource(resource)
        .await
        .is_err());

    before.assert_unchanged(&fixture.manager).await
}

#[cfg(unix)]
#[tokio::test]
async fn safe_replacement_dangling_bundled_policy_is_not_absence() -> Result<()> {
    let fixture = Fixture::new().await?;
    fixture.load_downloaded(1, None).await?;
    let before = InstalledState::capture(&fixture.manager).await?;
    let resource = downloaded(&component(2)?, None).await?;
    let policy_path = resource.as_ref().with_extension("policy.yaml");
    std::os::unix::fs::symlink(
        policy_path.with_file_name("missing-policy.yaml"),
        &policy_path,
    )?;
    assert!(fixture
        .manager
        .load_component_resource(resource)
        .await
        .is_err());
    before.assert_unchanged(&fixture.manager).await
}

#[cfg(unix)]
#[tokio::test]
async fn safe_replacement_dangling_acquired_policy_is_not_absence() -> Result<()> {
    let fixture = Fixture::new().await?;
    fixture.load(1).await?;
    let before = InstalledState::capture(&fixture.manager).await?;
    let source = fixture.source(&component(2)?).await?;
    let policy_path = source.with_extension("policy.yaml");
    std::os::unix::fs::symlink(
        policy_path.with_file_name("missing-policy.yaml"),
        &policy_path,
    )?;
    assert!(
        acquisition::acquire_component(&file_uri(&source), &fixture.manager.config, true)
            .await
            .is_err()
    );
    before.assert_unchanged(&fixture.manager).await
}

#[tokio::test]
async fn safe_replacement_malformed_or_unreadable_retained_policy_is_fatal() -> Result<()> {
    for directory in [false, true] {
        let fixture = Fixture::new().await?;
        fixture.load(1).await?;
        fixture.attach_policy().await?;
        let mut before = InstalledState::capture(&fixture.manager).await?;
        let policy_path = fixture.manager.storage.policy_path(&storage_key());
        before.files.retain(|(path, _)| path != &policy_path);
        let malformed = b"version: [";
        if directory {
            tokio::fs::remove_file(&policy_path).await?;
            tokio::fs::create_dir(&policy_path).await?;
            tokio::fs::write(policy_path.join("blocker"), malformed).await?;
        } else {
            tokio::fs::write(&policy_path, malformed).await?;
        }

        assert!(fixture.load(2).await.is_err());

        before.assert_pinned_unchanged(&fixture.manager).await?;
        assert!(fixture.manager.get_component_schema(ID).await.is_none());
        assert!(fixture
            .manager
            .execute_component_call(ID, "run", "{}")
            .await
            .is_err());
        if directory {
            assert!(policy_path.is_dir());
            assert_eq!(
                tokio::fs::read(policy_path.join("blocker")).await?,
                malformed
            );
        } else {
            assert_eq!(tokio::fs::read(policy_path).await?, malformed);
        }
    }
    Ok(())
}

#[tokio::test]
async fn safe_replacement_unbundled_replacement_retains_explicit_policy() -> Result<()> {
    for remote in [false, true] {
        let fixture = Fixture::new().await?;
        if remote {
            fixture.load_downloaded(1, None).await?;
        } else {
            fixture.load(1).await?;
        }
        fixture.attach_policy().await?;
        let policy_path = fixture.manager.storage.policy_path(&storage_key());
        let metadata_path = fixture.manager.storage.policy_metadata_path(&storage_key());
        let old_policy = tokio::fs::read(&policy_path).await?;
        let old_metadata = tokio::fs::read(&metadata_path).await?;
        let old_info = fixture.manager.get_policy_info(ID).await.unwrap();

        let outcome = if remote {
            fixture
                .manager
                .load_component_resource(downloaded(&component(2)?, None).await?)
                .await?
        } else {
            fixture.load(2).await?
        };

        assert_eq!(outcome.status, LoadResult::Replaced);
        assert_eq!(outcome.component_id, ID);
        assert_eq!(tokio::fs::read(policy_path).await?, old_policy);
        assert_eq!(tokio::fs::read(metadata_path).await?, old_metadata);
        assert_eq!(
            fixture
                .manager
                .get_policy_info(ID)
                .await
                .unwrap()
                .source_uri,
            old_info.source_uri
        );
        let template = fixture
            .manager
            .get_component(ID)
            .await
            .context("missing replacement instance")?
            .policy_template;
        assert!(template.allowed_hosts.contains("retained.example.invalid"));
        assert!(template.network_perms.allow_tcp);
        assert_call(&fixture.manager, 2).await?;
    }
    Ok(())
}

#[tokio::test]
async fn safe_replacement_installed_path_cannot_impersonate_original_source() -> Result<()> {
    let fixture = Fixture::new().await?;
    fixture.load(1).await?;
    fixture.attach_policy().await?;
    let installed = fixture.manager.component_path(KEY);
    let before = InstalledState::capture(&fixture.manager).await?;
    let error = fixture
        .manager
        .load_component(&file_uri(&installed))
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("reserved"), "{error:#}");
    before.assert_unchanged(&fixture.manager).await
}

#[tokio::test]
async fn safe_replacement_local_sibling_policy_is_not_adopted() -> Result<()> {
    let fixture = Fixture::new().await?;
    let sibling = fixture.source.path().join(format!("{KEY}.policy.yaml"));
    tokio::fs::write(&sibling, POLICY).await?;
    fixture.load(1).await?;
    assert!(fixture.manager.get_policy_info(ID).await.is_none());

    tokio::fs::write(&sibling, b"version: [").await?;
    fixture.load(2).await?;

    assert!(!fixture.manager.storage.policy_path(&storage_key()).exists());
    let snapshot = fixture.manager.store_snapshot(ID).await?;
    assert!(snapshot.policy.is_none());
    assert!(snapshot.receipt.policy.metadata.is_none());
    assert_eq!(
        snapshot.receipt.policy.provenance,
        store::PolicyProvenance::Default
    );
    assert!(fixture
        .manager
        .get_component(ID)
        .await
        .unwrap()
        .policy_template
        .allowed_hosts
        .is_empty());
    assert_eq!(tokio::fs::read(sibling).await?, b"version: [");
    assert_call(&fixture.manager, 2).await
}

#[tokio::test]
async fn safe_replacement_success_publishes_new_runtime_policy_and_native_cache() -> Result<()> {
    let fixture = Fixture::new().await?;
    let first = fixture.load_downloaded(1, None).await?;
    assert_eq!(first.status, LoadResult::New);
    assert_eq!(first.component_id, ID);
    assert_call(&fixture.manager, 1).await?;
    let old_native = tokio::fs::read(fixture.manager.component_precompiled_path(KEY)).await?;
    let replacement_policy = POLICY.replace("retained.example.invalid", "new.example.invalid");
    let candidate = component(2)?;

    let outcome = fixture
        .manager
        .load_component_resource(downloaded(&candidate, Some(replacement_policy.as_bytes())).await?)
        .await?;

    assert_eq!(outcome.status, LoadResult::Replaced);
    assert_eq!(outcome.component_id, ID);
    assert_eq!(outcome.tool_names, ["run"]);
    assert_eq!(fixture.manager.list_components().await, [ID]);
    assert_eq!(
        tokio::fs::read(fixture.manager.component_path(KEY)).await?,
        candidate
    );
    assert_ne!(
        tokio::fs::read(fixture.manager.component_precompiled_path(KEY)).await?,
        old_native
    );
    let metadata = fixture.manager.load_component_metadata(ID).await?.unwrap();
    let snapshot = fixture.manager.store_snapshot(ID).await?;
    assert_eq!(
        metadata.validation_stamp.content_hash.as_deref(),
        Some(snapshot.receipt.artifact_sha256.as_str())
    );
    assert_eq!(metadata.tool_names, ["run"]);
    assert_eq!(
        tokio::fs::read(fixture.manager.storage.policy_path(&storage_key())).await?,
        replacement_policy.as_bytes()
    );
    let template = fixture
        .manager
        .get_component(ID)
        .await
        .context("missing replacement instance")?
        .policy_template;
    assert!(template.allowed_hosts.contains("new.example.invalid"));
    assert!(!template.allowed_hosts.contains("retained.example.invalid"));
    assert_call(&fixture.manager, 2).await
}

#[tokio::test]
async fn safe_replacement_bundle_preserves_explicit_policy_grants() -> Result<()> {
    let fixture = Fixture::new().await?;
    fixture.load_downloaded(1, None).await?;
    fixture.attach_policy().await?;
    let before = fixture.manager.store_snapshot(ID).await?;
    let incoming = POLICY.replace("retained.example.invalid", "incoming.example.invalid");
    fixture
        .load_downloaded(2, Some(incoming.as_bytes()))
        .await?;
    let after = fixture.manager.store_snapshot(ID).await?;
    assert_ne!(before.receipt.revision, after.receipt.revision);
    assert_eq!(before.policy, after.policy);
    assert_eq!(before.receipt.policy, after.receipt.policy);
    let template = fixture
        .manager
        .get_component(ID)
        .await
        .unwrap()
        .policy_template;
    assert!(template.allowed_hosts.contains("retained.example.invalid"));
    assert!(!template.allowed_hosts.contains("incoming.example.invalid"));
    assert_call(&fixture.manager, 2).await
}

#[tokio::test]
async fn safe_replacement_invalid_first_install_leaves_no_live_entry() -> Result<()> {
    for candidate in [
        b"invalid binary".to_vec(),
        unsupported_import_component()?,
        wat::parse_str("(component $empty)")?,
    ] {
        let fixture = Fixture::new().await?;
        let source = fixture.source(&candidate).await?;

        assert!(fixture
            .manager
            .load_component(&file_uri(&source))
            .await
            .is_err());

        assert!(fixture.manager.list_components().await.is_empty());
        assert!(fixture.manager.list_components_known().await.is_empty());
        assert!(fixture.manager.list_tools().await.is_empty());
        assert!(fixture.manager.get_component_schema(ID).await.is_none());
        for path in [
            fixture.manager.component_path(KEY),
            fixture.manager.storage.metadata_path(&storage_key()),
            fixture.manager.component_precompiled_path(KEY),
            fixture.manager.storage.policy_path(&storage_key()),
            fixture.manager.storage.policy_metadata_path(&storage_key()),
            fixture
                .manager
                .component_root()
                .join(format!("{KEY}.install.json")),
        ] {
            assert!(!path.exists(), "failed install left {}", path.display());
        }
        assert!(fixture
            .manager
            .execute_component_call(ID, "run", "{}")
            .await
            .is_err());
    }
    Ok(())
}

#[tokio::test]
async fn safe_replacement_cache_obstruction_preserves_receipt_and_pinned_runtime() -> Result<()> {
    for native in [false, true] {
        let fixture = Fixture::new().await?;
        fixture.load(1).await?;
        fixture.attach_policy().await?;
        let mut before = InstalledState::capture(&fixture.manager).await?;
        let metadata = fixture.manager.storage.metadata_path(&storage_key());
        let precompiled = fixture.manager.component_precompiled_path(KEY);
        before
            .files
            .retain(|(path, _)| path != &metadata && path != &precompiled);
        let blocked_path = if native { precompiled } else { metadata };
        tokio::fs::remove_file(&blocked_path).await?;
        tokio::fs::create_dir(&blocked_path).await?;
        tokio::fs::write(blocked_path.join("blocker"), b"keep this directory").await?;

        assert!(fixture.load(2).await.is_err());

        assert!(blocked_path.is_dir());
        assert_eq!(
            tokio::fs::read(blocked_path.join("blocker")).await?,
            b"keep this directory"
        );
        before.assert_pinned_unchanged(&fixture.manager).await?;
        assert!(fixture
            .manager
            .execute_component_call(ID, "run", "{}")
            .await
            .is_err());
    }
    Ok(())
}

#[tokio::test]
async fn safe_replacement_preserves_configured_environment_and_refreshes_default_secrets(
) -> Result<()> {
    for with_policy in [false, true] {
        let fixture = Fixture::new().await?;
        fixture.load(1).await?;
        fixture
            .manager
            .set_component_secrets(
                ID,
                &[("REPLACEMENT_SECRET".into(), "initial-test-value".into())],
            )
            .await?;
        if with_policy {
            fixture.attach_policy().await?;
        }

        fixture.load(2).await?;

        let template = fixture
            .manager
            .get_component(ID)
            .await
            .context("missing replacement instance")?
            .policy_template;
        assert_eq!(
            template
                .config_vars
                .get("REPLACEMENT_CONFIG")
                .map(String::as_str),
            Some("configured-test-value")
        );
        assert_eq!(
            template
                .config_vars
                .get("REPLACEMENT_SECRET")
                .map(String::as_str),
            Some("initial-test-value")
        );
        if !with_policy {
            fixture
                .manager
                .set_component_secrets(
                    ID,
                    &[("REPLACEMENT_SECRET".into(), "updated-test-value".into())],
                )
                .await?;
            let instance = fixture.manager.get_component(ID).await.unwrap();
            let fresh = fixture
                .manager
                .policy_manager
                .prepare_bound_template(
                    instance
                        .secret_binding
                        .as_ref()
                        .context("missing secret binding")?,
                    instance.effective_policy.as_deref(),
                )
                .await?;
            assert_eq!(
                fresh
                    .config_vars
                    .get("REPLACEMENT_SECRET")
                    .map(String::as_str),
                Some("updated-test-value")
            );
            assert_eq!(
                fresh
                    .config_vars
                    .get("REPLACEMENT_CONFIG")
                    .map(String::as_str),
                Some("configured-test-value")
            );
        }
        assert_call(&fixture.manager, 2).await?;
    }
    Ok(())
}

#[tokio::test]
async fn safe_replacement_cancelled_caller_does_not_cancel_publication() -> Result<()> {
    let fixture = Fixture::new().await?;
    fixture.load(1).await?;
    let candidate = component(2)?;
    let source = fixture.source(&candidate).await?;
    let guard = fixture.manager.load_guard(ID).await;
    let registry = fixture.manager.registry.state.write().await;
    let caller = tokio::spawn({
        let manager = fixture.manager.clone();
        async move { manager.load_component(&file_uri(&source)).await }
    });

    let promoted = tokio::time::timeout(WAIT, async {
        loop {
            if fixture.manager.store_snapshot(ID).await?.wasm == candidate {
                return Ok::<_, anyhow::Error>(());
            }
            if caller.is_finished() {
                bail!("replacement finished without promoting the candidate");
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    caller.abort();
    let cancelled = caller.await.unwrap_err();
    assert!(cancelled.is_cancelled());
    promoted.context("replacement never reached publication")??;
    assert!(
        guard.try_lock().is_err(),
        "the worker must retain the per-ID guard while registration is blocked"
    );
    drop(registry);

    let finished = tokio::time::timeout(WAIT, guard.lock())
        .await
        .context("replacement worker did not finish after registry unlock")?;
    drop(finished);
    assert_eq!(fixture.manager.list_components().await, [ID]);
    assert!(fixture.manager.load_component_metadata(ID).await?.is_some());
    assert!(fixture.manager.component_precompiled_path(KEY).is_file());
    assert_call(&fixture.manager, 2).await
}

#[tokio::test]
async fn safe_replacement_captured_input_is_frozen_before_transactional_install() -> Result<()> {
    let fixture = Fixture::new().await?;
    fixture.load(1).await?;
    let before = InstalledState::capture(&fixture.manager).await?;
    let captured_wasm = component(2)?;
    let source_wasm = fixture.source(&captured_wasm).await?;
    let source_policy = fixture.source.path().join(format!("{KEY}.policy.yaml"));
    let captured_policy = POLICY.replace("retained.example.invalid", "captured.example.invalid");
    tokio::fs::write(&source_policy, &captured_policy).await?;
    let acquired =
        acquisition::acquire_component(&file_uri(&source_wasm), &fixture.manager.config, true)
            .await?;
    assert_eq!(acquired.wasm, captured_wasm);
    assert_eq!(acquired.policy.as_deref(), Some(captured_policy.as_bytes()));
    before.assert_unchanged(&fixture.manager).await?;

    let mutated_wasm = component(3)?;
    let mutated_policy = POLICY.replace("retained.example.invalid", "mutated.example.invalid");
    tokio::fs::write(&source_wasm, &mutated_wasm).await?;
    tokio::fs::write(&source_policy, &mutated_policy).await?;
    before.assert_unchanged(&fixture.manager).await?;

    let outcome = fixture.manager.install_acquired(acquired, None).await?;

    assert_eq!(outcome.status, LoadResult::Replaced);
    assert_eq!(outcome.component_id, ID);
    assert_eq!(
        tokio::fs::read(fixture.manager.component_path(KEY)).await?,
        captured_wasm
    );
    assert_eq!(
        tokio::fs::read(fixture.manager.storage.policy_path(&storage_key())).await?,
        captured_policy.as_bytes()
    );
    assert_eq!(tokio::fs::read(source_wasm).await?, mutated_wasm);
    assert_eq!(
        tokio::fs::read(source_policy).await?,
        mutated_policy.as_bytes()
    );
    let template = fixture
        .manager
        .get_component(ID)
        .await
        .context("missing replacement instance")?
        .policy_template;
    assert!(template.allowed_hosts.contains("captured.example.invalid"));
    assert!(!template.allowed_hosts.contains("mutated.example.invalid"));
    assert!(!template.allowed_hosts.contains("retained.example.invalid"));
    assert_call(&fixture.manager, 2).await
}

#[tokio::test]
async fn safe_replacement_oci_bundle_uses_configured_http_client() -> Result<()> {
    use oci_client::client::{ClientConfig, ClientProtocol, Config, ImageLayer};
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let layers = [
        ImageLayer::new(component(2)?, oci_wasm::WASM_LAYER_MEDIA_TYPE.into(), None),
        ImageLayer::new(
            POLICY.as_bytes().to_vec(),
            "application/vnd.wassette.policy+yaml".into(),
            None,
        ),
    ];
    let config = Config::new(
        serde_json::to_vec(&serde_json::json!({
            "created": "1970-01-01T00:00:00Z",
            "author": null,
            "architecture": oci_wasm::WASM_ARCHITECTURE,
            "os": oci_wasm::COMPONENT_OS,
            "layerDigests": layers.iter().map(ImageLayer::sha256_digest).collect::<Vec<_>>(),
            "component": { "exports": ["run"], "imports": [], "target": null }
        }))?,
        oci_wasm::WASM_MANIFEST_CONFIG_MEDIA_TYPE.into(),
        None,
    );
    let mut manifest = oci_client::manifest::OciImageManifest::build(&layers, &config, None);
    manifest.media_type = Some(oci_wasm::WASM_MANIFEST_MEDIA_TYPE.into());
    let manifest_bytes = serde_json::to_vec(&manifest)?;
    let manifest_digest = format!("sha256:{}", hex::encode(Sha256::digest(&manifest_bytes)));
    let mut routes = HashMap::from([
        (
            "/v2/".to_owned(),
            ("application/json".to_owned(), b"{}".to_vec()),
        ),
        (
            format!("/v2/{KEY}/manifests/latest"),
            (
                oci_wasm::WASM_MANIFEST_MEDIA_TYPE.to_owned(),
                manifest_bytes.clone(),
            ),
        ),
        (
            format!("/v2/{KEY}/manifests/{manifest_digest}"),
            (
                oci_wasm::WASM_MANIFEST_MEDIA_TYPE.to_owned(),
                manifest_bytes,
            ),
        ),
        (
            format!("/v2/{KEY}/blobs/{}", manifest.config.digest),
            (config.media_type, config.data.to_vec()),
        ),
    ]);
    for layer in layers {
        routes.insert(
            format!("/v2/{KEY}/blobs/{}", layer.sha256_digest()),
            (layer.media_type, layer.data.to_vec()),
        );
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let components = test_dir()?;
    let secrets = test_dir()?;
    let manager = LifecycleManager::builder(components.path())
        .with_secrets_dir(secrets.path())
        .with_eager_loading(false)
        .with_oci_client(oci_client::Client::new(ClientConfig {
            protocol: ClientProtocol::Http,
            ..Default::default()
        }))
        .build()
        .await?;
    let server: tokio::task::JoinHandle<Result<()>> = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await?;
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = socket.read(&mut buffer).await?;
                if count == 0 || request.len() + count > 8192 {
                    bail!("registry received an incomplete or oversized request");
                }
                request.extend_from_slice(&buffer[..count]);
            }
            let request = String::from_utf8(request)?;
            let mut words = request.split_whitespace();
            let method = words.next().context("missing request method")?;
            let path = words.next().context("missing request path")?;
            let (media_type, body) = routes
                .get(path)
                .with_context(|| format!("unexpected registry route: {path}"))?;
            let digest = hex::encode(Sha256::digest(body));
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: {media_type}\r\n\
                 Docker-Content-Digest: sha256:{digest}\r\n\
                 Docker-Distribution-Api-Version: registry/2.0\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(headers.as_bytes()).await?;
            if method != "HEAD" {
                socket.write_all(body).await?;
            }
        }
    });

    let uri = format!("oci://{address}/{KEY}:latest");
    let loaded = tokio::time::timeout(WAIT, manager.load_component(&uri)).await;
    server.abort();
    match server.await {
        Err(error) if error.is_cancelled() => {}
        result => result??,
    }
    let outcome = loaded.context("OCI bundle load timed out")??;
    assert_eq!(outcome.component_id, ID);
    assert_eq!(outcome.status, LoadResult::New);
    assert_eq!(
        tokio::fs::read(manager.component_path(KEY)).await?,
        component(2)?
    );
    assert_eq!(
        tokio::fs::read(manager.storage.policy_path(&storage_key())).await?,
        POLICY.as_bytes()
    );
    let template = manager.get_component(ID).await.unwrap().policy_template;
    assert!(template.allowed_hosts.contains("retained.example.invalid"));
    assert!(template.network_perms.allow_tcp);
    assert_call(&manager, 2).await
}

#[tokio::test]
async fn wasm_directory_package_install_is_digest_pinned_until_explicit_load() -> Result<()> {
    use oci_client::client::{ClientConfig, ClientProtocol, Config, ImageLayer};
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use url::Url;
    use wasm_directory::{PackageId, PackageSelector, WasmDirectoryClient};

    let layers = [ImageLayer::new(
        component(7)?,
        oci_wasm::WASM_LAYER_MEDIA_TYPE.into(),
        None,
    )];
    let config = Config::new(
        serde_json::to_vec(&serde_json::json!({
            "created": "1970-01-01T00:00:00Z",
            "author": null,
            "architecture": oci_wasm::WASM_ARCHITECTURE,
            "os": oci_wasm::COMPONENT_OS,
            "layerDigests": layers.iter().map(ImageLayer::sha256_digest).collect::<Vec<_>>(),
            "component": { "exports": ["run"], "imports": [], "target": null }
        }))?,
        oci_wasm::WASM_MANIFEST_CONFIG_MEDIA_TYPE.into(),
        None,
    );
    let mut manifest = oci_client::manifest::OciImageManifest::build(&layers, &config, None);
    manifest.media_type = Some(oci_wasm::WASM_MANIFEST_MEDIA_TYPE.into());
    let manifest_bytes = serde_json::to_vec(&manifest)?;
    let manifest_digest = format!("sha256:{}", hex::encode(Sha256::digest(&manifest_bytes)));

    let registry_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let registry_address = registry_listener.local_addr()?;
    let registry = registry_address.to_string();
    let repository = "owner/safe-replacement";
    let colliding_repository = "another/safe-replacement";
    let mut routes = HashMap::from([
        (
            "/v2/".to_owned(),
            ("application/json".to_owned(), b"{}".to_vec()),
        ),
        (
            format!("/v2/{repository}/manifests/{manifest_digest}"),
            (
                oci_wasm::WASM_MANIFEST_MEDIA_TYPE.to_owned(),
                manifest_bytes,
            ),
        ),
        (
            format!("/v2/{repository}/blobs/{}", manifest.config.digest),
            (config.media_type.clone(), config.data.to_vec()),
        ),
    ]);
    for layer in layers {
        for repository in [repository, colliding_repository] {
            routes.insert(
                format!("/v2/{repository}/blobs/{}", layer.sha256_digest()),
                (layer.media_type.clone(), layer.data.to_vec()),
            );
        }
    }
    routes.insert(
        format!("/v2/{colliding_repository}/manifests/{manifest_digest}"),
        (
            oci_wasm::WASM_MANIFEST_MEDIA_TYPE.to_owned(),
            serde_json::to_vec(&manifest)?,
        ),
    );
    routes.insert(
        format!(
            "/v2/{colliding_repository}/blobs/{}",
            manifest.config.digest
        ),
        (config.media_type.clone(), config.data.to_vec()),
    );
    let registry_server = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = registry_listener.accept().await else {
                break;
            };
            let routes = routes.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    let count = socket.read(&mut buffer).await?;
                    if count == 0 || request.len() + count > 8192 {
                        bail!("registry received an incomplete or oversized request");
                    }
                    request.extend_from_slice(&buffer[..count]);
                }
                let request = String::from_utf8(request)?;
                let mut words = request.split_whitespace();
                let method = words.next().context("missing request method")?;
                let path = words.next().context("missing request path")?;
                let (media_type, body) = routes
                    .get(path)
                    .with_context(|| format!("unexpected registry route: {path}"))?;
                let digest = hex::encode(Sha256::digest(body));
                let headers = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: {media_type}\r\n\
                     Docker-Content-Digest: sha256:{digest}\r\n\
                     Docker-Distribution-Api-Version: registry/2.0\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                socket.write_all(headers.as_bytes()).await?;
                if method != "HEAD" {
                    socket.write_all(body).await?;
                }
                Ok::<_, anyhow::Error>(())
            });
        }
    });

    let api_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let api_address = api_listener.local_addr()?;
    let api_digest = manifest_digest.clone();
    let api_server = tokio::spawn(async move {
        for _ in 0..4 {
            let (mut socket, _) = api_listener.accept().await?;
            let mut request = [0; 4096];
            let bytes_read = socket.read(&mut request).await?;
            let request = String::from_utf8_lossy(&request[..bytes_read]);
            let request_path = request.split_whitespace().nth(1).unwrap_or_default();
            let repository = if request_path.ends_with(colliding_repository) {
                colliding_repository
            } else {
                repository
            };
            let body = serde_json::to_vec(&serde_json::json!({
                "registry": registry,
                "repository": repository,
                "kind": "component",
                "versions": [{"tag": "1.2.3", "digest": api_digest}],
            }))?;
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(headers.as_bytes()).await?;
            socket.write_all(&body).await?;
        }
        Ok::<_, anyhow::Error>(())
    });

    let components = test_dir()?;
    let secrets = test_dir()?;
    let manager = LifecycleManager::builder(components.path())
        .with_secrets_dir(secrets.path())
        .with_eager_loading(false)
        .with_oci_client(oci_client::Client::new(ClientConfig {
            protocol: ClientProtocol::Http,
            ..Default::default()
        }))
        .build()
        .await?;
    let directory = WasmDirectoryClient::new(Url::parse(&format!("http://{api_address}"))?)?;
    let package = PackageSelector::Package(PackageId::new(
        registry_address.to_string(),
        repository.to_owned(),
    )?);
    let (resolved, outcome) =
        tokio::time::timeout(WAIT, manager.install_package(&directory, &package, None)).await??;
    let (_, unchanged) =
        tokio::time::timeout(WAIT, manager.install_package(&directory, &package, None)).await??;
    let colliding_package = PackageSelector::Package(PackageId::new(
        registry_address.to_string(),
        colliding_repository.to_owned(),
    )?);
    let collision = tokio::time::timeout(
        WAIT,
        manager.install_package(&directory, &colliding_package, None),
    )
    .await?
    .unwrap_err();
    assert!(collision.to_string().contains("conflict"), "{collision:#}");

    assert_eq!(resolved.manifest_digest, manifest_digest);
    assert_eq!(resolved.selected_version, "1.2.3");
    let receipt = match &outcome.entry {
        store::StoredEntry::Installed(receipt) => receipt,
        store::StoredEntry::Retired(_) => bail!("package installation returned a retired entry"),
    };
    assert_eq!(receipt.component_id.as_str(), package.to_string().as_str());
    assert_eq!(receipt.storage_key.as_str(), "local_safe-replacement");
    assert!(outcome.change.is_some());
    assert!(unchanged.change.is_none());
    assert_eq!(unchanged.entry.revision(), outcome.entry.revision());
    assert_eq!(receipt.origin.location, format!("wasm.directory:{package}"));
    assert_eq!(
        receipt.origin.immutable_uri.as_deref(),
        Some(resolved.oci_reference.as_str())
    );
    assert!(manager.get_component(ID).await.is_none());
    assert_eq!(manager.catalog().await?.tools.len(), 1);
    assert_eq!(
        tokio::fs::read(manager.component_path("local_safe-replacement")).await?,
        component(7)?
    );

    let (loaded, load_outcome) =
        tokio::time::timeout(WAIT, manager.load_package(&directory, &package, None)).await??;
    let _loaded_receipt = match &load_outcome.commit.entry {
        store::StoredEntry::Installed(receipt) => receipt,
        store::StoredEntry::Retired(_) => bail!("package load returned a retired entry"),
    };
    assert_eq!(loaded.manifest_digest, manifest_digest);
    assert!(manager
        .get_component(package.to_string().as_str())
        .await
        .is_some());
    assert!(!manager.catalog().await?.tools.is_empty());
    let result = manager
        .execute_component_call(package.to_string().as_str(), "run", "{}")
        .await?;
    assert_eq!(
        serde_json::from_str::<Value>(&result)?,
        serde_json::json!({ "result": 7 })
    );
    api_server.await??;
    registry_server.abort();
    match registry_server.await {
        Err(error) if error.is_cancelled() => {}
        result => result?,
    }
    Ok(())
}

#[tokio::test]
async fn safe_replacement_public_load_uses_configured_http_client() -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let proxy_address = listener.local_addr()?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .proxy(reqwest::Proxy::https(format!("http://{proxy_address}"))?)
        .timeout(WAIT)
        .build()?;
    let fixture = Fixture::with_http_client(Some(client)).await?;
    fixture.load(1).await?;
    fixture.attach_policy().await?;
    let before = InstalledState::capture(&fixture.manager).await?;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let count = socket.read(&mut buffer).await?;
            if count == 0 || request.len() + count > 8192 {
                bail!("proxy received an incomplete or oversized request");
            }
            request.extend_from_slice(&buffer[..count]);
        }
        socket
            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await?;
        Ok::<_, anyhow::Error>(String::from_utf8(request)?)
    });

    // Only HTTPS is accepted by the loader. Reject CONNECT locally to prove client injection
    // without a TLS fixture or a request to any external host.
    let (loaded, request) = tokio::join!(
        fixture
            .manager
            .load_component("https://safe-replacement.invalid/safe-replacement.wasm"),
        tokio::time::timeout(WAIT, server),
    );
    assert!(loaded.is_err());
    let request = request.context("configured HTTP proxy was never contacted")???;
    assert!(request.starts_with("CONNECT safe-replacement.invalid:443 HTTP/1.1\r\n"));
    before.assert_unchanged(&fixture.manager).await
}

async fn directory_manager(components: &TempDir, secrets: &TempDir) -> Result<LifecycleManager> {
    use oci_client::client::{ClientConfig, ClientProtocol};

    LifecycleManager::builder(components.path())
        .with_secrets_dir(secrets.path())
        .with_eager_loading(false)
        .with_oci_client(oci_client::Client::new(ClientConfig {
            protocol: ClientProtocol::Http,
            ..Default::default()
        }))
        .build()
        .await
}

#[tokio::test]
async fn wit_selector_installs_exactly_matching_digest_pinned_package() -> Result<()> {
    use wasm_directory::fixtures::{FixturePackage, WasmDirectoryFixture};
    use wasm_directory::PackageSelector;

    let fixture = WasmDirectoryFixture::start(vec![
        FixturePackage::new(
            "owner/safe-replacement",
            Some("demo:replacement"),
            [("1.0.0", component(1)?), ("1.2.0", component(2)?)],
        ),
        FixturePackage::new(
            "owner/other",
            Some("demo:replacement-extra"),
            [("9.0.0", component(9)?)],
        ),
    ])
    .await?;
    let directory = fixture.directory()?;
    let components = test_dir()?;
    let secrets = test_dir()?;
    let manager = directory_manager(&components, &secrets).await?;

    let selector = PackageSelector::parse("demo:replacement")?;
    let (resolved, outcome) =
        tokio::time::timeout(WAIT, manager.install_package(&directory, &selector, None)).await??;
    assert_eq!(
        resolved.package_id.to_string(),
        fixture.package_id("owner/safe-replacement")
    );
    assert_eq!(resolved.selected_version, "1.2.0");
    assert_eq!(
        resolved.manifest_digest,
        fixture.digest("owner/safe-replacement", "1.2.0")
    );
    let crate::store::StoredEntry::Installed(receipt) = &outcome.entry else {
        bail!("expected an installed receipt");
    };
    assert_eq!(
        receipt.component_id.as_str(),
        fixture.package_id("owner/safe-replacement").as_str()
    );
    assert_eq!(
        receipt.origin.manifest_digest.as_deref(),
        Some(resolved.manifest_digest.as_str())
    );

    let pinned = PackageSelector::parse("demo:replacement@1.0.0")?;
    let (resolved, _) =
        tokio::time::timeout(WAIT, manager.install_package(&directory, &pinned, None)).await??;
    assert_eq!(resolved.selected_version, "1.0.0");
    assert_eq!(resolved.requested_version.as_deref(), Some("1.0.0"));
    assert_eq!(
        resolved.manifest_digest,
        fixture.digest("owner/safe-replacement", "1.0.0")
    );
    let conflict = manager
        .install_package(&directory, &pinned, Some("1.2.0"))
        .await
        .unwrap_err();
    assert!(
        format!("{conflict:#}").contains("Conflicting versions"),
        "{conflict:#}"
    );
    Ok(())
}

#[tokio::test]
async fn wit_selector_rejects_missing_and_ambiguous_identities() -> Result<()> {
    use wasm_directory::fixtures::{FixturePackage, WasmDirectoryFixture};
    use wasm_directory::PackageSelector;

    let fixture = WasmDirectoryFixture::start(vec![
        FixturePackage::new("first/tool", Some("demo:tool"), [("1.0.0", component(1)?)]),
        FixturePackage::new("second/tool", Some("demo:tool"), [("1.0.0", component(2)?)]),
        FixturePackage::new("third/tool", Some("Demo:tool"), [("1.0.0", component(3)?)]),
    ])
    .await?;
    let directory = fixture.directory()?;
    let components = test_dir()?;
    let secrets = test_dir()?;
    let manager = directory_manager(&components, &secrets).await?;

    let missing = manager
        .install_package(&directory, &PackageSelector::parse("demo:absent")?, None)
        .await
        .unwrap_err();
    assert!(
        format!("{missing:#}")
            .contains("No wasm.directory component package has WIT identity demo:absent"),
        "{missing:#}"
    );

    let ambiguous = manager
        .install_package(&directory, &PackageSelector::parse("demo:tool")?, None)
        .await
        .unwrap_err();
    let message = format!("{ambiguous:#}");
    assert!(message.contains("matches multiple"), "{message}");
    assert!(
        message.contains(&fixture.package_id("first/tool")),
        "{message}"
    );
    assert!(
        message.contains(&fixture.package_id("second/tool")),
        "{message}"
    );
    assert!(
        !message.contains(&fixture.package_id("third/tool")),
        "{message}"
    );
    assert!(manager.store_snapshot(ID).await.is_err());
    Ok(())
}

#[tokio::test]
async fn nameless_registry_package_uses_canonical_registry_id() -> Result<()> {
    use wasm_directory::fixtures::{FixturePackage, WasmDirectoryFixture};
    use wasm_directory::PackageSelector;

    let nameless = wat::parse_str(
        r#"(component
            (core module $m
                (func (export "run") (result i32) i32.const 1))
            (core instance $i (instantiate $m))
            (func (export "run") (result u32)
                (canon lift (core func $i "run"))))"#,
    )?;
    let fixture = WasmDirectoryFixture::start(vec![FixturePackage::new(
        "owner/nameless",
        Some("demo:nameless"),
        [("2.0.6", nameless)],
    )])
    .await?;
    let directory = fixture.directory()?;
    let components = test_dir()?;
    let secrets = test_dir()?;
    let manager = directory_manager(&components, &secrets).await?;

    let (_, outcome) = manager
        .install_package(&directory, &PackageSelector::parse("demo:nameless")?, None)
        .await?;
    let crate::store::StoredEntry::Installed(receipt) = outcome.entry else {
        bail!("expected an installed receipt");
    };
    assert_eq!(
        receipt.component_id.as_str(),
        fixture.package_id("owner/nameless")
    );
    assert_eq!(manager.catalog().await?.tools.len(), 1);
    Ok(())
}
