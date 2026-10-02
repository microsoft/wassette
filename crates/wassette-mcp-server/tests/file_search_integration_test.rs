// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Loads the default `wassette:file-search` tool and searches real repository
//! files through the storage policy.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};
use test_log::test;
use wassette::LifecycleManager;

mod common;
use common::build_file_search_component;

const ID: &str = "local:file_search";

fn crate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .unwrap()
}

async fn search(manager: &LifecycleManager, root: &Path, pattern: &str) -> Result<Value> {
    let arguments = json!({
        "root": root.to_str().unwrap(),
        "pattern": pattern,
        "literal": true,
        "case-insensitive": false,
        "glob": "*.rs",
        "max-results": 50,
    });
    let output = manager
        .execute_component_call(ID, "search-files", &arguments.to_string())
        .await?;
    let value: Value = serde_json::from_str(&output)?;
    Ok(value.get("result").cloned().unwrap_or(value))
}

fn error_text(result: Result<Value>) -> String {
    match result {
        Ok(value) => value["err"]
            .as_str()
            .unwrap_or_else(|| panic!("expected an error, got {value}"))
            .to_owned(),
        Err(error) => format!("{error:#}"),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn file_search_reads_only_granted_roots() -> Result<()> {
    let component = build_file_search_component()?;
    let store = tempfile::tempdir()?;
    let manager = LifecycleManager::builder(store.path())
        .with_secrets_dir(store.path().join("secrets"))
        .build()
        .await?;

    let outcome = manager
        .load_component(&format!("file://{}", component.display()))
        .await?;
    assert_eq!(outcome.component_id, ID);
    let schema = manager
        .get_component_schema(ID)
        .await
        .context("file-search exposes tools")?;
    let tool = schema["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "search-files")
        .context("search-files tool")?;
    assert!(tool["description"]
        .as_str()
        .unwrap()
        .contains("grant-storage-permission"));

    let granted = crate_dir().join("tests");
    let denied = crate_dir().join("src");
    let before = error_text(search(&manager, &granted, "fn file_search_reads_only").await);
    assert!(before.contains("grant-storage-permission"), "{before}");

    manager
        .grant_permission(
            ID,
            "storage",
            &json!({"uri": format!("fs://{}", granted.display()), "access": ["read"]}),
        )
        .await?;

    // Split so the search pattern does not match its own source line.
    let pattern = concat!("async fn ", "file_search_reads_only_granted_roots");
    let found = search(&manager, &granted, pattern).await?;
    let found = &found["ok"];
    let matches = found["matches"].as_array().context("matches")?;
    assert_eq!(matches.len(), 1, "{found}");
    assert!(matches[0]["path"]
        .as_str()
        .unwrap()
        .ends_with("file_search_integration_test.rs"));
    assert!(matches[0]["line-number"].as_u64().unwrap() > 1);
    assert_eq!(found["truncated"], false);

    let sibling = error_text(search(&manager, &denied, "fn").await);
    assert!(sibling.contains("grant-storage-permission"), "{sibling}");
    let escape = error_text(search(&manager, &granted.join("../src"), "fn").await);
    assert!(escape.contains("grant-storage-permission"), "{escape}");
    Ok(())
}

#[test]
fn shipped_policy_grants_nothing_by_default() -> Result<()> {
    let path = crate_dir().join("../../components/file-search/policy.yaml");
    let policy = policy::PolicyParser::parse_file(path)?;
    assert!(policy.permissions.storage.is_none());
    assert!(policy.permissions.network.is_none());
    assert!(policy.permissions.environment.is_none());
    Ok(())
}
