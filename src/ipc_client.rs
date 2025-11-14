// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! IPC client for communicating with the Wassette IPC server
//!
//! This module provides a client for sending commands to a running Wassette
//! server via Unix domain socket (Unix/macOS) or named pipe (Windows).

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::timeout;
use wassette::{IpcCommand, IpcResponse, IpcServerConfig};

/// IPC client for communicating with the Wassette server
pub struct IpcClient {
    socket_path: PathBuf,
    timeout_duration: Duration,
}

impl IpcClient {
    /// Create a new IPC client with the default socket path
    pub fn new() -> Result<Self> {
        let socket_path = IpcServerConfig::default_socket_path()
            .context("Failed to determine default socket path")?;
        Ok(Self {
            socket_path,
            timeout_duration: Duration::from_secs(5),
        })
    }

    /// Create a new IPC client with a custom socket path
    pub fn with_socket_path(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            timeout_duration: Duration::from_secs(5),
        }
    }

    /// Set the timeout duration for requests
    pub fn with_timeout(mut self, duration: Duration) -> Self {
        self.timeout_duration = duration;
        self
    }

    /// Send a command to the server and wait for a response
    pub async fn send_command(&self, command: IpcCommand) -> Result<IpcResponse> {
        // Platform-specific implementation
        #[cfg(unix)]
        {
            self.send_command_unix(command).await
        }

        #[cfg(windows)]
        {
            self.send_command_windows(command).await
        }

        #[cfg(not(any(unix, windows)))]
        {
            anyhow::bail!("Unsupported platform for IPC client")
        }
    }

    /// Unix-specific implementation
    #[cfg(unix)]
    async fn send_command_unix(&self, command: IpcCommand) -> Result<IpcResponse> {
        use tokio::net::UnixStream;

        // Check if socket exists
        if !self.socket_path.exists() {
            return Err(anyhow!(
                "Wassette server is not running. Socket not found at: {}\n\
                 \n\
                 Start the server first with: wassette serve --sse",
                self.socket_path.display()
            ));
        }

        // Connect to the socket with timeout
        let stream = timeout(
            self.timeout_duration,
            UnixStream::connect(&self.socket_path),
        )
        .await
        .context("Connection timeout")?
        .with_context(|| {
            format!(
                "Failed to connect to Wassette server at {}\n\
                 \n\
                 Possible causes:\n\
                 - Server is not running (start with: wassette serve --sse)\n\
                 - Permission denied (check file permissions on socket)\n\
                 - Socket is stale (remove {} and restart server)",
                self.socket_path.display(),
                self.socket_path.display()
            )
        })?;

        let mut reader = BufReader::new(stream);

        // Serialize and send command
        let command_json =
            serde_json::to_string(&command).context("Failed to serialize command to JSON")?;

        timeout(
            self.timeout_duration,
            reader.get_mut().write_all(command_json.as_bytes()),
        )
        .await
        .context("Write timeout")?
        .context("Failed to write command to socket")?;

        timeout(self.timeout_duration, reader.get_mut().write_all(b"\n"))
            .await
            .context("Write timeout")?
            .context("Failed to write newline to socket")?;

        // Read response
        let mut response_line = String::new();
        timeout(self.timeout_duration, reader.read_line(&mut response_line))
            .await
            .context("Response timeout - server may be unresponsive")?
            .context("Failed to read response from socket")?;

        // Parse response
        let response: IpcResponse = serde_json::from_str(&response_line).with_context(|| {
            format!("Failed to parse server response as JSON: {}", response_line)
        })?;

        Ok(response)
    }

    /// Windows-specific implementation (stub)
    #[cfg(windows)]
    async fn send_command_windows(&self, _command: IpcCommand) -> Result<IpcResponse> {
        // TODO: Implement Windows named pipe client
        anyhow::bail!(
            "Windows named pipe client not yet implemented\n\
             \n\
             The IPC client currently only supports Unix/macOS platforms.\n\
             On Windows, please use file-based secret management for now."
        )
    }

    /// Helper method to set a secret
    pub async fn set_secret(
        &self,
        component_id: &str,
        key: &str,
        value: &str,
    ) -> Result<IpcResponse> {
        let command = IpcCommand::SetSecret {
            component_id: component_id.to_string(),
            key: key.to_string(),
            value: value.to_string(),
        };
        self.send_command(command).await
    }

    /// Helper method to delete a secret
    pub async fn delete_secret(&self, component_id: &str, key: &str) -> Result<IpcResponse> {
        let command = IpcCommand::DeleteSecret {
            component_id: component_id.to_string(),
            key: key.to_string(),
        };
        self.send_command(command).await
    }

    /// Helper method to list secrets
    pub async fn list_secrets(&self, component_id: &str, show_values: bool) -> Result<IpcResponse> {
        let command = IpcCommand::ListSecrets {
            component_id: component_id.to_string(),
            show_values,
        };
        self.send_command(command).await
    }

    /// Helper method to ping the server
    pub async fn ping(&self) -> Result<IpcResponse> {
        let command = IpcCommand::Ping;
        self.send_command(command).await
    }
}

/// Read secrets from stdin in KEY=VALUE format
pub fn read_secrets_from_stdin() -> Result<Vec<(String, String)>> {
    let mut secrets = Vec::new();
    let stdin = std::io::stdin();
    let mut buffer = String::new();

    loop {
        buffer.clear();
        let bytes_read = std::io::BufRead::read_line(&mut stdin.lock(), &mut buffer)?;

        // EOF reached
        if bytes_read == 0 {
            break;
        }

        let line = buffer.trim();

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Parse KEY=VALUE
        if let Some(pos) = line.find('=') {
            let key = line[..pos].trim().to_string();
            let value = line[pos + 1..].trim().to_string();

            if key.is_empty() {
                eprintln!("Warning: Skipping line with empty key: {}", line);
                continue;
            }

            secrets.push((key, value));
        } else {
            eprintln!(
                "Warning: Skipping invalid line (expected KEY=VALUE): {}",
                line
            );
        }
    }

    Ok(secrets)
}

/// Prompt user for confirmation
pub fn prompt_confirmation(message: &str) -> Result<bool> {
    print!("{} [y/N]: ", message);
    std::io::stdout().flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    Ok(input.trim().eq_ignore_ascii_case("y"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipc_client_creation() {
        let client = IpcClient::new();
        assert!(client.is_ok());
    }

    #[test]
    fn test_ipc_client_with_custom_path() {
        let path = PathBuf::from("/tmp/test.sock");
        let client = IpcClient::with_socket_path(path.clone());
        assert_eq!(client.socket_path, path);
    }

    #[test]
    fn test_ipc_client_with_timeout() {
        let client = IpcClient::new()
            .unwrap()
            .with_timeout(Duration::from_secs(10));
        assert_eq!(client.timeout_duration, Duration::from_secs(10));
    }
}
