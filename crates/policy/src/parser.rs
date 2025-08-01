use std::fs;
use std::path::Path;

use anyhow::Context;

use crate::{PolicyDocument, PolicyResult};

pub struct PolicyParser;

impl PolicyParser {
    /// Parse a policy document from a YAML string
    ///
    /// # Example
    ///
    /// ```rust
    /// use policy::PolicyParser;
    ///
    /// let yaml_content = r#"
    /// version: "1.0"
    /// description: "Test policy"
    /// permissions:
    ///   storage:
    ///     allow:
    ///     - uri: "fs://work/agent/**"
    ///       access: ["read", "write"]
    /// "#;
    ///
    /// let policy = PolicyParser::parse_str(yaml_content).unwrap();
    /// assert_eq!(policy.version, "1.0");
    /// ```
    pub fn parse_str(content: impl AsRef<str>) -> PolicyResult<PolicyDocument> {
        let document: PolicyDocument = serde_yaml::from_str(content.as_ref())?;
        document.validate()?;
        Ok(document)
    }

    /// Parse a policy document from a file path
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use policy::PolicyParser;
    ///
    /// let policy = PolicyParser::parse_file("./testdata/docker.yaml").unwrap();
    /// println!("Loaded policy: {}", policy.description.unwrap_or_default());
    /// ```
    pub fn parse_file<P: AsRef<Path>>(path: P) -> PolicyResult<PolicyDocument> {
        let content = fs::read_to_string(path)?;
        Self::parse_str(&content)
    }

    /// Parse a policy document from bytes
    ///
    /// # Example
    ///
    /// ```rust
    /// use policy::PolicyParser;
    ///
    /// let policy = PolicyParser::parse_bytes(b"version: '1.0'\npermissions: {}").unwrap();
    /// assert_eq!(policy.version, "1.0");
    /// ```
    pub fn parse_bytes(bytes: &[u8]) -> PolicyResult<PolicyDocument> {
        let content = std::str::from_utf8(bytes).context("Not valid UTF-8")?;
        Self::parse_str(content)
    }

    /// Serialize a policy document to YAML string
    ///
    /// # Example
    ///
    /// ```rust
    /// use policy::{PolicyParser, PolicyDocument, Permissions};
    ///
    /// let policy = PolicyDocument {
    ///     version: "1.0".to_string(),
    ///     description: Some("Test policy".to_string()),
    ///     permissions: Permissions::default(),
    /// };
    ///
    /// let yaml = PolicyParser::to_yaml(&policy).unwrap();
    /// assert!(yaml.contains("version: '1.0'"));
    /// ```
    pub fn to_yaml(document: &PolicyDocument) -> PolicyResult<String> {
        document.validate()?;
        let yaml = serde_yaml::to_string(document)?;
        Ok(yaml)
    }

    /// Write a policy document to a file
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use policy::PolicyParser;
    ///
    /// let policy = PolicyParser::parse_file("./testdata/docker.yaml").unwrap();
    /// PolicyParser::write_file(&policy, "./testdata/docker.yaml").unwrap();
    /// ```
    pub fn write_file<P: AsRef<Path>>(document: &PolicyDocument, path: P) -> PolicyResult<()> {
        let yaml = Self::to_yaml(document)?;
        fs::write(path, yaml)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;
    use crate::{AccessType, CapabilityAction, PermissionList, Permissions, StoragePermission};

    #[test]
    fn test_parse_str_valid() {
        let yaml_content = r#"
version: "1.0"
description: "Test policy"
permissions:
  storage:
    allow:
    - uri: "fs://work/agent/**"
      access: ["read", "write"]
"#;

        let policy = PolicyParser::parse_str(yaml_content).unwrap();
        assert_eq!(policy.version, "1.0");
        assert_eq!(policy.description, Some("Test policy".to_string()));

        let storage = policy.permissions.storage.unwrap();
        let allow_list = storage.allow.unwrap();
        assert_eq!(allow_list.len(), 1);
        assert_eq!(allow_list[0].uri, "fs://work/agent/**");
        assert_eq!(
            allow_list[0].access,
            vec![AccessType::Read, AccessType::Write]
        );
    }

    #[test]
    fn test_parse_str_invalid_version() {
        let yaml_content = r#"
version: "2.0"
description: "Test policy"
permissions: {}
"#;

        let result = PolicyParser::parse_str(yaml_content);
        assert!(result.is_err());
        result.unwrap_err();
    }

    #[test]
    fn test_parse_str_invalid_yaml() {
        let yaml_content = r#"
invalid: yaml: content
  - malformed
"#;

        let result = PolicyParser::parse_str(yaml_content);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().len() > 0);
    }

