// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::sync::atomic::{AtomicUsize, Ordering};

use wasm_encoder::{ComponentSection, Encode};

use super::*;

pub(crate) fn ordinary_component() -> Vec<u8> {
    named_ordinary("ordinary")
}

fn named_ordinary(name: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(component $"{name}"
            (core module $m (func (export "run")))
            (core instance $i (instantiate $m))
            (func (export "run") (canon lift (core func $i "run")))
        )"#
    ))
    .unwrap()
}

fn provider_component(name: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(component $"{name}"
            (instance $agent)
            (export "wassette:acp/agent@7.0.0" (instance $agent))
        )"#
    ))
    .unwrap()
}

fn test_dir() -> Result<tempfile::TempDir> {
    Ok(tempfile::Builder::new()
        .prefix(".kind-gate-")
        .tempdir_in(std::env::current_dir()?)?)
}

async fn manager(root: &Path) -> Result<LifecycleManager> {
    LifecycleManager::builder(root.join("components"))
        .with_secrets_dir(root.join("secrets"))
        .with_eager_loading(false)
        .build()
        .await
}

async fn resource(key: &str, bytes: &[u8]) -> Result<DownloadedResource> {
    let directory = test_dir()?;
    let path = directory.path().join(format!("{key}.wasm"));
    tokio::fs::write(&path, bytes).await?;
    Ok(DownloadedResource::Temp((directory, path)))
}

async fn install_tool(
    manager: &LifecycleManager,
    key: &str,
    bytes: &[u8],
) -> Result<ComponentLoadOutcome> {
    manager
        .load_component_resource(resource(key, bytes).await?)
        .await
}

async fn publish_metadata(
    manager: &LifecycleManager,
    id: &str,
    metadata: &ComponentMetadata,
    native: Vec<u8>,
) -> Result<()> {
    let snapshot = manager.store_snapshot(id).await?;
    manager.component_store().publish_cache(
        id,
        &snapshot.receipt.revision,
        store::PreparedCache {
            artifact_sha256: snapshot.receipt.artifact_sha256,
            engine: manager.cache_engine(),
            schema: store_runtime::CACHE_SCHEMA.into(),
            metadata: serde_json::to_value(metadata)?,
            native,
        },
    )?;
    Ok(())
}

async fn write_legacy_metadata(manager: &LifecycleManager, key: &str) -> Result<()> {
    let file = tokio::fs::metadata(manager.component_path(key)).await?;
    let metadata = ComponentMetadata {
        component_id: key.to_owned(),
        tool_schemas: vec![serde_json::json!({
            "name": "run",
            "inputSchema": { "type": "object" }
        })],
        function_identifiers: vec![FunctionIdentifier {
            package_name: None,
            interface_name: None,
            function_name: "run".to_owned(),
        }],
        tool_names: vec!["run".to_owned()],
        validation_stamp: ValidationStamp {
            file_size: file.len(),
            mtime: file
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
            content_hash: None,
        },
        created_at: 0,
    };
    tokio::fs::write(
        manager.storage.metadata_path(&StorageKey::parse(key)?),
        serde_json::to_vec(&metadata)?,
    )
    .await?;
    Ok(())
}

fn pad(bytes: &mut Vec<u8>) {
    let padding = vec![0; 4096 - bytes.len()];
    bytes.push(0);
    wasm_encoder::CustomSection {
        name: "padding".into(),
        data: padding.into(),
    }
    .encode(bytes);
}

fn explicitly_named_fixture(mut bytes: Vec<u8>, name: &str) -> Result<Vec<u8>> {
    match inspect_artifact(&bytes)?.identity {
        Ok(_) => {}
        Err(IdentityError::Missing) => {
            let mut names = wasm_encoder::ComponentNameSection::new();
            names.component(name);
            bytes.push(names.id());
            names.encode(&mut bytes);
        }
        Err(error) => return Err(error.into()),
    }
    Ok(bytes)
}

