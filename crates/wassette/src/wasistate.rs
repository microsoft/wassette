use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};

use policy::{AccessType, PolicyDocument};

/// A struct that presents the arguments passed to `wasmtime_wasi::WasiCtxBuilder::preopened_dir`
#[derive(Clone)]
pub struct PreopenedDir {
    pub host_path: PathBuf,
    pub guest_path: String,
    pub dir_perms: wasmtime_wasi::DirPerms,
    pub file_perms: wasmtime_wasi::FilePerms,
}

/// A struct that presents the network permissions passed to wasmtime_wasi::WasiContextBuilder
#[derive(Default, Clone)]
pub struct NetworkPermissions {
    pub allow_tcp: bool,
    pub allow_udp: bool,
    pub allow_ip_name_lookup: bool,
}

/// A template for the wasi state
/// this includes the wasmtime_wasi, wasmtime_wasi_config and wasmtime_wasi_http states
#[derive(Clone)]
pub struct WasiStateTemplate {
    pub allow_stdout: bool,
    pub allow_stderr: bool,
    pub allow_args: bool,
    pub network_perms: NetworkPermissions,
    pub config_vars: HashMap<String, String>, // wamstime_wasi_config specific state
    pub preopened_dirs: Vec<PreopenedDir>,
    pub allowed_hosts: HashSet<String>, // allowed network hosts for HTTP requests
}

impl Default for WasiStateTemplate {
    fn default() -> Self {
        Self {
            allow_stdout: true,
            allow_stderr: true,
            allow_args: true,
            network_perms: NetworkPermissions::default(),
            config_vars: HashMap::new(),
            preopened_dirs: Vec::new(),
            allowed_hosts: HashSet::new(),
        }
    }
}

/// Maps the policy-mcp capabiltiies to the wasi state template
pub fn create_wasi_state_template_from_policy(
    policy: &PolicyDocument,
    plugin_dir: &Path,
) -> anyhow::Result<WasiStateTemplate> {
    let env_vars = extract_env_vars(policy)?;
    let network_perms = extract_network_perms(policy);
    let preopened_dirs = extract_storage_permissions(policy, plugin_dir)?;
    let allowed_hosts = extract_allowed_hosts(policy);

    Ok(WasiStateTemplate {
        network_perms,
        config_vars: env_vars,
        preopened_dirs,
        allowed_hosts,
        ..Default::default()
    })
}

pub(crate) fn extract_env_vars(policy: &PolicyDocument) -> anyhow::Result<HashMap<String, String>> {
    let mut env_vars = HashMap::new();
    if let Some(env_perms) = &policy.permissions.environment {
        if let Some(env_allow_vec) = &env_perms.allow {
            for env_allow in env_allow_vec {
                if let Ok(value) = env::var(&env_allow.key) {
                    env_vars.insert(env_allow.key.clone(), value);
                }
            }
        }
    }
    Ok(env_vars)
}

pub(crate) fn extract_network_perms(policy: &PolicyDocument) -> NetworkPermissions {
    if let Some(network_perms) = &policy.permissions.network {
        let has_network_perms =
            network_perms.allow.is_some() && !network_perms.allow.as_ref().unwrap().is_empty();
        NetworkPermissions {
            allow_tcp: has_network_perms,
            allow_udp: has_network_perms,
            allow_ip_name_lookup: has_network_perms,
        }
    } else {
        NetworkPermissions::default()
    }
}

/// Extract allowed hosts from the policy document
pub(crate) fn extract_allowed_hosts(policy: &PolicyDocument) -> HashSet<String> {
    let mut allowed_hosts = HashSet::new();

    if let Some(network_perms) = &policy.permissions.network {
        if let Some(allow_list) = &network_perms.allow {
            for allow_entry in allow_list {
                // The policy uses serde_json::Value, so we need to extract the host field
                if let Ok(json_value) = serde_json::to_value(allow_entry) {
                    if let Some(host) = json_value.get("host").and_then(|h| h.as_str()) {
                        allowed_hosts.insert(host.to_string());
                    }
                }
            }
        }
    }

    allowed_hosts
}

