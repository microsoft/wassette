// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Integration tests for stateless MCP over Streamable HTTP.
//!
//! Protocol revision `2026-07-28` (SEP-2567) removes the `initialize`
//! handshake and the protocol-level session, and SEP-2243 mirrors selected
//! body fields into HTTP headers so intermediaries can route without parsing
//! the body. A stateless client therefore sends a single POST carrying its
//! client info in `params._meta` and gets a reply, with no prior round trip.
//!
//! These tests exercise `wassette serve --streamable-http` end to end over
//! real HTTP so the behaviour is pinned at the wire, not at the rmcp API.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};

/// The protocol revision that removed sessions and added the mirrored headers.
const STATELESS_VERSION: &str = "2026-07-28";

/// A protocol revision that still uses the `initialize` handshake.
const LEGACY_VERSION: &str = "2025-06-18";

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

fn mcp_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

/// Build the `params._meta` block every stateless request must carry.
///
/// The protocol version in `_meta` is the source of truth; the
/// `MCP-Protocol-Version` header only mirrors it.
fn stateless_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": STATELESS_VERSION,
        "io.modelcontextprotocol/clientInfo": {
            "name": "wassette-stateless-test",
            "version": "1.0.0",
        },
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

/// POST a stateless JSON-RPC request with the SEP-2243 headers derived from it.
///
/// `name_header` is the `Mcp-Name` value, required for `tools/call`,
/// `resources/read` and `prompts/get`.
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
        .post(mcp_url(port))
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

/// Read a JSON-RPC message out of either response shape.
///
/// A server may answer a request with `application/json` or with a
/// request-scoped SSE stream, and the client must handle both. For a
/// single-response request the first `data:` line carries the whole message.
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
            .find_map(|line| line.strip_prefix("data:"))
            .context("SSE response carried no data line")?;
        return serde_json::from_str(payload.trim()).context("malformed SSE JSON-RPC payload");
    }
    serde_json::from_str(&text).context("malformed JSON-RPC payload")
}

/// A stateless `tools/list` succeeds with no `initialize` and no session id.
#[tokio::test]
async fn stateless_tools_list_needs_no_initialize() -> Result<()> {
    let port = find_open_port().await?;
    let temp_dir = tempfile::tempdir()?;
    let _server = spawn_server(port, temp_dir.path(), &[]).await?;
    wait_until_listening(port).await?;

    let client = reqwest::Client::new();
    let response = post_stateless(
        &client,
        port,
        "tools/list",
        None,
        json!({ "_meta": stateless_meta() }),
    )
    .await?;

    assert_eq!(
        response.status(),
        200,
        "a stateless tools/list must be served without an initialize handshake"
    );
    assert!(
        response.headers().get("mcp-session-id").is_none(),
        "a 2026-07-28 server must not mint a session id"
    );

    let message = read_json_rpc(response).await?;
    assert!(
        message.get("error").is_none(),
        "stateless tools/list returned an error: {message}"
    );
    let tools = message
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .context("response carried no result.tools array")?;
    assert!(
        tools.iter().any(|tool| tool["name"] == "list-components"),
        "builtin tools should be listed, got: {tools:?}"
    );

    Ok(())
}

/// A mirrored header that disagrees with the body is rejected with -32020.
///
/// This matters because an intermediary may route on the header while the
/// server executes the body; the two must never diverge.
#[tokio::test]
async fn stateless_header_body_mismatch_is_rejected() -> Result<()> {
    let port = find_open_port().await?;
    let temp_dir = tempfile::tempdir()?;
    let _server = spawn_server(port, temp_dir.path(), &[]).await?;
    wait_until_listening(port).await?;

    let client = reqwest::Client::new();
    // `Mcp-Method` claims tools/call while the body asks for tools/list.
    let response = client
        .post(mcp_url(port))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", STATELESS_VERSION)
        .header("Mcp-Method", "tools/call")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": { "_meta": stateless_meta() },
        }))
        .send()
        .await?;

    assert_eq!(response.status(), 400, "a header mismatch must be a 400");
    let message = read_json_rpc(response).await?;
    assert_eq!(
        message.pointer("/error/code").and_then(Value::as_i64),
        Some(-32020),
        "a header mismatch must report HeaderMismatch (-32020), got: {message}"
    );

    Ok(())
}

/// A missing `Mcp-Method` header is rejected for a 2026-07-28 request.
#[tokio::test]
async fn stateless_missing_method_header_is_rejected() -> Result<()> {
    let port = find_open_port().await?;
    let temp_dir = tempfile::tempdir()?;
    let _server = spawn_server(port, temp_dir.path(), &[]).await?;
    wait_until_listening(port).await?;

    let client = reqwest::Client::new();
    let response = client
        .post(mcp_url(port))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", STATELESS_VERSION)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": { "_meta": stateless_meta() },
        }))
        .send()
        .await?;

    assert_eq!(
        response.status(),
        400,
        "a 2026-07-28 request without Mcp-Method must be rejected"
    );
    let message = read_json_rpc(response).await?;
    assert_eq!(
        message.pointer("/error/code").and_then(Value::as_i64),
        Some(-32020),
        "a missing standard header must report HeaderMismatch (-32020), got: {message}"
    );

    Ok(())
}

/// A stateless `tools/call` carries `Mcp-Name` alongside `Mcp-Method`.
#[tokio::test]
async fn stateless_tools_call_round_trips() -> Result<()> {
    let port = find_open_port().await?;
    let temp_dir = tempfile::tempdir()?;
    let _server = spawn_server(port, temp_dir.path(), &[]).await?;
    wait_until_listening(port).await?;

    let client = reqwest::Client::new();
    let response = post_stateless(
        &client,
        port,
        "tools/call",
        Some("list-components"),
        json!({
            "name": "list-components",
            "arguments": {},
            "_meta": stateless_meta(),
        }),
    )
    .await?;

    assert_eq!(response.status(), 200, "stateless tools/call should succeed");
    let message = read_json_rpc(response).await?;
    assert!(
        message.get("error").is_none(),
        "stateless tools/call returned an error: {message}"
    );

    Ok(())
}

/// The legacy `initialize` handshake still works and still mints a session id.
///
/// Stateless support is additive: clients pinned to an older revision must
/// keep working against the same endpoint.
#[tokio::test]
async fn legacy_initialize_still_issues_a_session() -> Result<()> {
    let port = find_open_port().await?;
    let temp_dir = tempfile::tempdir()?;
    let _server = spawn_server(port, temp_dir.path(), &[]).await?;
    wait_until_listening(port).await?;

    let client = reqwest::Client::new();
    let response = client
        .post(mcp_url(port))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": LEGACY_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "legacy-client", "version": "1.0.0" },
            },
        }))
        .send()
        .await?;

    assert_eq!(response.status(), 200, "legacy initialize should succeed");
    assert!(
        response.headers().get("mcp-session-id").is_some(),
        "a legacy initialize must still mint a session id"
    );

    Ok(())
}
