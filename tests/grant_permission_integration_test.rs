use std::sync::Arc;

use anyhow::{Context, Result};
use tempfile::TempDir;
use test_log::test;
use wassette::LifecycleManager;

mod common;
use common::build_fetch_component;

async fn cleanup_components(manager: &LifecycleManager) -> Result<()> {
    let component_ids = manager.list_components().await;
    for id in component_ids {
        manager.unload_component(&id).await;
    }
    Ok(())
}

async fn setup_lifecycle_manager() -> Result<(Arc<LifecycleManager>, TempDir)> {
    let tempdir = tempfile::tempdir()?;

    let manager = Arc::new(
        LifecycleManager::new(&tempdir)
            .await
            .context("Failed to create LifecycleManager")?,
    );

    cleanup_components(&manager).await?;

    Ok((manager, tempdir))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn test_grant_permission_storage_basic() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    // Test granting storage permission
    let result = manager
        .grant_permission(
            &component_id,
            "storage",
            &serde_json::json!({"uri": "fs:///tmp/test", "access": ["read", "write"]}),
        )
        .await;

    assert!(result.is_ok());

    // Verify policy file was created and contains the permission
    let policy_info = manager.get_policy_info(&component_id).await;
    assert!(policy_info.is_some());
    let policy_info = policy_info.unwrap();

    // Verify policy contains the permission
    let policy_content = tokio::fs::read_to_string(&policy_info.local_path).await?;
    assert!(policy_content.contains("fs:///tmp/test"));
    assert!(policy_content.contains("storage"));
    assert!(policy_content.contains("read"));
    assert!(policy_content.contains("write"));

    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn test_grant_permission_multiple_permissions() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    // Grant multiple different permissions
    let network_result = manager
        .grant_permission(
            &component_id,
            "network",
            &serde_json::json!({"host": "api.example.com"}),
        )
        .await;

    let storage_result = manager
        .grant_permission(
            &component_id,
            "storage",
            &serde_json::json!({"uri": "fs:///tmp/test", "access": ["read"]}),
        )
        .await;

    assert!(network_result.is_ok());
    assert!(storage_result.is_ok());

    // Verify policy file contains all permissions
    let policy_info = manager.get_policy_info(&component_id).await;
    assert!(policy_info.is_some());
    let policy_info = policy_info.unwrap();
    let policy_content = tokio::fs::read_to_string(&policy_info.local_path).await?;

    assert!(policy_content.contains("api.example.com"));
    assert!(policy_content.contains("fs:///tmp/test"));
    assert!(policy_content.contains("network"));
    assert!(policy_content.contains("storage"));

    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn test_grant_permission_duplicate_prevention() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    // Grant the same network permission twice
    let details = serde_json::json!({"host": "api.example.com"});
    let first_result = manager
        .grant_permission(&component_id, "network", &details)
        .await;
    let second_result = manager
        .grant_permission(&component_id, "network", &details)
        .await;

    assert!(first_result.is_ok());
    assert!(second_result.is_ok());

    // Verify policy file contains only one instance
    let policy_info = manager.get_policy_info(&component_id).await;
    assert!(policy_info.is_some());
    let policy_info = policy_info.unwrap();
    let policy_content = tokio::fs::read_to_string(&policy_info.local_path).await?;

    // Count occurrences of the host - should be exactly 1
    let occurrences = policy_content.matches("api.example.com").count();
    assert_eq!(occurrences, 1);

    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn test_grant_permission_storage_access_merging() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    // Grant read access first
    let read_result = manager
        .grant_permission(
            &component_id,
            "storage",
            &serde_json::json!({"uri": "fs:///tmp/test", "access": ["read"]}),
        )
        .await;

    // Grant write access to the same URI
    let write_result = manager
        .grant_permission(
            &component_id,
            "storage",
            &serde_json::json!({"uri": "fs:///tmp/test", "access": ["write"]}),
        )
        .await;

    assert!(read_result.is_ok());
    assert!(write_result.is_ok());

    // Verify policy file contains both access types for the same URI
    let policy_info = manager.get_policy_info(&component_id).await;
    assert!(policy_info.is_some());
    let policy_info = policy_info.unwrap();
    let policy_content = tokio::fs::read_to_string(&policy_info.local_path).await?;

    assert!(policy_content.contains("read"));
    assert!(policy_content.contains("write"));

    // Should only have one URI entry
    let uri_occurrences = policy_content.matches("fs:///tmp/test").count();
    assert_eq!(uri_occurrences, 1);

    Ok(())
}

