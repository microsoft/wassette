// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::sync::atomic::{AtomicUsize, Ordering};

use wasm_encoder::Encode;

use super::*;

pub(crate) fn ordinary_component() -> Vec<u8> {
    wat::parse_str(
        r#"(component $ordinary
            (core module $m (func (export "run")))
            (core instance $i (instantiate $m))
            (func (export "run") (canon lift (core func $i "run")))
        )"#,
    )
    .unwrap()
}

fn provider_component() -> Vec<u8> {
    wat::parse_str(
        r#"(component $provider
            (instance $agent)
            (export "wassette:acp/agent@7.0.0" (instance $agent))
        )"#,
    )
    .unwrap()
}

async fn cache_metadata(manager: &LifecycleManager, id: &str) -> Result<()> {
    manager
        .storage
        .write_metadata(&ComponentMetadata {
            component_id: id.to_owned(),
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
            validation_stamp: manager
                .storage
                .create_validation_stamp(&manager.component_path(id), false)
                .await?,
            created_at: 0,
        })
        .await
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

#[tokio::test]
async fn current_shape_gates_same_stamp_metadata_and_all_restore_paths() -> Result<()> {
    let root = tempfile::tempdir()?;
    let manager = LifecycleManager::new_unloaded(root.path()).await?;
    let path = manager.component_path("agent");
    let mut ordinary = ordinary_component();
    let mut provider = provider_component();
    pad(&mut ordinary);
    pad(&mut provider);
    assert_eq!(ordinary.len(), provider.len());
    tokio::fs::write(&path, &ordinary).await?;
    cache_metadata(&manager, "agent").await?;
    let modified = std::fs::metadata(&path)?.modified()?;
    let cached_native = manager.runtime.precompile_component(&ordinary)?;
    manager
        .storage
        .write_precompiled("agent", &cached_native)
        .await?;

    tokio::fs::write(&path, &provider).await?;
    std::fs::File::options()
        .write(true)
        .open(&path)?
        .set_times(std::fs::FileTimes::new().set_modified(modified))?;
    let metadata = manager.storage.read_metadata("agent").await?.unwrap();
    assert!(ComponentStorage::validate_stamp(&path, &metadata.validation_stamp).await);

    assert!(manager.get_component_schema("agent").await.is_none());
    manager.populate_registry_from_metadata().await?;
    assert!(manager.list_tools().await.is_empty());
    manager.load_all_components().await?;
    assert!(manager.list_components().await.is_empty());
    let notifications = Arc::new(AtomicUsize::new(0));
    let notified = Arc::clone(&notifications);
    manager
        .load_existing_components_async(
            Some(1),
            Some(move || {
                notified.fetch_add(1, Ordering::Relaxed);
            }),
        )
        .await?;
    assert_eq!(notifications.load(Ordering::Relaxed), 0);
    let error = manager.ensure_component_loaded("agent").await.unwrap_err();
    assert!(format!("{error:#}").contains("Cannot load ACP or unsupported"));
    assert!(manager.list_tools().await.is_empty());
    assert_eq!(
        tokio::fs::read(manager.component_precompiled_path("agent")).await?,
        cached_native
    );
    Ok(())
}

#[tokio::test]
async fn ordinary_source_discards_stale_acp_native_cache() -> Result<()> {
    let root = tempfile::tempdir()?;
    let manager = LifecycleManager::new_unloaded(root.path()).await?;
    tokio::fs::write(manager.component_path("ordinary"), ordinary_component()).await?;
    let provider_cache = manager
        .runtime
        .precompile_component(&provider_component())?;
    manager
        .storage
        .write_precompiled("ordinary", &provider_cache)
        .await?;
    manager.ensure_component_loaded("ordinary").await?;
    assert_eq!(manager.get_component_id_for_tool("run").await?, "ordinary");
    assert_ne!(
        tokio::fs::read(manager.component_precompiled_path("ordinary")).await?,
        provider_cache
    );
    Ok(())
}

#[tokio::test]
async fn non_tool_replacement_is_rejected_before_staging() -> Result<()> {
    let root = tempfile::tempdir()?;
    let source = tempfile::tempdir()?;
    let manager = LifecycleManager::new_unloaded(root.path()).await?;
    let path = manager.component_path("agent");
    tokio::fs::write(&path, ordinary_component()).await?;
    manager.ensure_component_loaded("agent").await?;
    let old_metadata = tokio::fs::read(manager.storage.metadata_path("agent")).await?;
    let old_native = tokio::fs::read(manager.component_precompiled_path("agent")).await?;
    let replacement = source.path().join("agent.wasm");
    tokio::fs::write(&replacement, provider_component()).await?;
    let error = manager
        .load_component(&format!("file://{}", replacement.display()))
        .await
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("Cannot load ACP or unsupported"),
        "{error:#}"
    );
    assert_eq!(tokio::fs::read(&path).await?, ordinary_component());
    assert_eq!(
        tokio::fs::read(manager.storage.metadata_path("agent")).await?,
        old_metadata
    );
    assert_eq!(
        tokio::fs::read(manager.component_precompiled_path("agent")).await?,
        old_native
    );
    assert_eq!(manager.get_component_id_for_tool("run").await?, "agent");
    Ok(())
}