    #[test]
    fn test_round_trip_serialization() {
        let mut permissions = Permissions::default();
        permissions.storage = Some(PermissionList {
            allow: Some(vec![StoragePermission {
                uri: "fs://work/agent/**".to_string(),
                access: vec![AccessType::Read, AccessType::Write],
            }]),
            deny: None,
        });

        let original = PolicyDocument {
            version: "1.0".to_string(),
            description: Some("Test policy".to_string()),
            permissions,
        };

        let yaml = PolicyParser::to_yaml(&original).unwrap();
        let parsed = PolicyParser::parse_str(&yaml).unwrap();

        assert_eq!(original, parsed);
    }

    #[test]
    fn test_parse_file() {
        let mut temp_file = NamedTempFile::new().unwrap();
        let yaml_content = r#"
version: "1.0"
description: "File test policy"
permissions:
  environment:
    allow:
    - key: "PATH"
    - key: "HOME"
"#;

        temp_file.write_all(yaml_content.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        let policy = PolicyParser::parse_file(temp_file.path()).unwrap();
        assert_eq!(policy.version, "1.0");
        assert_eq!(policy.description, Some("File test policy".to_string()));

        let env = policy.permissions.environment.unwrap();
        let allow_list = env.allow.unwrap();
        assert_eq!(allow_list.len(), 2);
        assert_eq!(allow_list[0].key, "PATH");
        assert_eq!(allow_list[1].key, "HOME");
    }

    #[test]
    fn test_write_file() {
        let permissions = Permissions::default();
        let policy = PolicyDocument {
            version: "1.0".to_string(),
            description: Some("Write test policy".to_string()),
            permissions,
        };

        let temp_file = NamedTempFile::new().unwrap();
        PolicyParser::write_file(&policy, temp_file.path()).unwrap();

        let loaded_policy = PolicyParser::parse_file(temp_file.path()).unwrap();
        assert_eq!(policy, loaded_policy);
    }

    #[test]
    fn test_parse_bytes() {
        let yaml_content = r#"
version: "1.0"
description: "Bytes test"
permissions: {}
"#;

        let policy = PolicyParser::parse_bytes(yaml_content.as_bytes()).unwrap();
        assert_eq!(policy.version, "1.0");
        assert_eq!(policy.description, Some("Bytes test".to_string()));
    }

    #[test]
    fn test_parse_bytes_invalid_utf8() {
        let invalid_utf8 = &[0xC0, 0xC1];
        let result = PolicyParser::parse_bytes(invalid_utf8);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_testdata_minimal() {
        let policy = PolicyParser::parse_file("testdata/minimal.yaml").unwrap();
        assert_eq!(policy.version, "1.0");
        assert_eq!(policy.description, Some("Minimal valid policy".to_string()));
        assert!(policy.permissions.storage.is_none());
        assert!(policy.permissions.network.is_none());
        assert!(policy.permissions.environment.is_none());
        assert!(policy.permissions.runtime.is_none());
    }

    #[test]
    fn test_parse_testdata_storage_only() {
        let policy = PolicyParser::parse_file("testdata/storage-only.yaml").unwrap();
        assert_eq!(policy.version, "1.1");
        assert_eq!(
            policy.description,
            Some("Storage-only policy for file system access".to_string())
        );

        let storage = policy.permissions.storage.unwrap();
        let allow_list = storage.allow.unwrap();
        assert_eq!(allow_list.len(), 3);
        assert_eq!(allow_list[0].uri, "fs://tmp/**");
        assert_eq!(allow_list[1].uri, "fs://var/log/*.log");
        assert_eq!(allow_list[2].uri, "fs://home/user/documents/*");

        let deny_list = storage.deny.unwrap();
        assert_eq!(deny_list.len(), 2);
        assert_eq!(deny_list[0].uri, "fs://etc/**");
        assert_eq!(deny_list[1].uri, "fs://root/**");
    }

    #[test]
    fn test_parse_testdata_network_only() {
        let policy = PolicyParser::parse_file("testdata/network-only.yaml").unwrap();
        assert_eq!(policy.version, "1.0");
        assert_eq!(
            policy.description,
            Some("Network-only policy for API access".to_string())
        );

        let network = policy.permissions.network.unwrap();
        let allow_list = network.allow.unwrap();
        assert_eq!(allow_list.len(), 5);

        let deny_list = network.deny.unwrap();
        assert_eq!(deny_list.len(), 3);
    }

    #[test]
    fn test_parse_testdata_environment_only() {
        let policy = PolicyParser::parse_file("testdata/environment-only.yaml").unwrap();
        assert_eq!(policy.version, "1.0");
        assert_eq!(
            policy.description,
            Some("Environment-only policy for basic shell access".to_string())
        );

        let env = policy.permissions.environment.unwrap();
        let allow_list = env.allow.unwrap();
        assert_eq!(allow_list.len(), 8);
        assert_eq!(allow_list[0].key, "PATH");
        assert_eq!(allow_list[1].key, "HOME");
        assert_eq!(allow_list[2].key, "USER");
        assert_eq!(allow_list[7].key, "PYTHON_PATH");
    }

    #[test]
    fn test_parse_testdata_comprehensive() {
        let policy = PolicyParser::parse_file("testdata/comprehensive.yaml").unwrap();
        assert_eq!(policy.version, "1.0");
        assert_eq!(
            policy.description,
            Some("Comprehensive policy with all permission types".to_string())
        );

        assert!(policy.permissions.storage.is_some());
        assert!(policy.permissions.network.is_some());
        assert!(policy.permissions.environment.is_some());
        assert!(policy.permissions.runtime.is_some());
        assert!(policy.permissions.resources.is_some());
        assert!(policy.permissions.ipc.is_some());

        let resources = policy.permissions.resources.unwrap();
        assert_eq!(resources.cpu, Some(50.0));
        assert_eq!(resources.memory, Some(1024));
        assert_eq!(resources.io, Some(1000));
    }

    #[test]
    fn test_parse_testdata_docker_privileged() {
        let policy = PolicyParser::parse_file("testdata/docker-privileged.yaml").unwrap();
        assert_eq!(policy.version, "1.0");
        assert_eq!(
            policy.description,
            Some("Policy with privileged Docker runtime for system administration".to_string())
        );

        let runtime = policy.permissions.runtime.unwrap();
        let docker_runtime = runtime.docker.unwrap();
        let security = docker_runtime.security.unwrap();
        assert_eq!(security.privileged, Some(true));
        assert_eq!(security.no_new_privileges, Some(false));

        let capabilities = security.capabilities.unwrap();
        assert_eq!(capabilities.drop, Some(vec![]));
        let add_caps = capabilities.add.unwrap();
        assert_eq!(add_caps.len(), 3);
    }

    #[test]
    fn test_parse_testdata_restricted() {
        let policy = PolicyParser::parse_file("testdata/restricted.yaml").unwrap();
        assert_eq!(policy.version, "1.0");
        assert_eq!(
            policy.description,
            Some("Highly restricted policy for untrusted code".to_string())
        );

        let storage = policy.permissions.storage.unwrap();
        let allow_list = storage.allow.unwrap();
        assert_eq!(allow_list.len(), 2);
        assert_eq!(allow_list[0].access, vec![AccessType::Read]);
        assert_eq!(allow_list[1].access, vec![AccessType::Write]);

        let resources = policy.permissions.resources.unwrap();
        assert_eq!(resources.cpu, Some(10.0));
        assert_eq!(resources.memory, Some(128));
        assert_eq!(resources.io, Some(100));
    }

    #[test]
    fn test_parse_testdata_development() {
        let policy = PolicyParser::parse_file("testdata/development.yaml").unwrap();
        assert_eq!(policy.version, "1.0");
        assert_eq!(
            policy.description,
            Some("Development environment policy with broad permissions".to_string())
        );

        let storage = policy.permissions.storage.unwrap();
        let allow_list = storage.allow.unwrap();
        assert_eq!(allow_list.len(), 3);

        let network = policy.permissions.network.unwrap();
        let allow_list = network.allow.unwrap();
        assert_eq!(allow_list.len(), 8);

        let env = policy.permissions.environment.unwrap();
        let allow_list = env.allow.unwrap();
        assert_eq!(allow_list.len(), 11);
    }

    #[test]
    fn test_parse_testdata_web_service() {
        let policy = PolicyParser::parse_file("testdata/web-service.yaml").unwrap();
        assert_eq!(policy.version, "1.0");
        assert_eq!(
            policy.description,
            Some("Web service policy for HTTP server deployment".to_string())
        );

        let storage = policy.permissions.storage.unwrap();
        let allow_list = storage.allow.unwrap();
        assert_eq!(allow_list.len(), 3);
        let deny_list = storage.deny.unwrap();
        assert_eq!(deny_list.len(), 1);

        let env = policy.permissions.environment.unwrap();
        let allow_list = env.allow.unwrap();
        assert_eq!(allow_list.len(), 7);
        assert!(allow_list.iter().any(|e| e.key == "DATABASE_URL"));
        assert!(allow_list.iter().any(|e| e.key == "STRIPE_API_KEY"));

        let resources = policy.permissions.resources.unwrap();
        assert_eq!(resources.cpu, Some(75.0));
        assert_eq!(resources.memory, Some(512));
        assert_eq!(resources.io, Some(500));
    }

    #[test]
    fn test_parse_testdata_docker() {
        let policy = PolicyParser::parse_file("testdata/docker.yaml").unwrap();
        assert_eq!(policy.version, "1.0");
        assert_eq!(
            policy.description,
            Some("Permission policy for docker container".to_string())
        );

        let storage = policy.permissions.storage.unwrap();
        let allow_list = storage.allow.unwrap();
        assert_eq!(allow_list.len(), 2);
        assert_eq!(allow_list[0].uri, "fs://work/agent/**");
        assert_eq!(allow_list[1].uri, "fs://work/agent/config.yaml");

        let network = policy.permissions.network.unwrap();
        let allow_list = network.allow.unwrap();
        assert_eq!(allow_list.len(), 3);

        let env = policy.permissions.environment.unwrap();
        let allow_list = env.allow.unwrap();
        assert_eq!(allow_list.len(), 2);
        assert_eq!(allow_list[0].key, "PATH");
        assert_eq!(allow_list[1].key, "HOME");

        let runtime = policy.permissions.runtime.unwrap();
        let docker_runtime = runtime.docker.unwrap();
        let security = docker_runtime.security.unwrap();
        assert_eq!(security.privileged, Some(false));
        assert_eq!(security.no_new_privileges, Some(true));

        let capabilities = security.capabilities.unwrap();
        assert_eq!(capabilities.drop, Some(vec![CapabilityAction::All]));
        assert_eq!(
            capabilities.add,
            Some(vec![CapabilityAction::NetBindService])
        );
    }

    #[test]
    fn test_round_trip_all_testdata() {
        let test_files = [
            "testdata/minimal.yaml",
            "testdata/storage-only.yaml",
            "testdata/network-only.yaml",
            "testdata/environment-only.yaml",
            "testdata/comprehensive.yaml",
            "testdata/docker-privileged.yaml",
            "testdata/restricted.yaml",
            "testdata/development.yaml",
            "testdata/web-service.yaml",
            "testdata/docker.yaml",
        ];

        for file_path in &test_files {
            let original_policy = PolicyParser::parse_file(file_path).unwrap();
            let yaml_string = PolicyParser::to_yaml(&original_policy).unwrap();
            let reparsed_policy = PolicyParser::parse_str(&yaml_string).unwrap();
            assert_eq!(
                original_policy, reparsed_policy,
                "Round trip failed for {}",
                file_path
            );
        }
    }

    #[test]
    fn test_validation_all_testdata() {
        let test_files = [
            "testdata/minimal.yaml",
            "testdata/storage-only.yaml",
            "testdata/network-only.yaml",
            "testdata/environment-only.yaml",
            "testdata/comprehensive.yaml",
            "testdata/docker-privileged.yaml",
            "testdata/restricted.yaml",
            "testdata/development.yaml",
            "testdata/web-service.yaml",
            "testdata/docker.yaml",
        ];

        for file_path in &test_files {
            let policy = PolicyParser::parse_file(file_path).unwrap();
            policy.validate().unwrap_or_else(|e| {
                panic!("Validation failed for {}: {}", file_path, e);
            });
        }
    }
}
