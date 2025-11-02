// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.
use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;
use tempfile::TempDir;
use test_log::test;
use tokio::process::Command as AsyncCommand;

mod common;
use common::build_fetch_component;

/// Helper struct for managing the test environment
struct CliTestContext {
    #[allow(dead_code)] // Needed to keep temp directory alive
    temp_dir: TempDir,
    plugin_dir: PathBuf,
    wassette_bin: PathBuf,
}

impl CliTestContext {
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
        cmd.arg("--plugin-dir").arg(&self.plugin_dir);

        let output = tokio::time::timeout(Duration::from_secs(120), cmd.output())
            .await
            .context("Command timed out")?
            .context("Failed to execute command")?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        Ok((stdout, stderr, exit_code))
    }

    /// Execute a wassette CLI command without --plugin-dir (for commands that don't need it)
    async fn run_command_no_plugin_dir(&self, args: &[&str]) -> Result<(String, String, i32)> {
        let mut cmd = AsyncCommand::new(&self.wassette_bin);
        cmd.args(args);

        let output = tokio::time::timeout(Duration::from_secs(120), cmd.output())
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
async fn test_cli_component_list_empty() -> Result<()> {
    let ctx = CliTestContext::new().await?;

    let (stdout, stderr, exit_code) = ctx.run_command(&["component", "list"]).await?;

    assert_eq!(exit_code, 0, "Command failed with stderr: {stderr}");

    let output: Value = ctx.parse_json_output(&stdout)?;
    assert_eq!(output["components"].as_array().unwrap().len(), 0);
    assert_eq!(output["total"], 0);

    Ok(())
}

#[test(tokio::test)]
async fn test_cli_component_load_and_list() -> Result<()> {
    let ctx = CliTestContext::new().await?;
    let component_path = build_fetch_component().await?;

    // Load the component
    let (stdout, stderr, exit_code) = ctx
        .run_command(&[
            "component",
            "load",
            &format!("file://{}", component_path.display()),
        ])
        .await?;

    assert_eq!(exit_code, 0, "Load command failed with stderr: {stderr}");

    let load_output: Value = ctx.parse_json_output(&stdout)?;
    assert_eq!(load_output["status"], "component loaded successfully");
    assert!(load_output["id"].is_string());
    assert!(load_output["tools"].is_array());

    let component_id = load_output["id"].as_str().unwrap();

    // List components to verify it was loaded
    let (stdout, stderr, exit_code) = ctx.run_command(&["component", "list"]).await?;

    assert_eq!(exit_code, 0, "List command failed with stderr: {stderr}");

    let list_output: Value = ctx.parse_json_output(&stdout)?;
    assert_eq!(list_output["total"], 1);
    assert_eq!(list_output["components"][0]["id"], component_id);
    assert!(
        list_output["components"][0]["tools_count"]
            .as_u64()
            .unwrap()
            > 0
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_cli_component_load_unload() -> Result<()> {
    let ctx = CliTestContext::new().await?;
    let component_path = build_fetch_component().await?;

    // Load the component
    let (stdout, stderr, exit_code) = ctx
        .run_command(&[
            "component",
            "load",
            &format!("file://{}", component_path.display()),
        ])
        .await?;

    assert_eq!(exit_code, 0, "Load command failed with stderr: {stderr}");

    let load_output: Value = ctx.parse_json_output(&stdout)?;
    let component_id = load_output["id"].as_str().unwrap();
    assert!(load_output["tools"].is_array());

    // Unload the component
    let (stdout, stderr, exit_code) = ctx
        .run_command(&["component", "unload", component_id])
        .await?;

    assert_eq!(exit_code, 0, "Unload command failed with stderr: {stderr}");

    let unload_output: Value = ctx.parse_json_output(&stdout)?;
    assert_eq!(unload_output["status"], "component unloaded successfully");
    assert_eq!(unload_output["id"], component_id);

    // Verify component is no longer listed
    let (stdout, stderr, exit_code) = ctx.run_command(&["component", "list"]).await?;

    assert_eq!(
        exit_code, 0,
        "List command after unload failed with stderr: {stderr}"
    );

    let list_output: Value = ctx.parse_json_output(&stdout)?;
    assert_eq!(list_output["total"], 0);

    Ok(())
}

#[test(tokio::test)]
async fn test_cli_component_load_invalid_path() -> Result<()> {
    let ctx = CliTestContext::new().await?;

    let (stdout, stderr, exit_code) = ctx
        .run_command(&["component", "load", "file:///nonexistent/path.wasm"])
        .await?;

    assert_ne!(exit_code, 0, "Command should have failed");
    assert!(
        stderr.contains("Failed to load component") || stdout.contains("Failed to load component")
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_cli_component_unload_invalid_id() -> Result<()> {
    let ctx = CliTestContext::new().await?;

    let (_stdout, _stderr, exit_code) = ctx
        .run_command(&["component", "unload", "nonexistent-component"])
        .await?;

    assert_eq!(exit_code, 0, "Command should succeed (idempotent behavior)");
    // Unloading a non-existent component should succeed due to idempotent behavior

    Ok(())
}

#[test(tokio::test)]
async fn test_cli_policy_get_nonexistent_component() -> Result<()> {
    let ctx = CliTestContext::new().await?;

    let (stdout, stderr, exit_code) = ctx
        .run_command(&["policy", "get", "nonexistent-component"])
        .await?;

    assert_ne!(exit_code, 0, "Command should have failed");
    assert!(stderr.contains("Component not found") || stdout.contains("Component not found"));

    Ok(())
}

#[test(tokio::test)]
async fn test_cli_permission_grant_storage() -> Result<()> {
    let ctx = CliTestContext::new().await?;
    let component_path = build_fetch_component().await?;

    // Load the component first
    let (stdout, _, exit_code) = ctx
        .run_command(&[
            "component",
            "load",
            &format!("file://{}", component_path.display()),
        ])
        .await?;

    assert_eq!(exit_code, 0);
    let load_output: Value = ctx.parse_json_output(&stdout)?;
    let component_id = load_output["id"].as_str().unwrap();

    // Grant storage permission
    let (stdout, stderr, exit_code) = ctx
        .run_command(&[
            "permission",
            "grant",
            "storage",
            component_id,
            "fs:///tmp/test",
            "--access",
            "read,write",
        ])
        .await?;

    assert_eq!(
        exit_code, 0,
        "Grant storage permission failed with stderr: {stderr}"
    );

    let permission_output: Value = ctx.parse_json_output(&stdout)?;
    assert_eq!(
        permission_output["status"],
        "permission granted successfully"
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_cli_permission_grant_network() -> Result<()> {
    let ctx = CliTestContext::new().await?;
    let component_path = build_fetch_component().await?;

    // Load the component first
    let (stdout, _, exit_code) = ctx
        .run_command(&[
            "component",
            "load",
            &format!("file://{}", component_path.display()),
        ])
        .await?;

    assert_eq!(exit_code, 0);
    let load_output: Value = ctx.parse_json_output(&stdout)?;
    let component_id = load_output["id"].as_str().unwrap();

    // Grant network permission
    let (stdout, stderr, exit_code) = ctx
        .run_command(&[
            "permission",
            "grant",
            "network",
            component_id,
            "example.com",
        ])
        .await?;

    assert_eq!(
        exit_code, 0,
        "Grant network permission failed with stderr: {stderr}"
    );

    let permission_output: Value = ctx.parse_json_output(&stdout)?;
    assert_eq!(
        permission_output["status"],
        "permission granted successfully"
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_cli_permission_grant_environment_variable() -> Result<()> {
    let ctx = CliTestContext::new().await?;
    let component_path = build_fetch_component().await?;

    // Load the component first
    let (stdout, _, exit_code) = ctx
        .run_command(&[
            "component",
            "load",
            &format!("file://{}", component_path.display()),
        ])
        .await?;

    assert_eq!(exit_code, 0);
    let load_output: Value = ctx.parse_json_output(&stdout)?;
    let component_id = load_output["id"].as_str().unwrap();

    // Grant environment variable permission
    let (stdout, stderr, exit_code) = ctx
        .run_command(&[
            "permission",
            "grant",
            "environment-variable",
            component_id,
            "TEST_VAR",
        ])
        .await?;

    assert_eq!(
        exit_code, 0,
        "Grant env var permission failed with stderr: {stderr}"
    );

    let permission_output: Value = ctx.parse_json_output(&stdout)?;
    assert_eq!(
        permission_output["status"],
        "permission granted successfully"
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_cli_permission_revoke_and_reset() -> Result<()> {
    let ctx = CliTestContext::new().await?;
    let component_path = build_fetch_component().await?;

    // Load the component first
    let (stdout, _, exit_code) = ctx
        .run_command(&[
            "component",
            "load",
            &format!("file://{}", component_path.display()),
        ])
        .await?;

    assert_eq!(exit_code, 0);
    let load_output: Value = ctx.parse_json_output(&stdout)?;
    let component_id = load_output["id"].as_str().unwrap();

    // Grant storage permission
    let (_, stderr, exit_code) = ctx
        .run_command(&[
            "permission",
            "grant",
            "storage",
            component_id,
            "fs:///tmp/test",
            "--access",
            "read",
        ])
        .await?;

    assert_eq!(
        exit_code, 0,
        "Grant storage permission failed with stderr: {stderr}"
    );

    // Revoke storage permission
    let (stdout, stderr, exit_code) = ctx
        .run_command(&[
            "permission",
            "revoke",
            "storage",
            component_id,
            "fs:///tmp/test",
        ])
        .await?;

    assert_eq!(
        exit_code, 0,
        "Revoke storage permission failed with stderr: {stderr}"
    );

    let revoke_output: Value = ctx.parse_json_output(&stdout)?;
    assert_eq!(revoke_output["status"], "permission revoked successfully");

    // Reset all permissions
    let (stdout, stderr, exit_code) = ctx
        .run_command(&["permission", "reset", component_id])
        .await?;

    assert_eq!(
        exit_code, 0,
        "Reset permissions failed with stderr: {stderr}"
    );

    let reset_output: Value = ctx.parse_json_output(&stdout)?;
    assert_eq!(reset_output["status"], "permissions reset successfully");

    Ok(())
}

#[test(tokio::test)]
async fn test_cli_json_output_default() -> Result<()> {
    let ctx = CliTestContext::new().await?;

    let (stdout, stderr, exit_code) = ctx.run_command(&["component", "list"]).await?;

    assert_eq!(exit_code, 0, "Command failed with stderr: {stderr}");

    // Verify the output is valid JSON and pretty-formatted (contains newlines and indentation)
    let _: Value = ctx.parse_json_output(&stdout)?;
    assert!(
        stdout.contains('\n'),
        "JSON output should contain newlines by default"
    );
    assert!(
        stdout.contains("  "),
        "JSON output should contain indentation by default"
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_cli_output_format_json() -> Result<()> {
    let ctx = CliTestContext::new().await?;

    let (stdout, stderr, exit_code) = ctx
        .run_command(&["component", "list", "-o", "json"])
        .await?;

    assert_eq!(exit_code, 0, "Command failed with stderr: {stderr}");

    // Verify the output is valid JSON and pretty-formatted
    let _: Value = ctx.parse_json_output(&stdout)?;
    assert!(stdout.contains('\n'), "JSON output should contain newlines");
    assert!(
        stdout.contains("  "),
        "JSON output should contain indentation"
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_cli_output_format_yaml() -> Result<()> {
    let ctx = CliTestContext::new().await?;

    let (stdout, stderr, exit_code) = ctx
        .run_command(&["component", "list", "-o", "yaml"])
        .await?;

    assert_eq!(exit_code, 0, "Command failed with stderr: {stderr}");

    // YAML output should contain YAML formatting indicators
    assert!(
        stdout.contains("components:") || stdout.contains("total:"),
        "YAML output should contain YAML-formatted keys"
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_cli_output_format_table() -> Result<()> {
    let ctx = CliTestContext::new().await?;

    let (stdout, stderr, exit_code) = ctx
        .run_command(&["component", "list", "-o", "table"])
        .await?;

    assert_eq!(exit_code, 0, "Command failed with stderr: {stderr}");

    // Table output should contain table headers
    assert!(
        stdout.contains("ID") && stdout.contains("Tools Count"),
        "Table output should contain table headers"
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_cli_version_command() -> Result<()> {
    let ctx = CliTestContext::new().await?;

    let (stdout, stderr, exit_code) = ctx.run_command(&["--version"]).await?;

    assert_eq!(exit_code, 0, "Version command failed with stderr: {stderr}");
    assert!(
        stdout.contains("version.BuildInfo"),
        "Version output should contain build info"
    );
    assert!(
        stdout.contains("RustVersion"),
        "Version output should contain Rust version"
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_cli_help_command() -> Result<()> {
    let ctx = CliTestContext::new().await?;

    let (stdout, stderr, exit_code) = ctx.run_command(&["--help"]).await?;

    assert_eq!(exit_code, 0, "Help command failed with stderr: {stderr}");
    assert!(
        stdout.contains("component"),
        "Help should contain component subcommand"
    );
    assert!(
        stdout.contains("policy"),
        "Help should contain policy subcommand"
    );
    assert!(
        stdout.contains("permission"),
        "Help should contain permission subcommand"
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_cli_invalid_command() -> Result<()> {
    let ctx = CliTestContext::new().await?;

    let (_, stderr, exit_code) = ctx.run_command(&["invalid-command"]).await?;

    assert_ne!(exit_code, 0, "Invalid command should fail");
    assert!(stderr.contains("unrecognized subcommand") || stderr.contains("invalid"));

    Ok(())
}

#[test(tokio::test)]
async fn test_cli_secret_set_component_not_found() -> Result<()> {
    let ctx = CliTestContext::new().await?;

    // Try to set secrets for a non-existent component
    let (stdout, stderr, exit_code) = ctx
        .run_command(&["secret", "set", "non-existent-component", "KEY=value"])
        .await?;

    assert_ne!(
        exit_code, 0,
        "Command should fail for non-existent component"
    );
    assert!(
        stderr.contains("Component not found") || stdout.contains("Component not found"),
        "Error message should indicate component not found. stdout: {}, stderr: {}",
        stdout,
        stderr
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_cli_secret_set_and_list() -> Result<()> {
    let ctx = CliTestContext::new().await?;
    let component_path = build_fetch_component().await?;

    // Load the component first
    let (stdout, _, exit_code) = ctx
        .run_command(&[
            "component",
            "load",
            &format!("file://{}", component_path.display()),
        ])
        .await?;

    assert_eq!(exit_code, 0);
    let load_output: Value = ctx.parse_json_output(&stdout)?;
    let component_id = load_output["id"]
        .as_str()
        .expect("Load output should contain 'id' field");

    // Set secrets for the component
    let (stdout, stderr, exit_code) = ctx
        .run_command(&[
            "secret",
            "set",
            component_id,
            "API_KEY=secret123",
            "REGION=us-west-2",
        ])
        .await?;

    assert_eq!(
        exit_code, 0,
        "Secret set should succeed for existing component. stderr: {}",
        stderr
    );

    let secret_output: Value = ctx.parse_json_output(&stdout)?;
    assert_eq!(secret_output["status"], "success");
    assert_eq!(secret_output["component_id"], component_id);

    // List secrets to verify they were set
    let (stdout, stderr, exit_code) = ctx.run_command(&["secret", "list", component_id]).await?;

    assert_eq!(
        exit_code, 0,
        "Secret list should succeed. stderr: {}",
        stderr
    );

    let list_output: Value = ctx.parse_json_output(&stdout)?;
    assert_eq!(list_output["component_id"], component_id);
    let secrets = list_output["secrets"]
        .as_array()
        .expect("List output should contain 'secrets' array");
    assert_eq!(secrets.len(), 2);

    // Verify the keys are present (values should not be shown without --show-values)
    let keys: Vec<&str> = secrets
        .iter()
        .map(|s| s["key"].as_str().expect("Secret should have 'key' field"))
        .collect();
    assert!(keys.contains(&"API_KEY"));
    assert!(keys.contains(&"REGION"));

    Ok(())
}

#[test(tokio::test)]
async fn test_cli_inspect_component() -> Result<()> {
    let ctx = CliTestContext::new().await?;
    let component_path = build_fetch_component().await?;

    // Run inspect command on the component
    let (stdout, stderr, exit_code) = ctx
        .run_command_no_plugin_dir(&["inspect", component_path.to_str().unwrap()])
        .await?;

    assert_eq!(exit_code, 0, "Inspect command failed with stderr: {stderr}");

    // Verify the output contains expected schema information
    assert!(
        stdout.contains("input schema:"),
        "Output should contain input schema"
    );
    assert!(
        stdout.contains("output schema:"),
        "Output should contain output schema"
    );
    assert!(
        stdout.contains("fetch"),
        "Output should contain function name 'fetch'"
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_cli_inspect_invalid_path() -> Result<()> {
    let ctx = CliTestContext::new().await?;

    // Try to inspect a non-existent file
    let (_, stderr, exit_code) = ctx
        .run_command_no_plugin_dir(&["inspect", "/nonexistent/path.wasm"])
        .await?;

    assert_ne!(exit_code, 0, "Command should fail for invalid path");
    assert!(
        stderr.contains("Error") || stderr.contains("Failed"),
        "Error message should indicate failure"
    );

    Ok(())
}
