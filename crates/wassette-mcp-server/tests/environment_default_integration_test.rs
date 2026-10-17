// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::json;
use test_log::test;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

mod common;
use common::build_brave_search_component;

async fn read_json_line(
    stdout: &mut BufReader<tokio::process::ChildStdout>,
    description: &str,
) -> Result<serde_json::Value> {
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(30), stdout.read_line(&mut line))
        .await
        .with_context(|| format!("Timeout waiting for {description}"))??;
    serde_json::from_str(&line).with_context(|| format!("Invalid {description}: {line}"))
}

async fn read_response_with_id(
    stdout: &mut BufReader<tokio::process::ChildStdout>,
    id: i64,
    description: &str,
) -> Result<serde_json::Value> {
    for _ in 0..10 {
        let value = read_json_line(stdout, description).await?;
        if value.get("id").and_then(serde_json::Value::as_i64) == Some(id) {
            return Ok(value);
        }
    }
    anyhow::bail!("Did not receive response id {id} while waiting for {description}")
}

#[test(tokio::test)]
async fn policy_free_run_does_not_inherit_the_host_environment() -> Result<()> {
    let component = build_brave_search_component().await?;
    let store = tempfile::tempdir()?;
    let binary = std::path::PathBuf::from(env!("CARGO_BIN_EXE_wassette"));
    let component_dir = format!("--component-dir={}", store.path().display());

    let load = Command::new(&binary)
        .args([
            "component",
            "load",
            &format!("file://{}", component.display()),
            &component_dir,
        ])
        .output()
        .await
        .context("Failed to load the environment-reading test component")?;
    assert!(
        load.status.success(),
        "component load failed: {}",
        String::from_utf8_lossy(&load.stderr)
    );

    let mut child = Command::new(&binary)
        .args(["run", &component_dir, "--disable-builtin-tools"])
        .env("BRAVE_SEARCH_API_KEY", "synthetic-canary-value")
        .env("RUST_LOG", "off")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to start wassette run")?;
    let mut stdin = child.stdin.take().context("Failed to get stdin")?;
    let mut stdout = BufReader::new(child.stdout.take().context("Failed to get stdout")?);

    stdin
        .write_all(
            format!(
                "{}\n",
                json!({
                    "jsonrpc": "2.0",
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {},
                        "clientInfo": {"name": "environment-default-test", "version": "1.0.0"}
                    },
                    "id": 1
                })
            )
            .as_bytes(),
        )
        .await?;
    stdin.flush().await?;
    let initialized = read_response_with_id(&mut stdout, 1, "initialize response").await?;
    assert_eq!(initialized["id"], 1);

    stdin
        .write_all(
            format!(
                "{}\n",
                json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized",
                    "params": {}
                })
            )
            .as_bytes(),
        )
        .await?;
    stdin
        .write_all(
            format!(
                "{}\n",
                json!({
                    "jsonrpc": "2.0",
                    "method": "tools/call",
                    "params": {"name": "search", "arguments": {"query": "synthetic-canary"}},
                    "id": 2
                })
            )
            .as_bytes(),
        )
        .await?;
    stdin.flush().await?;

    let response = read_response_with_id(&mut stdout, 2, "tool response").await?;
    assert_eq!(response["id"], 2);
    let result = response["result"]["structuredContent"]["result"]["err"]
        .as_str()
        .context("Missing component error result")?;
    assert_eq!(result, "BRAVE_SEARCH_API_KEY environment variable not set");

    drop(stdin);
    child.start_kill().context("Failed to stop wassette")?;
    tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .context("Timed out waiting for wassette to exit")??;
    Ok(())
}
