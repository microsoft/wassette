# Configuration Files Reference

This page provides a comprehensive reference for all configuration files used in Wassette. These files control various aspects of the server's behavior and build configuration.

## Overview

Wassette uses several configuration files for different purposes:

| File | Purpose | Location | Format |
|------|---------|----------|--------|
| [`config.toml`](#configtoml) | Main server configuration | `~/.config/wassette/` | TOML |

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
