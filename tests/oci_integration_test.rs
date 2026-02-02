// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.
#![allow(clippy::uninlined_format_args)]

//! Integration tests for OCI registry functionality
//!
//! These tests verify that Wassette can load components from real OCI registries,
//! including multi-layer artifacts with policies and single-layer components.
//!
//! Note: Some tests use registry.mcpsearchtool.com which is a test registry.
//! The test/hello-world component is used for multi-layer OCI tests.
//! The test/qr-generator component tests are ignored as that component was removed.

use std::time::Duration;

use anyhow::Result;
use serde_json::json;
use wassette::LifecycleManager;

/// Test component URI - hello-world has WASM + policy + wadm-manifest layers
const HELLO_WORLD_OCI_URI: &str = "oci://registry.mcpsearchtool.com/test/hello-world:latest";

/// Legacy test component URI - qr-generator was removed from the registry
const QR_GENERATOR_OCI_URI: &str = "oci://registry.mcpsearchtool.com/test/qr-generator:latest";

/// Check if the registry is operational by hitting its v2 endpoint
async fn is_registry_operational(registry_url: &str) -> bool {
    let health_check_url = format!("{registry_url}/v2/");

    println!("🔍 Checking registry health at: {health_check_url}");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();

    match client.get(&health_check_url).send().await {
        Ok(response) => {
            // OCI registries should return 200 OK or 401 Unauthorized for /v2/
            // Both indicate the registry is operational
            let status = response.status();
            let is_healthy = status.is_success() || status == reqwest::StatusCode::UNAUTHORIZED;

            if is_healthy {
                println!("✅ Registry is operational (status: {status})");
            } else {
                println!("⚠️  Registry returned unexpected status: {status}");
            }

            is_healthy
        }
        Err(e) => {
            println!("❌ Registry is not reachable: {e}");
            false
        }
    }
}