pub(crate) fn extract_storage_permissions(
    policy: &PolicyDocument,
    plugin_dir: &Path,
) -> anyhow::Result<Vec<PreopenedDir>> {
    let mut preopened_dirs = Vec::new();
    if let Some(storage) = &policy.permissions.storage {
        if let Some(allow) = &storage.allow {
            for storage_permission in allow {
                if storage_permission.uri.starts_with("fs://") {
                    let uri = storage_permission.uri.strip_prefix("fs://").unwrap();
                    let path = Path::new(uri);
                    let (file_perms, dir_perms) = calculate_permissions(&storage_permission.access);
                    let guest_path = path.to_string_lossy().to_string();
                    let host_path = plugin_dir.join(path);
                    preopened_dirs.push(PreopenedDir {
                        host_path,
                        guest_path,
                        dir_perms,
                        file_perms,
                    });
                }
            }
        }
    }
    Ok(preopened_dirs)
}

pub(crate) fn calculate_permissions(
    access_types: &[AccessType],
) -> (wasmtime_wasi::FilePerms, wasmtime_wasi::DirPerms) {
    let file_perms = access_types
        .iter()
        .fold(wasmtime_wasi::FilePerms::empty(), |acc, access| {
            acc | match access {
                AccessType::Read => wasmtime_wasi::FilePerms::READ,
                AccessType::Write => wasmtime_wasi::FilePerms::WRITE,
            }
        });

    let dir_perms = access_types
        .iter()
        .fold(wasmtime_wasi::DirPerms::empty(), |acc, access| {
            acc | match access {
                AccessType::Read => wasmtime_wasi::DirPerms::READ,
                AccessType::Write => {
                    wasmtime_wasi::DirPerms::READ | wasmtime_wasi::DirPerms::MUTATE
                }
            }
        });

    (file_perms, dir_perms)
}

#[cfg(test)]
mod tests {
    use policy::{AccessType, PolicyParser};
    use proptest::prelude::*;
    use tempfile::TempDir;

    use super::*;

    fn create_zero_permission_policy() -> PolicyDocument {
        let yaml_content = r#"
version: "1.0"
description: "Minimal test policy"
permissions:
"#;
        PolicyParser::parse_str(yaml_content).unwrap()
    }

    fn create_test_policy() -> PolicyDocument {
        let yaml_content = r#"
version: "1.0"
description: "Test policy for wassette"
permissions:
  network:
    allow:
      - host: "api.example.com"
  environment:
    allow:
      - key: "TEST_VAR"
      - key: "NONEXISTENT_VAR"
  storage:
    allow:
      - uri: "fs://test/path"
        access: ["read"]
      - uri: "fs://write/path"
        access: ["write"]
      - uri: "fs://readwrite/path"
        access: ["read", "write"]
      - uri: "http://not-fs"
        access: ["read"]
"#;
        PolicyParser::parse_str(yaml_content).unwrap()
    }

    fn create_policy_with_duplicated_access() -> PolicyDocument {
        let yaml_content = r#"
version: "1.0"
description: "Policy with duplicated access types"
permissions:
  storage:
    allow:
      - uri: "fs://duplicate/path"
        access: ["read", "write", "read", "write"]
"#;
        PolicyParser::parse_str(yaml_content).unwrap()
    }

    fn create_policy_without_permissions() -> PolicyDocument {
        let yaml_content = r#"
version: "1.0"
description: "Policy without permissions node"
permissions:
"#;
        PolicyParser::parse_str(yaml_content).unwrap()
    }

    #[test]
    fn test_calculate_permissions_read_only() {
        let access_types = vec![AccessType::Read];
        let (file_perms, dir_perms) = calculate_permissions(&access_types);

        assert_eq!(file_perms, wasmtime_wasi::FilePerms::READ);
        assert_eq!(dir_perms, wasmtime_wasi::DirPerms::READ);
    }

