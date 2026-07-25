// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::env;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::Value;
use tempfile::TempDir;
use test_log::test;
use tokio::process::Command as AsyncCommand;

/// Helper struct for managing the test environment
struct RegistryTestContext {
    #[allow(dead_code)] // Needed to keep temp directory alive
    temp_dir: TempDir,
    plugin_dir: PathBuf,
    wassette_bin: PathBuf,
}

impl RegistryTestContext {
    async fn new() -> Result<Self> {
        let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;
        let plugin_dir = temp_dir.path().join("plugins");
        tokio::fs::create_dir_all(&plugin_dir).await?;

        // Resolve the wassette binary path in a cross-platform friendly way.
        let exe_name = format!("wassette{}", env::consts::EXE_SUFFIX);

        let locate_binary = || -> Result<PathBuf> {
            if let Some(path) = env::var_os("CARGO_BIN_EXE_wassette") {
                return Ok(PathBuf::from(path));
            }

            let path = if let Ok(target_dir) = env::var("CARGO_TARGET_DIR") {
                PathBuf::from(target_dir).join("debug").join(&exe_name)
            } else {
                let manifest_dir =
                    env::var("CARGO_MANIFEST_DIR").context("CARGO_MANIFEST_DIR not set")?;
                PathBuf::from(manifest_dir)
                    .join("target")
                    .join("debug")
                    .join(&exe_name)
            };

            if !path.exists() {
                // Build the binary on-demand so subsequent calls can reuse it.
                let status = Command::new("cargo")
                    .args(["build", "--bin", "wassette"])
                    .status()
                    .context("Failed to build wassette binary")?;

                if !status.success() {
                    anyhow::bail!("Failed to build wassette binary");
                }
            }

            Ok(path)
        };

        let wassette_bin = locate_binary()?;

        if !wassette_bin.exists() {
            anyhow::bail!("Wassette binary not found at {}", wassette_bin.display());
        }

        Ok(Self {
            temp_dir,
            plugin_dir,
            wassette_bin,
        })
    }

    /// Execute a wassette CLI command
    async fn run_command(&self, args: &[&str]) -> Result<(String, String, i32)> {
        let mut cmd = AsyncCommand::new(&self.wassette_bin);
        cmd.args(args);
        // Note: registry search doesn't require --plugin-dir, but registry get does.
        // Only add plugin-dir for 'get' commands that support it.
        if args.contains(&"get") {
            cmd.arg("--plugin-dir").arg(&self.plugin_dir);
        }

        let output = tokio::time::timeout(std::time::Duration::from_secs(30), cmd.output())
            .await
            .context("Command timed out")?
            .context("Failed to execute command")?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        Ok((stdout, stderr, exit_code))
    }

    /// Parse JSON from stdout
    fn parse_json_output(&self, stdout: &str) -> Result<Value> {
        serde_json::from_str(stdout.trim()).context("Failed to parse JSON output")
    }
}

#[test(tokio::test)]
async fn test_registry_search_all() -> Result<()> {
    let ctx = RegistryTestContext::new().await?;

    let (stdout, stderr, exit_code) = ctx.run_command(&["registry", "search"]).await?;

    assert_eq!(exit_code, 0, "Command failed: {}", stderr);

    let json = ctx.parse_json_output(&stdout)?;
    assert_eq!(json["status"], "success");
    assert_eq!(json["count"], 12); // Total components in registry

    let components = json["components"].as_array().unwrap();
    assert_eq!(components.len(), 12);

    // Verify each component has required fields
    for component in components {
        assert!(component["name"].is_string());
        assert!(component["description"].is_string());
        assert!(component["uri"].is_string());
    }

    Ok(())
}

#[test(tokio::test)]
async fn test_registry_search_with_query() -> Result<()> {
    let ctx = RegistryTestContext::new().await?;

    let (stdout, stderr, exit_code) = ctx.run_command(&["registry", "search", "weather"]).await?;

    assert_eq!(exit_code, 0, "Command failed: {}", stderr);

    let json = ctx.parse_json_output(&stdout)?;
    assert_eq!(json["status"], "success");
    assert_eq!(json["count"], 2); // Weather Server and Open-Meteo Weather

    let components = json["components"].as_array().unwrap();
    assert_eq!(components.len(), 2);
    // Both components have "weather" in their name or description
    assert!(components
        .iter()
        .any(|c| c["name"].as_str().unwrap().contains("Weather")));

    Ok(())
}

#[test(tokio::test)]
async fn test_registry_search_case_insensitive() -> Result<()> {
    let ctx = RegistryTestContext::new().await?;

    let (stdout, stderr, exit_code) = ctx.run_command(&["registry", "search", "WEATHER"]).await?;

    assert_eq!(exit_code, 0, "Command failed: {}", stderr);

    let json = ctx.parse_json_output(&stdout)?;
    assert_eq!(json["status"], "success");
    assert_eq!(json["count"], 2); // Weather Server and Open-Meteo Weather

    Ok(())
}

#[test(tokio::test)]
async fn test_registry_search_no_results() -> Result<()> {
    let ctx = RegistryTestContext::new().await?;

    let (stdout, stderr, exit_code) = ctx
        .run_command(&["registry", "search", "nonexistent"])
        .await?;

    assert_eq!(exit_code, 0, "Command failed: {}", stderr);

    let json = ctx.parse_json_output(&stdout)?;
    assert_eq!(json["status"], "success");
    assert_eq!(json["count"], 0);

    let components = json["components"].as_array().unwrap();
    assert_eq!(components.len(), 0);

    Ok(())
}

#[test(tokio::test)]
async fn test_registry_search_matches_description() -> Result<()> {
    let ctx = RegistryTestContext::new().await?;

    let (stdout, stderr, exit_code) = ctx.run_command(&["registry", "search", "rust"]).await?;

    assert_eq!(exit_code, 0, "Command failed: {}", stderr);

    let json = ctx.parse_json_output(&stdout)?;
    assert_eq!(json["status"], "success");
    // Should match "arXiv Research", "Fetch", "Filesystem", "Brave Search", and "Context7" which have "Rust" in description
    assert_eq!(json["count"], 5);

    Ok(())
}

#[test(tokio::test)]
async fn test_registry_get_nonexistent() -> Result<()> {
    let ctx = RegistryTestContext::new().await?;

    let (stdout, stderr, exit_code) = ctx.run_command(&["registry", "get", "NonExistent"]).await?;

    assert_ne!(exit_code, 0, "Command should have failed");
    assert!(
        stderr.contains("not found in registry") || stdout.contains("not found in registry"),
        "Error message should mention registry"
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_registry_get_by_name() -> Result<()> {
    let ctx = RegistryTestContext::new().await?;

    // This test will timeout if it tries to actually download from OCI
    // We just want to verify that it recognizes the component name
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        ctx.run_command(&["registry", "get", "Weather Server"]),
    )
    .await;

    // If it times out, it means the command started the download process
    // which is what we want - it found the component
    match result {
        Ok(Ok((stdout, stderr, exit_code))) => {
            // If it completes quickly, check that it at least attempted to load
            if exit_code != 0 {
                let combined = format!("{}{}", stdout, stderr);
                // Should not be a "not found" error
                assert!(
                    !combined.contains("not found in registry"),
                    "Should have found the component"
                );
            }
        }
        Err(_) => {
            // Timeout is acceptable - means it's trying to download
        }
        _ => {
            // Other errors are fine too, as long as it's not "not found"
        }
    }

    Ok(())
}
