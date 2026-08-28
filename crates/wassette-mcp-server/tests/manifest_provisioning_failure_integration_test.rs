// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};

mod common;

const STATELESS_VERSION: &str = "2026-07-28";

async fn find_open_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("failed to bind a random loopback port")?;
    Ok(listener.local_addr()?.port())
}

async fn wait_until_listening(port: u16) -> Result<()> {
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("wassette did not start listening on 127.0.0.1:{port}")
}

struct ServerGuard(Child);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.start_kill();
    }
}

fn spawn_server(port: u16, component_dir: &Path, extra_args: &[&str]) -> Result<Child> {
    let bind_address = format!("127.0.0.1:{port}");
    let component_dir = format!("--component-dir={}", component_dir.display());
    let mut args = vec![
        "serve",
        "--streamable-http",
        "--bind-address",
        &bind_address,
        &component_dir,
        "--disable-builtin-tools",
    ];
    args.extend_from_slice(extra_args);

    Command::new(env!("CARGO_BIN_EXE_wassette"))
        .args(&args)
        .env("RUST_LOG", "error")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn `wassette serve --streamable-http`")
}

/// Write a manifest with one component that can be provisioned and one that cannot,
/// so the failure needs no network access.
async fn write_mixed_manifest(dir: &Path, good_component: &Path) -> Result<String> {
    let missing = dir.join("definitely-missing.wasm");
    let manifest_path = dir.join("manifest.yaml");
    tokio::fs::write(
        &manifest_path,
        format!(
            "version: 1\ncomponents:\n  - uri: file://{}\n    name: fetch\n    permissions: {{}}\n  - uri: file://{}\n    name: missing\n    permissions: {{}}\n",
            good_component.display(),
            missing.display()
        ),
    )
    .await?;

    Ok(manifest_path
        .to_str()
        .context("manifest path was not valid UTF-8")?
        .to_string())
}

#[tokio::test]
async fn provisioning_failure_aborts_startup_by_default() -> Result<()> {
    let component_path = common::build_fetch_component().await?;
    let temp_dir = tempfile::tempdir()?;
    let manifest = write_mixed_manifest(temp_dir.path(), &component_path).await?;

    let port = find_open_port().await?;
    let mut child = spawn_server(port, temp_dir.path(), &["--manifest", &manifest])?;

    let status = tokio::time::timeout(Duration::from_secs(60), child.wait())
        .await
        .context("wassette kept running despite a provisioning failure")?
        .context("failed to await the wassette process")?;

    assert!(
        !status.success(),
        "wassette should exit non-zero when a manifest component fails to provision, got {status:?}"
    );
    assert!(
        TcpStream::connect(("127.0.0.1", port)).await.is_err(),
        "wassette should never listen on {port} when provisioning fails"
    );

    Ok(())
}

#[tokio::test]
async fn continue_on_provisioning_failure_serves_the_components_that_loaded() -> Result<()> {
    let component_path = common::build_fetch_component().await?;
    let temp_dir = tempfile::tempdir()?;
    let manifest = write_mixed_manifest(temp_dir.path(), &component_path).await?;

    let port = find_open_port().await?;
    let _server = ServerGuard(spawn_server(
        port,
        temp_dir.path(),
        &[
            "--manifest",
            &manifest,
            "--continue-on-provisioning-failure",
        ],
    )?);
    wait_until_listening(port).await?;

    let client = reqwest::Client::new();
    let response = client
        .post(format!("http://127.0.0.1:{port}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", STATELESS_VERSION)
        .header("Mcp-Method", "tools/list")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": STATELESS_VERSION,
                    "io.modelcontextprotocol/clientInfo": {
                        "name": "wassette-provisioning-failure-test",
                        "version": "1.0.0",
                    },
                    "io.modelcontextprotocol/clientCapabilities": {},
                },
            },
        }))
        .send()
        .await
        .context("failed to POST stateless tools/list")?;
    assert_eq!(response.status(), 200, "tools/list should return HTTP 200");

    let message = read_json_rpc(response).await?;
    let tools = message
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .with_context(|| format!("tools/list returned no tool array: {message}"))?;

    assert!(
        tools
            .iter()
            .any(|tool| tool.get("name").and_then(Value::as_str) == Some("fetch")),
        "degraded server should still serve the component that provisioned: {message}"
    );

    Ok(())
}

async fn read_json_rpc(response: reqwest::Response) -> Result<Value> {
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let text = response.text().await.context("failed to read body")?;

    if content_type.starts_with("text/event-stream") {
        let payload = text
            .lines()
            .filter_map(|line| line.strip_prefix("data:").map(str::trim))
            .find(|payload| !payload.is_empty())
            .context("SSE response carried no data line")?;
        return serde_json::from_str(payload).context("malformed SSE JSON-RPC payload");
    }
    serde_json::from_str(&text).context("malformed JSON-RPC payload")
}
