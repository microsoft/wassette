// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::path::Path;

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::{oneshot, Semaphore};
use tokio::time::timeout;

use super::*;
use crate::store::{
    CacheSnapshot, InstallOptions, PreparedInstall, PreparedPolicy, ValidationEvidence,
};

const ALPHA: &str = "example:catalog-alpha/tool";
const BETA: &str = "example:catalog-beta/tool";
const ALIAS: &str = "math_run";
const VERSIONED: &str = "example:math/ops@1.0.0-a.b+c";
const COLLIDING: &str = "example:math/ops@1.0.0-a+b.c";
const DEADLINE: Duration = Duration::from_secs(10);
const PROCESS_DEADLINE: Duration = Duration::from_secs(30);
const OLD_SECRET: &str = "old-private-token";
const NEW_SECRET: &str = "new-private-token";

fn directory() -> Result<tempfile::TempDir> {
    Ok(tempfile::Builder::new()
        .prefix(".catalog-test-")
        .tempdir_in(std::env::current_dir()?)?)
}

async fn manager(root: &Path) -> Result<LifecycleManager> {
    LifecycleManager::builder(root.join("store"))
        .with_secrets_dir(root.join("secrets"))
        .with_environment_var("CATALOG_SETTING", "configured-setting")
        .with_eager_loading(false)
        .build()
        .await
}

fn component(name: &str, interfaces: &[(&str, &str, u32)]) -> Result<Vec<u8>> {
    let mut exports = String::new();
    for (index, (interface, result_type, value)) in interfaces.iter().enumerate() {
        exports.push_str(&format!(
            r#"
            (core module $m{index}
                (func (export "run") (result i32) i32.const {value}))
            (core instance $i{index} (instantiate $m{index}))
            (func $f{index} (result {result_type})
                (canon lift (core func $i{index} "run")))
            (instance $e{index} (export "run" (func $f{index})))
            (export "{interface}" (instance $e{index}))
            "#
        ));
    }
    Ok(wat::parse_str(format!("(component ${name} {exports})"))?)
}

async fn install_bytes(
    manager: &LifecycleManager,
    root: &Path,
    file_stem: &str,
    bytes: &[u8],
) -> Result<ComponentId> {
    let id = inspect_artifact(bytes)?.identity?;
    let path = root.join(format!("{file_stem}.wasm"));
    tokio::fs::write(&path, bytes).await?;
    let outcome = manager
        .load_component(&format!("file://{}", path.display()))
        .await?;
    assert_eq!(outcome.component_id, id.as_str());
    assert_ne!(id.as_str(), file_stem);
    Ok(id)
}

async fn install(
    manager: &LifecycleManager,
    root: &Path,
    result_type: &str,
    value: u32,
) -> Result<ComponentId> {
    install_bytes(
        manager,
        root,
        "portable-alpha",
        &component(ALPHA, &[("math", result_type, value)])?,
    )
    .await
}

async fn only_tool(manager: &LifecycleManager) -> Result<ToolDescriptor> {
    let mut catalog = manager.catalog().await?;
    assert_eq!(catalog.tools.len(), 1);
    Ok(catalog.tools.remove(0))
}

fn returned(output: &ToolOutput) -> Result<Value> {
    Ok(serde_json::from_str(&output.raw_result)?)
}

async fn invoke(manager: &LifecycleManager, tool: &ToolDescriptor) -> Result<ToolOutput> {
    Ok(manager
        .prepare_invocation(&tool.reference, &json!({}))
        .await?
        .run()
        .await?)
}

async fn publish_fixture_cache(
    manager: &LifecycleManager,
    receipt: &InstallReceipt,
    cache: &CacheSnapshot,
    metadata: Value,
) -> Result<()> {
    let id = receipt.component_id.as_str().to_owned();
    let revision = receipt.revision.clone();
    let cache = PreparedCache {
        artifact_sha256: receipt.artifact_sha256.clone(),
        engine: manager.cache_engine(),
        schema: CACHE_SCHEMA.to_owned(),
        metadata,
        native: cache.native.clone(),
    };
    store_operation(&manager.store, move |store| {
        Ok(store.publish_cache(&id, &revision, cache)?)
    })
    .await
}

#[tokio::test]
async fn independent_managers_refresh_install_replace_policy_and_remove() -> Result<()> {
    let root = directory()?;
    let writer = manager(root.path()).await?;
    let reader = manager(root.path()).await?;
    let clone = reader.clone();
    let empty = reader.catalog().await?;
    assert!(empty.tools.is_empty());
    assert!(Arc::ptr_eq(&reader.catalog_runtime, &clone.catalog_runtime));
    assert_eq!(clone.catalog().await?.generation, empty.generation);
    let initial = reader.refresh_from_store().await?;
    assert!(!initial.changed);
    assert_eq!(initial.generation, empty.generation);

    install(&writer, root.path(), "u32", 7).await?;
    let installed = reader.refresh_from_store().await?;
    assert!(installed.changed);
    assert!(installed.diagnostics.is_empty());
    assert_ne!(installed.generation, empty.generation);
    let first = only_tool(&reader).await?;
    assert_eq!(first.reference.key().component_id.as_str(), ALPHA);
    assert_eq!(
        returned(&invoke(&clone, &first).await?)?,
        json!({"result": 7})
    );
    assert_eq!(clone.catalog().await?.generation, installed.generation);
    assert!(!clone.refresh_from_store().await?.changed);

    let cold = manager(root.path()).await?;
    let hydrated = cold.refresh_from_store().await?;
    assert!(hydrated.changed);
    assert_eq!(hydrated.cursor, installed.cursor);
    assert_ne!(hydrated.generation, installed.generation);
    assert_eq!(only_tool(&cold).await?, first);
    assert!(cold.get_component(ALPHA).await.is_none());
    let writer_report = writer.refresh_from_store().await?;
    assert_eq!(writer_report.cursor, hydrated.cursor);
    assert_ne!(writer_report.generation, hydrated.generation);

    install(&writer, root.path(), "bool", 1).await?;
    let replaced = clone.refresh_from_store().await?;
    assert!(replaced.changed);
    assert_ne!(replaced.cursor, installed.cursor);
    let second = only_tool(&reader).await?;
    assert_eq!(second.tool.key, first.tool.key);
    assert_ne!(second.reference.revision(), first.reference.revision());
    assert_ne!(second.tool.schema, first.tool.schema);
    assert_eq!(
        returned(&invoke(&reader, &second).await?)?,
        json!({"result": true})
    );

    writer
        .grant_permission(ALPHA, "network", &json!({"host": "allowed.example"}))
        .await?;
    let policy = reader.refresh_from_store().await?;
    assert!(policy.changed);
    let third = only_tool(&clone).await?;
    assert_eq!(third.tool, second.tool);
    assert_ne!(third.reference.revision(), second.reference.revision());
    assert_eq!(
        returned(&invoke(&reader, &third).await?)?,
        json!({"result": true})
    );

    writer.unload_component(ALPHA).await?;
    let removed = clone.refresh_from_store().await?;
    assert!(removed.changed);
    assert!(reader.catalog().await?.tools.is_empty());
    assert!(reader.get_component(ALPHA).await.is_none());
    assert!(matches!(
        reader
            .prepare_invocation(&third.reference, &json!({}))
            .await,
        Err(ToolInvocationError::Stale(_))
    ));
    assert!(!reader.refresh_from_store().await?.changed);
    Ok(())
}

