use crate::{PolicyDocument, PolicyResult};
use anyhow::Context;
use std::fs;
use std::path::Path;

pub struct PolicyParser;

impl PolicyParser {
    /// Parse a policy document from a YAML string
    ///
    /// # Example
    ///
    /// ```rust
    /// use policy_mcp::PolicyParser;
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
    /// use policy_mcp::PolicyParser;
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
    /// use policy_mcp::PolicyParser;
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
    /// use policy_mcp::{PolicyParser, PolicyDocument, Permissions};
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
    /// use policy_mcp::PolicyParser;
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
    use super::*;
    use crate::{AccessType, PermissionList, Permissions, StoragePermission};
    use std::io::Write;
    use tempfile::NamedTempFile;

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
}
