// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Integration tests for IPC client communicating with IPC server

#![cfg(unix)] // IPC client is only fully implemented for Unix

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tempfile::TempDir;
use wassette::{IpcServer, IpcServerConfig, SecretsManager};
use wassette_mcp_server::ipc_client::IpcClient;

/// Helper to start an IPC server in the background
async fn start_test_server(
    socket_path: std::path::PathBuf,
    secrets_dir: std::path::PathBuf,
) -> Result<tokio::task::JoinHandle<()>> {
    let secrets_manager = Arc::new(SecretsManager::new(secrets_dir));
    secrets_manager.ensure_secrets_dir().await?;

    let config = IpcServerConfig::new(socket_path, secrets_manager);
    let mut server = IpcServer::new(config);

    let handle = tokio::spawn(async move {
        if let Err(e) = server.start().await {
            eprintln!("IPC server error: {}", e);
        }
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    Ok(handle)
}

#[tokio::test]
async fn test_ipc_client_ping() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let secrets_dir = temp_dir.path().join("secrets");

    let _server_handle = start_test_server(socket_path.clone(), secrets_dir).await?;

    let client = IpcClient::with_socket_path(socket_path);
    let response = client.ping().await?;

    assert!(response.is_success());
    assert_eq!(response.message, "pong");

    Ok(())
}

#[tokio::test]
async fn test_ipc_client_set_secret() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let secrets_dir = temp_dir.path().join("secrets");

    let _server_handle = start_test_server(socket_path.clone(), secrets_dir.clone()).await?;

    let client = IpcClient::with_socket_path(socket_path);
    let response = client
        .set_secret("test-component", "API_KEY", "secret123")
        .await?;

    assert!(response.is_success());
    assert!(response.message.contains("API_KEY"));
    assert!(response.message.contains("test-component"));

    // Verify the secret was actually saved
    let secrets_manager = SecretsManager::new(secrets_dir);
    let secrets = secrets_manager
        .list_component_secrets("test-component", true)
        .await?;
    assert_eq!(secrets.get("API_KEY"), Some(&Some("secret123".to_string())));

    Ok(())
}

#[tokio::test]
async fn test_ipc_client_list_secrets() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let secrets_dir = temp_dir.path().join("secrets");

    let _server_handle = start_test_server(socket_path.clone(), secrets_dir.clone()).await?;

    let client = IpcClient::with_socket_path(socket_path);

    // Set some secrets first
    client
        .set_secret("test-component", "KEY1", "value1")
        .await?;
    client
        .set_secret("test-component", "KEY2", "value2")
        .await?;

    // List secrets without values
    let response = client.list_secrets("test-component", false).await?;
    assert!(response.is_success());
    assert!(response.data.is_some());

    let data = response.data.unwrap();
    let keys = data["keys"].as_array().unwrap();
    assert_eq!(keys.len(), 2);
    assert!(keys.contains(&serde_json::json!("KEY1")) || keys.contains(&serde_json::json!("KEY2")));

    // List secrets with values
    let response = client.list_secrets("test-component", true).await?;
    assert!(response.is_success());
    assert!(response.data.is_some());

    let data = response.data.unwrap();
    let secrets = data["secrets"].as_object().unwrap();
    assert_eq!(secrets.len(), 2);
    assert_eq!(secrets.get("KEY1").and_then(|v| v.as_str()), Some("value1"));
    assert_eq!(secrets.get("KEY2").and_then(|v| v.as_str()), Some("value2"));

    Ok(())
}

#[tokio::test]
async fn test_ipc_client_delete_secret() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let secrets_dir = temp_dir.path().join("secrets");

    let _server_handle = start_test_server(socket_path.clone(), secrets_dir.clone()).await?;

    let client = IpcClient::with_socket_path(socket_path);

    // Set a secret
    client
        .set_secret("test-component", "API_KEY", "secret123")
        .await?;

    // Delete the secret
    let response = client.delete_secret("test-component", "API_KEY").await?;
    assert!(response.is_success());
    assert!(response.message.contains("API_KEY"));

    // Verify the secret was deleted
    let secrets_manager = SecretsManager::new(secrets_dir);
    let secrets = secrets_manager
        .list_component_secrets("test-component", true)
        .await?;
    assert!(!secrets.contains_key("API_KEY"));

    Ok(())
}

