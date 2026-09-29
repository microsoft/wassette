// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use serde_json::json;

use super::*;
use crate::tool::ToolSelector;

const ALPHA: &str = "example:alpha/tool";
const BETA: &str = "example:beta/tool";
const ALIAS: &str = "math_run";
const VERSIONED_MATH: &str = "example:math/ops@1.0.0-a.b+c";
const OTHER_VERSIONED_MATH: &str = "example:math/ops@1.0.0-a+b.c";

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

async fn manager(root: &Path) -> Result<LifecycleManager> {
    LifecycleManager::builder(root.join("store"))
        .with_secrets_dir(root.join("secrets"))
        .with_eager_loading(false)
        .build()
        .await
}

async fn install(
    manager: &LifecycleManager,
    root: &Path,
    file_stem: &str,
    name: &str,
    interfaces: &[(&str, &str, u32)],
) -> Result<ComponentId> {
    let bytes = component(name, interfaces)?;
    let id = inspect_artifact(&bytes)?.identity?;
    let path = root.join(format!("{file_stem}.wasm"));
    tokio::fs::write(&path, bytes).await?;
    let outcome = manager
        .load_component(&format!("file://{}", path.display()))
        .await?;
    assert_eq!(outcome.component_id, id.as_str());
    Ok(id)
}

fn returned(output: &ScopedToolOutput) -> Result<Value> {
    Ok(serde_json::from_str(&output.raw_result)?)
}

#[tokio::test]
async fn scoped_calls_disambiguate_components_and_exact_exports_in_both_load_orders() -> Result<()>
{
    for reverse in [false, true] {
        let root = tempfile::tempdir()?;
        let manager = manager(root.path()).await?;
        let mut fixtures = [
            ("private-alpha", ALPHA, "math", "u32", 7),
            ("private-beta", BETA, "MATH", "bool", 1),
        ];
        if reverse {
            fixtures.reverse();
        }
        for (file, name, interface, result_type, value) in fixtures {
            install(
                &manager,
                root.path(),
                file,
                name,
                &[(interface, result_type, value)],
            )
            .await?;
        }
        for (name, expected, expected_type) in [
            (ALPHA, json!({"result": 7}), "number"),
            (BETA, json!({"result": true}), "boolean"),
        ] {
            let id = ComponentId::from_declared_name(name)?;
            let tools = manager.list_tools_for_component(&id).await?;
            assert_eq!(tools.len(), 1);
            assert_eq!(tools[0].key.component_id, id);
            assert_eq!(tools[0].schema["name"], ALIAS);
            assert_eq!(
                tools[0].schema["outputSchema"]["properties"]["result"]["type"],
                expected_type
            );
            assert_eq!(manager.describe_scoped_tool(&tools[0].key).await?, tools[0]);
            let output = manager
                .invoke_scoped_tool(&tools[0].key, &json!({}))
                .await?;
            assert_eq!(output.descriptor, tools[0]);
            assert_eq!(returned(&output)?, expected);
            let legacy = manager.execute_component_call(name, ALIAS, "{}").await?;
            assert_eq!(serde_json::from_str::<Value>(&legacy)?, expected);
        }
        let error = manager
            .invoke_unique_tool(ALIAS, &json!({}))
            .await
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<ToolLookupError>(),
            Some(ToolLookupError::Ambiguous { .. })
        ));
        assert_eq!(manager.list_tool_descriptors().await?.len(), 2);
        let cold = self::manager(root.path()).await?;
        cold.populate_registry_from_metadata().await?;
        let error = cold
            .invoke_unique_tool(ALIAS, &json!({}))
            .await
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<ToolLookupError>(),
            Some(ToolLookupError::Ambiguous { .. })
        ));
        assert!(cold.list_components().await.is_empty());
        assert!(manager
            .execute_component_call("private-alpha", ALIAS, "{}")
            .await
            .is_err());
    }
    Ok(())
}