#[test(tokio::test)]
async fn test_grant_permission_component_not_found() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;

    // Try to grant permission to non-existent component
    let result = manager
        .grant_permission(
            "non-existent-component",
            "network",
            &serde_json::json!({"host": "api.example.com"}),
        )
        .await;

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Component not found"));

    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn test_grant_permission_invalid_permission_type() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    // Try to grant invalid permission type
    let result = manager
        .grant_permission(
            &component_id,
            "invalid-permission",
            &serde_json::json!({"host": "api.example.com"}),
        )
        .await;

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Unknown permission type"));

    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn test_grant_permission_missing_required_fields() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    // Test network permission without host field
    let network_result = manager
        .grant_permission(&component_id, "network", &serde_json::json!({}))
        .await;

    assert!(network_result.is_err());
    assert!(network_result
        .unwrap_err()
        .to_string()
        .contains("Missing 'host' field"));

    // Test storage permission without uri field
    let storage_result = manager
        .grant_permission(
            &component_id,
            "storage",
            &serde_json::json!({"access": ["read"]}),
        )
        .await;

    assert!(storage_result.is_err());
    assert!(storage_result
        .unwrap_err()
        .to_string()
        .contains("Missing 'uri' field"));

    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn test_grant_permission_validation_errors() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    // Test empty host
    let empty_host_result = manager
        .grant_permission(&component_id, "network", &serde_json::json!({"host": ""}))
        .await;

    assert!(empty_host_result.is_err());
    assert!(empty_host_result
        .unwrap_err()
        .to_string()
        .contains("Network host cannot be empty"));

    // Test empty access array
    let empty_access_result = manager
        .grant_permission(
            &component_id,
            "storage",
            &serde_json::json!({"uri": "fs:///tmp/test", "access": []}),
        )
        .await;

    assert!(empty_access_result.is_err());
    assert!(empty_access_result
        .unwrap_err()
        .to_string()
        .contains("Storage access cannot be empty"));

    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn test_grant_permission_to_existing_policy() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    // First, create and attach a policy using the existing system
    let policy_content = r#"
version: "1.0"
description: "Initial policy"
permissions:
  network:
    allow:
      - host: "initial.example.com"