#[tokio::test]
async fn remove_and_reinstall_identical_bytes_cannot_revive_a_reference() -> Result<()> {
    let root = directory()?;
    let writer = manager(root.path()).await?;
    let reader = manager(root.path()).await?;
    install(&writer, root.path(), "u32", 7).await?;
    let first = only_tool(&reader).await?;
    let prepared = reader
        .prepare_invocation(&first.reference, &json!({}))
        .await?;
    let before = writer.component_store().read(ALPHA)?;
    writer.unload_component(ALPHA).await?;
    install(&writer, root.path(), "u32", 7).await?;
    let after = writer.component_store().read(ALPHA)?;
    assert_eq!(before.wasm, after.wasm);
    assert_eq!(
        before.receipt.artifact_sha256,
        after.receipt.artifact_sha256
    );
    assert_ne!(before.receipt.revision, after.receipt.revision);
    assert!(matches!(
        prepared.run().await,
        Err(ToolInvocationError::Stale(_))
    ));
    assert!(matches!(
        reader
            .prepare_invocation(&first.reference, &json!({}))
            .await,
        Err(ToolInvocationError::Stale(_))
    ));
    let current = only_tool(&reader).await?;
    assert_eq!(current.tool, first.tool);
    assert_ne!(current.reference, first.reference);
    assert_eq!(
        returned(&invoke(&reader, &current).await?)?,
        json!({"result": 7})
    );
    Ok(())
}

#[tokio::test]
async fn recreated_store_epoch_cannot_revive_a_reference() -> Result<()> {
    let root = directory()?;
    let original = manager(root.path()).await?;
    install(&original, root.path(), "u32", 7).await?;
    let old = only_tool(&original).await?;
    let prepared = original
        .prepare_invocation(&old.reference, &json!({}))
        .await?;
    let old_receipt = original.component_store().read(ALPHA)?.receipt;
    let old_generation = original.catalog().await?.generation;

    // Both directories belong to this fixture; no shared store files are deleted.
    tokio::fs::rename(root.path().join("store"), root.path().join("retired-store")).await?;
    let recreated = manager(root.path()).await?;
    install(&recreated, root.path(), "u32", 7).await?;
    let new_receipt = recreated.component_store().read(ALPHA)?.receipt;
    assert_eq!(old_receipt.artifact_sha256, new_receipt.artifact_sha256);
    assert_ne!(old_receipt.revision, new_receipt.revision);
    assert!(matches!(
        prepared.run().await,
        Err(ToolInvocationError::Stale(_))
    ));
    let report = original.resnapshot_from_store().await?;
    assert!(report.changed);
    assert_ne!(report.generation, old_generation);
    let current = only_tool(&original).await?;
    assert_eq!(current.tool, old.tool);
    assert_ne!(current.reference, old.reference);
    assert_eq!(
        returned(&invoke(&original, &current).await?)?,
        json!({"result": 7})
    );
    Ok(())
}

#[tokio::test]
async fn preparation_does_not_admit_calls_across_code_or_policy_mutations() -> Result<()> {
    for change_code in [true, false] {
        let root = directory()?;
        let writer = manager(root.path()).await?;
        let reader = manager(root.path()).await?;
        install(&writer, root.path(), "u32", 7).await?;
        let old = only_tool(&reader).await?;
        let prepared = reader
            .prepare_invocation(&old.reference, &json!({}))
            .await?;
        if change_code {
            install(&writer, root.path(), "bool", 1).await?;
        } else {
            writer
                .grant_permission(ALPHA, "network", &json!({"host": "changed.example"}))
                .await?;
        }
        assert!(matches!(
            prepared.run().await,
            Err(ToolInvocationError::Stale(_))
        ));
        assert!(matches!(
            reader.prepare_invocation(&old.reference, &json!({})).await,
            Err(ToolInvocationError::Stale(_))
        ));
        let error = reader.describe_tool(&old.reference).await.unwrap_err();
        assert!(matches!(
            error.downcast_ref::<ToolInvocationError>(),
            Some(ToolInvocationError::Stale(_))
        ));
        let current = only_tool(&reader).await?;
        assert_ne!(current.reference, old.reference);
        assert_eq!(
            returned(&invoke(&reader, &current).await?)?,
            if change_code {
                json!({"result": true})
            } else {
                json!({"result": 7})
            }
        );
    }
    Ok(())
}

