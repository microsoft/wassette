// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::env;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::Value;
use tempfile::TempDir;
use test_log::test;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::process::Command as AsyncCommand;
use tokio::task::JoinHandle;

/// Helper struct for managing the test environment
struct RegistryTestContext {
    #[allow(dead_code)] // Needed to keep temp directory alive
    temp_dir: TempDir,
    plugin_dir: PathBuf,
    wassette_bin: PathBuf,
    wasm_directory_url: String,
    mock_server: JoinHandle<()>,
}

impl RegistryTestContext {
    async fn new() -> Result<Self> {
        let (wasm_directory_url, mock_server) = start_mock_wasm_directory().await?;
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
            wasm_directory_url,
            mock_server,
        })
    }

    /// Execute a wassette CLI command
    async fn run_command(&self, args: &[&str]) -> Result<(String, String, i32)> {
        self.run_command_with_directory_url(args, &self.wasm_directory_url)
            .await
    }

    async fn run_command_with_directory_url(
        &self,
        args: &[&str],
        directory_url: &str,
    ) -> Result<(String, String, i32)> {
        let mut cmd = AsyncCommand::new(&self.wassette_bin);
        cmd.args(args);
        // Note: registry search doesn't require --plugin-dir, but registry get does.
        // Only add plugin-dir for 'get' commands that support it.
        if args.contains(&"get") {
            cmd.arg("--plugin-dir").arg(&self.plugin_dir);
        }
        cmd.env("WASSETTE_WASM_DIRECTORY_URL", directory_url);

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

impl Drop for RegistryTestContext {
    fn drop(&mut self) {
        self.mock_server.abort();
    }
}

async fn start_mock_wasm_directory() -> Result<(String, JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut request = vec![0; 4096];
                let Ok(bytes_read) = socket.read(&mut request).await else {
                    return;
                };
                let request_line = String::from_utf8_lossy(&request[..bytes_read])
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let query = request_line
                    .split_once("?q=")
                    .and_then(|(_, query)| query.split('&').next())
                    .unwrap_or("");
                let body = if query.contains("weather") {
                    r#"[{"registry":"ghcr.io","repository":"microsoft/weather","kind":"component","description":"Weather tools","tags":["1.0.0"]}]"#
                } else if query.contains("rust") {
                    r#"[{"registry":"ghcr.io","repository":"microsoft/rust-tools","kind":"component","description":"Rust tools","tags":["1.0.0"]}]"#
                } else if query.is_empty() {
                    r#"[
                        {"registry":"ghcr.io","repository":"microsoft/weather","kind":"component","description":"Weather tools","tags":["1.0.0"]},
                        {"registry":"ghcr.io","repository":"microsoft/rust-tools","kind":"component","description":"Rust tools","tags":["1.0.0"]},
                        {"registry":"ghcr.io","repository":"microsoft/interfaces","kind":"interface","description":"WIT interfaces","tags":[]}
                    ]"#
                } else {
                    "[]"
                };
                let (status, body) = if request_line.starts_with("get /v1/packages/detail/") {
                    ("404 Not Found", "{}")
                } else {
                    ("200 OK", body)
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });

    Ok((format!("http://{address}"), server))
}

#[test(tokio::test)]
async fn test_registry_search_all() -> Result<()> {
    let ctx = RegistryTestContext::new().await?;

    let (stdout, stderr, exit_code) = ctx.run_command(&["registry", "search"]).await?;

    assert_eq!(exit_code, 0, "Command failed: {}", stderr);

    let json = ctx.parse_json_output(&stdout)?;
    assert_eq!(json["status"], "success");
    assert_eq!(json["source"], "wasm.directory");
    assert_eq!(json["discovery_only"], true);
    assert_eq!(json["count"], 2);
    assert_eq!(json["upstream_count"], 3);
    assert_eq!(json["may_have_more"], false);

    let components = json["components"].as_array().unwrap();
    assert_eq!(components.len(), 2);

    // Verify each component has required fields
    for component in components {
        assert!(component["package_id"].is_string());
        assert!(component["description"].is_string());
        assert_eq!(component["advertised_kind"], "component");
    }

    Ok(())
}