    #[test]
    fn test_calculate_permissions_write_only() {
        let access_types = vec![AccessType::Write];
        let (file_perms, dir_perms) = calculate_permissions(&access_types);

        assert_eq!(file_perms, wasmtime_wasi::FilePerms::WRITE);
        assert_eq!(
            dir_perms,
            wasmtime_wasi::DirPerms::READ | wasmtime_wasi::DirPerms::MUTATE
        );
    }

    #[test]
    fn test_calculate_permissions_read_write() {
        let access_types = vec![AccessType::Read, AccessType::Write];
        let (file_perms, dir_perms) = calculate_permissions(&access_types);

        assert_eq!(
            file_perms,
            wasmtime_wasi::FilePerms::READ | wasmtime_wasi::FilePerms::WRITE
        );
        assert_eq!(
            dir_perms,
            wasmtime_wasi::DirPerms::READ | wasmtime_wasi::DirPerms::MUTATE
        );
    }

    #[test]
    fn test_calculate_permissions_empty() {
        let access_types = vec![];
        let (file_perms, dir_perms) = calculate_permissions(&access_types);

        assert_eq!(file_perms, wasmtime_wasi::FilePerms::empty());
        assert_eq!(dir_perms, wasmtime_wasi::DirPerms::empty());
    }

    #[test]
    fn test_calculate_permissions_duplicated_access() {
        let access_types = vec![
            AccessType::Read,
            AccessType::Write,
            AccessType::Read,
            AccessType::Write,
        ];
        let (file_perms, dir_perms) = calculate_permissions(&access_types);

        assert_eq!(
            file_perms,
            wasmtime_wasi::FilePerms::READ | wasmtime_wasi::FilePerms::WRITE
        );
        assert_eq!(
            dir_perms,
            wasmtime_wasi::DirPerms::READ | wasmtime_wasi::DirPerms::MUTATE
        );
    }

    #[test]
    fn test_extract_environment_variables_with_isolation() {
        let policy = create_test_policy();

        temp_env::with_vars(vec![("TEST_VAR", Some("isolated_value"))], || {
            let env_vars = extract_env_vars(&policy).unwrap();
            assert_eq!(
                env_vars.get("TEST_VAR"),
                Some(&"isolated_value".to_string())
            );
            assert!(!env_vars.contains_key("NONEXISTENT_VAR"));
        });
    }

    #[test]
    fn test_extract_environment_variables_missing_env() {
        let policy = create_test_policy();

        temp_env::with_vars(vec![("TEST_VAR", None::<&str>)], || {
            let env_vars = extract_env_vars(&policy).unwrap();
            assert!(!env_vars.contains_key("TEST_VAR"));
        });
    }

    #[test]
    fn test_extract_environment_variables_no_permissions() {
        let policy = create_zero_permission_policy();
        let env_vars = extract_env_vars(&policy).unwrap();
        assert!(env_vars.is_empty());
    }

    #[test]
    fn test_extract_environment_variables_empty_allow_list() {
        let yaml_content = r#"
version: "1.0"
description: "Policy with empty environment allow list"
permissions:
  environment:
    allow:
"#;
        let policy = PolicyParser::parse_str(yaml_content).unwrap();
        let env_vars = extract_env_vars(&policy).unwrap();
        assert!(env_vars.is_empty());
    }

    #[test]
    fn test_extract_network_permissions_with_allow() {
        let policy = create_test_policy();
        let network_perms = extract_network_perms(&policy);

        assert!(network_perms.allow_tcp);
        assert!(network_perms.allow_udp);
        assert!(network_perms.allow_ip_name_lookup);
    }

    #[test]
    fn test_extract_network_permissions_no_permissions() {
        let policy = create_zero_permission_policy();
        let network_perms = extract_network_perms(&policy);

        assert!(!network_perms.allow_tcp);
        assert!(!network_perms.allow_udp);
        assert!(!network_perms.allow_ip_name_lookup);
    }

    #[test]
    fn test_extract_network_permissions_empty_allow_list() {
        let yaml_content = r#"
version: "1.0"
description: "Policy with empty network allow list"
permissions:
  network:
    allow: []
"#;
        let policy = PolicyParser::parse_str(yaml_content).unwrap();
        let network_perms = extract_network_perms(&policy);

        assert!(!network_perms.allow_tcp);
        assert!(!network_perms.allow_udp);
        assert!(!network_perms.allow_ip_name_lookup);
    }

