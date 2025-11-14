// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! IPC server for dynamic secret management
//!
//! This module provides an IPC server that runs alongside the MCP server
//! to handle dynamic secret provisioning and revocation through a Unix domain
//! socket (Unix/macOS) or named pipe (Windows).
//!
//! **Note:** The Windows implementation is currently a stub and not yet functional.
//! Only Unix/macOS platforms are supported at this time.
//!
//! # Security
//! - Peer authentication ensures only same-user connections are allowed
//! - Socket/pipe permissions are set to owner-only (0700/0600)
//! - All connection attempts and authentication results are logged
//!
//! # Protocol
//! JSON-based request/response over the IPC channel:
//! - Request: `{"command": "set_secret", "component_id": "...", "key": "...", "value": "..."}`
//! - Response: `{"status": "success" | "error", "message": "..."}`

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::ipc_protocol::{IpcCommand, IpcResponse};
use crate::SecretsManager;

/// IPC server configuration
#[derive(Debug, Clone)]
pub struct IpcServerConfig {
    /// Path to the socket/pipe
    pub socket_path: PathBuf,
    /// Secrets manager for handling secret operations
    pub secrets_manager: Arc<SecretsManager>,
}

impl IpcServerConfig {
    /// Create a new IPC server configuration
    pub fn new(socket_path: PathBuf, secrets_manager: Arc<SecretsManager>) -> Self {
        Self {
            socket_path,
            secrets_manager,
        }
    }

    /// Get the default socket path for the current platform
    pub fn default_socket_path() -> Result<PathBuf> {
        #[cfg(unix)]
        {
            // Use XDG_RUNTIME_DIR or fallback to /tmp
            let runtime_dir =
                std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
            let socket_dir = PathBuf::from(runtime_dir).join("wassette");
            Ok(socket_dir.join("wassette.sock"))
        }

        #[cfg(windows)]
        {
            // Windows named pipe path
            Ok(PathBuf::from(r"\\.\pipe\wassette"))
        }

        #[cfg(not(any(unix, windows)))]
        {
            anyhow::bail!("Unsupported platform for IPC server")
        }
    }
}

/// IPC server for handling secret management requests
pub struct IpcServer {
    config: IpcServerConfig,
    shutdown_tx: Option<mpsc::Sender<()>>,
}

impl IpcServer {
    /// Create a new IPC server
    pub fn new(config: IpcServerConfig) -> Self {
        Self {
            config,
            shutdown_tx: None,
        }
    }

    /// Start the IPC server
    pub async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
        self.shutdown_tx = Some(shutdown_tx);

        let config = self.config.clone();

        // Platform-specific server implementation
        #[cfg(unix)]
        {
            unix_server(config, &mut shutdown_rx).await
        }

        #[cfg(windows)]
        {
            windows_server(config, &mut shutdown_rx).await
        }

        #[cfg(not(any(unix, windows)))]
        {
            anyhow::bail!("Unsupported platform for IPC server")
        }
    }

    /// Shutdown the IPC server gracefully
    pub async fn shutdown(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }
        Ok(())
    }
}

