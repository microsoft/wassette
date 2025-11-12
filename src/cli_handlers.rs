// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! CLI command handlers for wassette

use std::path::PathBuf;

use anyhow::{Context, Result};
use mcp_server::components::{
    handle_list_components, handle_load_component_cli, handle_unload_component_cli,
};
use mcp_server::tools::{
    handle_get_policy, handle_grant_environment_variable_permission,
    handle_grant_memory_permission, handle_grant_network_permission,
    handle_grant_storage_permission, handle_reset_permission,
    handle_revoke_environment_variable_permission, handle_revoke_network_permission,
    handle_revoke_storage_permission,
};
use mcp_server::LifecycleManager;
use rmcp::model::CallToolRequestParam;
use serde_json::{Map, Value};

use crate::config;
use crate::format::{print_result, OutputFormat};
use crate::tools::ToolName;

/// Handle CLI tool commands by creating appropriate tool call requests
pub async fn handle_tool_cli_command(
    lifecycle_manager: &LifecycleManager,
    tool_name: &str,
    args: Map<String, Value>,
    output_format: OutputFormat,
) -> Result<()> {
    let tool = ToolName::try_from(tool_name)?;

    let req = CallToolRequestParam {
        name: tool.as_str().to_string().into(),
        arguments: Some(args),
    };

    let result = match tool {
        ToolName::LoadComponent => handle_load_component_cli(&req, lifecycle_manager).await?,
        ToolName::UnloadComponent => handle_unload_component_cli(&req, lifecycle_manager).await?,
        ToolName::ListComponents => handle_list_components(lifecycle_manager).await?,
        ToolName::GetPolicy => handle_get_policy(&req, lifecycle_manager).await?,
        ToolName::GrantStoragePermission => {
            handle_grant_storage_permission(&req, lifecycle_manager).await?
        }
        ToolName::GrantNetworkPermission => {
            handle_grant_network_permission(&req, lifecycle_manager).await?
        }
        ToolName::GrantEnvironmentVariablePermission => {
            handle_grant_environment_variable_permission(&req, lifecycle_manager).await?
        }
        ToolName::GrantMemoryPermission => {
            handle_grant_memory_permission(&req, lifecycle_manager).await?
        }
        ToolName::RevokeStoragePermission => {
            handle_revoke_storage_permission(&req, lifecycle_manager).await?
        }
        ToolName::RevokeNetworkPermission => {
            handle_revoke_network_permission(&req, lifecycle_manager).await?
        }
        ToolName::RevokeEnvironmentVariablePermission => {
            handle_revoke_environment_variable_permission(&req, lifecycle_manager).await?
        }
        ToolName::ResetPermission => handle_reset_permission(&req, lifecycle_manager).await?,
    };

    // Print the result using the format module
    print_result(&result, output_format)?;

    // Exit with error code if the tool result indicates an error
    if result.is_error.unwrap_or(false) {
        std::process::exit(1);
    }

    Ok(())
}

/// Create LifecycleManager from component directory
///
/// For CLI responsiveness, we create an unloaded lifecycle manager which
/// initializes engine/linker without compiling/scanning all components.
/// Component metadata or lazy loads are used by individual handlers.
pub async fn create_lifecycle_manager(component_dir: Option<PathBuf>) -> Result<LifecycleManager> {
    let config = if let Some(dir) = component_dir {
        config::Config {
            component_dir: dir,
            secrets_dir: config::get_secrets_dir().unwrap_or_else(|_| {
                eprintln!("WARN: Unable to determine default secrets directory, using `secrets` directory in the current working directory");
                PathBuf::from("./secrets")
            }),
            environment_vars: std::collections::HashMap::new(),
            bind_address: "127.0.0.1:9001".to_string(),
        }
    } else {
        config::Config::from_serve(&crate::commands::Serve {
            component_dir: None,
            transport: Default::default(),
            env_vars: vec![],
            env_file: None,
            disable_builtin_tools: false,
            bind_address: None,
            manifest: None,
        })
        .context("Failed to load configuration")?
    };

    // Use unloaded manager for fast CLI startup, but preserve custom secrets dir
    let config::Config {
        component_dir,
        secrets_dir,
        environment_vars,
        bind_address: _,
    } = config;

    LifecycleManager::builder(component_dir)
        .with_environment_vars(environment_vars)
        .with_secrets_dir(secrets_dir)
        .with_oci_client(oci_client::Client::default())
        .with_http_client(reqwest::Client::default())
        .with_eager_loading(false)
        .build()
        .await
}