#[tokio::test]
async fn current_shape_gates_same_stamp_metadata_and_all_restore_paths() -> Result<()> {
    let root = test_dir()?;
    let manager = manager(root.path()).await?;
    let key = "agent-private";
    let mut ordinary = named_ordinary("agent");
    let mut provider = provider_component("agent");
    pad(&mut ordinary);
    pad(&mut provider);
    assert_eq!(ordinary.len(), provider.len());
    install_tool(&manager, key, &ordinary).await?;
    let descriptor = manager.list_tool_descriptors().await?.remove(0);
    manager.registry.remove_component("agent").await;
    let path = manager.component_path(key);
    let old_metadata = std::fs::metadata(&path)?;
    let modified = old_metadata.modified()?;
    let cached_native = tokio::fs::read(manager.component_precompiled_path(key)).await?;

    // This deliberate external edit defeats the old size/mtime gate, not the receipt hash.
    tokio::fs::write(&path, &provider).await?;
    std::fs::File::options()
        .write(true)
        .open(&path)?
        .set_times(std::fs::FileTimes::new().set_modified(modified))?;
    let changed_metadata = std::fs::metadata(&path)?;
    assert_eq!(changed_metadata.len(), old_metadata.len());
    assert_eq!(changed_metadata.modified()?, modified);
    assert!(manager.store_snapshot("agent").await.is_err());
    assert!(manager.get_component_schema("agent").await.is_none());
    assert!(manager.list_tool_descriptors().await.is_err());
    assert!(manager.describe_scoped_tool(&descriptor.key).await.is_err());
    assert!(manager
        .invoke_scoped_tool(&descriptor.key, &serde_json::json!({}))
        .await
        .is_err());
    assert!(manager.populate_registry_from_metadata().await.is_err());
    assert!(manager.list_tools().await.is_empty());
    assert!(manager.load_all_components().await.is_err());
    assert!(manager.list_components().await.is_empty());
    let notifications = Arc::new(AtomicUsize::new(0));
    let notified = Arc::clone(&notifications);
    assert!(manager
        .load_existing_components_async(
            Some(1),
            Some(move || {
                notified.fetch_add(1, Ordering::Relaxed);
            }),
        )
        .await
        .is_err());
    assert_eq!(notifications.load(Ordering::Relaxed), 0);
    let error = manager.ensure_component_loaded("agent").await.unwrap_err();
    assert!(format!("{error:#}").contains("artifact hash"), "{error:#}");
    assert!(manager.list_tools().await.is_empty());
    assert_eq!(
        tokio::fs::read(manager.component_precompiled_path(key)).await?,
        cached_native
    );
    Ok(())
}

