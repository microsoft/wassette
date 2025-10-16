# Configuration Files Reference

This page provides a comprehensive reference for all configuration files used in Wassette. These files control various aspects of the server's behavior, component management, permissions, and deployment options.

## Overview

Wassette uses several configuration files for different purposes:

| File | Purpose | Location | Format |
|------|---------|----------|--------|
| [`config.toml`](#configtoml) | Main server configuration | `~/.config/wassette/` | TOML |
| [`policy.yaml`](#policyyaml) | Component permissions | Component directory | YAML |
| [`component-registry.json`](#component-registryjson) | Pre-configured components | Repository root | JSON |
| [`docker-compose.yml`](#docker-composeyml) | Docker deployment | User-defined | YAML |

## config.toml

The main configuration file for the Wassette MCP server. This file is optional and provides defaults for server behavior.

### Location

- **Linux/macOS**: `$XDG_CONFIG_HOME/wassette/config.toml` (typically `~/.config/wassette/config.toml`)
- **Windows**: `%APPDATA%\wassette\config.toml`
- **Custom**: Set via `WASSETTE_CONFIG_FILE` environment variable

### Configuration Priority

Configuration values are merged with the following precedence (highest to lowest):

1. Command-line options (e.g., `--plugin-dir`)
2. Environment variables prefixed with `WASSETTE_`
3. Configuration file (`config.toml`)

### Schema

```toml
# Directory where WebAssembly components are stored
# Default: $XDG_DATA_HOME/wassette/components (~/.local/share/wassette/components)
plugin_dir = "/path/to/components"

# Directory where secrets are stored (API keys, credentials, etc.)
# Default: $XDG_CONFIG_HOME/wassette/secrets (~/.config/wassette/secrets)
secrets_dir = "/path/to/secrets"

# Environment variables to be made available to components
# These are global defaults and can be overridden per-component in policy files
[environment_vars]
API_KEY = "your_api_key"
LOG_LEVEL = "info"
DATABASE_URL = "postgresql://localhost/mydb"
```

### Fields

#### `plugin_dir`

- **Type**: String (path)
- **Default**: Platform-specific data directory
- **Description**: Directory where loaded WebAssembly components are stored. Components loaded via `wassette component load` or the MCP interface are saved here.

#### `secrets_dir`

- **Type**: String (path)
- **Default**: Platform-specific config directory
- **Description**: Directory for storing sensitive data like API keys and credentials. This directory should have restricted permissions (e.g., `chmod 600`).

#### `environment_vars`

- **Type**: Table/Map
- **Default**: Empty
- **Description**: Key-value pairs of environment variables to make available to components. Note that components must explicitly request access to environment variables via their policy files.

### Example Configurations

**Minimal Configuration:**
```toml
# Use all defaults
```

**Development Configuration:**
```toml
plugin_dir = "./dev-components"
secrets_dir = "./dev-secrets"

[environment_vars]
LOG_LEVEL = "debug"
RUST_LOG = "trace"
```

**Production Configuration:**
```toml
plugin_dir = "/opt/wassette/components"
secrets_dir = "/opt/wassette/secrets"

[environment_vars]
LOG_LEVEL = "info"
NODE_ENV = "production"
```

### Environment Variables

You can override any configuration value using environment variables with the `WASSETTE_` prefix:

```bash
# Override plugin directory
export WASSETTE_PLUGIN_DIR=/custom/components

# Override config file location
export WASSETTE_CONFIG_FILE=/etc/wassette/config.toml

# Start server
wassette serve --stdio
```

## policy.yaml

Permission policy files control what resources each WebAssembly component can access. Each component should have its own `policy.yaml` file that defines its security boundaries.

### Location

Policy files are stored alongside components in the plugin directory, typically:
- `~/.local/share/wassette/components/<component-id>/policy.yaml`

### Schema Version

Wassette uses the Policy MCP schema for policy files. The current version is `1.0`.

```yaml
$schema: https://raw.githubusercontent.com/microsoft/policy-mcp/main/schema/policy-v1.0.schema.json
version: "1.0"
description: "Human-readable description of the policy"
permissions:
  # Permission definitions here
```

### Permission Types

#### Storage Permissions

Control file system access for reading and writing files.

```yaml
permissions:
  storage:
    allow:
      - uri: "fs://workspace/**"
        access: ["read", "write"]
      - uri: "fs://config/app.yaml"
        access: ["read"]
    deny:
      - uri: "fs://system/**"
        access: ["read", "write"]
```

**URI Patterns:**
- `fs://path/to/file` - Exact file path
- `fs://path/**` - Recursive directory access
- `fs://path/*` - Single-level directory access

**Access Types:**
- `read` - Read files
- `write` - Write/create/delete files

#### Network Permissions

Control outbound network access to specific hosts and IP ranges.

```yaml
permissions:
  network:
    allow:
      - host: "api.openai.com"
      - host: "*.internal.company.com"
      - cidr: "10.0.0.0/8"
      - cidr: "172.16.0.0/12"
    deny:
      - host: "*.malicious.com"
      - cidr: "0.0.0.0/0"
```

**Host Patterns:**
- `api.example.com` - Exact domain
- `*.example.com` - Wildcard subdomain
- `localhost:8080` - Localhost with port

**CIDR Blocks:**
- Standard CIDR notation (e.g., `10.0.0.0/8`)

#### Environment Variable Permissions

Control access to environment variables.

```yaml
permissions:
  environment:
    allow:
      - key: "PATH"
      - key: "HOME"
      - key: "API_KEY"
      - key: "DATABASE_URL"
```

**Note:** Components can only access environment variables that are:
1. Defined in the policy's `allow` list
2. Available in the environment (either from system, `config.toml`, or component-specific configuration)

#### Resource Limits

Set resource limits for components.

```yaml
permissions:
  resources:
    limits:
      cpu: "50"        # CPU cores (can be fractional, e.g., "0.5")
      memory: "1Gi"    # Memory limit (supports Ki, Mi, Gi units)
```

**Memory Units:**
- `Ki` - Kibibytes (1024 bytes)
- `Mi` - Mebibytes (1024 KiB)
- `Gi` - Gibibytes (1024 MiB)

#### IPC Permissions

Control inter-process communication access.

```yaml
permissions:
  ipc:
    allow:
      - uri: "pipe://app-service"
      - uri: "socket://unix:/tmp/app.sock"
    deny:
      - uri: "pipe://system-service"
```

#### Runtime Permissions

Configure runtime-specific security settings (Docker, etc.).

```yaml
permissions:
  runtime:
    docker:
      security:
        privileged: false
        no_new_privileges: true
        capabilities:
          drop: ["ALL"]
          add: ["NET_BIND_SERVICE"]
```

### Example Policy Files

**Minimal (no permissions):**
```yaml
$schema: https://raw.githubusercontent.com/microsoft/policy-mcp/main/schema/policy-v1.0.schema.json
version: "1.0"
description: "Minimal valid policy"
permissions: {}
```

**Web Service:**
```yaml
$schema: https://raw.githubusercontent.com/microsoft/policy-mcp/main/schema/policy-v1.0.schema.json
version: "1.0"
description: "Web service with API access"
permissions:
  storage:
    allow:
      - uri: "fs://app/**"
        access: ["read"]
      - uri: "fs://logs/**"
        access: ["write"]
  
  network:
    allow:
      - host: "api.stripe.com"
      - host: "*.amazonaws.com"
  
  environment:
    allow:
      - key: "PORT"
      - key: "DATABASE_URL"
      - key: "STRIPE_API_KEY"
  
  resources:
    limits:
      cpu: "2"
      memory: "512Mi"
```

**Weather API Component:**
```yaml
$schema: https://raw.githubusercontent.com/microsoft/policy-mcp/main/schema/policy-v1.0.schema.json
version: "1.0"
description: "Weather API component"
permissions:
  network:
    allow:
      - host: "api.openweathermap.org"
  environment:
    allow:
      - key: "OPENWEATHER_API_KEY"
```

### Best Practices

1. **Principle of Least Privilege**: Only grant permissions that are absolutely necessary
2. **Use Deny Rules**: Explicitly deny access to sensitive resources
3. **Document Permissions**: Use descriptive `description` fields
4. **Test Policies**: Verify that components work with the defined permissions
5. **Version Control**: Keep policy files in version control alongside components

## component-registry.json

A registry file that lists pre-configured components available for easy loading. This file is primarily used for examples and quick setup.

### Location

- Repository root: `/home/runner/work/wassette/wassette/component-registry.json`
- Can be placed anywhere and referenced via CLI

### Schema

```json
[
  {
    "name": "Human-readable component name",
    "description": "Brief description of what the component does",
    "uri": "oci://registry/path:tag or file:///path/to/component.wasm"
  }
]
```

### Fields

#### `name`

- **Type**: String
- **Required**: Yes
- **Description**: Human-readable name for the component

#### `description`

- **Type**: String
- **Required**: Yes
- **Description**: Brief description of the component's functionality

#### `uri`

- **Type**: String (URI)
- **Required**: Yes
- **Description**: Location of the component. Supports:
  - `oci://` - OCI registry (e.g., `oci://ghcr.io/microsoft/component:latest`)
  - `file://` - Local file path (e.g., `file:///path/to/component.wasm`)

### Example

```json
[
  {
    "name": "Weather Server",
    "description": "A weather component written in JavaScript",
    "uri": "oci://ghcr.io/microsoft/get-weather-js:latest"
  },
  {
    "name": "Time Server",
    "description": "A time server component written in JavaScript",
    "uri": "oci://ghcr.io/microsoft/time-server-js:latest"
  },
  {
    "name": "Fetch",
    "description": "A fetch component written in Rust",
    "uri": "oci://ghcr.io/microsoft/fetch-rs:latest"
  }
]
```

### Usage

The component registry is used for:
- **Documentation**: Listing available example components
- **Quick Setup**: Easily loading pre-configured components
- **Testing**: Reference components for development and testing

You can load components from the registry using the CLI or MCP interface.

## docker-compose.yml

Docker Compose configuration for deploying Wassette in containerized environments. This file is optional and used for container orchestration.

### Location

User-defined, typically in the project root or deployment directory.

### Example File

See `docker-compose.example.yml` in the repository root for a complete example.

### Key Sections

#### Service Definition

```yaml
services:
  wassette:
    build: .
    image: wassette:latest
    
    # Expose ports
    ports:
      - "9001:9001"
```

#### Volume Mounts

```yaml
    volumes:
      # Component directory (read-only)
      - ./components:/home/wassette/.local/share/wassette/components:ro
      
      # Secrets directory (read-only)
      - ./secrets:/home/wassette/.config/wassette/secrets:ro
      
      # Optional: Custom configuration
      - ./config.toml:/home/wassette/.config/wassette/config.toml:ro
```

#### Environment Variables

```yaml
    environment:
      # Logging level
      - RUST_LOG=info
      
      # Component environment variables
      - OPENWEATHER_API_KEY=your_key_here
```

#### Transport Configuration

```yaml
    # Default: streamable-http (uses port 9001)
    # Override with:
    # command: ["wassette", "serve", "--stdio"]
    # command: ["wassette", "serve", "--sse"]
```

#### Security Configuration

```yaml
    # Resource limits
    deploy:
      resources:
        limits:
          cpus: '1.0'
          memory: 512M
    
    # Drop capabilities
    cap_drop:
      - ALL
    
    # Prevent privilege escalation
    security_opt:
      - no-new-privileges:true
```

#### Health Checks

```yaml
    healthcheck:
      test: ["CMD-SHELL", "curl -f http://localhost:9001/health || exit 1"]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 40s
```

### Best Practices

1. **Use Read-Only Mounts**: Mount volumes as `:ro` when possible
2. **Set Resource Limits**: Prevent resource exhaustion
3. **Drop Capabilities**: Use `cap_drop: [ALL]` for security
4. **Health Checks**: Monitor server availability
5. **Secrets Management**: Never commit secrets to version control

## Other Configuration Files

### Cargo.toml

Rust package manifest for building Wassette from source.

**Location**: Repository root

**Purpose**: Defines dependencies, build configuration, and workspace members for the Rust project.

**Key sections:**
- `[package]`: Project metadata
- `[dependencies]`: Runtime dependencies
- `[workspace]`: Multi-crate workspace configuration
- `[profile.release]`: Release build optimizations

### rust-toolchain.toml

Rust toolchain specification.

**Location**: Repository root

**Purpose**: Specifies the Rust version and components required to build Wassette.

**Content:**
```toml
[toolchain]
channel = "1.90"
components = ["rustfmt", "clippy"]
targets = ["wasm32-wasip2", "wasm32-wasip1"]
```

### rustfmt.toml

Rust code formatting configuration.

**Location**: Repository root

**Purpose**: Configures code formatting rules for the project.

**Key settings:**
- `unstable_features = true`: Enables nightly-only formatting features
- `group_imports = "StdExternalCrate"`: Groups imports by category
- `imports_granularity = "Module"`: Merges imports from the same module

**Note**: Requires `cargo +nightly fmt` due to unstable features.

### _typos.toml

Spell checking configuration for the `typos` tool.

**Location**: Repository root

**Purpose**: Configures spell checking, excluding specific patterns and defining allowed words.

### audit.toml

Cargo audit configuration.

**Location**: Repository root

**Purpose**: Configures security advisory checking with `cargo-audit`, including a list of ignored advisories with justification.

### deny.toml

Cargo deny configuration.

**Location**: Repository root

**Purpose**: Configures dependency licensing, security advisories, and ban policies.

**Key sections:**
- `[advisories]`: Security advisory ignores
- `[licenses]`: Allowed open source licenses
- `[bans]`: Dependency ban policies
- `[sources]`: Allowed package sources

### book.toml

mdBook configuration.

**Location**: `docs/book.toml`

**Purpose**: Configures the documentation build process.

**Key settings:**
- Output directory: `docs/book`
- Preprocessors: Mermaid diagrams, tabs
- Search configuration
- Theme customization

## See Also

- [CLI Reference](cli.md) - Command-line usage and options
- [Permissions Guide](permissions.md) - Working with permissions
- [Docker Deployment](../deployment/docker.md) - Detailed Docker setup