/// Handle a single IPC request
async fn handle_request(
    request: IpcCommand,
    secrets_manager: &SecretsManager,
) -> Result<IpcResponse> {
    match &request {
        IpcCommand::Ping => debug!("Handling IPC request: Ping"),
        IpcCommand::SetSecret {
            component_id, key, ..
        } => {
            debug!(
                "Handling IPC request: SetSecret for component: {}, key: {}",
                component_id, key
            )
        }
        IpcCommand::DeleteSecret { component_id, key } => {
            debug!(
                "Handling IPC request: DeleteSecret for component: {}, key: {}",
                component_id, key
            )
        }
        IpcCommand::ListSecrets { component_id, .. } => {
            debug!(
                "Handling IPC request: ListSecrets for component: {}",
                component_id
            )
        }
    }

    match request {
        IpcCommand::Ping => Ok(IpcResponse::success("pong")),

        IpcCommand::SetSecret {
            component_id,
            key,
            value,
        } => {
            secrets_manager
                .set_component_secrets(&component_id, &[(key.clone(), value)])
                .await
                .context("Failed to set secret")?;
            Ok(IpcResponse::success(format!(
                "Secret '{}' set for component '{}'",
                key, component_id
            )))
        }

        IpcCommand::DeleteSecret { component_id, key } => {
            secrets_manager
                .delete_component_secrets(&component_id, std::slice::from_ref(&key))
                .await
                .context("Failed to delete secret")?;
            Ok(IpcResponse::success(format!(
                "Secret '{}' deleted from component '{}'",
                key, component_id
            )))
        }

        IpcCommand::ListSecrets {
            component_id,
            show_values,
        } => {
            let secrets = secrets_manager
                .list_component_secrets(&component_id, show_values)
                .await
                .context("Failed to list secrets")?;

            if show_values {
                let secrets_map: std::collections::HashMap<String, String> = secrets
                    .into_iter()
                    .filter_map(|(k, v)| v.map(|val| (k, val)))
                    .collect();
                Ok(IpcResponse::success_with_data(
                    format!("Listed {} secret(s) with values", secrets_map.len()),
                    serde_json::json!({ "secrets": secrets_map }),
                ))
            } else {
                let keys: Vec<String> = secrets.into_keys().collect();
                Ok(IpcResponse::success_with_data(
                    format!("Listed {} secret(s)", keys.len()),
                    serde_json::json!({ "keys": keys }),
                ))
            }
        }
    }
}

