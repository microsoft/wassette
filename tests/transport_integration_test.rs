use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use bytes::Bytes;
use http_body_util::Full;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use oci_wasm::WasmClient;
use tempfile::TempDir;
use test_log::test;
use testcontainers::core::WaitFor;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, Image};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::time::sleep;
use wassette::LifecycleManager;

mod common;
use common::build_fetch_component;

const DOCKER_REGISTRY_PORT: u16 = 5000;

pub async fn find_open_port() -> Result<u16> {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .await
        .context("failed to bind random port")?
        .local_addr()
        .map(|addr| addr.port())
        .context("failed to get local address from opened TCP socket")
}

#[derive(Default)]
struct DockerRegistry {
    _priv: (),
}

impl Image for DockerRegistry {
    fn name(&self) -> &str {
        "registry"
    }

    fn tag(&self) -> &str {
        "2"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stderr("listening on")]
    }
}

async fn setup_registry() -> anyhow::Result<ContainerAsync<DockerRegistry>> {
    DockerRegistry::default()
        .start()
        .await
        .context("Failed to start docker registry")
}

async fn cleanup_components(manager: &LifecycleManager) -> Result<()> {
    let component_ids = manager.list_components().await;
    for id in component_ids {
        manager.unload_component(&id).await;
    }
    Ok(())
}

async fn setup_lifecycle_manager() -> Result<(Arc<LifecycleManager>, TempDir)> {
    setup_lifecycle_manager_with_client(reqwest::Client::default()).await
}