#[tokio::test]
async fn exact_keys_disambiguate_interfaces_within_one_component() -> Result<()> {
    let root = tempfile::tempdir()?;
    let manager = manager(root.path()).await?;
    let id = install(
        &manager,
        root.path(),
        "private",
        ALPHA,
        &[
            (VERSIONED_MATH, "u32", 7),
            (OTHER_VERSIONED_MATH, "bool", 1),
        ],
    )
    .await?;
    let tools = manager.list_tools_for_component(&id).await?;
    assert_eq!(tools.len(), 2);
    let alias = tools[0].schema["name"].as_str().unwrap().to_owned();
    for tool in tools {
        assert_eq!(tool.schema["name"], alias);
        assert_eq!(manager.describe_scoped_tool(&tool.key).await?, tool);
        let output = manager.invoke_scoped_tool(&tool.key, &json!({})).await?;
        let expected = if tool.key.export.interface_name.as_deref() == Some(VERSIONED_MATH) {
            json!({"result": 7})
        } else {
            json!({"result": true})
        };
        assert_eq!(returned(&output)?, expected);
        assert_eq!(output.descriptor, tool);
    }
    assert!(manager
        .get_tool_schema_for_component(ALPHA, &alias)
        .await
        .is_none());
    let error = manager
        .execute_component_call(ALPHA, &alias, "{}")
        .await
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<ToolLookupError>(),
        Some(ToolLookupError::Ambiguous { .. })
    ));
    assert!(manager
        .invoke_unique_tool(&alias, &json!({}))
        .await
        .is_err());
    Ok(())
}

#[tokio::test]
async fn cached_descriptors_match_loaded_descriptors_without_compiling() -> Result<()> {
    let root = tempfile::tempdir()?;
    let first = manager(root.path()).await?;
    let id = install(&first, root.path(), "private", ALPHA, &[("math", "u32", 7)]).await?;
    let expected = first.list_tools_for_component(&id).await?;
    let cold = manager(root.path()).await?;
    assert_eq!(cold.list_tool_descriptors().await?, expected);
    assert_eq!(
        cold.describe_scoped_tool(&expected[0].key).await?,
        expected[0]
    );
    assert!(cold.get_component(ALPHA).await.is_none());
    cold.populate_registry_from_metadata().await?;
    let output = cold.invoke_unique_tool(ALIAS, &json!({})).await?;
    assert_eq!(output.descriptor, expected[0]);
    assert_eq!(returned(&output)?, json!({"result": 7}));
    install(
        &first,
        root.path(),
        "private",
        ALPHA,
        &[("math", "bool", 1)],
    )
    .await?;
    let replaced = cold.describe_scoped_tool(&expected[0].key).await?;
    assert_ne!(replaced.schema, expected[0].schema);
    let output = cold.invoke_scoped_tool(&replaced.key, &json!({})).await?;
    assert_eq!(output.descriptor, replaced);
    assert_eq!(returned(&output)?, json!({"result": true}));
    first.unload_component(ALPHA).await?;
    assert!(cold.list_tool_descriptors().await?.is_empty());
    assert!(cold.describe_scoped_tool(&replaced.key).await.is_err());
    assert!(cold
        .invoke_scoped_tool(&replaced.key, &json!({}))
        .await
        .is_err());
    Ok(())
}

#[tokio::test]
async fn missing_metadata_uses_receipt_bound_lazy_restoration() -> Result<()> {
    let root = tempfile::tempdir()?;
    let first = manager(root.path()).await?;
    let id = install(&first, root.path(), "private", ALPHA, &[("math", "u32", 7)]).await?;
    let expected = first.list_tools_for_component(&id).await?;
    let receipt = first.component_store().read(id.as_str())?.receipt;
    tokio::fs::remove_file(first.storage.metadata_path(&receipt.storage_key)).await?;

    let cold = manager(root.path()).await?;
    assert!(cold.list_components().await.is_empty());
    assert_eq!(cold.list_tools_for_component(&id).await?, expected);
    assert!(cold.get_component(id.as_str()).await.is_some());
    let output = cold
        .invoke_scoped_tool(&expected[0].key, &json!({}))
        .await?;
    assert_eq!(output.descriptor, expected[0]);
    assert_eq!(returned(&output)?, json!({"result": 7}));
    Ok(())
}