#[cfg(unix)]
mod unix_impl {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// Get peer credentials (uid, gid) from a Unix stream
    #[cfg(target_os = "linux")]
    fn get_peer_creds(stream: &tokio::net::UnixStream) -> Result<(u32, u32)> {
        use std::os::unix::io::AsRawFd;

        let fd = stream.as_raw_fd();
        let mut ucred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut ucred_size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;

        let ret = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut ucred as *mut _ as *mut libc::c_void,
                &mut ucred_size,
            )
        };

        if ret != 0 {
            anyhow::bail!(
                "Failed to get peer credentials: {}",
                std::io::Error::last_os_error()
            );
        }

        Ok((ucred.uid, ucred.gid))
    }

    /// Get peer credentials (uid, gid) from a Unix stream on macOS
    #[cfg(target_os = "macos")]
    fn get_peer_creds(stream: &tokio::net::UnixStream) -> Result<(u32, u32)> {
        use std::os::unix::io::AsRawFd;

        let fd = stream.as_raw_fd();
        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;

        let ret = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };

        if ret != 0 {
            anyhow::bail!(
                "Failed to get peer credentials: {}",
                std::io::Error::last_os_error()
            );
        }

        Ok((uid, gid))
    }

    /// Verify that the peer has the same uid/gid as the server
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn verify_peer_identity(stream: &tokio::net::UnixStream) -> Result<bool> {
        let (peer_uid, peer_gid) = get_peer_creds(stream)?;
        let server_uid = unsafe { libc::getuid() };
        let server_gid = unsafe { libc::getgid() };

        let authorized = peer_uid == server_uid && peer_gid == server_gid;

        if !authorized {
            warn!(
                "Unauthorized connection attempt: peer uid={} gid={}, server uid={} gid={}",
                peer_uid, peer_gid, server_uid, server_gid
            );
        } else {
            debug!(
                "Authorized connection: peer uid={} gid={} matches server",
                peer_uid, peer_gid
            );
        }

        Ok(authorized)
    }

    /// Handle a single Unix socket connection
    async fn handle_connection(
        stream: tokio::net::UnixStream,
        secrets_manager: Arc<SecretsManager>,
    ) {
        // Verify peer identity
        match verify_peer_identity(&stream) {
            Ok(true) => {
                info!("Connection authenticated successfully");
            }
            Ok(false) => {
                error!("Connection rejected: authentication failed");
                return;
            }
            Err(e) => {
                error!("Failed to verify peer identity: {}", e);
                return;
            }
        }

        let mut reader = BufReader::new(stream);
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => {
                    debug!("Client disconnected");
                    break;
                }
                Ok(_) => {
                    let request: IpcCommand = match serde_json::from_str(&line) {
                        Ok(req) => req,
                        Err(e) => {
                            error!("Failed to parse request: {}", e);
                            let response = IpcResponse::error(format!("Invalid request: {}", e));
                            let response_json = match serde_json::to_string(&response) {
                                Ok(json) => json,
                                Err(e) => {
                                    error!("Failed to serialize error response: {}", e);
                                    r#"{"status":"error","message":"Internal server error"}"#
                                        .to_string()
                                }
                            };
                            if let Err(e) =
                                reader.get_mut().write_all(response_json.as_bytes()).await
                            {
                                error!("Failed to write error response: {}", e);
                            }
                            if let Err(e) = reader.get_mut().write_all(b"\n").await {
                                error!("Failed to write newline: {}", e);
                            }
                            continue;
                        }
                    };

                    let response = match handle_request(request, &secrets_manager).await {
                        Ok(resp) => resp,
                        Err(e) => {
                            error!("Failed to handle request: {}", e);
                            IpcResponse::error(format!("Request failed: {}", e))
                        }
                    };

                    let response_json = match serde_json::to_string(&response) {
                        Ok(json) => json,
                        Err(e) => {
                            error!("Failed to serialize response: {}", e);
                            let error_response =
                                IpcResponse::error("Internal server error".to_string());
                            serde_json::to_string(&error_response).unwrap_or_else(|_| {
                                r#"{"status":"error","message":"Internal server error"}"#
                                    .to_string()
                            })
                        }
                    };
                    if let Err(e) = reader.get_mut().write_all(response_json.as_bytes()).await {
                        error!("Failed to write response: {}", e);
                        break;
                    }
                    if let Err(e) = reader.get_mut().write_all(b"\n").await {
                        error!("Failed to write newline: {}", e);
                        break;
                    }
                }
                Err(e) => {
                    error!("Failed to read from stream: {}", e);
                    break;
                }
            }
        }
    }

    pub async fn unix_server(
        config: IpcServerConfig,
        shutdown_rx: &mut mpsc::Receiver<()>,
    ) -> Result<()> {
        // Ensure socket directory exists with proper permissions
        if let Some(parent) = config.socket_path.parent() {
            if !parent.exists() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .context("Failed to create socket directory")?;

                // Set directory permissions to 0700 (user only)
                let metadata = tokio::fs::metadata(parent)
                    .await
                    .context("Failed to get directory metadata")?;
                let mut perms = metadata.permissions();
                perms.set_mode(0o700);
                tokio::fs::set_permissions(parent, perms)
                    .await
                    .context("Failed to set directory permissions")?;
            }
        }

        // Remove existing socket if it exists
        if config.socket_path.exists() {
            tokio::fs::remove_file(&config.socket_path)
                .await
                .context("Failed to remove existing socket")?;
        }

        // Create Unix domain socket
        let listener = tokio::net::UnixListener::bind(&config.socket_path)
            .context("Failed to bind Unix socket")?;

        // Set socket permissions to 0600 (user read/write only)
        let metadata = tokio::fs::metadata(&config.socket_path)
            .await
            .context("Failed to get socket metadata")?;
        let mut perms = metadata.permissions();
        perms.set_mode(0o600);
        tokio::fs::set_permissions(&config.socket_path, perms)
            .await
            .context("Failed to set socket permissions")?;

        info!("IPC server listening on {}", config.socket_path.display());

        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, _addr)) => {
                            debug!("New IPC connection");
                            let secrets_manager = config.secrets_manager.clone();
                            tokio::spawn(async move {
                                handle_connection(stream, secrets_manager).await;
                            });
                        }
                        Err(e) => {
                            error!("Failed to accept connection: {}", e);
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    info!("IPC server shutting down");
                    break;
                }
            }
        }

        // Cleanup socket
        if config.socket_path.exists() {
            let _ = tokio::fs::remove_file(&config.socket_path).await;
        }

        Ok(())
    }
}

#[cfg(unix)]
use unix_impl::unix_server;

#[cfg(windows)]
mod windows_impl {
    use super::*;

    pub async fn windows_server(
        _config: IpcServerConfig,
        _shutdown_rx: &mut mpsc::Receiver<()>,
    ) -> Result<()> {
        // TODO: Implement Windows named pipe server
        anyhow::bail!("Windows named pipe server not yet implemented")
    }
}

