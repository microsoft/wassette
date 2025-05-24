//! Capability Policy for Local MCP Servers
//!
//! Parser for MCP server policy files. Supports storage, network, environment
//! and runtime permissions.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub mod parser;
pub mod types;

pub use parser::PolicyParser;
pub use types::*;

/// Policy document structure
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PolicyDocument {
    /// Policy format version
    pub version: String,

    /// Human-readable description of the policy
    pub description: Option<String>,

    /// Permission definitions
    pub permissions: Permissions,
}

impl PolicyDocument {
    /// Validate the policy document
    pub fn validate(&self) -> Result<()> {
        // Only supporting v1.x for now - will add v2 when we know what it looks like
        if !self.version.starts_with("1.") {
            bail!("Unsupported version: {}", self.version);
        }
        self.permissions
            .validate()
            .context("Permission validation failed")?;
        Ok(())
    }

    /// Create a new policy document with default permissions
    pub fn new(version: impl Into<String>, description: Option<String>) -> Self {
        Self {
            version: version.into(),
            description,
            ..Default::default()
        }
    }
}

pub type PolicyResult<T> = Result<T>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_validation() {
        let policy = PolicyDocument {
            version: "1.0".to_string(),
            description: Some("Test policy".to_string()),
            permissions: Permissions::default(),
        };

        assert!(policy.validate().is_ok());
    }

    #[test]
    fn test_policy_new_constructor() {
        let policy = PolicyDocument::new("1.0", Some("Test policy".to_string()));
        assert_eq!(policy.version, "1.0");
        assert_eq!(policy.description, Some("Test policy".to_string()));
        assert!(policy.validate().is_ok());

        let policy2 = PolicyDocument::new("1.1".to_string(), None);
        assert_eq!(policy2.version, "1.1");
        assert_eq!(policy2.description, None);
    }

    #[test]
    fn test_invalid_version() {
        let policy = PolicyDocument {
            version: "2.0".to_string(),
            description: None,
            permissions: Permissions::default(),
        };

        let result = policy.validate();
        assert!(result.is_err());
        let error_message = result.unwrap_err().to_string();
        assert!(error_message.contains("Unsupported version: 2.0"));
    }

    #[test]
    fn test_parse_docker_yaml() {
        let policy = PolicyParser::parse_file("testdata/docker.yaml").unwrap();
        policy.validate().unwrap();

        assert_eq!(policy.version, "1.0");
        assert_eq!(
            policy.description,
            Some("Permission policy for docker container".to_string())
        );

        let storage = policy.permissions.storage.as_ref().unwrap();
        let storage_allow = storage.allow.as_ref().unwrap();
        assert_eq!(storage_allow.len(), 2);
        assert_eq!(storage_allow[0].uri, "fs://work/agent/**");
        assert_eq!(
            storage_allow[0].access,
            vec![AccessType::Read, AccessType::Write]
        );

        assert_eq!(storage_allow[1].uri, "fs://work/agent/config.yaml");
        assert_eq!(storage_allow[1].access, vec![AccessType::Read]);

        let network = policy.permissions.network.as_ref().unwrap();
        let network_allow = network.allow.as_ref().unwrap();
        assert_eq!(network_allow.len(), 3);

        match &network_allow[0] {
            NetworkPermission::Host(host) => assert_eq!(host.host, "api.openai.com"),
            _ => panic!("Expected host permission"),
        }

        match &network_allow[1] {
            NetworkPermission::Host(host) => assert_eq!(host.host, "*.internal.myorg.com"),
            _ => panic!("Expected host permission"),
        }

        match &network_allow[2] {
            NetworkPermission::Cidr(cidr) => assert_eq!(cidr.cidr, "10.0.0.0/8"),
            _ => panic!("Expected CIDR permission"),
        }

        let env = policy.permissions.environment.as_ref().unwrap();
        let env_allow = env.allow.as_ref().unwrap();
        assert_eq!(env_allow.len(), 2);
        assert_eq!(env_allow[0].key, "PATH");
        assert_eq!(env_allow[1].key, "HOME");

        let runtime = policy.permissions.runtime.as_ref().unwrap();
        let docker_runtime = runtime.docker.as_ref().unwrap();
        let docker_security = docker_runtime.security.as_ref().unwrap();

        assert_eq!(docker_security.privileged, Some(false));
        assert_eq!(docker_security.no_new_privileges, Some(true));

        let capabilities = docker_security.capabilities.as_ref().unwrap();
        assert_eq!(capabilities.drop, Some(vec![CapabilityAction::All]));
        assert_eq!(
            capabilities.add,
            Some(vec![CapabilityAction::NetBindService])
        );

        assert!(
            runtime.hyperlight.is_none(),
            "Hyperlight should be None since it's just a comment"
        );
    }

    #[test]
    fn test_round_trip_docker_yaml() {
        let original_policy = PolicyParser::parse_file("testdata/docker.yaml").unwrap();
        let yaml_string = PolicyParser::to_yaml(&original_policy).unwrap();
        let reparsed_policy = PolicyParser::parse_str(&yaml_string).unwrap();

        assert_eq!(original_policy, reparsed_policy);
    }
}
