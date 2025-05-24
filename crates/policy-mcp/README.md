# A Rust Crate for MCP Capability Policies

A Rust crate for parsing and manipulating MCP (Model Context Protocol) server capability policy files.

## Basic Usage

```rust
use policy_mcp::{PolicyParser, PolicyDocument};

let policy = PolicyParser::parse_file("policy.yaml")?;
println!("Policy version: {}", policy.version);
```

## Policy Structure

Basic YAML format:

```yaml
version: "1.0"
description: "My policy"
permissions:
  storage:
    allow:
    - uri: "fs://work/agent/**"
      access: ["read", "write"]
  network:
    allow:
    - host: "api.example.com"
```

### Storage Permissions

```yaml
storage:
  allow:
    - uri: "fs://work/agent/**"
      access: ["read", "write"]
    - uri: "fs://work/temp/*"
      access: ["read"]
```

### Network Permissions

```yaml
network:
  allow:
    - host: "api.service.com"
    - host: "*.internal.com"
    - cidr: "10.0.0.0/8"
```

### Environment Variables

```yaml
environment:
  allow:
    - key: "PATH"
    - key: "HOME"
```

### Docker Runtime

```yaml
runtime:
  docker:
    security:
      privileged: false
      capabilities:
        drop: ["ALL"]
        add: ["NET_BIND_SERVICE"]
```
