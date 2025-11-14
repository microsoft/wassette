// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Shared IPC protocol definitions for client-server communication
//!
//! This module defines the request and response types used for communication
//! between the IPC client (CLI commands) and the IPC server (running alongside
//! the MCP server).

use serde::{Deserialize, Serialize};

/// IPC command sent from client to server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command")]
pub enum IpcCommand {
    /// Ping command for testing connectivity
    #[serde(rename = "ping")]
    Ping,
    /// Set a secret for a component
    #[serde(rename = "set_secret")]
    SetSecret {
        /// Component identifier
        component_id: String,
        /// Secret key
        key: String,
        /// Secret value
        value: String,
    },
    /// Delete a secret from a component
    #[serde(rename = "delete_secret")]
    DeleteSecret {
        /// Component identifier
        component_id: String,
        /// Secret key
        key: String,
    },
    /// List secrets for a component
    #[serde(rename = "list_secrets")]
    ListSecrets {
        /// Component identifier
        component_id: String,
        /// Whether to include secret values in the response
        show_values: bool,
    },
}

/// IPC response sent from server to client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponse {
    /// Response status ("success" or "error")
    pub status: String,
    /// Response message
    pub message: String,
    /// Optional response data
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl IpcResponse {
    /// Create a success response
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            status: "success".to_string(),
            message: message.into(),
            data: None,
        }
    }

    /// Create a success response with data
    pub fn success_with_data(message: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            status: "success".to_string(),
            message: message.into(),
            data: Some(data),
        }
    }

    /// Create an error response
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            status: "error".to_string(),
            message: message.into(),
            data: None,
        }
    }

    /// Check if the response indicates success
    pub fn is_success(&self) -> bool {
        self.status == "success"
    }

    /// Check if the response indicates an error
    pub fn is_error(&self) -> bool {
        self.status == "error"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipc_command_serialization() {
        let cmd = IpcCommand::Ping;
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(json, r#"{"command":"ping"}"#);

        let cmd = IpcCommand::SetSecret {
            component_id: "test".to_string(),
            key: "API_KEY".to_string(),
            value: "secret123".to_string(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let parsed: IpcCommand = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, IpcCommand::SetSecret { .. }));
    }

    #[test]
    fn test_ipc_response_serialization() {
        let resp = IpcResponse::success("test message");
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains(r#""status":"success"#));
        assert!(json.contains(r#""message":"test message"#));

        let resp = IpcResponse::error("error message");
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains(r#""status":"error"#));
    }

    #[test]
    fn test_ipc_response_helpers() {
        let resp = IpcResponse::success("ok");
        assert!(resp.is_success());
        assert!(!resp.is_error());

        let resp = IpcResponse::error("fail");
        assert!(resp.is_error());
        assert!(!resp.is_success());
    }
}
