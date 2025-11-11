// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Tool name definitions and conversions for the MCP server

use anyhow::Result;

/// Represents the different types of tools available in the MCP server
#[derive(Debug, Clone, PartialEq)]
pub enum ToolName {
    LoadComponent,
    UnloadComponent,
    ListComponents,
    GetPolicy,
    GrantStoragePermission,
    GrantNetworkPermission,
    GrantEnvironmentVariablePermission,
    GrantMemoryPermission,
    RevokeStoragePermission,
    RevokeNetworkPermission,
    RevokeEnvironmentVariablePermission,
    ResetPermission,
}

impl ToolName {
    /// Get the tool name as a string constant
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::LoadComponent => Self::LOAD_COMPONENT,
            Self::UnloadComponent => Self::UNLOAD_COMPONENT,
            Self::ListComponents => Self::LIST_COMPONENTS,
            Self::GetPolicy => Self::GET_POLICY,
            Self::GrantStoragePermission => Self::GRANT_STORAGE_PERMISSION,
            Self::GrantNetworkPermission => Self::GRANT_NETWORK_PERMISSION,
            Self::GrantEnvironmentVariablePermission => Self::GRANT_ENVIRONMENT_VARIABLE_PERMISSION,
            Self::GrantMemoryPermission => Self::GRANT_MEMORY_PERMISSION,
            Self::RevokeStoragePermission => Self::REVOKE_STORAGE_PERMISSION,
            Self::RevokeNetworkPermission => Self::REVOKE_NETWORK_PERMISSION,
            Self::RevokeEnvironmentVariablePermission => {
                Self::REVOKE_ENVIRONMENT_VARIABLE_PERMISSION
            }
            Self::ResetPermission => Self::RESET_PERMISSION,
        }
    }

    // String constants for tool names
    const LOAD_COMPONENT: &'static str = "load-component";
    const UNLOAD_COMPONENT: &'static str = "unload-component";
    const LIST_COMPONENTS: &'static str = "list-components";
    const GET_POLICY: &'static str = "get-policy";
    const GRANT_STORAGE_PERMISSION: &'static str = "grant-storage-permission";
    const GRANT_NETWORK_PERMISSION: &'static str = "grant-network-permission";
    const GRANT_ENVIRONMENT_VARIABLE_PERMISSION: &'static str =
        "grant-environment-variable-permission";
    const GRANT_MEMORY_PERMISSION: &'static str = "grant-memory-permission";
    const REVOKE_STORAGE_PERMISSION: &'static str = "revoke-storage-permission";
    const REVOKE_NETWORK_PERMISSION: &'static str = "revoke-network-permission";
    const REVOKE_ENVIRONMENT_VARIABLE_PERMISSION: &'static str =
        "revoke-environment-variable-permission";
    const RESET_PERMISSION: &'static str = "reset-permission";
}

impl TryFrom<&str> for ToolName {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            Self::LOAD_COMPONENT => Ok(Self::LoadComponent),
            Self::UNLOAD_COMPONENT => Ok(Self::UnloadComponent),
            Self::LIST_COMPONENTS => Ok(Self::ListComponents),
            Self::GET_POLICY => Ok(Self::GetPolicy),
            Self::GRANT_STORAGE_PERMISSION => Ok(Self::GrantStoragePermission),
            Self::GRANT_NETWORK_PERMISSION => Ok(Self::GrantNetworkPermission),
            Self::GRANT_ENVIRONMENT_VARIABLE_PERMISSION => {
                Ok(Self::GrantEnvironmentVariablePermission)
            }
            Self::GRANT_MEMORY_PERMISSION => Ok(Self::GrantMemoryPermission),
            Self::REVOKE_STORAGE_PERMISSION => Ok(Self::RevokeStoragePermission),
            Self::REVOKE_NETWORK_PERMISSION => Ok(Self::RevokeNetworkPermission),
            Self::REVOKE_ENVIRONMENT_VARIABLE_PERMISSION => {
                Ok(Self::RevokeEnvironmentVariablePermission)
            }
            Self::RESET_PERMISSION => Ok(Self::ResetPermission),
            _ => Err(anyhow::anyhow!("Unknown tool name: {}", value)),
        }
    }
}

