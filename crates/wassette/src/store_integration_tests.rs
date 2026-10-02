// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use anyhow::Result;

use super::*;

const FILE_ID: &str = "local:private-key";

fn named_tool(name: &str, value: u32) -> Result<Vec<u8>> {
    Ok(wat::parse_str(format!(
        r#"(component ${name}
            (core module $m
                (func (export "value") (result i32) i32.const {value}))
            (core instance $i (instantiate $m))
            (func (export "value") (result u32)
                (canon lift (core func $i "value"))))"#
    ))?)
}

async fn manager(root: &Path) -> Result<LifecycleManager> {
    LifecycleManager::builder(root.join("components"))
        .with_secrets_dir(root.join("secrets"))
        .with_eager_loading(false)
        .build()
        .await
}

#[tokio::test]
async fn semantic_lookup_preserves_private_artifact_and_secret_keys() -> Result<()> {
    let root = tempfile::tempdir()?;
    let source = root.path().join("private-key.wasm");
    tokio::fs::write(&source, named_tool("example:actual/name", 7)?).await?;
    let manager = manager(root.path()).await?;
    let outcome = manager
        .load_component(&format!("file://{}", source.display()))
        .await?;
    assert_eq!(outcome.component_id, FILE_ID);
    assert!(root.path().join("components/private-key.wasm").is_file());
    assert_eq!(manager.get_component_id_for_tool("value").await?, FILE_ID);
    manager
        .set_component_secrets(FILE_ID, &[("TOKEN".into(), "test-value".into())])
        .await?;
    assert!(root.path().join("secrets/private-key.yaml").is_file());
    assert_eq!(
        manager.load_component_secrets(FILE_ID).await?["TOKEN"],
        "test-value"
    );
    assert!(manager.load_component_secrets("private-key").await.is_err());
    Ok(())
}

#[tokio::test]
async fn failed_replacement_preserves_receipt_policy_and_callable_instance() -> Result<()> {
    let root = tempfile::tempdir()?;
    let source = root.path().join("private-key.wasm");
    tokio::fs::write(&source, named_tool("semantic", 7)?).await?;
    let manager = manager(root.path()).await?;
    let uri = format!("file://{}", source.display());
    manager
        .load_component_with_policy(&uri, "version: '1.0'\npermissions: {}\n", "manifest:inline")
        .await?;
    let before = manager.component_store().read(FILE_ID)?;
    let value = manager
        .execute_component_call(FILE_ID, "value", "{}")
        .await?;
    tokio::fs::write(&source, b"not a component").await?;
    assert!(manager.load_component(&uri).await.is_err());
    let after = manager.component_store().read(FILE_ID)?;
    assert_eq!(before.receipt, after.receipt);
    assert_eq!(before.wasm, after.wasm);
    assert_eq!(before.policy, after.policy);
    assert_eq!(
        manager
            .execute_component_call(FILE_ID, "value", "{}")
            .await?,
        value
    );
    Ok(())
}

#[tokio::test]
async fn another_manager_observes_policy_revision_without_replacing_wasm() -> Result<()> {
    let root = tempfile::tempdir()?;
    let source = root.path().join("private-key.wasm");
    tokio::fs::write(&source, named_tool("semantic", 1)?).await?;
    let first = manager(root.path()).await?;
    first
        .load_component(&format!("file://{}", source.display()))
        .await?;
    let second = manager(root.path()).await?;
    second.ensure_component_loaded(FILE_ID).await?;
    assert!(second
        .get_component(FILE_ID)
        .await
        .unwrap()
        .policy_template
        .allowed_hosts
        .is_empty());
    let before = second.component_store().read(FILE_ID)?;
    first
        .grant_permission(
            FILE_ID,
            "network",
            &serde_json::json!({"host": "example.test"}),
        )
        .await?;
    let after = second.component_store().read(FILE_ID)?;
    assert_ne!(before.receipt.revision, after.receipt.revision);
    assert_eq!(
        before.receipt.artifact_sha256,
        after.receipt.artifact_sha256
    );
    assert_eq!(before.wasm, after.wasm);
    assert!(String::from_utf8(after.policy.unwrap())?.contains("example.test"));
    second.ensure_component_loaded(FILE_ID).await?;
    let refreshed = second.get_component(FILE_ID).await.unwrap();
    assert_eq!(refreshed.revision.as_ref(), Some(&after.receipt.revision));
    assert!(refreshed
        .policy_template
        .allowed_hosts
        .contains("example.test"));
    first.reset_permission(FILE_ID).await?;
    let cleared = second.component_store().read(FILE_ID)?;
    assert!(cleared.policy.is_none());
    assert_ne!(cleared.receipt.revision, after.receipt.revision);
    second.ensure_component_loaded(FILE_ID).await?;
    assert!(second
        .get_component(FILE_ID)
        .await
        .unwrap()
        .policy_template
        .allowed_hosts
        .is_empty());
    Ok(())
}

