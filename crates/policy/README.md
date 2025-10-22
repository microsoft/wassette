# Policy Crate

A Rust library for parsing, validating, and managing capability-based security policies for Model Context Protocol (MCP) servers.

## Overview

The `policy` crate provides a robust framework for defining and enforcing fine-grained security policies for WebAssembly-based MCP tools. It supports capability-based access control with allow/deny lists for storage, network, environment variables, runtime configurations, and resource limits.

This crate is designed to be used by projects like [policy-mcp](https://github.com/microsoft/policy-mcp) and other MCP server implementations that need to enforce security boundaries.

## Features

- **Storage Permissions**: File system access control with URI patterns supporting wildcards
- **Network Permissions**: Host and CIDR-based network access control
- **Environment Permissions**: Environment variable access control
- **Runtime Configuration**: Support for Docker and other runtime-specific settings
- **Resource Limits**: Kubernetes-style resource limits (CPU, memory)
- **Policy Validation**: Comprehensive validation of policy documents
- **YAML Parsing**: Parse and serialize policy documents from/to YAML

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
policy = "0.1.0"
```

## Quick Start

```rust
use policy::{PolicyParser, PolicyDocument};

// Parse a policy from a YAML file
let policy = PolicyParser::parse_file("policy.yaml")?;

// Validate the policy
policy.validate()?;

// Access permissions
if let Some(storage) = &policy.permissions.storage {
    if let Some(allow) = &storage.allow {
        for perm in allow {
            println!("Allowed storage URI: {}", perm.uri);
        }
    }
}
```

## Policy Format

Policies are defined in YAML format. Here's a complete example:

```yaml
version: "1.0"
description: "Example policy for an MCP tool"

permissions:
  storage:
    allow:
      - uri: "fs://workspace/**"
        access: ["read", "write"]
      - uri: "fs://config/*.yaml"
        access: ["read"]
    deny:
      - uri: "fs://secrets/**"
        access: ["read", "write"]

  network:
    allow:
      - host: "api.openai.com"
      - host: "*.example.com"
      - cidr: "10.0.0.0/8"
    deny:
      - host: "*.malicious.com"

  environment:
    allow:
      - key: "PATH"
      - key: "HOME"
      - key: "API_KEY"

  resources:
    limits:
      cpu: "500m"      # 500 millicores
      memory: "512Mi"   # 512 mebibytes

  runtime:
    docker:
      security:
        privileged: false
        no_new_privileges: true
        capabilities:
          drop: ["ALL"]
          add: ["NET_BIND_SERVICE"]
```

## Permission Types

### Storage Permissions

Control file system access with URI patterns:

```rust
use policy::{StoragePermission, AccessType};

let perm = StoragePermission {
    uri: "fs://workspace/**".to_string(),
    access: vec![AccessType::Read, AccessType::Write],
};
```

Supported wildcard patterns:
- `**` - Matches any number of directories (must be its own segment)
- `*` - Matches any single directory or filename

### Network Permissions

Control network access by host or CIDR:

```rust
use policy::{NetworkPermission, NetworkHostPermission, NetworkCidrPermission};

// Host-based permission
let host_perm = NetworkPermission::Host(NetworkHostPermission {
    host: "*.example.com".to_string(),
});

// CIDR-based permission
let cidr_perm = NetworkPermission::Cidr(NetworkCidrPermission {
    cidr: "10.0.0.0/8".to_string(),
});
```

### Resource Limits

Define resource constraints using Kubernetes-style values:

```rust
use policy::{ResourceLimits, ResourceLimitValues, CpuLimit, MemoryLimit};

let resources = ResourceLimits {
    limits: Some(ResourceLimitValues::new(
        Some(CpuLimit::String("500m".to_string())),  // 0.5 cores
        Some(MemoryLimit::String("512Mi".to_string())), // 512 MiB
    )),
    cpu: None,
    memory: None,
    io: None,
};
```

Supported formats:
- **CPU**: Millicores (`"500m"`) or cores (`"1"`, `"2.5"`)
- **Memory**: Ki, Mi, Gi, Ti suffixes (`"512Mi"`, `"1Gi"`)

## API Documentation

### PolicyParser

Primary interface for parsing and serializing policies:

```rust
use policy::PolicyParser;

// Parse from string
let policy = PolicyParser::parse_str(yaml_content)?;

// Parse from file
let policy = PolicyParser::parse_file("policy.yaml")?;

// Parse from bytes
let policy = PolicyParser::parse_bytes(yaml_bytes)?;

// Serialize to YAML
let yaml = PolicyParser::to_yaml(&policy)?;

// Write to file
PolicyParser::write_file(&policy, "output.yaml")?;
```

### PolicyDocument

Main policy structure:

```rust
use policy::{PolicyDocument, Permissions};

// Create a new policy
let policy = PolicyDocument::new("1.0", Some("My policy".to_string()));

// Validate the policy
policy.validate()?;

// Access permissions
let perms = &policy.permissions;
```

## Validation

The crate provides comprehensive validation for all policy components:

```rust
use policy::PolicyDocument;

let policy = PolicyDocument {
    version: "1.0".to_string(),
    description: Some("Test policy".to_string()),
    permissions: Permissions::default(),
};

// Validates version, permissions, and all nested structures
match policy.validate() {
    Ok(_) => println!("Policy is valid"),
    Err(e) => eprintln!("Validation error: {}", e),
}
```

Validation checks:
- Version compatibility (only v1.x supported)
- URI pattern syntax (no `***`, proper `**` usage)
- Network host patterns (wildcard placement)
- Environment key format (no wildcards)
- CIDR notation (must contain `/`)
- Resource limit values (non-negative, proper units)

## Examples

See the [testdata](./testdata) directory for comprehensive examples:

- [`minimal.yaml`](./testdata/minimal.yaml) - Minimal valid policy
- [`docker.yaml`](./testdata/docker.yaml) - Docker runtime configuration
- [`comprehensive.yaml`](./testdata/comprehensive.yaml) - All permission types
- [`resource-limits.yaml`](./testdata/resource-limits.yaml) - K8s-style resource limits

## Related Projects

- [policy-mcp](https://github.com/microsoft/policy-mcp) - MCP server implementation using this crate
- [Wassette](https://github.com/microsoft/wassette) - Security-oriented MCP server runtime

## Contributing

Contributions are welcome! Please see the main [Wassette contributing guide](https://github.com/microsoft/wassette/blob/main/CONTRIBUTING.md).

## License

This project is licensed under the [MIT License](../../LICENSE).

## Support

For issues and questions:
- [GitHub Issues](https://github.com/microsoft/wassette/issues)
- [GitHub Discussions](https://github.com/microsoft/wassette/discussions)
- [Discord](https://discord.gg/microsoft-open-source) - Join the `#wassette` channel