#[tokio::test]
async fn non_runnable_and_missing_artifacts_never_publish_cached_tools() -> Result<()> {
    for wat in [
        "(module)",
        "(component)",
        "(component (type (func)))",
        r#"(component (instance $client) (export "wassette:acp/client@7.0.0" (instance $client)))"#,
    ] {
        let root = tempfile::tempdir()?;
        let manager = LifecycleManager::new_unloaded(root.path()).await?;
        let path = manager.component_path("test");
        tokio::fs::write(&path, wat::parse_str(wat)?).await?;
        cache_metadata(&manager, "test").await?;
        assert!(manager.get_component_schema("test").await.is_none());
        manager.populate_registry_from_metadata().await?;
        assert!(manager.list_tools().await.is_empty(), "{wat}");
        assert!(manager.ensure_component_loaded("test").await.is_err());
        tokio::fs::remove_file(path).await?;
        assert!(manager.get_component_schema("test").await.is_none());
        manager.populate_registry_from_metadata().await?;
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
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../components")
            .join(name)
            .join("target/wasm32-wasip2/release")
            .join(format!("{}.wasm", name.replace('-', "_")));
        let bytes = tokio::fs::read(&path)
            .await
            .context("Build ACP fixtures with `just build-acp-examples`")?;
        assert_eq!(inspect_artifact(&bytes)?.shape, expected);
        let root = tempfile::tempdir()?;
        let manager = LifecycleManager::new_unloaded(root.path()).await?;
        let error = manager
            .load_component(&format!("file://{}", path.display()))
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
    let mut bytes = tokio::fs::read(path).await?;
    // Use the source-built async fixture under a different interface namespace.
    // Equal-length names keep the binary framing intact; the async engine below
    // verifies that this remains a valid component rather than malformed input.
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
    let root = tempfile::tempdir()?;
    let manager = LifecycleManager::new_unloaded(root.path()).await?;
    tokio::fs::write(manager.component_path("async-tool"), bytes).await?;
    let error = manager
        .ensure_component_loaded("async-tool")
        .await
        .unwrap_err();
    let diagnostic = format!("{error:#}");
    assert!(
        diagnostic.contains("Failed to compile component")
            || diagnostic.contains("failed to instantiate component"),
        "{diagnostic}"
    );
    assert!(manager.list_tools().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn cached_acp_identifiers_and_mismatched_keys_are_not_published() -> Result<()> {
    let root = tempfile::tempdir()?;
    let manager = LifecycleManager::new_unloaded(root.path()).await?;
    tokio::fs::write(manager.component_path("ordinary"), ordinary_component()).await?;
    cache_metadata(&manager, "ordinary").await?;
    let metadata = manager.storage.read_metadata("ordinary").await?.unwrap();
    let mut acp = metadata.clone();
    acp.function_identifiers[0].package_name = Some("wassette:acp".to_owned());
    acp.function_identifiers[0].interface_name = Some("agent".to_owned());
    manager.storage.write_metadata(&acp).await?;
    assert!(manager.get_component_schema("ordinary").await.is_none());
    manager.populate_registry_from_metadata().await?;
    assert!(manager.list_tools().await.is_empty());
    let mut mismatched = metadata.clone();
    mismatched.component_id = "different".to_owned();
    tokio::fs::write(
        manager.storage.metadata_path("ordinary"),
        serde_json::to_vec(&mismatched)?,
    )
    .await?;
    assert!(manager.get_component_schema("ordinary").await.is_none());
    manager.populate_registry_from_metadata().await?;
    assert!(manager.list_tools().await.is_empty());
    assert!(!is_acp_identifier(&FunctionIdentifier {
        package_name: None,
        interface_name: None,
        function_name: "wassette-acp-agent".to_owned(),
    }));
    manager.storage.write_metadata(&metadata).await?;
    manager.populate_registry_from_metadata().await?;
    assert_eq!(manager.get_component_id_for_tool("run").await?, "ordinary");
    Ok(())
}