    #[test]
    fn test_extract_storage_permissions() {
        let temp_dir = TempDir::new().unwrap();
        let plugin_dir = temp_dir.path();

        let policy = create_test_policy();
        let preopened_dirs = extract_storage_permissions(&policy, plugin_dir).unwrap();

        assert_eq!(preopened_dirs.len(), 3);

        let read_only = &preopened_dirs[0];
        assert_eq!(read_only.guest_path, "test/path");
        assert_eq!(read_only.host_path, plugin_dir.join("test/path"));
        assert_eq!(read_only.file_perms, wasmtime_wasi::FilePerms::READ);
        assert_eq!(read_only.dir_perms, wasmtime_wasi::DirPerms::READ);

        let write_only = &preopened_dirs[1];
        assert_eq!(write_only.guest_path, "write/path");
        assert_eq!(write_only.file_perms, wasmtime_wasi::FilePerms::WRITE);
        assert_eq!(
            write_only.dir_perms,
            wasmtime_wasi::DirPerms::READ | wasmtime_wasi::DirPerms::MUTATE
        );

        let read_write = &preopened_dirs[2];
        assert_eq!(read_write.guest_path, "readwrite/path");
        assert_eq!(
            read_write.file_perms,
            wasmtime_wasi::FilePerms::READ | wasmtime_wasi::FilePerms::WRITE
        );
        assert_eq!(
            read_write.dir_perms,
            wasmtime_wasi::DirPerms::READ | wasmtime_wasi::DirPerms::MUTATE
        );
    }

    #[test]
    fn test_extract_storage_permissions_skips_non_fs_uri() {
        let temp_dir = TempDir::new().unwrap();
        let plugin_dir = temp_dir.path();

        let policy = create_test_policy();
        let preopened_dirs = extract_storage_permissions(&policy, plugin_dir).unwrap();

        for dir in &preopened_dirs {
            assert!(
                dir.guest_path.starts_with("test/")
                    || dir.guest_path.starts_with("write/")
                    || dir.guest_path.starts_with("readwrite/")
            );
        }
        assert_eq!(preopened_dirs.len(), 3);
    }

    #[test]
    fn test_extract_storage_permissions_no_permissions() {
        let temp_dir = TempDir::new().unwrap();
        let plugin_dir = temp_dir.path();

        let policy = create_zero_permission_policy();
        let preopened_dirs = extract_storage_permissions(&policy, plugin_dir).unwrap();

        assert!(preopened_dirs.is_empty());
    }

    #[test]
    fn test_extract_storage_permissions_empty_allow_list() {
        let temp_dir = TempDir::new().unwrap();
        let plugin_dir = temp_dir.path();

        let yaml_content = r#"
version: "1.0"
description: "Policy with empty storage allow list"
permissions:
  storage:
    allow: []
"#;
        let policy = PolicyParser::parse_str(yaml_content).unwrap();
        let preopened_dirs = extract_storage_permissions(&policy, plugin_dir).unwrap();

        assert!(preopened_dirs.is_empty());
    }

    #[test]
    fn test_extract_storage_permissions_duplicated_access_has_no_effect() {
        let temp_dir = TempDir::new().unwrap();
        let plugin_dir = temp_dir.path();

        let policy = create_policy_with_duplicated_access();
        let preopened_dirs = extract_storage_permissions(&policy, plugin_dir).unwrap();

        assert_eq!(preopened_dirs.len(), 1);
        let dir = &preopened_dirs[0];
        assert_eq!(
            dir.file_perms,
            wasmtime_wasi::FilePerms::READ | wasmtime_wasi::FilePerms::WRITE
        );
        assert_eq!(
            dir.dir_perms,
            wasmtime_wasi::DirPerms::READ | wasmtime_wasi::DirPerms::MUTATE
        );
    }