#[tokio::test]
async fn test_ipc_client_server_not_running() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("nonexistent.sock");

    let client = IpcClient::with_socket_path(socket_path);
    let result = client.ping().await;

    assert!(result.is_err());
    let error_msg = result.unwrap_err().to_string();
    assert!(error_msg.contains("not running") || error_msg.contains("not found"));

    Ok(())
}

#[tokio::test]
async fn test_ipc_client_timeout() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let secrets_dir = temp_dir.path().join("secrets");

    let _server_handle = start_test_server(socket_path.clone(), secrets_dir).await?;

    // Create client with very short timeout
    let client = IpcClient::with_socket_path(socket_path).with_timeout(Duration::from_millis(1));

    // This might timeout or succeed depending on timing
    // We just want to ensure the timeout mechanism doesn't panic
    let _ = client.ping().await;

    Ok(())
}

#[tokio::test]
async fn test_ipc_client_multiple_operations() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let secrets_dir = temp_dir.path().join("secrets");

    let _server_handle = start_test_server(socket_path.clone(), secrets_dir.clone()).await?;

    let client = IpcClient::with_socket_path(socket_path);

    // Set multiple secrets
    client.set_secret("comp1", "KEY1", "value1").await?;
    client.set_secret("comp1", "KEY2", "value2").await?;
    client.set_secret("comp2", "KEY3", "value3").await?;

    // List secrets for comp1
    let response = client.list_secrets("comp1", true).await?;
    assert!(response.is_success());
    let data = response.data.unwrap();
    let secrets = data["secrets"].as_object().unwrap();
    assert_eq!(secrets.len(), 2);

    // List secrets for comp2
    let response = client.list_secrets("comp2", true).await?;
    assert!(response.is_success());
    let data = response.data.unwrap();
    let secrets = data["secrets"].as_object().unwrap();
    assert_eq!(secrets.len(), 1);

    // Delete a secret
    client.delete_secret("comp1", "KEY1").await?;

    // Verify deletion
    let response = client.list_secrets("comp1", true).await?;
    let data = response.data.unwrap();
    let secrets = data["secrets"].as_object().unwrap();
    assert_eq!(secrets.len(), 1);
    assert!(!secrets.contains_key("KEY1"));

    Ok(())
}

#[tokio::test]
async fn test_ipc_client_special_characters_in_values() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let secrets_dir = temp_dir.path().join("secrets");

    let _server_handle = start_test_server(socket_path.clone(), secrets_dir).await?;

    let client = IpcClient::with_socket_path(socket_path);

    // Set secret with special characters
    let special_value = r#"{"key": "value", "nested": {"foo": "bar"}}"#;
    client
        .set_secret("test-component", "JSON_SECRET", special_value)
        .await?;

    // List and verify
    let response = client.list_secrets("test-component", true).await?;
    assert!(response.is_success());
    let data = response.data.unwrap();
    let secrets = data["secrets"].as_object().unwrap();
    assert_eq!(
        secrets.get("JSON_SECRET").and_then(|v| v.as_str()),
        Some(special_value)
    );

    Ok(())
}

#[tokio::test]
async fn test_ipc_client_component_with_special_characters() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test.sock");
    let secrets_dir = temp_dir.path().join("secrets");

    let _server_handle = start_test_server(socket_path.clone(), secrets_dir).await?;

    let client = IpcClient::with_socket_path(socket_path);

    // Component IDs with special characters (will be sanitized by the server)
    client
        .set_secret("my-component.v1", "KEY1", "value1")
        .await?;
    client.set_secret("my/component", "KEY2", "value2").await?;

    // List secrets - component IDs are sanitized on the server side
    let response = client.list_secrets("my-component.v1", true).await?;
    assert!(response.is_success());

    let response = client.list_secrets("my/component", true).await?;
    assert!(response.is_success());

    Ok(())
}