#[tokio::test]
async fn admitted_call_retains_code_schema_policy_and_private_configuration() -> Result<()> {
    let root = directory()?;
    let writer = manager(root.path()).await?;
    let reader = manager(root.path()).await?;
    install(&writer, root.path(), "u32", 7).await?;
    writer
        .grant_permission(ALPHA, "network", &json!({"host": "old.example"}))
        .await?;
    writer
        .grant_permission(ALPHA, "environment", &json!({"key": "CATALOG_SETTING"}))
        .await?;
    writer
        .set_component_secrets(ALPHA, &[("CATALOG_TOKEN".into(), OLD_SECRET.into())])
        .await?;
    let old = only_tool(&reader).await?;
    let prepared = reader
        .prepare_invocation(&old.reference, &json!({}))
        .await?;
    prepared.admit().await?;
    let captured = prepared.component.policy_template.clone();
    let captured_policy = prepared.component.effective_policy.clone();
    assert!(captured.allowed_hosts.contains("old.example"));
    assert!(captured
        .config_vars
        .get("CATALOG_SETTING")
        .is_some_and(|v| v == "configured-setting"));
    assert!(captured
        .config_vars
        .get("CATALOG_TOKEN")
        .is_some_and(|v| v == OLD_SECRET));
    assert!(!format!("{prepared:?}").contains(OLD_SECRET));

    install(&writer, root.path(), "bool", 1).await?;
    writer
        .grant_permission(ALPHA, "network", &json!({"host": "new.example"}))
        .await?;
    writer
        .set_component_secrets(ALPHA, &[("CATALOG_TOKEN".into(), NEW_SECRET.into())])
        .await?;
    reader.refresh_from_store().await?;
    assert!(captured.allowed_hosts.contains("old.example"));
    assert!(!captured.allowed_hosts.contains("new.example"));
    assert!(captured
        .config_vars
        .get("CATALOG_TOKEN")
        .is_some_and(|v| v == OLD_SECRET));
    assert!(prepared.component.effective_policy == captured_policy);
    let output = prepared.execute_admitted().await?;
    assert_eq!(output.descriptor, old);
    assert_eq!(returned(&output)?, json!({"result": 7}));

    let current = only_tool(&reader).await?;
    let next = reader
        .prepare_invocation(&current.reference, &json!({}))
        .await?;
    assert!(next
        .component
        .policy_template
        .allowed_hosts
        .contains("new.example"));
    assert!(next
        .component
        .policy_template
        .config_vars
        .get("CATALOG_TOKEN")
        .is_some_and(|v| v == NEW_SECRET));
    assert!(!format!("{next:?}").contains(NEW_SECRET));
    let output = next.run().await?;
    assert_eq!(output.descriptor, current);
    assert_ne!(output.descriptor.tool.schema, old.tool.schema);
    assert_eq!(returned(&output)?, json!({"result": true}));
    Ok(())
}

#[tokio::test]
async fn valid_cache_hydration_and_policy_refresh_reuse_compiled_code() -> Result<()> {
    let root = directory()?;
    let writer = manager(root.path()).await?;
    install(&writer, root.path(), "u32", 7).await?;
    let expected = only_tool(&writer).await?;
    let reader = manager(root.path()).await?;
    assert_eq!(only_tool(&reader).await?, expected);
    assert!(reader.get_component(ALPHA).await.is_none());
    invoke(&reader, &expected).await?;
    let loaded = reader
        .get_component(ALPHA)
        .await
        .context("Loaded fixture missing")?;
    writer
        .grant_permission(ALPHA, "network", &json!({"host": "reused.example"}))
        .await?;
    assert!(reader.refresh_from_store().await?.changed);
    let refreshed = reader
        .get_component(ALPHA)
        .await
        .context("Refreshed fixture missing")?;
    assert!(Arc::ptr_eq(&loaded.component, &refreshed.component));
    assert!(Arc::ptr_eq(&loaded.instance_pre, &refreshed.instance_pre));
    assert_ne!(loaded.revision, refreshed.revision);
    assert!(loaded.policy_template.allowed_hosts.is_empty());
    assert!(refreshed
        .policy_template
        .allowed_hosts
        .contains("reused.example"));
    let current = only_tool(&reader).await?;
    assert_eq!(current.tool, expected.tool);
    assert_ne!(current.reference, expected.reference);
    assert_eq!(
        returned(&invoke(&reader, &current).await?)?,
        json!({"result": 7})
    );
    Ok(())
}

#[tokio::test]
async fn missing_invalid_and_hashless_metadata_fall_back_for_catalog_and_name_calls() -> Result<()>
{
    for damage in ["missing", "invalid", "hashless"] {
        for name_first in [false, true] {
            let root = directory()?;
            let writer = manager(root.path()).await?;
            install(&writer, root.path(), "u32", 7).await?;
            let expected = only_tool(&writer).await?;
            let receipt = writer.component_store().read(ALPHA)?.receipt;
            let metadata = writer.storage.metadata_path(&receipt.storage_key);
            match damage {
                "missing" => tokio::fs::remove_file(&metadata).await?,
                "invalid" => tokio::fs::write(&metadata, b"{invalid").await?,
                "hashless" => {
                    let mut envelope: Value =
                        serde_json::from_slice(&tokio::fs::read(&metadata).await?)?;
                    envelope["metadata"]["validation_stamp"]["content_hash"] = Value::Null;
                    tokio::fs::write(&metadata, serde_json::to_vec(&envelope)?).await?;
                }
                _ => unreachable!(),
            }
            let reader = manager(root.path()).await?;
            assert!(reader.get_component(ALPHA).await.is_none());
            if name_first {
                let output = reader.invoke_unique_tool(ALIAS, &json!({})).await?;
                assert_eq!(output.descriptor, expected.tool);
                assert_eq!(
                    serde_json::from_str::<Value>(&output.raw_result)?,
                    json!({"result": 7})
                );
            }
            assert_eq!(only_tool(&reader).await?, expected);
            assert!(reader.get_component(ALPHA).await.is_some());
            assert_eq!(
                returned(&invoke(&reader, &expected).await?)?,
                json!({"result": 7})
            );
            assert!(reader
                .component_store()
                .read_cache(
                    ALPHA,
                    &receipt.revision,
                    &reader.cache_engine(),
                    CACHE_SCHEMA
                )?
                .is_some());
            assert!(!reader.refresh_from_store().await?.changed);
        }
    }
    Ok(())
}

fn replace_through_store(manager: &LifecycleManager, bytes: Vec<u8>) -> Result<InstallReceipt> {
    let snapshot = manager.component_store().read(ALPHA)?;
    let receipt = snapshot.receipt;
    let expected =
        manager
            .component_store()
            .observe(ALPHA, &receipt.storage_key, &receipt.source)?;
    let policy = match snapshot.policy {
        Some(bytes) => PreparedPolicy::parse(bytes, receipt.policy.provenance)?,
        None => PreparedPolicy::absent(receipt.policy.provenance),
    }
    .with_metadata(receipt.policy.metadata)?;
    let prepared = PreparedInstall::prepare(
        bytes,
        InstallOptions {
            storage_key: receipt.storage_key,
            source: receipt.source,
            origin: receipt.origin,
            owner: receipt.owner,
            policy,
            observation: receipt.observation,
        },
        |bytes, inspection, _| {
            let component = wasmtime::component::Component::new(manager.runtime.as_ref(), bytes)?;
            if inspection.shape == ArtifactShape::ToolCandidate {
                manager.prepare_component_instance(component, bytes)?;
                Ok(ValidationEvidence::OrdinaryPrepared {
                    runtime: manager.cache_engine(),
                })
            } else {
                assert_eq!(inspection.shape, ArtifactShape::AcpProvider);
                assert_eq!(inspection.acp_exports, ["wassette:acp/agent@0.1.0"]);
                Ok(ValidationEvidence::AcpCompiledAndExportChecked {
                    runtime: manager.cache_engine(),
                })
            }
        },
    )?;
    let committed = manager
        .component_store()
        .commit_install(prepared, expected)?;
    let StoredEntry::Installed(receipt) = committed.entry else {
        bail!("Expected installed receipt");
    };
    Ok(receipt)
}