#[tokio::test]
async fn invalid_manifest_policy_does_not_publish_artifact_or_tools() -> Result<()> {
    let root = tempfile::tempdir()?;
    let source = root.path().join("private-key.wasm");
    tokio::fs::write(&source, named_tool("semantic", 1)?).await?;
    let manager = manager(root.path()).await?;
    assert!(manager
        .load_component_with_policy(
            &format!("file://{}", source.display()),
            "invalid: [",
            "manifest:inline"
        )
        .await
        .is_err());
    assert!(!root.path().join("components/private-key.wasm").exists());
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
async fn uninstall_does_not_allow_an_unrelated_source_to_inherit_secrets() -> Result<()> {
    let root = tempfile::tempdir()?;
    let source = root.path().join("private-key.wasm");
    tokio::fs::write(&source, named_tool("semantic", 1)?).await?;
    let manager = manager(root.path()).await?;
    let uri = format!("file://{}", source.display());
    manager.load_component(&uri).await?;
    manager
        .set_component_secrets(FILE_ID, &[("TOKEN".into(), "test-value".into())])
        .await?;
    manager.unload_component(FILE_ID).await?;
    assert!(root.path().join("secrets/private-key.yaml").is_file());
    let unrelated = root.path().join("unrelated");
    tokio::fs::create_dir(&unrelated).await?;
    let unrelated = unrelated.join("private-key.wasm");
    tokio::fs::write(&unrelated, named_tool("semantic", 2)?).await?;
    assert!(manager
        .load_component(&format!("file://{}", unrelated.display()))
        .await
        .is_err());
    tokio::fs::write(&source, named_tool("semantic", 3)?).await?;
    manager.load_component(&uri).await?;
    assert_eq!(
        manager.load_component_secrets(FILE_ID).await?["TOKEN"],
        "test-value"
    );
    Ok(())
}

#[tokio::test]
async fn unnamed_legacy_files_are_protected_not_filename_named_tools() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = root.path().join("components");
    tokio::fs::create_dir(&store).await?;
    let legacy = wat::parse_str("(component)")?;
    tokio::fs::write(store.join("private-key.wasm"), &legacy).await?;
    let manager = manager(root.path()).await?;
    assert!(manager.load_all_components().await.is_err());
    assert!(manager.catalog().await.is_err());
    assert!(manager.list_tools().await.is_empty());
    let source = root.path().join("private-key.wasm");
    tokio::fs::write(&source, named_tool("semantic", 1)?).await?;
    assert!(manager
        .load_component(&format!("file://{}", source.display()))
        .await
        .is_err());
    assert_eq!(
        tokio::fs::read(store.join("private-key.wasm")).await?,
        legacy
    );
    Ok(())
}

#[tokio::test]
async fn legacy_install_intent_field_does_not_hide_tools() -> Result<()> {
    let root = tempfile::tempdir()?;
    let source = root.path().join("private-key.wasm");
    tokio::fs::write(&source, named_tool("semantic", 7)?).await?;
    let uri = format!("file://{}", source.display());
    let installer = manager(root.path()).await?;
    installer.load_component(&uri).await?;
    let receipt = installer.component_store().read(FILE_ID)?.receipt;
    let receipt_path = root
        .path()
        .join("components")
        .join(format!("{}.install.json", receipt.storage_key.as_str()));
    let mut record: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(&receipt_path).await?)?;
    record["Installed"]["intent"] = serde_json::json!("InstallOnly");
    tokio::fs::write(&receipt_path, serde_json::to_vec(&record)?).await?;

    let fresh = manager(root.path()).await?;
    let descriptors = fresh.list_tool_descriptors().await?;
    assert_eq!(descriptors.len(), 1);
    fresh
        .invoke_scoped_tool(&descriptors[0].key, &serde_json::json!({}))
        .await?;
    Ok(())
}