impl TryFrom<String> for ToolName {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl AsRef<str> for ToolName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_name_from_str() {
        assert_eq!(
            ToolName::try_from("load-component").unwrap(),
            ToolName::LoadComponent
        );
        assert_eq!(
            ToolName::try_from("unload-component").unwrap(),
            ToolName::UnloadComponent
        );
        assert_eq!(
            ToolName::try_from("list-components").unwrap(),
            ToolName::ListComponents
        );
        assert_eq!(
            ToolName::try_from("get-policy").unwrap(),
            ToolName::GetPolicy
        );
        assert_eq!(
            ToolName::try_from("grant-storage-permission").unwrap(),
            ToolName::GrantStoragePermission
        );
        assert_eq!(
            ToolName::try_from("grant-network-permission").unwrap(),
            ToolName::GrantNetworkPermission
        );
        assert_eq!(
            ToolName::try_from("grant-environment-variable-permission").unwrap(),
            ToolName::GrantEnvironmentVariablePermission
        );
        assert_eq!(
            ToolName::try_from("grant-memory-permission").unwrap(),
            ToolName::GrantMemoryPermission
        );
        assert_eq!(
            ToolName::try_from("revoke-storage-permission").unwrap(),
            ToolName::RevokeStoragePermission
        );
        assert_eq!(
            ToolName::try_from("revoke-network-permission").unwrap(),
            ToolName::RevokeNetworkPermission
        );
        assert_eq!(
            ToolName::try_from("revoke-environment-variable-permission").unwrap(),
            ToolName::RevokeEnvironmentVariablePermission
        );
        assert_eq!(
            ToolName::try_from("reset-permission").unwrap(),
            ToolName::ResetPermission
        );

        // Test invalid tool name
        assert!(ToolName::try_from("invalid-tool").is_err());
    }

    #[test]
    fn test_tool_name_as_str() {
        assert_eq!(ToolName::LoadComponent.as_str(), "load-component");
        assert_eq!(ToolName::UnloadComponent.as_str(), "unload-component");
        assert_eq!(ToolName::ListComponents.as_str(), "list-components");
        assert_eq!(ToolName::GetPolicy.as_str(), "get-policy");
        assert_eq!(
            ToolName::GrantStoragePermission.as_str(),
            "grant-storage-permission"
        );
        assert_eq!(
            ToolName::GrantNetworkPermission.as_str(),
            "grant-network-permission"
        );
        assert_eq!(
            ToolName::GrantEnvironmentVariablePermission.as_str(),
            "grant-environment-variable-permission"
        );
        assert_eq!(
            ToolName::GrantMemoryPermission.as_str(),
            "grant-memory-permission"
        );
        assert_eq!(
            ToolName::RevokeStoragePermission.as_str(),
            "revoke-storage-permission"
        );
        assert_eq!(
            ToolName::RevokeNetworkPermission.as_str(),
            "revoke-network-permission"
        );
        assert_eq!(
            ToolName::RevokeEnvironmentVariablePermission.as_str(),
            "revoke-environment-variable-permission"
        );
        assert_eq!(ToolName::ResetPermission.as_str(), "reset-permission");
    }

    #[test]
    fn test_tool_name_roundtrip() {
        let test_cases = [
            ToolName::LoadComponent,
            ToolName::UnloadComponent,
            ToolName::ListComponents,
            ToolName::GetPolicy,
            ToolName::GrantStoragePermission,
            ToolName::GrantNetworkPermission,
            ToolName::GrantEnvironmentVariablePermission,
            ToolName::GrantMemoryPermission,
            ToolName::RevokeStoragePermission,
            ToolName::RevokeNetworkPermission,
            ToolName::RevokeEnvironmentVariablePermission,
            ToolName::ResetPermission,
        ];

        for tool in test_cases {
            let str_repr = tool.as_str();
            let parsed = ToolName::try_from(str_repr).unwrap();
            assert_eq!(tool, parsed);
        }
    }
}