#[cfg(windows)]
use windows_impl::windows_server;

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn test_ipc_request_serialization() {
        let request = IpcCommand::Ping;
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(json, r#"{"command":"ping"}"#);

        let request = IpcCommand::SetSecret {
            component_id: "test".to_string(),
            key: "API_KEY".to_string(),
            value: "secret123".to_string(),
        };
        let json = serde_json::to_string(&request).unwrap();
        let parsed: IpcCommand = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, IpcCommand::SetSecret { .. }));
    }

    #[tokio::test]
    async fn test_ipc_response_serialization() {
        let response = IpcResponse::success("test message");
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains(r#""status":"success"#));
        assert!(json.contains(r#""message":"test message"#));

        let response = IpcResponse::error("error message");
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains(r#""status":"error"#));
    }

    #[tokio::test]
    async fn test_handle_ping_request() {
        let temp_dir = TempDir::new().unwrap();
        let secrets_dir = temp_dir.path().join("secrets");
        let secrets_manager = SecretsManager::new(secrets_dir);

        let request = IpcCommand::Ping;
        let response = handle_request(request, &secrets_manager).await.unwrap();

        assert_eq!(response.status, "success");
        assert_eq!(response.message, "pong");
    }

    #[tokio::test]
    async fn test_handle_set_secret_request() {
        let temp_dir = TempDir::new().unwrap();
        let secrets_dir = temp_dir.path().join("secrets");
        let secrets_manager = SecretsManager::new(secrets_dir);
        secrets_manager.ensure_secrets_dir().await.unwrap();

        let request = IpcCommand::SetSecret {
            component_id: "test-component".to_string(),
            key: "API_KEY".to_string(),
            value: "secret123".to_string(),
        };

        let response = handle_request(request, &secrets_manager).await.unwrap();
        assert_eq!(response.status, "success");

        // Verify secret was actually set
        let secrets = secrets_manager
            .list_component_secrets("test-component", true)
            .await
            .unwrap();
        assert_eq!(secrets.get("API_KEY"), Some(&Some("secret123".to_string())));
    }

    #[tokio::test]
    async fn test_handle_list_secrets_request() {
        let temp_dir = TempDir::new().unwrap();
        let secrets_dir = temp_dir.path().join("secrets");
        let secrets_manager = SecretsManager::new(secrets_dir);
        secrets_manager.ensure_secrets_dir().await.unwrap();

        // Set some secrets first
        secrets_manager
            .set_component_secrets(
                "test-component",
                &[
                    ("KEY1".to_string(), "value1".to_string()),
                    ("KEY2".to_string(), "value2".to_string()),
                ],
            )
            .await
            .unwrap();

        let request = IpcCommand::ListSecrets {
            component_id: "test-component".to_string(),
            show_values: false,
        };

        let response = handle_request(request, &secrets_manager).await.unwrap();
        assert_eq!(response.status, "success");
        assert!(response.data.is_some());

        let data = response.data.unwrap();
        let keys = data["keys"].as_array().unwrap();
        assert_eq!(keys.len(), 2);
    }

    #[tokio::test]
    async fn test_handle_delete_secret_request() {
        let temp_dir = TempDir::new().unwrap();
        let secrets_dir = temp_dir.path().join("secrets");
        let secrets_manager = SecretsManager::new(secrets_dir);
        secrets_manager.ensure_secrets_dir().await.unwrap();

        // Set a secret first
        secrets_manager
            .set_component_secrets(
                "test-component",
                &[("API_KEY".to_string(), "secret123".to_string())],
            )
            .await
            .unwrap();

        let request = IpcCommand::DeleteSecret {
            component_id: "test-component".to_string(),
            key: "API_KEY".to_string(),
        };

        let response = handle_request(request, &secrets_manager).await.unwrap();
        assert_eq!(response.status, "success");

        // Verify secret was actually deleted
        let secrets = secrets_manager
            .list_component_secrets("test-component", true)
            .await
            .unwrap();
        assert!(!secrets.contains_key("API_KEY"));
    }
}
