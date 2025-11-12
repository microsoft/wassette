# config.toml

This page provides a comprehensive reference for the `config.toml` configuration file used by the Wassette MCP server. This file is optional and provides defaults for server behavior, including component storage locations, secrets directory, and environment variables.

### Location

- **Linux/macOS**: `$XDG_CONFIG_HOME/wassette/config.toml` (typically `~/.config/wassette/config.toml`)
- **Windows**: `%APPDATA%\wassette\config.toml`
- **Custom**: Set via `WASSETTE_CONFIG_FILE` environment variable

### Configuration Priority

Configuration values are merged with the following precedence (highest to lowest):

1. Command-line options (e.g., `--component-dir`)
2. Environment variables prefixed with `WASSETTE_`
3. Configuration file (`config.toml`)

### Schema

```toml
# Directory where WebAssembly components are stored
# Default: $XDG_DATA_HOME/wassette/components (~/.local/share/wassette/components)
component_dir = "/path/to/components"

# Directory where secrets are stored (API keys, credentials, etc.)
# Default: $XDG_CONFIG_HOME/wassette/secrets (~/.config/wassette/secrets)
secrets_dir = "/path/to/secrets"

# Bind address for HTTP-based transports (SSE and StreamableHttp)
# Default: 127.0.0.1:9001
bind_address = "0.0.0.0:8080"

# Environment variables to be made available to components
# These are global defaults and can be overridden per-component in policy files
[environment_vars]
API_KEY = "your_api_key"
LOG_LEVEL = "info"
DATABASE_URL = "postgresql://localhost/mydb"
```

### Fields

#### `component_dir`

- **Type**: String (path)
- **Default**: Platform-specific data directory
- **Description**: Directory where loaded WebAssembly components are stored. Components loaded via `wassette component load` or the MCP interface are saved here.

#### `secrets_dir`

- **Type**: String (path)
- **Default**: Platform-specific config directory
- **Description**: Directory for storing sensitive data like API keys and credentials. This directory should have restricted permissions (e.g., `chmod 600`).

#### `bind_address`

- **Type**: String
- **Default**: `127.0.0.1:9001`
- **Description**: Bind address for HTTP-based transports (SSE and StreamableHttp). The address should be in the format `host:port`. Use `0.0.0.0` to bind to all network interfaces, or a specific IP address to bind to a particular interface. This setting is ignored when using stdio transport.

#### `environment_vars`

- **Type**: Table/Map
- **Default**: Empty
- **Description**: Key-value pairs of environment variables to make available to components. Note that components must explicitly request access to environment variables via their policy files. See the [Environment Variables reference](./environment-variables.md) for detailed usage patterns and examples.

### Example Configurations

**Minimal Configuration:**
```toml
# Use all defaults
```

**Development Configuration:**
```toml
component_dir = "./dev-components"
secrets_dir = "./dev-secrets"
bind_address = "127.0.0.1:9001"

[environment_vars]
LOG_LEVEL = "debug"
RUST_LOG = "trace"
```

**Production Configuration:**
```toml
component_dir = "/opt/wassette/components"
secrets_dir = "/opt/wassette/secrets"
bind_address = "0.0.0.0:8080"

[environment_vars]
LOG_LEVEL = "info"
NODE_ENV = "production"
```

### Environment Variables

You can override any configuration value using environment variables with the `WASSETTE_` prefix:

```bash
# Override component directory
export WASSETTE_COMPONENT_DIR=/custom/components

# Override bind address using PORT and BIND_HOST
export PORT=8080
export BIND_HOST=0.0.0.0

# Override config file location
export WASSETTE_CONFIG_FILE=/etc/wassette/config.toml

# Start server
wassette serve --sse
```

## See Also

- [CLI Reference](cli.md) - Command-line usage and options
- [Permissions Guide](permissions.md) - Working with permissions
- [Docker Deployment](../deployment/docker.md) - Detailed Docker setup
