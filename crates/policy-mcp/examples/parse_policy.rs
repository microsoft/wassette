//! Example demonstrating usage of the MCP policy parser library

use policy_mcp::{
    AccessType, CapabilityAction, DockerCapabilities, DockerRuntime, DockerSecurity,
    EnvironmentPermission, EnvironmentPermissions, NetworkCidrPermission, NetworkHostPermission,
    NetworkPermission, PermissionList, Permissions, PolicyDocument, PolicyParser, Runtime,
    StoragePermission,
};

fn main() -> anyhow::Result<()> {
    // println!("Starting policy parsing example");
    example_parse_docker_yaml()?;
    example_create_policy_programmatically()?;
    Ok(())
}

fn example_parse_docker_yaml() -> anyhow::Result<()> {
    let policy = PolicyParser::parse_file("testdata/docker.yaml")?;

    println!("   Version: {}", policy.version);
    println!("   Description: {}", policy.description.unwrap_or_default());

    if let Some(storage) = &policy.permissions.storage {
        if let Some(allow_list) = &storage.allow {
            println!("   Storage permissions ({} allowed):", allow_list.len());
            for perm in allow_list {
                println!("     - URI: {}", perm.uri);
                println!("       Access: {:?}", perm.access);
            }
        }
    }

    if let Some(network) = &policy.permissions.network {
        if let Some(allow_list) = &network.allow {
            println!("   Network permissions ({} allowed):", allow_list.len());
            for perm in allow_list {
                match perm {
                    NetworkPermission::Host(host) => {
                        println!("     - Host: {}", host.host);
                    }
                    NetworkPermission::Cidr(cidr) => {
                        println!("     - CIDR: {}", cidr.cidr);
                    }
                }
            }
        }
    }

    if let Some(runtime) = &policy.permissions.runtime {
        if let Some(docker) = &runtime.docker {
            if let Some(security) = &docker.security {
                println!("   Docker Security:");
                println!("     - Privileged: {:?}", security.privileged);
                println!("     - No new privileges: {:?}", security.no_new_privileges);

                if let Some(caps) = &security.capabilities {
                    if let Some(drop) = &caps.drop {
                        println!("     - Drop capabilities: {:?}", drop);
                    }
                    if let Some(add) = &caps.add {
                        println!("     - Add capabilities: {:?}", add);
                    }
                }
            }
        }
    }

    println!();
    Ok(())
}

fn example_create_policy_programmatically() -> anyhow::Result<()> {
    // Could probably simplify this but it works
    let storage_permissions = PermissionList {
        allow: Some(vec![
            StoragePermission {
                uri: "fs://app/data/**".to_string(),
                access: vec![AccessType::Read, AccessType::Write],
            },
            StoragePermission {
                uri: "fs://app/config.json".to_string(),
                access: vec![AccessType::Read],
            },
        ]),
        deny: Some(vec![StoragePermission {
            uri: "fs://system/**".to_string(),
            access: vec![AccessType::Read, AccessType::Write],
        }]),
    };

    let network_permissions = PermissionList {
        allow: Some(vec![
            NetworkPermission::Host(NetworkHostPermission {
                host: "api.example.com".to_string(),
            }),
            NetworkPermission::Cidr(NetworkCidrPermission {
                cidr: "192.168.1.0/24".to_string(),
            }),
        ]),
        deny: None,
    };

    let env_permissions = EnvironmentPermissions {
        allow: Some(vec![
            EnvironmentPermission {
                key: "APP_CONFIG".to_string(),
            },
            EnvironmentPermission {
                key: "LOG_LEVEL".to_string(),
            },
        ]),
    };

    let docker_runtime = DockerRuntime {
        security: Some(DockerSecurity {
            privileged: Some(false),
            no_new_privileges: Some(true),
            capabilities: Some(DockerCapabilities {
                drop: Some(vec![CapabilityAction::All]),
                add: Some(vec![CapabilityAction::NetBindService]),
            }),
        }),
    };

    let permissions = Permissions {
        storage: Some(storage_permissions),
        network: Some(network_permissions),
        environment: Some(env_permissions),
        runtime: Some(Runtime {
            docker: Some(docker_runtime),
            hyperlight: None, // Maybe add this later
        }),
        resources: None,
        ipc: None,
    };

    let policy = PolicyDocument {
        version: "1.0".to_string(),
        description: Some("Programmatically created policy".to_string()),
        permissions,
    };

    policy.validate()?;

    let yaml = PolicyParser::to_yaml(&policy)?;
    println!("   Generated YAML:");
    // Just show first few lines - full output is too much
    for line in yaml.lines().take(10) {
        println!("     {}", line);
    }
    println!("     ... (truncated)\n");

    // TODO: Maybe write this to a file for testing?
    // PolicyParser::write_file(&policy, "generated_policy.yaml")?;

    Ok(())
}
