use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use policy_mcp::{PolicyDocument, PolicyParser, EnvironmentPermission};

/// Loads policy from a file and returns environment variables as a HashMap.
/// 
/// This uses the policy-mcp crate to parse policy files in YAML format.
/// For backward compatibility, it extracts environment variables from the policy.
pub fn load_policy<P: AsRef<Path>>(path: P) -> Result<HashMap<String, String>> {
    let policy: PolicyDocument = PolicyParser::parse_file(path)?;
    
    let mut env_vars = HashMap::new();
    
    // Extract environment variables from the policy
    if let Some(env_permissions) = &policy.permissions.environment {
        if let Some(allowed_vars) = &env_permissions.allow {
            for env_var in allowed_vars {
                // For now, we're just capturing the variable name, not setting any values
                // This maintains backward compatibility with current behavior
                env_vars.insert(env_var.key.clone(), String::new());
            }
        }
    }
    
    Ok(env_vars)
}

/// Gets storage permissions from the policy file
/// 
/// Returns a vector of (path, access_type) tuples where access_type can be:
/// - "r" for read-only
/// - "w" for write-only
/// - "rw" for read-write
pub fn get_storage_permissions<P: AsRef<Path>>(path: P) -> Result<Vec<(String, String)>> {
    let policy: PolicyDocument = PolicyParser::parse_file(path)?;
    
    let mut storage_perms = Vec::new();
    
    if let Some(storage) = &policy.permissions.storage {
        if let Some(allowed) = &storage.allow {
            for perm in allowed {
                let uri = perm.uri.clone();
                
                // Extract the path from fs:// URI format
                if uri.starts_with("fs://") {
                    let fs_path = uri.trim_start_matches("fs://").to_string();
                    
                    // Determine access mode
                    let mut access = String::new();
                    for access_type in &perm.access {
                        match access_type {
                            policy_mcp::AccessType::Read => {
                                if !access.contains('r') {
                                    access.push('r');
                                }
                            },
                            policy_mcp::AccessType::Write => {
                                if !access.contains('w') {
                                    access.push('w');
                                }
                            },
                        }
                    }
                    
                    storage_perms.push((fs_path, access));
                }
            }
        }
    }
    
    Ok(storage_perms)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::{load_policy, get_storage_permissions};

    #[test]
    fn test_valid_policy() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "version: \"1.0\"").unwrap();
        writeln!(file, "permissions:").unwrap();
        writeln!(file, "  environment:").unwrap();
        writeln!(file, "    allow:").unwrap();
        writeln!(file, "      - key: \"FOO\"").unwrap();
        writeln!(file, "      - key: \"BAZ\"").unwrap();

        let vars = load_policy(file.path()).unwrap();
        assert_eq!(vars.get("FOO"), Some(&"".to_string()));
        assert_eq!(vars.get("BAZ"), Some(&"".to_string()));
    }

    #[test]
    fn test_missing_env_section() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "version: \"1.0\"").unwrap();
        writeln!(file, "permissions: {}").unwrap();
        
        let vars = load_policy(file.path()).unwrap();
        assert!(vars.is_empty());
    }

    #[test]
    #[should_panic]
    fn test_malformed_policy() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "version: 1.0").unwrap(); // Missing quotes
        writeln!(file, "invalid_yaml_format").unwrap();
        load_policy(file.path()).unwrap();
    }
    
    #[test]
    fn test_storage_permissions() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "version: \"1.0\"").unwrap();
        writeln!(file, "permissions:").unwrap();
        writeln!(file, "  storage:").unwrap();
        writeln!(file, "    allow:").unwrap();
        writeln!(file, "      - uri: \"fs://work/agent/**\"").unwrap();
        writeln!(file, "        access: [\"read\", \"write\"]").unwrap();
        writeln!(file, "      - uri: \"fs://configs/readonly.yml\"").unwrap();
        writeln!(file, "        access: [\"read\"]").unwrap();
        
        let perms = get_storage_permissions(file.path()).unwrap();
        assert_eq!(perms.len(), 2);
        
        assert_eq!(perms[0].0, "work/agent/**");
        assert_eq!(perms[0].1, "rw");
        
        assert_eq!(perms[1].0, "configs/readonly.yml");
        assert_eq!(perms[1].1, "r");
    }
}
