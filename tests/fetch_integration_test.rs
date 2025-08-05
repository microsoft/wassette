// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use anyhow::{Context, Result};
use tempfile::TempDir;
use wassette::LifecycleManager;

mod common;
use common::build_fetch_component;

async fn setup_lifecycle_manager() -> Result<(LifecycleManager, TempDir)> {
    let tempdir = tempfile::tempdir().context("Failed to create temporary directory")?;
    let manager = LifecycleManager::new(&tempdir).await?;
    Ok((manager, tempdir))
}

#[tokio::test]
async fn test_fetch_with_network_policy_enforcement() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    let target_url = "https://example.com/";

    println!("Attempting to fetch {target_url} without network permissions...");

    let result = manager
        .execute_component_call(
            &component_id,
            "fetch",
            &serde_json::json!({"url": target_url}).to_string(),
        )
        .await;

    match result {
        Ok(response) => {
            println!("Component response: {response}");

            // Check if the response contains an error indicating the request was blocked
            if response.contains("HttpRequestDenied") {
                println!("✅ Network request properly blocked by policy!");
            } else {
                panic!(
                    "Expected network request to be blocked, but got successful response: {response}"
                );
            }
        }
        Err(e) => {
            panic!("Expected network request to be blocked, but got successful response: {e}");
        }
    }

    // Then grant network permission for example.com

    let grant_result = manager
        .grant_permission(
            &component_id,
            "network",
            &serde_json::json!({"host": "example.com"}),
        )
        .await;

    assert!(grant_result.is_ok(), "Failed to grant network permission");

    // Then try to fetch with network permissions - should succeed
    println!("Attempting to fetch {target_url} with network permissions...");

    let result = manager
        .execute_component_call(
            &component_id,
            "fetch",
            &serde_json::json!({"url": target_url}).to_string(),
        )
        .await;

    match result {
        Ok(response) => {
            println!("Fetch response after granting permission: {response}");

            if response.contains("HttpRequestDenied") {
                panic!("Network request still being blocked after granting permission: {response}");
            } else {
                assert!(
                    response.contains("Example Domain") || response.contains("example"),
                    "Expected response to contain example.com content, got: {response}"
                );
                println!("✅ Network request succeeded after granting permission!");
            }
        }
        Err(e) => {
            panic!("Expected network request to be blocked, but got successful response: {e}");
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_fetch_with_different_host_still_denied() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    // Grant permission for example.com
    manager
        .grant_permission(
            &component_id,
            "network",
            &serde_json::json!({"host": "example.com"}),
        )
        .await?;

    let different_url = "https://httpbin.org/get";

    let result = manager
        .execute_component_call(
            &component_id,
            "fetch",
            &serde_json::json!({"url": different_url}).to_string(),
        )
        .await;

    match result {
        Err(e) => {
            panic!("Expected request to httpbin.org to be denied when only example.com is allowed, got: {e}");
        }
        Ok(response) => {
            if response.contains("HttpRequestDenied") {
                println!("✅ Request to unauthorized host properly blocked!");
            } else {
                panic!("Expected request to httpbin.org to be denied when only example.com is allowed, got: {response}");
            }
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_fetch_with_scheme_specific_permissions() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    // Grant permission for HTTPS example.com specifically
    manager
        .grant_permission(
            &component_id,
            "network",
            &serde_json::json!({"host": "https://example.com"}),
        )
        .await?;

    // Try HTTPS request - should work
    let https_result = manager
        .execute_component_call(
            &component_id,
            "fetch",
            &serde_json::json!({"url": "https://example.com/"}).to_string(),
        )
        .await;

    // HTTPS should succeed or fail for non-policy reasons
    match https_result {
        Ok(response) => {
            println!("HTTPS fetch response: {response}");

            if response.contains("HttpRequestDenied") {
                panic!("HTTPS request should not be blocked by policy, got: {response}");
            } else {
                println!("✅ HTTPS request allowed as expected");
            }
        }
        Err(e) => {
            let error_msg = e.to_string();
            assert!(
                !error_msg.contains("denied")
                    && !error_msg.contains("HttpRequestUriInvalid")
                    && !error_msg.contains("HttpRequestDenied"),
                "HTTPS request should not be denied by policy: {error_msg}"
            );
        }
    }

    // Try HTTP request to same host - should be denied
    let http_result = manager
        .execute_component_call(
            &component_id,
            "fetch",
            &serde_json::json!({"url": "http://example.com/"}).to_string(),
        )
        .await;

    match http_result {
        Err(e) => {
            let error_msg = e.to_string();
            println!("Expected HTTP denial: {error_msg}");

            assert!(
                error_msg.contains("HttpRequestDenied"),
                "Expected HTTP request to be denied when only HTTPS is allowed: {error_msg}"
            );
        }
        Ok(response) => {
            if response.contains("HttpRequestDenied") {
                println!("✅ HTTP request properly blocked when only HTTPS allowed!");
            } else {
                panic!(
                    "Expected HTTP request to be denied when only HTTPS is allowed, got: {response}"
                );
            }
        }
    }

    Ok(())
}