#[tokio::test]
async fn receipted_acp_artifacts_never_enter_ordinary_restore_paths() -> Result<()> {
    for exports in [
        vec!["wassette:acp/agent@7.0.0"],
        vec!["wassette:acp/agent@7.0.0", "wassette:acp/client@7.0.0"],
    ] {
        let root = test_dir()?;
        let manager = manager(root.path()).await?;
        let declarations = exports
            .iter()
            .map(|export| format!(r#"(export "{export}" (instance $stage))"#))
            .collect::<String>();
        let wasm = wat::parse_str(format!(
            r#"(component $agent
            (instance $stage)
            {declarations})"#
        ))?;
        let key = StorageKey::parse("agent-private")?;
        let source = store::SourceIdentity::File(root.path().join("agent-source.wasm"));
        let expected = manager.component_store().observe("agent", &key, &source)?;
        let prepared = store::PreparedInstall::prepare(
            wasm,
            store::InstallOptions {
                storage_key: key,
                source,
                origin: store::OriginEvidence {
                    location: "test-fixture:agent".into(),
                    requested_version: None,
                    selected_version: None,
                    manifest_digest: None,
                    immutable_uri: None,
                    generation: None,
                },
                owner: store::InstallOwner::Explicit,
                intent: store::InstallIntent::InstallOnly,
                policy: store::PreparedPolicy::absent(store::PolicyProvenance::Default),
                observation: None,
            },
            |bytes, inspection, _| {
                Component::new(manager.runtime.as_ref(), bytes)?;
                anyhow::ensure!(
                    inspection.acp_exports == exports,
                    "unexpected test ACP export"
                );
                Ok(store::ValidationEvidence::AcpCompiledAndExportChecked {
                    runtime: "test-fixture-acp-v7".into(),
                })
            },
        )?;
        manager
            .component_store()
            .commit_install(prepared, expected)?;
        assert!(manager.get_component_schema("agent").await.is_none());
        manager.populate_registry_from_metadata().await?;
        manager.load_all_components().await?;
        manager
            .load_existing_components_async(Some(1), None::<fn()>)
            .await?;
        assert!(manager.list_components_known().await.is_empty());
        assert!(manager.list_components().await.is_empty());
        assert!(manager.list_tools().await.is_empty());
        assert!(manager.list_tool_descriptors().await?.is_empty());
        let component_id = ComponentId::from_declared_name("agent")?;
        assert!(manager
            .list_tools_for_component(&component_id)
            .await
            .is_err());
        let key = ToolKey {
            component_id,
            export: FunctionIdentifier {
                package_name: None,
                interface_name: Some(exports[0].to_owned()),
                function_name: "run".to_owned(),
            },
        };
        assert!(manager.describe_scoped_tool(&key).await.is_err());
        assert!(manager
            .invoke_scoped_tool(&key, &serde_json::json!({}))
            .await
            .is_err());
        let error = manager.ensure_component_loaded("agent").await.unwrap_err();
        assert!(
            format!("{error:#}").contains("Cannot load ACP or unsupported"),
            "{error:#}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn ordinary_source_discards_stale_acp_native_cache() -> Result<()> {
    let root = test_dir()?;
    let manager = manager(root.path()).await?;
    let key = "ordinary-private";
    install_tool(&manager, key, &ordinary_component()).await?;
    let metadata = manager.load_component_metadata("ordinary").await?.unwrap();
    let provider_cache = manager
        .runtime
        .precompile_component(&provider_component("provider"))?;
    publish_metadata(&manager, "ordinary", &metadata, provider_cache.clone()).await?;
    manager.registry.remove_component("ordinary").await;

    manager.ensure_component_loaded("ordinary").await?;

    assert_eq!(manager.get_component_id_for_tool("run").await?, "ordinary");
    assert_ne!(
        tokio::fs::read(manager.component_precompiled_path(key)).await?,
        provider_cache
    );
    assert_eq!(
        manager.store_snapshot("ordinary").await?.wasm,
        ordinary_component()
    );
    Ok(())
}

#[tokio::test]
async fn non_tool_replacement_is_rejected_before_staging() -> Result<()> {
    let root = test_dir()?;
    let manager = manager(root.path()).await?;
    let key = "agent-private";
    install_tool(&manager, key, &named_ordinary("agent")).await?;
    let before = manager.store_snapshot("agent").await?;
    let old_metadata =
        tokio::fs::read(manager.storage.metadata_path(&StorageKey::parse(key)?)).await?;
    let old_native = tokio::fs::read(manager.component_precompiled_path(key)).await?;
    let error = manager
        .load_component_resource(resource(key, &provider_component("agent")).await?)
        .await
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("Cannot load ACP or unsupported"),
        "{error:#}"
    );
    let after = manager.store_snapshot("agent").await?;
    assert_eq!(before.receipt, after.receipt);
    assert_eq!(before.wasm, after.wasm);
    assert_eq!(
        tokio::fs::read(manager.storage.metadata_path(&StorageKey::parse(key)?)).await?,
        old_metadata
    );
    assert_eq!(
        tokio::fs::read(manager.component_precompiled_path(key)).await?,
        old_native
    );
    assert_eq!(manager.get_component_id_for_tool("run").await?, "agent");
    Ok(())
}

#[tokio::test]
async fn non_runnable_and_missing_legacy_artifacts_never_publish_cached_tools() -> Result<()> {
    for wat in [
        "(module)",
        "(component)",
        "(component $named-empty (type (func)))",
        r#"(component $client (instance $client) (export "wassette:acp/client@7.0.0" (instance $client)))"#,
        r#"(component $named-tool (instance $empty) (export "empty" (instance $empty)))"#,
    ] {
        let root = test_dir()?;
        let manager = manager(root.path()).await?;
        let path = manager.component_path("test");
        tokio::fs::write(&path, wat::parse_str(wat)?).await?;
        write_legacy_metadata(&manager, "test").await?;
        let inventory = manager
            .component_store()
            .snapshot_if_changed(None)?
            .unwrap();
        assert!(inventory.entries.is_empty());
        assert_eq!(inventory.protected.len(), 1);
        assert!(inventory.protected[0].diagnostic.is_some());
        assert!(manager.get_component_schema("test").await.is_none());
        assert!(manager.populate_registry_from_metadata().await.is_err());
        assert!(manager.list_tools().await.is_empty(), "{wat}");
        assert!(manager.ensure_component_loaded("test").await.is_err());
        tokio::fs::remove_file(path).await?;
        assert!(manager.get_component_schema("test").await.is_none());
        assert!(manager.populate_registry_from_metadata().await.is_err());
        assert!(manager.list_tools().await.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn real_async_acp_components_are_inspected_without_the_ordinary_engine() -> Result<()> {
    for (name, expected) in [
        ("acp-echo-provider", ArtifactShape::AcpProvider),
        ("acp-uppercase-layer", ArtifactShape::AcpLayer),
    ] {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../components")
            .join(name)
            .join("target/wasm32-wasip2/release")
            .join(format!("{}.wasm", name.replace('-', "_")));
        let bytes = explicitly_named_fixture(
            tokio::fs::read(&fixture)
                .await
                .context("Build ACP fixtures with `just build-acp-examples`")?,
            name,
        )?;
        let inspection = inspect_artifact(&bytes)?;
        assert!(inspection.identity.is_ok());
        assert_eq!(inspection.shape, expected);
        let root = test_dir()?;
        let manager = manager(root.path()).await?;
        let source = root.path().join(format!("{name}.wasm"));
        tokio::fs::write(&source, bytes).await?;
        let error = manager
            .load_component(&format!("file://{}", source.display()))
            .await
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("Cannot load ACP or unsupported"),
            "{error:#}"
        );
        assert!(manager.list_components_known().await.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn async_non_acp_shape_does_not_promise_ordinary_runtime_compatibility() -> Result<()> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../components/acp-echo-provider/target/wasm32-wasip2/release/acp_echo_provider.wasm",
    );
    let mut bytes = explicitly_named_fixture(tokio::fs::read(path).await?, "async-tool")?;
    // Equal-length namespace substitution retains a valid async type graph.
    let from = b"wassette:acp/";
    let to = b"examplex:acp/";
    for offset in 0..=bytes.len() - from.len() {
        if bytes[offset..].starts_with(from) {
            bytes[offset..offset + from.len()].copy_from_slice(to);
        }
    }
    let mut config = wasmtime::Config::new();
    config.wasm_component_model_async(true);
    config.wasm_component_model_more_async_builtins(true);
    config.wasm_component_model_async_stackful(true);
    config.wasm_features(wasmtime::WasmFeatures::CM_ASYNC, true);
    config.wasm_features(wasmtime::WasmFeatures::CM_MORE_ASYNC_BUILTINS, true);
    config.wasm_features(wasmtime::WasmFeatures::CM_ASYNC_STACKFUL, true);
    let async_engine = wasmtime::Engine::new(&config)?;
    Component::new(&async_engine, &bytes)
        .map_err(anyhow::Error::from)
        .context("fixture must be a valid async component")?;
    assert_eq!(
        inspect_artifact(&bytes)?.shape,
        ArtifactShape::ToolCandidate
    );
    let root = test_dir()?;
    let manager = manager(root.path()).await?;
    let source = root.path().join("async-tool.wasm");
    tokio::fs::write(&source, bytes).await?;
    let error = manager
        .load_component(&format!("file://{}", source.display()))
        .await
        .unwrap_err();
    let diagnostic = format!("{error:#}");
    assert!(
        diagnostic.contains("Failed to compile captured component")
            || diagnostic.contains("failed to instantiate component"),
        "{diagnostic}"
    );
    assert!(manager.list_tools().await.is_empty());
    assert!(manager
        .component_store()
        .snapshot_if_changed(None)?
        .unwrap()
        .entries
        .is_empty());
    Ok(())
}

#[tokio::test]
async fn cached_acp_identifiers_and_mismatched_keys_are_not_published() -> Result<()> {
    let root = test_dir()?;
    let manager = manager(root.path()).await?;
    let key = "ordinary-private";
    install_tool(&manager, key, &ordinary_component()).await?;
    let metadata = manager.load_component_metadata("ordinary").await?.unwrap();
    let native = tokio::fs::read(manager.component_precompiled_path(key)).await?;
    manager.registry.remove_component("ordinary").await;
    let mut acp = metadata.clone();
    acp.function_identifiers[0].package_name = Some("wassette:acp".to_owned());
    acp.function_identifiers[0].interface_name = Some("agent".to_owned());
    let mut mismatched = metadata.clone();
    mismatched.component_id = "different".to_owned();
    for invalid in [acp, mismatched] {
        manager.registry.remove_component("ordinary").await;
        publish_metadata(&manager, "ordinary", &invalid, native.clone()).await?;
        assert!(manager.get_component_schema("ordinary").await.is_some());
        manager.populate_registry_from_metadata().await?;
        let tools = manager.list_tool_descriptors().await?;
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].key.component_id.as_str(), "ordinary");
        assert_eq!(tools[0].key.export, metadata.function_identifiers[0]);
    }
    assert!(!is_acp_identifier(&FunctionIdentifier {
        package_name: None,
        interface_name: None,
        function_name: "wassette-acp-agent".to_owned(),
    }));
    publish_metadata(&manager, "ordinary", &metadata, native).await?;
    manager.populate_registry_from_metadata().await?;
    assert_eq!(manager.get_component_id_for_tool("run").await?, "ordinary");
    Ok(())
}