async fn setup_lifecycle_manager_with_client(
    http_client: reqwest::Client,
) -> Result<(Arc<LifecycleManager>, TempDir)> {
    let tempdir = tempfile::tempdir()?;

    let manager = Arc::new(
        LifecycleManager::new_with_clients(
            &tempdir,
            oci_client::Client::new(oci_client::client::ClientConfig {
                protocol: oci_client::client::ClientProtocol::Http,
                ..Default::default()
            }),
            http_client,
        )
        .await
        .context("Failed to create LifecycleManager")?,
    );

    cleanup_components(&manager).await?;

    Ok((manager, tempdir))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn test_fetch_component_workflow() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;

    let initial_components = manager.list_components().await;
    assert!(
        initial_components.is_empty(),
        "Expected no components initially"
    );

    let component_path = build_fetch_component().await?;

    let (id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    let components_after_load = manager.list_components().await;
    assert_eq!(components_after_load.len(), 1);
    assert_eq!(components_after_load[0], "fetch_rs");

    let schema = manager
        .get_component_schema(&id)
        .await
        .context("Component not found")?;
    assert!(schema["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["name"] == "fetch"));

    let grant_result = manager
        .grant_permission(&id, "network", &serde_json::json!({"host": "example.com"}))
        .await;
    assert!(grant_result.is_ok(), "Failed to grant network permission");

    let result = manager
        .execute_component_call(&id, "fetch", r#"{"url": "https://example.com/"}"#)
        .await?;

    let response_body = result;
    assert!(response_body.contains("Example Domain"));
    assert!(response_body.contains("This domain is for use in illustrative examples in documents"));

    // Copy the component to another name
    let mut component_path2 = component_path.clone();
    component_path2.set_file_name("fetch2.wasm");
    tokio::fs::copy(&component_path, &component_path2).await?;

    manager
        .load_component(&format!("file://{}", component_path2.to_str().unwrap()))
        .await?;

    // This should now fail because there are multiple components with the same tool
    let component_id_result = manager.get_component_id_for_tool("fetch").await;
    assert!(component_id_result.is_err());
    let error = component_id_result.unwrap_err();
    assert!(error
        .to_string()
        .contains("Multiple components found for tool 'fetch'"));
    assert!(error.to_string().contains("fetch_rs"));
    assert!(error.to_string().contains("fetch2"));

    Ok(())
}

async fn start_https_server(
    wasm_content: Vec<u8>,
) -> Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    use rustls::pki_types::PrivateKeyDer;
    use tokio_rustls::{rustls, TlsAcceptor};

    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()])?;
    let cert_der = cert.cert.der().clone();
    let key_bytes = cert.signing_key.serialize_der();
    let key_der = PrivateKeyDer::try_from(key_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to convert private key: {}", e))?;

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)?;

    let acceptor = TlsAcceptor::from(Arc::new(config));
    let wasm_bytes = Arc::new(wasm_content);

    let handle = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            let wasm_bytes = wasm_bytes.clone();

            tokio::spawn(async move {
                let tls_stream = match acceptor.accept(stream).await {
                    Ok(tls_stream) => tls_stream,
                    Err(e) => {
                        eprintln!("TLS handshake failed: {e:?}");
                        return;
                    }
                };

                let io = TokioIo::new(tls_stream);
                let service = service_fn(move |req: Request<hyper::body::Incoming>| {
                    let wasm_bytes = wasm_bytes.clone();
                    async move {
                        if req.uri().path() != "/fetch_rs.wasm" {
                            return Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(404)
                                    .body(Full::new(Bytes::from("Not Found")))
                                    .unwrap(),
                            );
                        }
                        let response = Response::builder()
                            .status(200)
                            .header("Content-Type", "application/wasm")
                            .body(Full::new(Bytes::from(wasm_bytes.as_ref().clone())))
                            .unwrap();
                        Ok::<_, hyper::Error>(response)
                    }
                });

                if let Err(err) = http1::Builder::new().serve_connection(io, service).await {
                    eprintln!("Error serving connection: {err:?}");
                }
            });
        }
    });

    Ok((addr, handle))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn test_load_component_from_https() -> Result<()> {
    // Create HTTP client that ignores certificate validation for testing
    let http_client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()?;

    let (manager, _tempdir) = setup_lifecycle_manager_with_client(http_client).await?;

    // Build the test component
    let component_path = build_fetch_component().await?;

    // Read the component bytes
    let wasm_bytes = tokio::fs::read(&component_path).await?;

    // Start HTTPS server
    let (addr, _server_handle) = start_https_server(wasm_bytes).await?;

    // Load from HTTPS
    let https_url = format!("https://{addr}/fetch_rs.wasm");
    let (id, _) = manager.load_component(&https_url).await?;

    // Verify component was loaded
    let components = manager.list_components().await;
    assert!(components.contains(&"fetch_rs".to_string()));

    // Test calling the component
    let result = manager
        .execute_component_call(&id, "fetch", r#"{"url": "https://example.com/"}"#)
        .await
        .context("Failed to execute component call")?;

    let response_body = result;
    assert!(!response_body.is_empty());

    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn test_load_component_from_oci() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;

    // Build the test component
    let component_path = build_fetch_component().await?;

    // Start OCI registry using testcontainers - skip if Docker is not available
    let container = match setup_registry().await {
        Ok(container) => container,
        Err(e) => {
            let error_msg = e.to_string();
            if error_msg.contains("Socket not found")
                || error_msg.contains("docker client")
                || error_msg.contains("Failed to start docker registry")
            {
                println!("Skipping OCI test: Docker is not available - {error_msg}");
                return Ok(());
            }
            return Err(e);
        }
    };
    let registry_port = container.get_host_port_ipv4(DOCKER_REGISTRY_PORT).await?;
    let registry_url = format!("localhost:{registry_port}");

    // Give the registry a moment to fully start
    sleep(Duration::from_millis(500)).await;

    // Read component bytes
    let (config, layer) = oci_wasm::WasmConfig::from_component(component_path, None).await?;

    // Create OCI client and push the component
    let oci_client = oci_client::Client::new(oci_client::client::ClientConfig {
        protocol: oci_client::client::ClientProtocol::Http,
        ..Default::default()
    });

    let wasm_client = WasmClient::new(oci_client);
    let reference = format!("{registry_url}/fetch_rs:latest");
    let oci_reference: oci_client::Reference = reference.parse()?;

    // Push to registry
    wasm_client
        .push(
            &oci_reference,
            &oci_client::secrets::RegistryAuth::Anonymous,
            layer,
            config,
            None,
        )
        .await?;

    // Load from OCI
    let oci_url = format!("oci://{reference}");
    manager.load_component(&oci_url).await?;

    // Verify component was loaded
    let components = manager.list_components().await;
    assert!(components.contains(&"fetch_rs".to_string()));

    Ok(())
}

#[test(tokio::test)]
async fn test_load_component_invalid_scheme() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;

    // Try to load with invalid scheme
    let result = manager
        .load_component("ftp://example.com/component.wasm")
        .await;
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(error.to_string().contains("Unsupported component scheme"));

    Ok(())
}

#[test(tokio::test)]
async fn test_load_component_https_404() -> Result<()> {
    // Create HTTP client that ignores certificate validation for testing
    let http_client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()?;

    let (manager, _tempdir) = setup_lifecycle_manager_with_client(http_client).await?;

    // Start HTTPS server
    let (addr, _server_handle) = start_https_server(Vec::new()).await?;

    // Try to load from HTTPS with 404
    let https_url = format!("https://{addr}/nonexistent.wasm");
    let result = manager.load_component(&https_url).await;
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Failed to download component from URL"),
        "Wrong error message found, got: {error}"
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_load_component_invalid_reference() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;

    // Try to load without scheme
    let result = manager.load_component("not_a_valid_reference").await;
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(error.to_string().contains("Invalid component reference"));

    Ok(())
}