#[tokio::test]
async fn lazy_loading_rechecks_global_name_collisions() -> Result<()> {
    let root = tempfile::tempdir()?;
    let manager = manager(root.path()).await?;
    install(&manager, root.path(), "alpha", ALPHA, &[("math", "u32", 7)]).await?;
    let guard = manager.load_guard(ALPHA).await;
    let held = guard.lock().await;
    let arguments = json!({});
    let mut invocation = Box::pin(manager.invoke_unique_tool(ALIAS, &arguments));
    assert!(futures::poll!(invocation.as_mut()).is_pending());
    let writer = self::manager(root.path()).await?;
    install(&writer, root.path(), "beta", BETA, &[("MATH", "bool", 1)]).await?;
    drop(held);
    let error = invocation.await.unwrap_err();
    assert!(matches!(
        error.downcast_ref::<ToolLookupError>(),
        Some(ToolLookupError::Ambiguous { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn selected_instance_export_and_schema_survive_registry_replacement_together() -> Result<()> {
    let root = tempfile::tempdir()?;
    let manager = manager(root.path()).await?;
    let id = install(
        &manager,
        root.path(),
        "private",
        ALPHA,
        &[("math", "u32", 7)],
    )
    .await?;
    let descriptor = manager.list_tools_for_component(&id).await?.remove(0);
    let (instance, selected) = manager
        .select_loaded_tool(ToolSelector::Exact(&descriptor.key))
        .await?;
    let state = manager.wasi_state_for_instance(&instance).await?;
    install(
        &manager,
        root.path(),
        "private",
        ALPHA,
        &[("math", "bool", 1)],
    )
    .await?;
    let output = manager
        .execute_tool_call(instance, selected, Vec::new(), state)
        .await?;
    assert_eq!(returned(&output)?, json!({"result": 7}));
    assert_eq!(output.descriptor, descriptor);
    let output = manager
        .invoke_scoped_tool(&descriptor.key, &json!({}))
        .await?;
    assert_eq!(returned(&output)?, json!({"result": true}));
    assert_ne!(output.descriptor.schema, descriptor.schema);
    Ok(())
}

#[tokio::test]
async fn scoped_lookup_reports_missing_exports_without_using_a_normalized_alias() -> Result<()> {
    let root = tempfile::tempdir()?;
    let manager = manager(root.path()).await?;
    let id = install(
        &manager,
        root.path(),
        "private",
        ALPHA,
        &[("math", "u32", 7)],
    )
    .await?;
    let descriptor = manager.list_tools_for_component(&id).await?.remove(0);
    let mut missing = descriptor.key.clone();
    missing.export.interface_name = Some("MATH".into());
    for error in [
        manager.describe_scoped_tool(&missing).await.unwrap_err(),
        manager
            .invoke_scoped_tool(&missing, &json!({}))
            .await
            .unwrap_err(),
        manager
            .invoke_unique_tool("missing", &json!({}))
            .await
            .unwrap_err(),
    ] {
        assert!(matches!(
            error.downcast_ref::<ToolLookupError>(),
            Some(ToolLookupError::NotFound { .. })
        ));
    }
    let error = manager
        .invoke_scoped_tool(&descriptor.key, &json!([]))
        .await
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<crate::ToolInvocationError>(),
        Some(crate::ToolInvocationError::InvalidArguments(_))
    ));
    assert!(error
        .chain()
        .any(|cause| cause.is::<component2json::ValError>()));
    manager.unload_component(ALPHA).await?;
    assert!(manager.describe_scoped_tool(&descriptor.key).await.is_err());
    assert!(manager
        .invoke_scoped_tool(&descriptor.key, &json!({}))
        .await
        .is_err());
    Ok(())
}

#[tokio::test]
async fn each_scoped_call_gets_a_fresh_guest_instance() -> Result<()> {
    let root = tempfile::tempdir()?;
    let manager = manager(root.path()).await?;
    let bytes = wat::parse_str(
        r#"(component $counter
            (core module $m
                (global $counter (mut i32) (i32.const 0))
                (func (export "next") (result i32)
                    global.get $counter i32.const 1 i32.add
                    global.set $counter global.get $counter))
            (core instance $i (instantiate $m))
            (func (export "next") (result u32)
                (canon lift (core func $i "next"))))"#,
    )?;
    let id = inspect_artifact(&bytes)?.identity?;
    let path = root.path().join("private.wasm");
    tokio::fs::write(&path, bytes).await?;
    manager
        .load_component(&format!("file://{}", path.display()))
        .await?;
    let descriptor = manager.list_tools_for_component(&id).await?.remove(0);
    let args = json!({});
    let (first, second) = tokio::join!(
        manager.invoke_scoped_tool(&descriptor.key, &args),
        manager.invoke_scoped_tool(&descriptor.key, &args)
    );
    assert_eq!(returned(&first?)?, json!({"result": 1}));
    assert_eq!(returned(&second?)?, json!({"result": 1}));
    Ok(())
}