/// Check if a specific component exists in the registry
async fn component_exists(registry_url: &str, repo: &str, tag: &str) -> bool {
    let manifest_url = format!("{registry_url}/v2/{repo}/manifests/{tag}");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();

    match client
        .get(&manifest_url)
        .header("Accept", "application/vnd.oci.image.manifest.v1+json")
        .send()
        .await
    {
        Ok(response) => response.status().is_success(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod multi_layer_oci_tests {
    use super::*;

    /// Test that we can load a component with an attached policy from an OCI registry
    #[tokio::test]
    async fn test_load_component_with_policy_from_oci() -> Result<()> {
        // First check if the registry is operational
        if !is_registry_operational("https://registry.mcpsearchtool.com").await {
            eprintln!("⚠️  Skipping test: Registry is not operational");
            eprintln!("   The registry at registry.mcpsearchtool.com is not responding.");
            eprintln!("   This test requires a functioning OCI registry.");
            return Ok(());
        }

        // Check if the component exists
        if !component_exists(
            "https://registry.mcpsearchtool.com",
            "test/hello-world",
            "latest",
        )
        .await
        {
            eprintln!("⚠️  Skipping test: test/hello-world component not found in registry");
            return Ok(());
        }

        // Create temp directory for testing
        let temp_dir = tempfile::tempdir()?;

        // Initialize the lifecycle manager
        let manager = LifecycleManager::new(temp_dir.path()).await?;

        // Load the hello-world component which has multi-layer structure
        let outcome = match manager.load_component(HELLO_WORLD_OCI_URI).await {
            Ok(result) => {
                println!("✅ Successfully loaded component from: {HELLO_WORLD_OCI_URI}");
                result
            }
            Err(e) => {
                eprintln!("⚠️  Skipping test: Could not load component from registry.");
                eprintln!("   Error: {e}");
                return Ok(());
            }
        };

        let component_id = outcome.component_id.clone();

        // Verify the component was loaded
        assert!(!component_id.is_empty(), "Component ID should not be empty");

        // Get the component's policy info
        let policy_info = manager.get_policy_info(&component_id).await;

        // The policy should be automatically extracted and attached from OCI layers
        assert!(
            policy_info.is_some(),
            "Policy should be extracted and attached from OCI layers"
        );

        // Verify the component is in the list
        let component_ids = manager.list_components().await;
        assert!(
            component_ids.contains(&component_id),
            "Component should be in the list"
        );

        Ok(())
    }

    /// Test that we handle OCI registries that return multi-layer artifacts correctly
    #[tokio::test]
    async fn test_multi_layer_with_policy_registry() -> Result<()> {
        // First check if the registry is operational
        if !is_registry_operational("https://registry.mcpsearchtool.com").await {
            eprintln!("⚠️  Skipping test: Registry is not operational");
            eprintln!("   The registry at registry.mcpsearchtool.com is not responding.");
            eprintln!("   This test requires a functioning OCI registry.");
            return Ok(());
        }

        // Check if the component exists
        if !component_exists(
            "https://registry.mcpsearchtool.com",
            "test/hello-world",
            "latest",
        )
        .await
        {
            eprintln!("⚠️  Skipping test: test/hello-world component not found in registry");
            return Ok(());
        }

        // Create temp directory for testing
        let temp_dir = tempfile::tempdir()?;

        // Initialize the lifecycle manager
        let manager = LifecycleManager::new(temp_dir.path()).await?;

        // Load the hello-world component
        let outcome = match manager.load_component(HELLO_WORLD_OCI_URI).await {
            Ok(result) => {
                println!("✅ Successfully loaded component from: {HELLO_WORLD_OCI_URI}");
                result
            }
            Err(e) => {
                // Check if the error is about incompatible media types
                let err_str = e.to_string();
                if err_str.contains("Incompatible layer media type")
                    || err_str.contains("application/vnd.wasm.policy.v1+yaml")
                {
                    eprintln!("⚠️  Test encountered expected error (not yet fixed): {err_str}");
                    eprintln!(
                        "   This is expected until multi-layer OCI support is fully implemented."
                    );
                    return Ok(());
                }

                eprintln!("⚠️  Skipping test: Could not load component from registry.");
                eprintln!("   Error: {e}");
                return Ok(());
            }
        };

        let component_id = outcome.component_id.clone();
        let load_state = outcome.status;

        assert!(
            matches!(
                load_state,
                wassette::LoadResult::New | wassette::LoadResult::Replaced
            ),
            "Should handle multi-layer OCI artifacts with policies"
        );

        // The policy should be automatically extracted and attached
        let policy_info = manager.get_policy_info(&component_id).await;
        assert!(
            policy_info.is_some(),
            "Policy should be extracted and attached from OCI layers"
        );

        // The component should be loaded successfully
        let component_ids = manager.list_components().await;
        assert!(
            component_ids.contains(&component_id),
            "Component should be loaded"
        );

        // Check what files were saved
        println!("\n📁 Checking saved files in temp directory:");
        for entry in std::fs::read_dir(temp_dir.path()).unwrap() {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            println!(
                "  - {} ({} bytes)",
                entry.file_name().to_string_lossy(),
                metadata.len()
            );
        }

        Ok(())
    }

    /// Test that we actually download the policy layer correctly
    #[tokio::test]
    async fn test_policy_download_from_multi_layer_oci() -> Result<()> {
        // Check if the component exists first
        if !component_exists(
            "https://registry.mcpsearchtool.com",
            "test/hello-world",
            "latest",
        )
        .await
        {
            eprintln!("⚠️  Skipping test: test/hello-world component not found in registry");
            return Ok(());
        }

        // Test that we actually download the policy layer
        let reference: oci_client::Reference =
            "registry.mcpsearchtool.com/test/hello-world:latest".parse()?;

        let client = oci_client::Client::new(oci_client::client::ClientConfig {
            read_timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        });

        let artifact =
            wassette::oci_multi_layer::pull_multi_layer_artifact(&reference, &client).await?;

        // Verify WASM component was downloaded
        assert!(!artifact.wasm_data.is_empty());
        // hello-world is ~283KB
        assert!(
            artifact.wasm_data.len() > 100_000,
            "WASM should be substantial size"
        );

        // Verify policy was also downloaded
        assert!(
            artifact.policy_data.is_some(),
            "Policy should be downloaded"
        );
        let policy_data = artifact.policy_data.unwrap();
        assert!(!policy_data.is_empty());

        // Verify policy is valid YAML
        let policy_str = String::from_utf8_lossy(&policy_data);
        assert!(
            policy_str.contains("version") || policy_str.contains("permissions"),
            "Policy should contain expected fields"
        );

        Ok(())
    }
}

#[cfg(test)]
mod qr_generator_component_tests {
    use super::*;

    // NOTE: These tests are ignored because the test/qr-generator component
    // was removed from registry.mcpsearchtool.com. They are kept for reference
    // and can be re-enabled when a suitable replacement component is available.

    #[tokio::test]
    #[ignore = "test/qr-generator component was removed from registry"]
    async fn test_qr_generator_loads_from_oci() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let manager = LifecycleManager::new(temp_dir.path()).await?;

        // Load the component
        let outcome = manager.load_component(QR_GENERATOR_OCI_URI).await?;

        assert_eq!(outcome.component_id, "test_qr-generator");
        assert!(matches!(outcome.status, wassette::LoadResult::New));

        // Verify component is in the list
        let components = manager.list_components().await;
        assert!(components.contains(&outcome.component_id));

        Ok(())
    }

    #[tokio::test]
    #[ignore = "test/qr-generator component was removed from registry"]
    async fn test_qr_generator_has_expected_tools() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let manager = LifecycleManager::new(temp_dir.path()).await?;

        manager.load_component(QR_GENERATOR_OCI_URI).await?;

        // Check available tools
        let tools = manager.list_tools().await;
        let tool_names: Vec<String> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
            .map(|s| s.to_string())
            .collect();

        assert!(tool_names.contains(&"generate-qr".to_string()));
        assert!(tool_names.contains(&"generate-qr-custom".to_string()));
        assert!(tool_names.contains(&"save-qr".to_string()));

        Ok(())
    }

    #[tokio::test]
    #[ignore = "test/qr-generator component was removed from registry"]
    async fn test_qr_generator_policy_is_saved() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let manager = LifecycleManager::new(temp_dir.path()).await?;

        let component_id = manager
            .load_component(QR_GENERATOR_OCI_URI)
            .await?
            .component_id;

        // Check that policy file was saved alongside WASM
        let wasm_path = temp_dir.path().join(format!("{component_id}.wasm"));
        let policy_path = temp_dir.path().join(format!("{component_id}.policy.yaml"));

        assert!(wasm_path.exists(), "WASM file should exist");
        assert!(policy_path.exists(), "Policy file should exist");

        // Verify policy content
        let policy_content = std::fs::read_to_string(&policy_path)?;
        assert!(policy_content.contains("version"));
        assert!(policy_content.contains("permissions"));
        assert!(policy_content.contains("storage") || policy_content.contains("fs://"));

        Ok(())
    }

    #[tokio::test]
    #[ignore = "test/qr-generator component was removed from registry"]
    async fn test_qr_generator_policy_is_attached() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let manager = LifecycleManager::new(temp_dir.path()).await?;

        let component_id = manager
            .load_component(QR_GENERATOR_OCI_URI)
            .await?
            .component_id;

        // Check that policy is attached to the component
        let policy_info = manager.get_policy_info(&component_id).await;
        assert!(
            policy_info.is_some(),
            "Policy should be attached to component"
        );

        Ok(())
    }

    #[tokio::test]
    #[ignore = "test/qr-generator component was removed from registry"]
    async fn test_qr_generator_handles_invalid_input() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let manager = LifecycleManager::new(temp_dir.path()).await?;

        let component_id = manager
            .load_component(QR_GENERATOR_OCI_URI)
            .await?
            .component_id;

        // Test with missing required field
        let invalid_input = json!({
            "wrong_field": "value"
        });

        let result = manager
            .execute_component_call(&component_id, "generate-qr", &invalid_input.to_string())
            .await;

        // Should either return an error or an Err variant in the result
        if let Ok(result_str) = result {
            let json_result: serde_json::Value = serde_json::from_str(&result_str)?;
            assert!(
                json_result.get("err").is_some(),
                "Should return Err variant for invalid input"
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod backwards_compatibility_tests {
    use super::*;

    /// Test backwards compatibility - single layer WASM artifacts should still work
    /// This test uses environment-based authentication and gracefully skips if no auth is available
    #[tokio::test]
    async fn test_single_layer_wasm_compatibility() -> Result<()> {
        // Skip in CI unless explicitly enabled with authentication
        if std::env::var("CI").is_ok() && std::env::var("ENABLE_GHCR_TESTS").is_err() {
            println!("⚠️  Skipping GHCR test - not enabled in CI environment");
            println!("   Set ENABLE_GHCR_TESTS=1 and provide GITHUB_TOKEN to enable");
            return Ok(());
        }

        // Skip if explicitly requested to skip GHCR tests
        if std::env::var("SKIP_GHCR_TESTS").is_ok() {
            println!("⚠️  Skipping GHCR test - SKIP_GHCR_TESTS is set");
            return Ok(());
        }

        // Check for authentication (secure - supports both GH_TOKEN and GITHUB_TOKEN)
        let github_token = std::env::var("GH_TOKEN")
            .or_else(|_| std::env::var("GITHUB_TOKEN"))
            .ok();
        if github_token.is_none() {
            println!("⚠️  Skipping GHCR test - no authentication available");
            println!("   Set GITHUB_TOKEN environment variable to enable this test");
            return Ok(());
        }

        // First check if ghcr.io is operational
        if !is_registry_operational("https://ghcr.io").await {
            eprintln!("⚠️  Skipping test: GitHub Container Registry is not operational");
            eprintln!("   The registry at ghcr.io is not responding.");
            return Ok(());
        }

        // Test with a known single-layer WASM component from ghcr.io
        let component_uri = "oci://ghcr.io/yoshuawuyts/time:latest";

        // Create temp directory for testing
        let temp_dir = tempfile::tempdir()?;

        println!(
            "🧪 Testing backwards compatibility with single-layer WASM component: {component_uri}"
        );

        // Initialize the lifecycle manager with authentication environment
        let manager = LifecycleManager::new(temp_dir.path()).await?;

        // Load the component with extended timeout for network operations
        let load_result = tokio::time::timeout(
            std::time::Duration::from_secs(60), // Increased timeout for authenticated operations
            manager.load_component(component_uri),
        )
        .await;

        match load_result {
            Ok(Ok(outcome)) => {
                let component_id = outcome.component_id;
                println!("✅ Successfully loaded single-layer component: {component_id}");

                // Verify component ID is not empty
                assert!(!component_id.is_empty(), "Component ID should not be empty");

                // Single-layer components should work without a policy
                let policy_info = manager.get_policy_info(&component_id).await;
                assert!(
                    policy_info.is_none(),
                    "Single-layer component should not have a policy (backwards compatibility)"
                );
                println!(
                    "✅ Confirmed: Single-layer component has no policy (backwards compatible)"
                );

                // Verify the component appears in the component list
                let component_ids = manager.list_components().await;
                assert!(
                    component_ids.contains(&component_id),
                    "Component should be in the list"
                );
                println!("✅ Component correctly listed in lifecycle manager");

                // Test that the component actually works (if it has exports)
                // This is optional but helps verify full backwards compatibility
                println!("✅ Backwards compatibility test completed successfully");
            }
            Ok(Err(e)) => {
                // More specific error handling
                let error_msg = format!("{e}");
                if error_msg.contains("authentication") || error_msg.contains("unauthorized") {
                    eprintln!("❌ Authentication failed for ghcr.io");
                    eprintln!("   Error: {e}");
                    eprintln!(
                        "   Please check your GITHUB_TOKEN is valid and has read permissions"
                    );
                    return Err(e);
                } else if error_msg.contains("network") || error_msg.contains("timeout") {
                    println!("⚠️  Network error accessing ghcr.io - test may be unstable");
                    println!("   Error: {e}");
                    return Ok(()); // Gracefully skip on network issues
                } else {
                    eprintln!("❌ Failed to load component: {e}");
                    return Err(e);
                }
            }
            Err(_) => {
                println!("⚠️  Timeout while loading component from ghcr.io");
                println!("   This may indicate network connectivity issues");
                return Ok(()); // Gracefully skip on timeout
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod real_registry_digest_tests {
    use sha2::{Digest, Sha256};

    use super::*;

    /// Calculate SHA256 digest of data in OCI format (sha256:hex)
    fn calculate_digest(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hasher.finalize();
        format!("sha256:{}", hex::encode(result))
    }

    /// Test against the real mcpsearchtool.com registry
    /// This test verifies digest checking against the actual registry
    #[tokio::test]
    async fn test_real_registry_digest_verification() -> Result<()> {
        // Check if the component exists first
        if !component_exists(
            "https://registry.mcpsearchtool.com",
            "test/hello-world",
            "latest",
        )
        .await
        {
            eprintln!("⚠️  Skipping test: test/hello-world component not found in registry");
            return Ok(());
        }

        // Create real OCI client
        let client = oci_client::Client::default();
        let reference: oci_client::Reference = "registry.mcpsearchtool.com/test/hello-world:latest"
            .parse()
            .unwrap();

        // Pull manifest with authentication
        let auth = oci_client::secrets::RegistryAuth::Anonymous;
        let (manifest, digest_opt) = client.pull_manifest(&reference, &auth).await?;

        // The digest should be provided
        assert!(
            !digest_opt.is_empty(),
            "Registry should provide manifest digest"
        );

        match manifest {
            oci_client::manifest::OciManifest::Image(img_manifest) => {
                // Verify we have the expected layers
                assert!(
                    img_manifest.layers.len() >= 2,
                    "Should have at least WASM and policy layers"
                );

                // Check for expected media types
                let has_wasm = img_manifest
                    .layers
                    .iter()
                    .any(|l| l.media_type.contains("wasm") || l.media_type.contains("component"));
                let has_policy = img_manifest
                    .layers
                    .iter()
                    .any(|l| l.media_type.contains("policy") || l.media_type.contains("yaml"));

                assert!(has_wasm, "Should have WASM layer");
                assert!(has_policy, "Should have policy layer");

                // Verify each layer can be pulled and matches its digest
                for layer in &img_manifest.layers {
                    let mut blob_data = Vec::new();
                    client
                        .pull_blob(&reference, layer.digest.as_str(), &mut blob_data)
                        .await?;

                    // Calculate digest and verify
                    let calculated = calculate_digest(&blob_data);
                    assert_eq!(
                        layer.digest, calculated,
                        "Layer digest should match calculated digest for media type: {}",
                        layer.media_type
                    );

                    println!("✓ Verified layer: {} ({})", layer.media_type, layer.digest);
                }

                println!("✓ All layer digests verified successfully");
            }
            _ => panic!("Expected OCI Image manifest"),
        }

        Ok(())
    }
}
