// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Integration test reproducing the DNS-rebinding vulnerability
//! (GHSA-m873-hg53-85r5).
//!
//! `wassette serve` exposes the MCP management plane over HTTP. Binding to
//! loopback is not sufficient to prevent a malicious web page from reaching
//! that endpoint through DNS rebinding, so the server must reject requests
//! whose `Host` header is not a trusted loopback value before MCP dispatch.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};

const FORGED_HOST: &str = "rebind.dev.pluto.security";

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

async fn post_initialize_status(port: u16, host_header: &str) -> Result<u16> {
    let body = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"#,
        r#""protocolVersion":"2024-11-05","capabilities":{},"#,
        r#""clientInfo":{"name":"dns-rebind-browser","version":"1"}}}"#,
    );
    let request = format!(
        concat!(
            "POST /mcp HTTP/1.1\r\n",
            "Host: {host}\r\n",
            "Accept: application/json, text/event-stream\r\n",
            "Content-Type: application/json\r\n",
            "Content-Length: {len}\r\n",
            "Connection: close\r\n",
            "\r\n",
            "{body}",
        ),
        host = host_header,
        len = body.len(),
        body = body,
    );

    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .context("failed to connect to wassette /mcp")?;
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;

    // A successful initialize response may remain open as an event stream.
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let read = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut chunk))
            .await
            .context("timeout reading /mcp response")?
            .context("failed to read /mcp response")?;
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..read]);
        if buf.windows(2).any(|window| window == b"\r\n") {
            break;
        }
    }

    let text = String::from_utf8_lossy(&buf);
    let status_line = text.lines().next().context("empty response from /mcp")?;
    status_line
        .split_whitespace()
        .nth(1)
        .context("malformed HTTP status line")?
        .parse::<u16>()
        .context("could not parse HTTP status code")
}

struct ServerGuard(Child);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.start_kill();
    }
}

async fn spawn_server(port: u16, component_dir: &Path) -> Result<ServerGuard> {
    let child = Command::new(env!("CARGO_BIN_EXE_wassette"))
        .args([
            "serve",
            "--streamable-http",
            "--bind-address",
            &format!("127.0.0.1:{port}"),
            &format!("--component-dir={}", component_dir.display()),
        ])
        .env("RUST_LOG", "error")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn `wassette serve --streamable-http`")?;
    Ok(ServerGuard(child))
}

#[tokio::test]
async fn dns_rebinding_foreign_host_is_rejected() -> Result<()> {
    let port = find_open_port().await?;
    let temp_dir = tempfile::tempdir()?;
    let _server = spawn_server(port, temp_dir.path()).await?;
    wait_until_listening(port).await?;

    let loopback_host = format!("127.0.0.1:{port}");
    let loopback_status = post_initialize_status(port, &loopback_host).await?;
    assert_eq!(
        loopback_status, 200,
        "loopback Host `{loopback_host}` should be accepted (got {loopback_status})"
    );

    let forged_host = format!("{FORGED_HOST}:{port}");
    let forged_status = post_initialize_status(port, &forged_host).await?;
    assert_eq!(
        forged_status, 403,
        "forged Host `{forged_host}` must be rejected with 403 to prevent DNS \
         rebinding (got {forged_status})"
    );

    Ok(())
}
