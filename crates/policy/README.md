# Policy Crate

A Rust library for parsing and validating capability-based security policies for Model Context Protocol (MCP) servers.

## Overview

The `policy` crate provides a framework for defining fine-grained security policies for WebAssembly-based MCP tools. It supports capability-based access control with allow/deny lists for storage, network, environment variables, runtime configurations, and resource limits.

This crate is used by [policy-mcp](https://github.com/microsoft/policy-mcp) and other MCP server implementations.

## Installation

```toml
[dependencies]
policy = "0.1.0"
```

## Quick Start

```rust
use policy::PolicyParser;

let policy = PolicyParser::parse_file("policy.yaml")?;
policy.validate()?;
```

## Example Policy

```yaml
version: "1.0"
permissions:
  storage:
    allow:
      - uri: "fs://workspace/**"
        access: ["read", "write"]
  network:
    allow:
      - host: "api.openai.com"
      - cidr: "10.0.0.0/8"
  environment:
    allow:
      - key: "PATH"
  resources:
    limits:
      cpu: "500m"
      memory: "512Mi"
```

## Features

- Storage permissions with URI patterns (`**`, `*` wildcards)
- Network permissions (host patterns, CIDR ranges)
- Environment variable access control
- Kubernetes-style resource limits (CPU, memory)
- Docker runtime configuration
- Comprehensive validation

## API

Parse policies from YAML:

```rust
use policy::PolicyParser;

// From file
let policy = PolicyParser::parse_file("policy.yaml")?;

// From string
let policy = PolicyParser::parse_str(yaml_content)?;

// To YAML
let yaml = PolicyParser::to_yaml(&policy)?;
```

Create policies programmatically:

```rust
use policy::{PolicyDocument, StoragePermission, AccessType};

let mut policy = PolicyDocument::new("1.0", Some("My policy".to_string()));
policy.validate()?;
```

## Examples

See [testdata](./testdata) for complete policy examples.

## Related Projects

- [policy-mcp](https://github.com/microsoft/policy-mcp) - MCP server using this crate
- [Wassette](https://github.com/microsoft/wassette) - Security-oriented MCP runtime

## License

Licensed under the [MIT License](../../LICENSE).

## Support

- [GitHub Issues](https://github.com/microsoft/wassette/issues) | [Discussions](https://github.com/microsoft/wassette/discussions) | [Discord](https://discord.gg/microsoft-open-source)