#[test(tokio::test)]
async fn test_registry_search_paginates_raw_upstream_records() -> Result<()> {
    let ctx = RegistryTestContext::new().await?;
    let (stdout, stderr, exit_code) = ctx
        .run_command(&["registry", "search", "--offset", "7", "--limit", "3"])
        .await?;

    assert_eq!(exit_code, 0, "Command failed: {}", stderr);
    let json = ctx.parse_json_output(&stdout)?;
    assert_eq!(json["offset"], 7);
    assert_eq!(json["limit"], 3);
    assert_eq!(json["upstream_count"], 3);
    assert_eq!(json["next_offset"], 10);
    assert_eq!(json["may_have_more"], true);
    assert_eq!(json["count"], 2);
    Ok(())
}

#[test(tokio::test)]
async fn test_registry_search_with_query() -> Result<()> {
    let ctx = RegistryTestContext::new().await?;

    let (stdout, stderr, exit_code) = ctx.run_command(&["registry", "search", "weather"]).await?;

    assert_eq!(exit_code, 0, "Command failed: {}", stderr);

    let json = ctx.parse_json_output(&stdout)?;
    assert_eq!(json["status"], "success");
    assert_eq!(json["count"], 1);

    let components = json["components"].as_array().unwrap();
    assert_eq!(components.len(), 1);
    assert_eq!(
        components[0]["package_id"].as_str(),
        Some("ghcr.io/microsoft/weather")
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_registry_search_case_insensitive() -> Result<()> {
    let ctx = RegistryTestContext::new().await?;

    let (stdout, stderr, exit_code) = ctx.run_command(&["registry", "search", "WEATHER"]).await?;

    assert_eq!(exit_code, 0, "Command failed: {}", stderr);

    let json = ctx.parse_json_output(&stdout)?;
    assert_eq!(json["status"], "success");
    assert_eq!(json["count"], 1);

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
async fn test_registry_search_does_not_fall_back_when_directory_is_unreachable() -> Result<()> {
    let ctx = RegistryTestContext::new().await?;
    let (stdout, stderr, exit_code) = ctx
        .run_command_with_directory_url(&["registry", "search"], "http://127.0.0.1:0")
        .await?;

    assert_ne!(exit_code, 0);
    assert!(
        stdout.contains("wasm.directory") || stderr.contains("wasm.directory"),
        "Expected explicit wasm.directory API error, got stdout={stdout:?}, stderr={stderr:?}"
    );
    Ok(())
}

#[test(tokio::test)]
async fn test_registry_search_rejects_oversized_pages() -> Result<()> {
    let ctx = RegistryTestContext::new().await?;
    let (_, stderr, exit_code) = ctx
        .run_command(&["registry", "search", "--limit", "101"])
        .await?;

    assert_ne!(exit_code, 0);
    assert!(stderr.contains("between 1 and 100"));
    Ok(())
}

#[test(tokio::test)]
async fn test_registry_search_matches_description() -> Result<()> {
    let ctx = RegistryTestContext::new().await?;

    let (stdout, stderr, exit_code) = ctx.run_command(&["registry", "search", "rust"]).await?;

    assert_eq!(exit_code, 0, "Command failed: {}", stderr);

    let json = ctx.parse_json_output(&stdout)?;
    assert_eq!(json["status"], "success");
    assert_eq!(json["count"], 1);

    Ok(())
}

#[test(tokio::test)]
async fn test_registry_get_reports_wasm_directory_not_found() -> Result<()> {
    let ctx = RegistryTestContext::new().await?;

    let (stdout, stderr, exit_code) = ctx
        .run_command(&["registry", "get", "ghcr.io/microsoft/nonexistent"])
        .await?;

    assert_ne!(exit_code, 0, "Command should have failed");
    assert!(
        stderr.contains("package detail API returned HTTP 404 Not Found")
            || stdout.contains("package detail API returned HTTP 404 Not Found"),
        "Expected wasm.directory package lookup error, got stdout={stdout:?}, stderr={stderr:?}"
    );

    Ok(())
}