    #[test]
    fn test_create_wasi_state_template_from_policy() {
        let temp_dir = TempDir::new().unwrap();
        let plugin_dir = temp_dir.path();
        let policy = create_test_policy();

        let template = create_wasi_state_template_from_policy(&policy, plugin_dir).unwrap();

        assert!(template.network_perms.allow_tcp);
        assert!(template.network_perms.allow_udp);
        assert!(template.network_perms.allow_ip_name_lookup);
        assert_eq!(template.preopened_dirs.len(), 3);
    }

    #[test]
    fn test_create_wasi_state_template_from_policy_no_permissions() {
        let temp_dir = TempDir::new().unwrap();
        let plugin_dir = temp_dir.path();
        let policy = create_policy_without_permissions();

        let template = create_wasi_state_template_from_policy(&policy, plugin_dir).unwrap();

        assert!(!template.network_perms.allow_tcp);
        assert!(!template.network_perms.allow_udp);
        assert!(!template.network_perms.allow_ip_name_lookup);
        assert!(template.config_vars.is_empty());
        assert!(template.preopened_dirs.is_empty());
        assert!(template.allow_stdout);
        assert!(template.allow_stderr);
        assert!(template.allow_args);
    }

    proptest! {
        #[test]
        fn test_calculate_permissions_union_property(
            access_types in prop::collection::vec(
                prop::strategy::Union::new([
                    Just(AccessType::Read),
                    Just(AccessType::Write),
                ]),
                0..10
            )
        ) {
            let (file_perms, dir_perms) = calculate_permissions(&access_types);

            let has_read = access_types.contains(&AccessType::Read);
            let has_write = access_types.contains(&AccessType::Write);

            if has_read && has_write {
                prop_assert_eq!(
                    file_perms,
                    wasmtime_wasi::FilePerms::READ | wasmtime_wasi::FilePerms::WRITE
                );
                prop_assert_eq!(
                    dir_perms,
                    wasmtime_wasi::DirPerms::READ | wasmtime_wasi::DirPerms::MUTATE
                );
            } else if has_read {
                prop_assert_eq!(file_perms, wasmtime_wasi::FilePerms::READ);
                prop_assert_eq!(dir_perms, wasmtime_wasi::DirPerms::READ);
            } else if has_write {
                prop_assert_eq!(file_perms, wasmtime_wasi::FilePerms::WRITE);
                prop_assert_eq!(
                    dir_perms,
                    wasmtime_wasi::DirPerms::READ | wasmtime_wasi::DirPerms::MUTATE
                );
            } else {
                prop_assert_eq!(file_perms, wasmtime_wasi::FilePerms::empty());
                prop_assert_eq!(dir_perms, wasmtime_wasi::DirPerms::empty());
            }
        }

        #[test]
        fn test_calculate_permissions_idempotence(
            access_types in prop::collection::vec(
                prop::strategy::Union::new([
                    Just(AccessType::Read),
                    Just(AccessType::Write),
                ]),
                0..10
            )
        ) {
            let (file_perms1, dir_perms1) = calculate_permissions(&access_types);
            let (file_perms2, dir_perms2) = calculate_permissions(&access_types);

            prop_assert_eq!(file_perms1, file_perms2);
            prop_assert_eq!(dir_perms1, dir_perms2);

            let mut doubled_access = access_types.clone();
            doubled_access.extend(access_types);
            let (file_perms3, dir_perms3) = calculate_permissions(&doubled_access);

            prop_assert_eq!(file_perms1, file_perms3);
            prop_assert_eq!(dir_perms1, dir_perms3);
        }

        #[test]
        fn test_calculate_permissions_commutativity(
            mut access_types in prop::collection::vec(
                prop::strategy::Union::new([
                    Just(AccessType::Read),
                    Just(AccessType::Write),
                ]),
                0..10
            )
        ) {
            let (file_perms1, dir_perms1) = calculate_permissions(&access_types);

            access_types.reverse();
            let (file_perms2, dir_perms2) = calculate_permissions(&access_types);

            prop_assert_eq!(file_perms1, file_perms2);
            prop_assert_eq!(dir_perms1, dir_perms2);
        }
    }
}
