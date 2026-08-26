// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::convert::Infallible;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use bytes::Bytes;
use http_body_util::Full;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

mod common;

const STATELESS_VERSION: &str = "2026-07-28";
const ORIGIN_BODY: &str = "manifest policy reached the loopback origin";

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

async fn spawn_server(port: u16, component_dir: &Path, extra_args: &[&str]) -> Result<ServerGuard> {
    let bind_address = format!("127.0.0.1:{port}");
    let component_dir = format!("--component-dir={}", component_dir.display());
    let mut args = vec![
        "serve",
        "--streamable-http",
        "--bind-address",
        &bind_address,
        &component_dir,
    ];
    args.extend_from_slice(extra_args);

    let child = Command::new(env!("CARGO_BIN_EXE_wassette"))
        .args(&args)
        .env("RUST_LOG", "error")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn `wassette serve --streamable-http`")?;
    Ok(ServerGuard(child))
}

fn stateless_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": STATELESS_VERSION,
        "io.modelcontextprotocol/clientInfo": {
            "name": "wassette-manifest-policy-test",
            "version": "1.0.0",
        },
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

async fn post_stateless(
    client: &reqwest::Client,
    port: u16,
    method: &str,
    name_header: Option<&str>,
    params: Value,
) -> Result<reqwest::Response> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });

    let mut request = client
        .post(format!("http://127.0.0.1:{port}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", STATELESS_VERSION)
        .header("Mcp-Method", method);
    if let Some(name) = name_header {
        request = request.header("Mcp-Name", name);
    }

    request
        .json(&body)
        .send()
        .await
        .with_context(|| format!("failed to POST stateless {method}"))
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

struct OriginGuard {
    port: u16,
    task: JoinHandle<Result<()>>,
}

impl Drop for OriginGuard {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn spawn_origin() -> Result<OriginGuard> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("failed to bind the loopback origin")?;
    let port = listener.local_addr()?.port();
    let task = tokio::spawn(async move {
        let (stream, _) = listener
            .accept()
            .await
            .context("failed to accept an origin connection")?;
        let service = service_fn(|_request: Request<hyper::body::Incoming>| async {
            Ok::<_, Infallible>(
                Response::builder()
                    .header("Content-Type", "text/plain")
                    .body(Full::new(Bytes::from_static(ORIGIN_BODY.as_bytes())))
                    .expect("static origin response should be valid"),
            )
        });
        http1::Builder::new()
            .serve_connection(TokioIo::new(stream), service)
            .await
            .context("failed to serve the origin response")
    });

    Ok(OriginGuard { port, task })
}

#[tokio::test]
async fn manifest_network_permission_allows_tool_call() -> Result<()> {
    let component_path = common::build_fetch_component().await?;
    let origin = spawn_origin().await?;
    let temp_dir = tempfile::tempdir()?;
    let manifest_path = temp_dir.path().join("manifest.yaml");
    tokio::fs::write(
        &manifest_path,
        format!(
            "version: 1\ncomponents:\n  - uri: file://{}\n    permissions:\n      network:\n        allow:\n          - host: 127.0.0.1\n",
            component_path.display()
        ),
    )
    .await?;

    let port = find_open_port().await?;
    let manifest = manifest_path
        .to_str()
        .context("manifest path was not valid UTF-8")?;
    let _server = spawn_server(port, temp_dir.path(), &["--manifest", manifest]).await?;
    wait_until_listening(port).await?;

    let client = reqwest::Client::new();
    let response = post_stateless(
        &client,
        port,
        "tools/call",
        Some("fetch"),
        json!({
            "name": "fetch",
            "arguments": {
                "url": format!("http://127.0.0.1:{}/", origin.port),
            },
            "_meta": stateless_meta(),
        }),
    )
    .await?;
    assert_eq!(response.status(), 200, "tools/call should return HTTP 200");

    let message = read_json_rpc(response).await?;
    let rendered = message.to_string();
    assert!(
        !rendered.contains("HttpRequestDenied"),
        "manifest-declared network permission was denied: {message}"
    );
    assert!(
        rendered.contains(ORIGIN_BODY),
        "fetch response did not contain the origin body: {message}"
    );

    Ok(())
}