#[test(tokio::test)]
async fn test_stdio_transport() -> Result<()> {
    // Create a temporary directory for this test to avoid loading existing components
    let temp_dir = tempfile::tempdir()?;
    let plugin_dir_arg = format!("--plugin-dir={}", temp_dir.path().display());

    // Get the path to the built binary
    let binary_path = std::env::current_dir()
        .context("Failed to get current directory")?
        .join("target/debug/wassette");

    // Start the server with stdio transport (disable logs to avoid stdout pollution)
    let mut child = tokio::process::Command::new(&binary_path)
        .args(["serve", &plugin_dir_arg])
        .env("RUST_LOG", "off")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to start wassette with stdio transport")?;

    let stdin = child.stdin.take().context("Failed to get stdin handle")?;
    let stdout = child.stdout.take().context("Failed to get stdout handle")?;
    let stderr = child.stderr.take().context("Failed to get stderr handle")?;

    let mut stdin = stdin;
    let mut stdout = BufReader::new(stdout);
    let mut stderr = BufReader::new(stderr);

    // Give the server time to start (less time needed with empty plugin dir)
    tokio::time::sleep(Duration::from_millis(1000)).await;

    // Check if the process is still running
    if let Ok(Some(status)) = child.try_wait() {
        // Process has exited, read stderr to see what went wrong
        let mut stderr_output = String::new();
        let _ = stderr.read_line(&mut stderr_output).await;
        return Err(anyhow::anyhow!(
            "Server process exited with status: {:?}, stderr: {}",
            status,
            stderr_output
        ));
    }

    // Send MCP initialize request
    let initialize_request = r#"{"jsonrpc": "2.0", "method": "initialize", "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "test-client", "version": "1.0.0"}}, "id": 1}
"#;

    stdin.write_all(initialize_request.as_bytes()).await?;
    stdin.flush().await?;

    // Read and verify response with longer timeout for component loading
    let mut response_line = String::new();
    match tokio::time::timeout(
        Duration::from_secs(10),
        stdout.read_line(&mut response_line),
    )
    .await
    {
        Ok(Ok(_)) => {
            // Successfully read a line
        }
        Ok(Err(e)) => {
            // Read error
            return Err(anyhow::anyhow!("Failed to read initialize response: {}", e));
        }
        Err(_) => {
            // Timeout - try to read stderr to see if there are any error messages
            let mut stderr_output = String::new();
            let _ =
                tokio::time::timeout(Duration::from_secs(1), stderr.read_line(&mut stderr_output))
                    .await;
            return Err(anyhow::anyhow!(
                "Timeout waiting for initialize response. Stderr: {}",
                stderr_output
            ));
        }
    }

    if response_line.trim().is_empty() {
        return Err(anyhow::anyhow!("Received empty response"));
    }

    let response: serde_json::Value =
        serde_json::from_str(&response_line).context("Failed to parse initialize response")?;

    // Verify the response structure
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 1);
    assert!(response["result"].is_object());
    assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
    assert!(response["result"]["capabilities"]["tools"]["listChanged"]
        .as_bool()
        .unwrap_or(false));

    // Send initialized notification (required by MCP protocol)
    let initialized_notification = r#"{"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}
"#;

    stdin.write_all(initialized_notification.as_bytes()).await?;
    stdin.flush().await?;

    // Send list_tools request
    let list_tools_request = r#"{"jsonrpc": "2.0", "method": "tools/list", "params": {}, "id": 2}
"#;

    stdin.write_all(list_tools_request.as_bytes()).await?;
    stdin.flush().await?;

    // Read and verify tools list response
    let mut tools_response_line = String::new();
    tokio::time::timeout(
        Duration::from_secs(10),
        stdout.read_line(&mut tools_response_line),
    )
    .await
    .context("Timeout waiting for tools/list response")?
    .context("Failed to read tools/list response")?;

    let tools_response: serde_json::Value = serde_json::from_str(&tools_response_line)
        .context("Failed to parse tools/list response")?;

    // Verify the tools response structure
    assert_eq!(tools_response["jsonrpc"], "2.0");
    assert_eq!(tools_response["id"], 2);
    assert!(tools_response["result"].is_object());
    assert!(tools_response["result"]["tools"].is_array());

    // Verify we have the expected built-in tools
    let tools = &tools_response["result"]["tools"].as_array().unwrap();
    assert!(tools.len() >= 2); // Should have at least load-component and unload-component

    let tool_names: Vec<String> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(tool_names.contains(&"load-component".to_string()));
    assert!(tool_names.contains(&"unload-component".to_string()));

    // Clean up
    child.kill().await.ok();

    Ok(())
}