#[tokio::test]
async fn refresh_excludes_acp_after_tool_replacement() -> Result<()> {
    {
        let root = directory()?;
        let writer = manager(root.path()).await?;
        let reader = manager(root.path()).await?;
        install(&writer, root.path(), "u32", 7).await?;
        let old = only_tool(&reader).await?;
        let prepared = reader
            .prepare_invocation(&old.reference, &json!({}))
            .await?;
        let previous = writer.component_store().read(ALPHA)?;
        let engine = writer.cache_engine();
        let cache = writer
            .component_store()
            .read_cache(ALPHA, &previous.receipt.revision, &engine, CACHE_SCHEMA)?
            .context("Fixture cache missing")?;
        let bytes = wat::parse_str(format!(
            r#"(component ${ALPHA}
                (instance $agent)
                (export "wassette:acp/agent@0.1.0" (instance $agent)))"#
        ))?;
        let receipt = replace_through_store(&writer, bytes)?;
        writer.component_store().publish_cache(
            ALPHA,
            &receipt.revision,
            PreparedCache {
                artifact_sha256: receipt.artifact_sha256,
                engine,
                schema: CACHE_SCHEMA.to_owned(),
                metadata: cache.metadata,
                native: cache.native,
            },
        )?;
        assert!(reader.get_component(ALPHA).await.is_some());
        assert!(writer.get_component(ALPHA).await.is_some());
        assert!(matches!(
            prepared.run().await,
            Err(ToolInvocationError::Stale(_))
        ));
        for target in [&reader, &writer] {
            assert!(target.refresh_from_store().await?.changed);
            assert!(target.catalog().await?.tools.is_empty());
            assert!(target.get_component(ALPHA).await.is_none());
            assert!(target.list_tool_descriptors().await?.is_empty());
            assert!(target.invoke_unique_tool(ALIAS, &json!({})).await.is_err());
            assert!(matches!(
                target.prepare_invocation(&old.reference, &json!({})).await,
                Err(ToolInvocationError::Stale(_))
            ));
        }
        let cold = manager(root.path()).await?;
        assert!(cold.catalog().await?.tools.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn complete_batches_preserve_component_and_exact_export_alias_collisions() -> Result<()> {
    let root = directory()?;
    let writer = manager(root.path()).await?;
    let reader = manager(root.path()).await?;
    let empty = reader.catalog().await?;
    install_bytes(
        &writer,
        root.path(),
        "portable-alpha",
        &component(ALPHA, &[(VERSIONED, "u32", 7), (COLLIDING, "bool", 1)])?,
    )
    .await?;
    install_bytes(
        &writer,
        root.path(),
        "portable-beta",
        &component(BETA, &[(VERSIONED, "u32", 11)])?,
    )
    .await?;
    let batch = reader.catalog().await?;
    assert_ne!(batch.generation, empty.generation);
    assert_eq!(batch.tools.len(), 3);
    let alias = batch.tools[0].tool.schema["name"]
        .as_str()
        .context("Missing alias")?;
    assert!(batch
        .tools
        .iter()
        .all(|tool| tool.tool.schema["name"] == alias));
    assert!(matches!(
        resolve_name(&batch.tools, None, alias),
        Err(ToolLookupError::Ambiguous { .. })
    ));
    assert!(matches!(
        resolve_name(&batch.tools, Some(ALPHA), alias),
        Err(ToolLookupError::Ambiguous { .. })
    ));
    assert_eq!(
        resolve_name(&batch.tools, Some(BETA), alias)?
            .tool
            .key
            .component_id
            .as_str(),
        BETA
    );
    let selected = resolve_name(&batch.tools, Some(BETA), alias)?;
    require_name_reference(&batch.tools, Some(BETA), alias, &selected.reference)?;
    let rebound = batch
        .tools
        .iter()
        .find(|tool| tool.tool.key.component_id.as_str() == ALPHA)
        .context("Missing alternative binding")?;
    for error in [
        require_name_reference(&[], None, alias, &selected.reference).unwrap_err(),
        require_name_reference(&batch.tools, None, alias, &selected.reference).unwrap_err(),
        require_name_reference(&batch.tools, Some(ALPHA), alias, &selected.reference).unwrap_err(),
        require_name_reference(
            std::slice::from_ref(rebound),
            None,
            alias,
            &selected.reference,
        )
        .unwrap_err(),
    ] {
        assert!(matches!(&error, ToolInvocationError::Stale(_)));
        assert!(matches!(
            invocation_error(anyhow::Error::new(error).context("Final name admission failed")),
            ToolInvocationError::Stale(_)
        ));
    }
    for tool in &batch.tools {
        let expected = if tool.tool.key.component_id.as_str() == BETA {
            json!({"result": 11})
        } else if tool.tool.key.export.interface_name.as_deref() == Some(VERSIONED) {
            json!({"result": 7})
        } else {
            json!({"result": true})
        };
        let output = invoke(&reader, tool).await?;
        assert_eq!(&output.descriptor, tool);
        assert_eq!(returned(&output)?, expected);
    }
    assert_eq!(reader.list_tool_descriptors().await?.len(), 3);
    let error = reader
        .invoke_unique_tool(alias, &json!({}))
        .await
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<ToolLookupError>(),
        Some(ToolLookupError::Ambiguous { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn invalid_arguments_traps_and_guest_errors_have_distinct_outcomes() -> Result<()> {
    let root = directory()?;
    let manager = manager(root.path()).await?;
    let trap = wat::parse_str(format!(
        r#"(component ${ALPHA}
            (core module $m (func (export "run") (param i32) (result i32) unreachable))
            (core instance $i (instantiate $m))
            (func (export "run") (param "value" u32) (result u32)
                (canon lift (core func $i "run"))))"#
    ))?;
    install_bytes(&manager, root.path(), "portable-alpha", &trap).await?;
    let tool = only_tool(&manager).await?;
    for arguments in [json!([]), json!({}), json!({"value": "not-a-number"})] {
        assert!(matches!(
            manager
                .prepare_invocation(&tool.reference, &arguments)
                .await,
            Err(ToolInvocationError::InvalidArguments(_))
        ));
    }
    let prepared = manager
        .prepare_invocation(&tool.reference, &json!({"value": 1}))
        .await?;
    assert!(matches!(
        prepared.run().await,
        Err(ToolInvocationError::ExecutionFailed(_))
    ));

    let guest_error = wat::parse_str(format!(
        r#"(component ${ALPHA}
            (core module $m (func (export "run") (result i32) i32.const 1))
            (core instance $i (instantiate $m))
            (type $out (result))
            (func (export "run") (result $out)
                (canon lift (core func $i "run"))))"#
    ))?;
    install_bytes(&manager, root.path(), "portable-alpha", &guest_error).await?;
    let tool = only_tool(&manager).await?;
    let output = invoke(&manager, &tool).await?;
    assert_eq!(returned(&output)?, json!({"result": {"err": null}}));
    assert_eq!(
        output.raw_result,
        manager.execute_component_call(ALPHA, "run", "{}").await?
    );
    Ok(())
}

#[tokio::test]
async fn unavailable_artifacts_are_not_masked_by_resident_code_and_can_recover() -> Result<()> {
    let root = directory()?;
    let manager = manager(root.path()).await?;
    install(&manager, root.path(), "u32", 7).await?;
    let tool = only_tool(&manager).await?;
    let before = manager.refresh_from_store().await?;
    let artifact = manager.component_store().read(ALPHA)?;
    let path = manager
        .storage
        .component_path(&artifact.receipt.storage_key);
    tokio::fs::write(&path, b"invalid-artifact").await?;
    assert!(matches!(
        manager
            .prepare_invocation(&tool.reference, &json!({}))
            .await,
        Err(ToolInvocationError::Unavailable(_))
    ));
    assert!(!manager.refresh_from_store().await?.changed);
    let error = manager.catalog().await.unwrap_err();
    let failed = &error
        .downcast_ref::<CatalogRefreshError>()
        .context("Missing refresh report")?
        .report;
    assert!(failed.changed);
    assert_eq!(failed.cursor, before.cursor);
    assert_ne!(failed.generation, before.generation);
    assert_eq!(failed.diagnostics.len(), 1);
    assert_eq!(
        failed.diagnostics[0].component_id.as_ref(),
        Some(&tool.tool.key.component_id)
    );
    assert!(!format!("{:?}", failed.diagnostics).contains("invalid-artifact"));
    assert!(manager.catalog_runtime.state.read().await.tools.is_empty());
    assert_eq!(
        manager.catalog_runtime.state.read().await.cursor.as_ref(),
        Some(&before.cursor)
    );
    assert!(manager.catalog().await.is_err());

    tokio::fs::write(&path, &artifact.wasm).await?;
    let recovered = manager.resnapshot_from_store().await?;
    assert!(recovered.changed);
    assert!(recovered.diagnostics.is_empty());
    assert_eq!(only_tool(&manager).await?, tool);
    assert_eq!(
        returned(&invoke(&manager, &tool).await?)?,
        json!({"result": 7})
    );
    Ok(())
}

#[tokio::test]
async fn wait_changed_has_no_lost_wakeup_and_recognizes_foreign_generations() -> Result<()> {
    let root = directory()?;
    let writer = manager(root.path()).await?;
    let reader = manager(root.path()).await?;
    let original = reader.catalog().await?;
    let foreign = writer.catalog().await?;
    assert_ne!(foreign.generation, original.generation);
    assert_eq!(
        timeout(DEADLINE, reader.wait_changed(&foreign.generation)).await??,
        original.generation
    );
    let mut waiting = Box::pin(reader.wait_changed(&original.generation));
    assert!(futures::poll!(waiting.as_mut()).is_pending());
    install(&writer, root.path(), "u32", 7).await?;
    assert!(futures::poll!(waiting.as_mut()).is_pending());
    let changed = reader.refresh_from_store().await?;
    assert_eq!(timeout(DEADLINE, waiting).await??, changed.generation);
    assert_eq!(
        timeout(DEADLINE, reader.wait_changed(&original.generation)).await??,
        changed.generation
    );
    let mut waiting = Box::pin(reader.wait_changed(&changed.generation));
    assert!(!reader.resnapshot_from_store().await?.changed);
    assert!(futures::poll!(waiting.as_mut()).is_pending());
    Ok(())
}

#[tokio::test]
async fn caller_owned_driver_observes_idle_updates_and_shuts_down_deterministically() -> Result<()>
{
    let root = directory()?;
    let writer = manager(root.path()).await?;
    let reader = manager(root.path()).await?;
    let original = reader.catalog().await?;
    assert!(reader
        .run_refresh_driver(Duration::ZERO, CancellationToken::new())
        .await
        .is_err());
    let shutdown = CancellationToken::new();
    let driver_manager = reader.clone();
    let driver_shutdown = shutdown.clone();
    let driver = tokio::spawn(async move {
        driver_manager
            .run_refresh_driver(Duration::from_millis(10), driver_shutdown)
            .await
    });
    install(&writer, root.path(), "u32", 7).await?;
    let generation = timeout(DEADLINE, reader.wait_changed(&original.generation)).await??;
    assert_ne!(generation, original.generation);
    shutdown.cancel();
    timeout(DEADLINE, driver)
        .await??
        .context("Refresh driver failed while observing the external installation")?;
    let observed = reader.catalog_runtime.state.read().await.clone();
    assert_eq!(observed.generation, generation);
    assert!(observed.diagnostics.is_empty());
    assert_eq!(observed.tools.len(), 1);

    install(&writer, root.path(), "bool", 1).await?;
    let mut stopped = Box::pin(reader.wait_changed(&generation));
    assert!(futures::poll!(stopped.as_mut()).is_pending());
    let already_cancelled = CancellationToken::new();
    already_cancelled.cancel();
    timeout(
        DEADLINE,
        reader.run_refresh_driver(Duration::from_millis(10), already_cancelled),
    )
    .await??;
    assert!(futures::poll!(stopped.as_mut()).is_pending());
    let refreshed = reader.refresh_from_store().await?;
    assert_eq!(timeout(DEADLINE, stopped).await??, refreshed.generation);
    Ok(())
}

#[tokio::test]
async fn idle_refresh_child() -> Result<()> {
    let Some(root) = std::env::var_os("WASSETTE_CATALOG_CHILD_ROOT") else {
        return Ok(());
    };
    let address = std::env::var("WASSETTE_CATALOG_CHILD_ADDRESS")?;
    timeout(PROCESS_DEADLINE, async {
        let writer = manager(Path::new(&root)).await?;
        let mut connection = TcpStream::connect(address).await?;
        connection.write_all(b"R").await?;
        ensure!(
            connection.read_u8().await? == b'C',
            "Missing commit request"
        );
        install(&writer, Path::new(&root), "u32", 73).await?;
        connection.write_all(b"D").await?;
        ensure!(
            connection.read_u8().await? == b'A',
            "Parent did not acknowledge the observed commit"
        );
        Ok::<_, anyhow::Error>(())
    })
    .await?
}

#[tokio::test]
async fn caller_owned_driver_observes_an_idle_cross_process_commit() -> Result<()> {
    let root = directory()?;
    let reader = manager(root.path()).await?;
    let before = reader.catalog().await?;
    assert!(before.tools.is_empty());
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let mut child = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "tool_catalog::tests::idle_refresh_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("WASSETTE_CATALOG_CHILD_ROOT", root.path())
        .env(
            "WASSETTE_CATALOG_CHILD_ADDRESS",
            listener.local_addr()?.to_string(),
        )
        .kill_on_drop(true)
        .spawn()?;

    let result = timeout(PROCESS_DEADLINE, async {
        let (mut connection, _) = listener.accept().await?;
        ensure!(connection.read_u8().await? == b'R', "Child was not ready");
        let shutdown = CancellationToken::new();
        let observer_shutdown = shutdown.clone();
        let observation = async {
            let _shutdown = observer_shutdown.drop_guard();
            timeout(DEADLINE, async {
                let mut changed = Box::pin(reader.wait_changed(&before.generation));
                assert!(futures::poll!(changed.as_mut()).is_pending());
                connection.write_all(b"C").await?;
                ensure!(connection.read_u8().await? == b'D', "Child did not commit");
                let generation = changed.await?;
                let published = reader.catalog_runtime.state.read().await.clone();
                ensure!(
                    generation != before.generation && published.generation == generation,
                    "External commit did not produce the observed local generation"
                );
                ensure!(
                    published.diagnostics.is_empty() && published.tools.len() == 1,
                    "External commit did not publish an available tool"
                );
                connection.write_all(b"A").await?;
                Ok::<_, anyhow::Error>(published)
            })
            .await?
        };
        let (driver, observed) = tokio::join!(
            reader.run_refresh_driver(Duration::from_millis(10), shutdown),
            observation
        );
        driver.context("Refresh driver failed while observing the child process")?;
        let published = observed?;
        ensure!(child.wait().await?.success(), "Child process failed");
        let tool = &published.tools[0];
        assert_eq!(tool.reference.key().component_id.as_str(), ALPHA);
        assert_eq!(
            returned(&invoke(&reader, tool).await?)?,
            json!({"result": 73})
        );
        Ok::<_, anyhow::Error>(())
    })
    .await;

    if !matches!(&result, Ok(Ok(()))) && child.try_wait()?.is_none() {
        timeout(DEADLINE, child.kill()).await??;
    }
    result?
}

#[tokio::test]
async fn competing_cache_publication_retries_without_unavailability_or_generation_change(
) -> Result<()> {
    let root = directory()?;
    let writer = manager(root.path()).await?;
    install(&writer, root.path(), "u32", 7).await?;
    let reader = manager(root.path()).await?;
    let before = reader.catalog().await?;
    assert!(reader.get_component(ALPHA).await.is_none());
    let state_before = reader.catalog_runtime.state.read().await.clone();
    let receipt = writer.component_store().read(ALPHA)?.receipt;
    let cache = writer
        .component_store()
        .read_cache(
            ALPHA,
            &receipt.revision,
            &writer.cache_engine(),
            CACHE_SCHEMA,
        )?
        .context("Missing valid fixture cache")?;
    publish_fixture_cache(&writer, &receipt, &cache, Value::Null).await?;

    let (reached_tx, reached_rx) = oneshot::channel();
    let (resume_tx, resume_rx) = oneshot::channel();
    reader
        .component_store()
        .pause_next_cache_publication(reached_tx, resume_rx);
    let mut changed = Box::pin(reader.wait_changed(&before.generation));
    assert!(futures::poll!(changed.as_mut()).is_pending());
    let refreshing = reader.clone();
    let refresh = tokio::spawn(async move { refreshing.resnapshot_from_store().await });
    timeout(DEADLINE, reached_rx).await??;
    assert!(Arc::ptr_eq(
        &*reader.catalog_runtime.state.read().await,
        &state_before
    ));
    timeout(
        DEADLINE,
        publish_fixture_cache(&writer, &receipt, &cache, cache.metadata.clone()),
    )
    .await??;
    assert!(writer
        .component_store()
        .snapshot_if_changed(state_before.cursor.as_ref())?
        .is_none());
    resume_tx
        .send(())
        .map_err(|_| anyhow!("Cache refresh stopped before the competing commit"))?;

    let report = timeout(DEADLINE, refresh).await???;
    assert!(!report.changed);
    assert!(report.diagnostics.is_empty());
    assert_eq!(report.generation, before.generation);
    assert_eq!(Some(&report.cursor), state_before.cursor.as_ref());
    let published = reader.catalog_runtime.state.read().await.clone();
    assert_eq!(published.tools, before.tools);
    assert!(published.diagnostics.is_empty());
    assert!(reader.get_component(ALPHA).await.is_none());
    assert!(futures::poll!(changed.as_mut()).is_pending());
    let output = invoke(&reader, &before.tools[0]).await?;
    assert_eq!(returned(&output)?, json!({"result": 7}));
    Ok(())
}

#[tokio::test]
async fn eight_competing_cache_commits_exhaust_retries_without_publishing_unavailability(
) -> Result<()> {
    let root = directory()?;
    let writer = manager(root.path()).await?;
    install(&writer, root.path(), "u32", 7).await?;
    let reader = manager(root.path()).await?;
    let before = reader.catalog().await?;
    assert!(reader.get_component(ALPHA).await.is_none());
    let state_before = reader.catalog_runtime.state.read().await.clone();
    let receipt = writer.component_store().read(ALPHA)?.receipt;
    let cache = writer
        .component_store()
        .read_cache(
            ALPHA,
            &receipt.revision,
            &writer.cache_engine(),
            CACHE_SCHEMA,
        )?
        .context("Missing valid fixture cache")?;
    publish_fixture_cache(&writer, &receipt, &cache, Value::Null).await?;

    assert_eq!(PUBLICATION_ATTEMPTS, 8);
    let mut pauses = Vec::new();
    for _ in 0..PUBLICATION_ATTEMPTS {
        let (reached_tx, reached_rx) = oneshot::channel();
        let (resume_tx, resume_rx) = oneshot::channel();
        reader
            .component_store()
            .pause_next_cache_publication(reached_tx, resume_rx);
        pauses.push((reached_rx, resume_tx));
    }
    let mut changed = Box::pin(reader.wait_changed(&before.generation));
    assert!(futures::poll!(changed.as_mut()).is_pending());
    let refreshing = reader.clone();
    let refresh = tokio::spawn(async move { refreshing.resnapshot_from_store().await });
    for (reached, resume) in pauses {
        timeout(DEADLINE, reached).await??;
        assert!(Arc::ptr_eq(
            &*reader.catalog_runtime.state.read().await,
            &state_before
        ));
        assert!(futures::poll!(changed.as_mut()).is_pending());
        timeout(
            DEADLINE,
            publish_fixture_cache(&writer, &receipt, &cache, Value::Null),
        )
        .await??;
        assert!(writer
            .component_store()
            .snapshot_if_changed(state_before.cursor.as_ref())?
            .is_none());
        resume
            .send(())
            .map_err(|_| anyhow!("Cache refresh stopped before all competing commits"))?;
    }
    let error = timeout(DEADLINE, refresh).await??.unwrap_err();
    assert!(error.to_string().contains("kept changing"));
    assert!(error.to_string().contains("retry"));
    assert!(error.downcast_ref::<CatalogRefreshError>().is_none());
    let unchanged = reader.catalog_runtime.state.read().await.clone();
    assert!(Arc::ptr_eq(&unchanged, &state_before));
    assert_eq!(unchanged.generation, before.generation);
    assert_eq!(unchanged.tools, before.tools);
    assert!(unchanged.diagnostics.is_empty());
    assert!(futures::poll!(changed.as_mut()).is_pending());

    publish_fixture_cache(&writer, &receipt, &cache, cache.metadata.clone()).await?;
    let recovered = timeout(DEADLINE, reader.resnapshot_from_store()).await??;
    assert!(!recovered.changed);
    assert_eq!(recovered.generation, before.generation);
    assert!(recovered.diagnostics.is_empty());
    assert!(futures::poll!(changed.as_mut()).is_pending());
    let output = invoke(&reader, &before.tools[0]).await?;
    assert_eq!(returned(&output)?, json!({"result": 7}));
    Ok(())
}

#[tokio::test]
async fn forced_resnapshot_and_concurrent_calls_share_one_coherent_generation() -> Result<()> {
    let root = directory()?;
    let writer = manager(root.path()).await?;
    let reader = manager(root.path()).await?;
    let clone = reader.clone();
    let initial = reader.catalog().await?;
    install(&writer, root.path(), "u32", 7).await?;
    let (refresh, forced, first, second) = tokio::join!(
        reader.refresh_from_store(),
        clone.resnapshot_from_store(),
        reader.catalog(),
        clone.catalog()
    );
    let (refresh, forced, first, second) = (refresh?, forced?, first?, second?);
    assert_eq!(refresh.generation, forced.generation);
    assert_eq!(first.generation, second.generation);
    assert_eq!(first.generation, refresh.generation);
    assert_eq!(first.generation.sequence, initial.generation.sequence + 1);
    assert_eq!(refresh.cursor, forced.cursor);
    assert_eq!(first.tools, second.tools);
    assert_eq!(first.tools.len(), 1);
    let tool = &first.tools[0];
    let args = json!({});
    let (first, second) = tokio::join!(
        reader.prepare_invocation(&tool.reference, &args),
        clone.prepare_invocation(&tool.reference, &args)
    );
    let (first, second) = tokio::join!(first?.run(), second?.run());
    assert_eq!(returned(&first?)?, json!({"result": 7}));
    assert_eq!(returned(&second?)?, json!({"result": 7}));
    let noop = reader.resnapshot_from_store().await?;
    assert!(!noop.changed);
    assert_eq!(noop.generation, refresh.generation);
    Ok(())
}

#[tokio::test]
async fn stale_prepared_catalog_is_retried_before_any_publication() -> Result<()> {
    let root = directory()?;
    let writer = manager(root.path()).await?;
    let reader = manager(root.path()).await?;
    install(&writer, root.path(), "u32", 7).await?;
    let initial = reader.catalog().await?;
    install(&writer, root.path(), "bool", 1).await?;
    let (reached_tx, reached_rx) = oneshot::channel();
    let (resume_tx, resume_rx) = oneshot::channel();
    *reader.catalog_runtime.before_publish.lock().unwrap() = Some((reached_tx, resume_rx));
    let refreshing = reader.clone();
    let refresh = tokio::spawn(async move { refreshing.refresh_from_store().await });
    timeout(DEADLINE, reached_rx).await??;
    assert_eq!(
        reader.catalog_runtime.state.read().await.generation,
        initial.generation
    );
    install(&writer, root.path(), "u32", 42).await?;
    resume_tx
        .send(())
        .map_err(|_| anyhow!("Refresh stopped before resume"))?;
    let report = timeout(DEADLINE, refresh).await???;
    assert!(report.changed);
    assert_eq!(report.generation.sequence, initial.generation.sequence + 1);
    assert_eq!(
        report.cursor,
        writer
            .component_store()
            .snapshot_if_changed(None)?
            .context("Missing snapshot")?
            .cursor
    );
    let current = only_tool(&reader).await?;
    assert_eq!(current.tool.schema, initial.tools[0].tool.schema);
    assert_ne!(current.reference, initial.tools[0].reference);
    assert_eq!(
        returned(&invoke(&reader, &current).await?)?,
        json!({"result": 42})
    );
    Ok(())
}

#[tokio::test]
async fn same_cursor_metadata_refresh_preserves_a_concurrently_warmed_instance() -> Result<()> {
    let root = directory()?;
    let writer = manager(root.path()).await?;
    install(&writer, root.path(), "u32", 7).await?;
    let reader = manager(root.path()).await?;
    let before = reader.catalog().await?;
    assert_eq!(before.tools.len(), 1);
    assert!(reader.get_component(ALPHA).await.is_none());
    let (reached_tx, reached_rx) = oneshot::channel();
    let (resume_tx, resume_rx) = oneshot::channel();
    *reader.catalog_runtime.before_publish.lock().unwrap() = Some((reached_tx, resume_rx));
    let refreshing = reader.clone();
    let refresh = tokio::spawn(async move { refreshing.resnapshot_from_store().await });
    timeout(DEADLINE, reached_rx).await??;

    let prepared = reader
        .prepare_invocation(&before.tools[0].reference, &json!({}))
        .await?;
    let warmed = reader
        .get_component(ALPHA)
        .await
        .context("Concurrent invocation did not warm the fixture")?;
    resume_tx
        .send(())
        .map_err(|_| anyhow!("Refresh stopped before concurrent warming completed"))?;
    let report = timeout(DEADLINE, refresh).await???;
    assert!(!report.changed);
    assert_eq!(report.generation, before.generation);
    let retained = reader
        .get_component(ALPHA)
        .await
        .context("Metadata publication erased a concurrently warmed instance")?;
    assert!(Arc::ptr_eq(&retained.component, &warmed.component));
    assert!(Arc::ptr_eq(&retained.instance_pre, &warmed.instance_pre));
    assert_eq!(returned(&prepared.run().await?)?, json!({"result": 7}));
    let mut unchanged = Box::pin(reader.wait_changed(&before.generation));
    assert!(futures::poll!(unchanged.as_mut()).is_pending());
    Ok(())
}

#[test]
fn catalog_and_prepared_invocation_types_are_owned_send_static() {
    fn owned_send<T: Send + 'static>() {}
    owned_send::<CatalogGeneration>();
    owned_send::<CatalogSnapshot>();
    owned_send::<RefreshReport>();
    owned_send::<ToolRef>();
    owned_send::<ToolDescriptor>();
    owned_send::<PreparedInvocation>();
    owned_send::<ToolOutput>();
    owned_send::<ToolInvocationError>();
}

#[tokio::test]
async fn committed_mutation_workers_publish_after_their_callers_are_cancelled() -> Result<()> {
    for mutation in ["replace", "policy", "remove"] {
        let root = directory()?;
        let manager = manager(root.path()).await?;
        install(&manager, root.path(), "u32", 7).await?;
        let before = manager.catalog().await?;
        let (reached_tx, reached_rx) = oneshot::channel();
        let (resume_tx, resume_rx) = oneshot::channel();
        *manager.catalog_runtime.before_publish.lock().unwrap() = Some((reached_tx, resume_rx));
        let updating = manager.clone();
        let source_root = root.path().to_owned();
        let caller = tokio::spawn(async move {
            match mutation {
                "replace" => {
                    install(&updating, &source_root, "bool", 1).await?;
                }
                "policy" => {
                    updating
                        .grant_permission(
                            ALPHA,
                            "network",
                            &json!({"host": "cancelled-caller.example"}),
                        )
                        .await?;
                }
                "remove" => updating.unload_component(ALPHA).await?,
                _ => unreachable!(),
            }
            Ok::<_, anyhow::Error>(())
        });
        timeout(DEADLINE, reached_rx).await??;
        assert_eq!(
            manager.catalog_runtime.state.read().await.generation,
            before.generation
        );
        let committed = manager
            .component_store()
            .snapshot_if_changed(None)?
            .context("Missing committed store snapshot")?;
        caller.abort();
        assert!(timeout(DEADLINE, caller).await?.unwrap_err().is_cancelled());
        resume_tx
            .send(())
            .map_err(|_| anyhow!("Mutation worker stopped before publication"))?;

        let generation = timeout(DEADLINE, manager.wait_changed(&before.generation)).await??;
        assert_ne!(generation, before.generation);
        let published = manager.catalog_runtime.state.read().await.clone();
        assert_eq!(published.cursor.as_ref(), Some(&committed.cursor));
        assert!(published.diagnostics.is_empty());
        if mutation == "remove" {
            assert!(published.tools.is_empty());
            assert!(matches!(
                committed.entries.as_slice(),
                [StoredEntry::Retired(_)]
            ));
        } else {
            assert_eq!(published.tools.len(), 1);
            let tool = &published.tools[0];
            assert_ne!(tool.reference, before.tools[0].reference);
            let output = invoke(&manager, tool).await?;
            assert_eq!(
                returned(&output)?,
                if mutation == "replace" {
                    json!({"result": true})
                } else {
                    json!({"result": 7})
                }
            );
            if mutation == "policy" {
                assert!(manager
                    .get_component(ALPHA)
                    .await
                    .context("Policy-updated component missing")?
                    .policy_template
                    .allowed_hosts
                    .contains("cancelled-caller.example"));
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn supervisor_retains_permit_and_admitted_work_after_receiver_cancellation() -> Result<()> {
    let root = directory()?;
    let writer = manager(root.path()).await?;
    let reader = manager(root.path()).await?;
    install(&writer, root.path(), "u32", 7).await?;
    let old = only_tool(&reader).await?;
    let prepared = reader
        .prepare_invocation(&old.reference, &json!({}))
        .await?;
    let permits = Arc::new(Semaphore::new(1));
    let supervisor_permits = permits.clone();
    let (admitted_tx, admitted_rx) = oneshot::channel();
    let (execute_tx, execute_rx) = oneshot::channel();
    let (result_tx, result_rx) = oneshot::channel();
    let receiver = tokio::spawn(result_rx);
    let supervisor = tokio::spawn(async move {
        let permit = supervisor_permits.clone().acquire_owned().await?;
        prepared.admit().await?;
        admitted_tx
            .send(())
            .map_err(|_| anyhow!("Admission receiver dropped"))?;
        timeout(DEADLINE, execute_rx).await??;
        let output = prepared.execute_admitted().await?;
        assert_eq!(supervisor_permits.available_permits(), 0);
        assert!(result_tx.send(output.clone()).is_err());
        drop(permit);
        Ok::<_, anyhow::Error>(output)
    });
    timeout(DEADLINE, admitted_rx).await??;
    assert_eq!(permits.available_permits(), 0);
    receiver.abort();
    assert!(timeout(DEADLINE, receiver)
        .await?
        .unwrap_err()
        .is_cancelled());
    assert_eq!(permits.available_permits(), 0);
    assert!(permits.clone().try_acquire_owned().is_err());
    install(&writer, root.path(), "bool", 1).await?;
    execute_tx
        .send(())
        .map_err(|_| anyhow!("Supervisor dropped admitted work"))?;
    let output = timeout(DEADLINE, supervisor).await???;
    assert_eq!(output.descriptor, old);
    assert_eq!(returned(&output)?, json!({"result": 7}));
    assert_eq!(permits.available_permits(), 1);
    Ok(())
}
