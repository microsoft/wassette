// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::json;
use tempfile::TempDir;
use test_log::test;
use wassette::LifecycleManager;

mod common;
use common::build_fetch_component;

async fn setup_lifecycle_manager() -> Result<(Arc<LifecycleManager>, TempDir)> {
    let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;
    let plugin_dir = temp_dir.path().join("plugins");
    tokio::fs::create_dir_all(&plugin_dir).await?;

    let manager = LifecycleManager::new(&plugin_dir)
        .await
        .context("Failed to create lifecycle manager")?;

    Ok((Arc::new(manager), temp_dir))
}

#[test(tokio::test)]
async fn test_component_lifecycle_with_policies() -> Result<()> {
    let (manager, _temp_dir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    // Load component
    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.display()))
        .await
        .context("Failed to load component")?;

    // Verify component is loaded
    let components = manager.list_components().await;
    assert!(components.contains(&component_id));

    // Grant storage permission
    manager
        .grant_permission(
            &component_id,
            "storage",
            &json!({
                "uri": "fs:///tmp/test",
                "access": ["read", "write"]
            }),
        )
        .await
        .context("Failed to grant storage permission")?;

    // Grant network permission
    manager
        .grant_permission(
            &component_id,
            "network",
            &json!({
                "host": "example.com"
            }),
        )
        .await
        .context("Failed to grant network permission")?;

    // Grant environment variable permission
    manager
        .grant_permission(
            &component_id,
            "environment-variable",
            &json!({
                "key": "TEST_VAR"
            }),
        )
        .await
        .context("Failed to grant environment variable permission")?;

    // Get policy info
    let policy_info = manager.get_policy_info(&component_id).await;
    assert!(policy_info.is_some());

    // Revoke storage permission
    manager
        .revoke_permission(
            &component_id,
            "storage",
            &json!({
                "uri": "fs:///tmp/test"
            }),
        )
        .await
        .context("Failed to revoke storage permission")?;

    // Reset all permissions
    manager
        .reset_permission(&component_id)
        .await
        .context("Failed to reset permissions")?;

    // Verify policy info is cleared
    let policy_info = manager.get_policy_info(&component_id).await;
    assert!(policy_info.is_none());

    // Unload component
    manager
        .unload_component(&component_id)
        .await
        .context("Failed to unload component")?;

    // Verify component is no longer loaded
    let components = manager.list_components().await;
    assert!(!components.contains(&component_id));

    Ok(())
}

#[test(tokio::test)]
async fn test_multiple_component_management() -> Result<()> {
    let (manager, _temp_dir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    // Load the same component multiple times to test reloading
    let (component_id_1, _) = manager
        .load_component(&format!("file://{}", component_path.display()))
        .await
        .context("Failed to load component first time")?;

    let (component_id_2, _) = manager
        .load_component(&format!("file://{}", component_path.display()))
        .await
        .context("Failed to load component second time")?;

    // Should be the same component ID (reloaded)
    assert_eq!(component_id_1, component_id_2);

    // Verify only one component is loaded
    let components = manager.list_components().await;
    assert_eq!(components.len(), 1);
    assert!(components.contains(&component_id_1));

    // Grant different permissions
    manager
        .grant_permission(
            &component_id_1,
            "storage",
            &json!({
                "uri": "fs:///tmp/data",
                "access": ["read"]
            }),
        )
        .await?;

    manager
        .grant_permission(
            &component_id_1,
            "network",
            &json!({
                "host": "api.example.com"
            }),
        )
        .await?;

    // Verify policy exists
    let policy_info = manager.get_policy_info(&component_id_1).await;
    assert!(policy_info.is_some());

    // Unload component
    manager.unload_component(&component_id_1).await?;

    // Verify component and policies are cleaned up
    let components = manager.list_components().await;
    assert!(components.is_empty());

    Ok(())
}

#[test(tokio::test)]
async fn test_concurrent_component_operations() -> Result<()> {
    let (manager, _temp_dir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    // Test concurrent loads
    let manager_clone = manager.clone();
    let path_clone = component_path.clone();

    let load_task_1 = tokio::spawn(async move {
        manager_clone
            .load_component(&format!("file://{}", path_clone.display()))
            .await
    });

    let manager_clone = manager.clone();
    let path_clone = component_path.clone();

    let load_task_2 = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        manager_clone
            .load_component(&format!("file://{}", path_clone.display()))
            .await
    });

    let (result_1, result_2) = tokio::try_join!(load_task_1, load_task_2)?;

    let (component_id_1, _) = result_1?;
    let (component_id_2, _) = result_2?;

    // Should be the same component ID
    assert_eq!(component_id_1, component_id_2);

    // Verify only one component is loaded
    let components = manager.list_components().await;
    assert_eq!(components.len(), 1);

    Ok(())
}

#[test(tokio::test)]
async fn test_permission_operations_error_handling() -> Result<()> {
    let (manager, _temp_dir) = setup_lifecycle_manager().await?;

    // Try to grant permission to non-existent component
    let result = manager
        .grant_permission(
            "non-existent-component",
            "storage",
            &json!({
                "uri": "fs:///tmp/test",
                "access": ["read"]
            }),
        )
        .await;

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Component not found"));

    // Try to revoke permission from non-existent component
    let result = manager
        .revoke_permission(
            "non-existent-component",
            "storage",
            &json!({
                "uri": "fs:///tmp/test"
            }),
        )
        .await;

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Component not found"));

    // Try to reset permissions for non-existent component
    let result = manager.reset_permission("non-existent-component").await;

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Component not found"));

    Ok(())
}

#[test(tokio::test)]
async fn test_tools_listing_integration() -> Result<()> {
    let (manager, _temp_dir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    // Initially no tools
    let tools = manager.list_tools().await;
    assert!(tools.is_empty());

    // Load component
    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.display()))
        .await?;

    // Now should have tools
    let tools = manager.list_tools().await;
    assert!(!tools.is_empty());

    // Verify we can get component for tool
    if let Some(tool) = tools.first() {
        if let Some(tool_name) = tool.get("name").and_then(|v| v.as_str()) {
            let found_component_id = manager.get_component_id_for_tool(tool_name).await?;
            assert_eq!(found_component_id, component_id);
        }
    }

    // Unload component
    manager.unload_component(&component_id).await?;

    // Tools should be gone
    let tools = manager.list_tools().await;
    assert!(tools.is_empty());

    Ok(())
}