#[test(tokio::test)]
async fn test_http_transport() -> Result<()> {
    // Use a random available port to avoid conflicts
    let port = find_open_port().await?;

    // We need to modify the source to support configurable bind address
    // For now, let's test with the default port but check if it's available
    let default_port = 9001u16;
    let test_port = if TcpListener::bind(format!("127.0.0.1:{default_port}"))
        .await
        .is_ok()
    {
        default_port
    } else {
        port
    };

    // If we're not using the default port, skip this test for now
    // since the server code uses a hardcoded bind address
    if test_port != default_port {
        println!("Skipping HTTP transport test: default port 9001 is not available");
        return Ok(());
    }

    // Create a temporary directory for this test to avoid loading existing components
    let temp_dir = tempfile::tempdir()?;
    let plugin_dir_arg = format!("--plugin-dir={}", temp_dir.path().display());

    // Get the path to the built binary
    let binary_path = std::env::current_dir()
        .context("Failed to get current directory")?
        .join("target/debug/wassette");

    // Start the server with HTTP transport
    let mut child = tokio::process::Command::new(&binary_path)
        .args(["serve", "--http", &plugin_dir_arg])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to start wassette with HTTP transport")?;

    // Give the server time to start (less time needed with empty plugin dir)
    tokio::time::sleep(Duration::from_millis(1000)).await;

    // Create HTTP client
    let client = reqwest::Client::new();
    let base_url = format!("http://127.0.0.1:{test_port}");

    // Test that the server is responding
    let response = tokio::time::timeout(Duration::from_secs(10), client.get(&base_url).send())
        .await
        .context("Timeout waiting for HTTP server response")?
        .context("Failed to connect to HTTP server")?;

    // The server should return some response (even if it's an error for GET requests)
    // The important thing is that it's listening and responding
    assert!(response.status().as_u16() >= 200);

    // Clean up
    child.kill().await.ok();

    Ok(())
}

#[test(tokio::test)]
async fn test_default_stdio_transport() -> Result<()> {
    // Create a temporary directory for this test to avoid loading existing components
    let temp_dir = tempfile::tempdir()?;
    let plugin_dir_arg = format!("--plugin-dir={}", temp_dir.path().display());

    // Get the path to the built binary
    let binary_path = std::env::current_dir()
        .context("Failed to get current directory")?
        .join("target/debug/wassette");

    // Start the server without any transport flags (should default to stdio)
    let mut child = tokio::process::Command::new(&binary_path)
        .args(["serve", &plugin_dir_arg])
        .env("RUST_LOG", "off")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to start wassette with default transport")?;

    let stdin = child.stdin.take().context("Failed to get stdin handle")?;
    let stdout = child.stdout.take().context("Failed to get stdout handle")?;

    let mut stdin = stdin;
    let mut stdout = BufReader::new(stdout);

    // Give the server time to start (less time needed with empty plugin dir)
    tokio::time::sleep(Duration::from_millis(1000)).await;

    // Check if the process is still running
    if let Ok(Some(status)) = child.try_wait() {
        return Err(anyhow::anyhow!(
            "Server process exited with status: {:?}",
            status
        ));
    }

    // Send MCP initialize request
    let initialize_request = r#"{"jsonrpc": "2.0", "method": "initialize", "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "test-client", "version": "1.0.0"}}, "id": 1}
"#;

    stdin.write_all(initialize_request.as_bytes()).await?;
    stdin.flush().await?;

    // Read and verify response
    let mut response_line = String::new();
    tokio::time::timeout(
        Duration::from_secs(10),
        stdout.read_line(&mut response_line),
    )
    .await
    .context("Timeout waiting for initialize response")?
    .context("Failed to read initialize response")?;

    let response: serde_json::Value =
        serde_json::from_str(&response_line).context("Failed to parse initialize response")?;

    // Verify the response structure (this confirms stdio transport is working)
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 1);
    assert!(response["result"].is_object());

    // Clean up
    child.kill().await.ok();

    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test(tokio::test)]
async fn test_grant_permission_network_basic() -> Result<()> {
    let (manager, _tempdir) = setup_lifecycle_manager().await?;
    let component_path = build_fetch_component().await?;

    let (component_id, _) = manager
        .load_component(&format!("file://{}", component_path.to_str().unwrap()))
        .await?;

    // Test granting network permission
    let result = manager
        .grant_permission(
            &component_id,
            "network",
            &serde_json::json!({"host": "api.example.com"}),
        )
        .await;

    assert!(result.is_ok());

    // Verify policy file was created and contains the permission
    let policy_info = manager.get_policy_info(&component_id).await;
    assert!(policy_info.is_some());
    let policy_info = policy_info.unwrap();

    // Verify policy contains the permission
    let policy_content = tokio::fs::read_to_string(&policy_info.local_path).await?;
    assert!(policy_content.contains("api.example.com"));
    assert!(policy_content.contains("network"));

    Ok(())
}
