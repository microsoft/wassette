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
use rmcp::model::CallToolRequestParams;
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

    let req = CallToolRequestParams::new(tool.as_str().to_string()).with_arguments(args);

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
    let config = config::Config::from_serve(
        &crate::commands::Serve {
            component_dir,
            transport: Default::default(),
            env_vars: vec![],
            env_file: None,
            disable_builtin_tools: false,
            bind_address: None,
            manifest: None,
            continue_on_provisioning_failure: false,
            allowed_hosts: None,
            legacy_sessions: None,
            json_response: None,
        },
        None,
    )
    .context("Failed to load configuration")?;

    // Use unloaded manager for fast CLI startup, but preserve custom secrets dir
    let config::Config {
        component_dir,
        secrets_dir,
        environment_vars,
        bind_address: _,
        allowed_hosts: _,
        legacy_sessions: _,
        json_response: _,
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

/// Create a LifecycleManager that can resolve tool names to their components.
///
/// Only the handlers that look a tool up by name need this. A one-shot CLI process never
/// runs the background restore that fills the registry, so without hydrating it a component
/// sitting on disk is invisible to tool-name lookup. Hydration reads the cached metadata for
/// every component in the directory, so the other subcommands keep the unloaded manager
/// described above and do not pay for a scan they cannot use.
pub async fn create_lifecycle_manager_for_tool_lookup(
    component_dir: Option<PathBuf>,
) -> Result<LifecycleManager> {
    let manager = create_lifecycle_manager(component_dir).await?;
    manager.populate_registry_from_metadata().await?;
    Ok(manager)
}

/// Create the LifecycleManager for `wassette tool invoke <name>`.
///
/// Only a component-exported tool name is resolved through the registry, so only that case
/// pays for hydrating it. A built-in name is dispatched straight off [`ToolName`] and never
/// consults the tool map, while hydration reads and validates the cached metadata of every
/// installed component. Deciding this before the manager is built is what keeps
/// `wassette tool invoke load-component` from scanning a component directory it will not look
/// at.
pub async fn create_lifecycle_manager_for_tool_invoke(
    component_dir: Option<PathBuf>,
    tool_name: &str,
) -> Result<LifecycleManager> {
    if ToolName::try_from(tool_name).is_ok() {
        create_lifecycle_manager(component_dir).await
    } else {
        create_lifecycle_manager_for_tool_lookup(component_dir).await
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// Writes the on-disk state a previous process leaves behind for an installed component:
    /// a real component plus cached tool metadata. Hydration inspects the binary
    /// and checks its stamp without compiling it.
    async fn install_cached_component(
        dir: &Path,
        component_id: &str,
        tool_name: &str,
    ) -> Result<()> {
        let sources = dir.join(".test-sources");
        tokio::fs::create_dir_all(&sources).await?;
        let artifact = sources.join(format!("{component_id}.wasm"));
        let bytes = wat::parse_str(format!(
            r#"(component ${component_id}
                (core module $m (func (export "run")))
                (core instance $i (instantiate $m))
                (func (export "{tool_name}") (canon lift (core func $i "run")))
            )"#,
        ))?;
        tokio::fs::write(&artifact, bytes).await?;
        let manager = LifecycleManager::builder(dir)
            .with_secrets_dir(dir.join("secrets"))
            .with_eager_loading(false)
            .build()
            .await?;
        manager
            .load_component(&format!("file://{}", artifact.display()))
            .await?;

        Ok(())
    }

    /// `wassette tool invoke` only has to resolve a name through the tool registry when that
    /// name belongs to a component. A built-in is dispatched straight off [`ToolName`] and
    /// never consults the tool map, while hydrating the registry reads and validates the
    /// cached metadata of every installed component. Built-in invocations must therefore not
    /// pay for hydration, and component tool invocations must still get it.
    #[tokio::test]
    async fn test_tool_invoke_hydrates_the_registry_only_for_component_tools() -> Result<()> {
        let dir = tempfile::tempdir()?;
        install_cached_component(dir.path(), "cached_component", "cached-tool").await?;

        let builtin = create_lifecycle_manager_for_tool_invoke(
            Some(dir.path().to_path_buf()),
            ToolName::LoadComponent.as_str(),
        )
        .await?;
        assert!(
            builtin.list_tools().await.is_empty(),
            "invoking a built-in must not scan and validate every installed component's metadata"
        );

        let component_tool =
            create_lifecycle_manager_for_tool_invoke(Some(dir.path().to_path_buf()), "cached-tool")
                .await?;
        assert_eq!(
            component_tool
                .get_component_id_for_tool("cached-tool")
                .await?,
            "cached_component",
            "invoking a component tool must still resolve it from the cached metadata"
        );

        Ok(())
    }
}