"#;
    let policy_path = _tempdir.path().join("initial-policy.yaml");
    tokio::fs::write(&policy_path, policy_content).await?;

    let policy_uri = format!("file://{}", policy_path.display());
    manager.attach_policy(&component_id, &policy_uri).await?;

    // Now grant additional permission using granular system
    let grant_result = manager
        .grant_permission(
            &component_id,
            "network",
            &serde_json::json!({"host": "additional.example.com"}),
        )
        .await;

    assert!(grant_result.is_ok());

    // Verify both permissions exist in the policy file
    let policy_info = manager.get_policy_info(&component_id).await;
    assert!(policy_info.is_some());
    let policy_info = policy_info.unwrap();
    let final_policy_content = tokio::fs::read_to_string(&policy_info.local_path).await?;

    assert!(final_policy_content.contains("initial.example.com"));
    assert!(final_policy_content.contains("additional.example.com"));

    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn test_grant_permission_policy_persistence() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    // Grant a permission
    let grant_result = manager
        .grant_permission(
            &component_id,
            "network",
            &serde_json::json!({"host": "api.example.com"}),
        )
        .await;

    assert!(grant_result.is_ok());

    // Verify policy file persists
    let policy_info = manager.get_policy_info(&component_id).await;
    assert!(policy_info.is_some());

    // Create a new manager with the same directory to test persistence
    let new_manager = wassette::LifecycleManager::new(_tempdir.path()).await?;

    // Load the same component
    let (new_component_id, _) = new_manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    // Verify the policy is still there and accessible
    let policy_info = new_manager.get_policy_info(&new_component_id).await;
    assert!(policy_info.is_some());

    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn test_grant_permission_policy_registry_update() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    // Grant a permission
    let grant_result = manager
        .grant_permission(
            &component_id,
            "network",
            &serde_json::json!({"host": "api.example.com"}),
        )
        .await;

    assert!(grant_result.is_ok());

    // Verify policy registry was updated by checking policy info
    let policy_info = manager.get_policy_info(&component_id).await;
    assert!(policy_info.is_some());

    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn test_grant_permission_multiple_hosts() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    // Grant multiple network permissions
    let hosts = vec!["api.example.com", "backup.example.com", "cdn.example.com"];

    for host in &hosts {
        let result = manager
            .grant_permission(&component_id, "network", &serde_json::json!({"host": host}))
            .await;
        assert!(result.is_ok());
    }

    // Verify all hosts are in the policy
    let policy_info = manager.get_policy_info(&component_id).await;
    assert!(policy_info.is_some());
    let policy_info = policy_info.unwrap();
    let policy_content = tokio::fs::read_to_string(&policy_info.local_path).await?;

    for host in &hosts {
        assert!(policy_content.contains(host));
    }

    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn test_grant_permission_complex_storage_permissions() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    // Grant different storage permissions
    let storage_configs = vec![
        ("fs:///tmp/cache", vec!["read"]),
        ("fs:///tmp/output", vec!["write"]),
        ("fs:///tmp/workspace", vec!["read", "write"]),
    ];

    for (uri, access) in &storage_configs {
        let result = manager
            .grant_permission(
                &component_id,
                "storage",
                &serde_json::json!({"uri": uri, "access": access}),
            )
            .await;
        assert!(result.is_ok());
    }

    // Verify all storage permissions are in the policy
    let policy_info = manager.get_policy_info(&component_id).await;
    assert!(policy_info.is_some());
    let policy_info = policy_info.unwrap();
    let policy_content = tokio::fs::read_to_string(&policy_info.local_path).await?;

    for (uri, access) in &storage_configs {
        assert!(policy_content.contains(uri));
        for access_type in access {
            assert!(policy_content.contains(access_type));
        }
    }

    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn test_grant_permission_invalid_storage_access_type() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    // Test invalid access type
    let result = manager
        .grant_permission(
            &component_id,
            "storage",
            &serde_json::json!({"uri": "fs:///tmp/test", "access": ["invalid"]}),
        )
        .await;

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Invalid access type"));

    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn test_grant_permission_component_execution_with_permissions() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    // Grant network permission for the component to access example.com
    let grant_result = manager
        .grant_permission(
            &component_id,
            "network",
            &serde_json::json!({"host": "example.com"}),
        )
        .await;

    assert!(grant_result.is_ok());

    // Try to execute the component's fetch function
    let execution_result = manager
        .execute_component_call(&component_id, "fetch", r#"{"url": "https://example.com/"}"#)
        .await;

    // The execution should succeed (the component should be able to access example.com)
    assert!(execution_result.is_ok());
    let response = execution_result.unwrap();
    assert!(response.contains("Example Domain"));

    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn test_grant_permission_schema_validation() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    // Test with malformed details (not an object)
    let malformed_result = manager
        .grant_permission(
            &component_id,
            "network",
            &serde_json::json!("not an object"),
        )
        .await;

    assert!(malformed_result.is_err());

    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn test_grant_permission_sequential_grants() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    // Grant multiple permissions one by one to avoid race conditions
    let api1_details = serde_json::json!({"host": "api1.example.com"});
    let api2_details = serde_json::json!({"host": "api2.example.com"});

    let result1 = manager
        .grant_permission(&component_id, "network", &api1_details)
        .await;
    let result2 = manager
        .grant_permission(&component_id, "network", &api2_details)
        .await;

    // All operations should succeed
    assert!(result1.is_ok());
    assert!(result2.is_ok());

    // Verify all permissions are in the policy
    let policy_info = manager.get_policy_info(&component_id).await;
    assert!(policy_info.is_some());
    let policy_info = policy_info.unwrap();
    let policy_content = tokio::fs::read_to_string(&policy_info.local_path).await?;

    assert!(policy_content.contains("api1.example.com"));
    assert!(policy_content.contains("api2.example.com"));

    Ok(())
}
