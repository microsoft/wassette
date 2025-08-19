# Wassette CLI Reference

The Wassette command-line interface provides comprehensive tools for managing WebAssembly components, policies, and permissions both locally and through the MCP server. This document covers all CLI functionality and usage patterns.

## Overview

Wassette offers two primary modes of operation:

1. **Server Mode**: Run as an MCP server that responds to client requests
2. **CLI Mode**: Direct command-line management of components and permissions

The CLI mode allows you to perform administrative tasks without requiring a running MCP server, making it ideal for automation, scripting, and local development workflows.

## Installation

For installation instructions, see the main [README](../README.md#installation). Once installed, the `wassette` command will be available in your PATH.

## Quick Start

```bash
# Check available commands
wassette --help

# List currently loaded components
wassette component list

# Load a component from an OCI registry
wassette component load oci://ghcr.io/yoshuawuyts/time:latest

# Load a component from a local file
wassette component load file:///path/to/component.wasm

# Start the MCP server (traditional mode)
wassette serve --stdio
```

## Command Structure

Wassette uses a hierarchical command structure organized around functional areas:

```
wassette
├── serve          # Start MCP server
├── component      # Component lifecycle management
│   ├── load       # Load components
│   ├── unload     # Remove components
│   └── list       # Show loaded components
├── policy         # Policy information
│   └── get        # Retrieve component policies
└── permission     # Permission management
    ├── grant      # Add permissions
    ├── revoke     # Remove permissions
    └── reset      # Clear all permissions
```

## Server Commands

### `wassette serve`

Start the Wassette MCP server to handle client requests.

**Stdio Transport (recommended for MCP clients):**
```bash
# Start server with stdio transport
wassette serve --stdio

# Use with specific configuration directory
wassette serve --stdio --plugin-dir /custom/components
```

**HTTP Transport (for development and debugging):**
```bash
# Start server with HTTP transport
wassette serve --http

# Use Server-Sent Events (SSE) transport
wassette serve --sse
```

**Options:**
- `--stdio`: Use stdio transport (recommended for MCP clients)
- `--http`: Use HTTP transport on 127.0.0.1:9001
- `--sse`: Use Server-Sent Events transport
- `--plugin-dir <PATH>`: Set component storage directory (default: `$XDG_DATA_HOME/wassette/components`)

## Component Management

### `wassette component load`

Load a WebAssembly component from various sources.

**Load from OCI registry:**
```bash
# Load a component from GitHub Container Registry
wassette component load oci://ghcr.io/yoshuawuyts/time:latest

# Load with custom plugin directory
wassette component load oci://ghcr.io/microsoft/gomodule:latest --plugin-dir /custom/components
```

**Load from local file:**
```bash
# Load a local component file
wassette component load file:///path/to/component.wasm

# Load with relative path
wassette component load file://./my-component.wasm
```

**Options:**
- `--plugin-dir <PATH>`: Component storage directory

### `wassette component unload`

Remove a loaded component by its ID.

```bash
# Unload a component
wassette component unload my-component-id

# Unload with custom plugin directory
wassette component unload my-component-id --plugin-dir /custom/components
```

**Options:**
- `--plugin-dir <PATH>`: Component storage directory

### `wassette component list`

Display all currently loaded components.

**Basic JSON output:**
```bash
wassette component list
# Output: {"components":[...],"total":1}
```

**Formatted output options:**
```bash
# Pretty-printed JSON
wassette component list --output-format json

# YAML format
wassette component list --output-format yaml

# Table format (human-readable)
wassette component list --output-format table
```

**Example outputs:**

*JSON format:*
```json
{
  "components": [
    {
      "id": "time-component",
      "schema": {
        "tools": [
          {
            "name": "get-current-time",
            "description": "Get the current time",
            "inputSchema": {
              "type": "object",
              "properties": {}
            }
          }
        ]
      },
      "tools_count": 1
    }
  ],
  "total": 1
}
```

*Table format:*
```
ID             | Tools | Description
---------------|-------|----------------------------------
time-component | 1     | Provides time-related functions
```

**Options:**
- `--output-format <FORMAT>`: Output format (json, yaml, table) [default: json]
- `--plugin-dir <PATH>`: Component storage directory

## Policy Management

### `wassette policy get`

Retrieve policy information for a specific component.

```bash
# Get policy for a component
wassette policy get my-component-id

# Get policy with pretty formatting
wassette policy get my-component-id --output-format json

# Get in YAML format
wassette policy get my-component-id --output-format yaml
```

**Example output:**
```json
{
  "component_id": "my-component",
  "permissions": {
    "storage": [
      {
        "uri": "fs://workspace/**",
        "access": ["read", "write"]
      }
    ],
    "network": [
      {
        "host": "api.openai.com"
      }
    ]
  }
}
```

**Options:**
- `--output-format <FORMAT>`: Output format (json, yaml, table) [default: json]
- `--plugin-dir <PATH>`: Component storage directory

## Permission Management

### `wassette permission grant`

Grant specific permissions to a component.

**Storage permissions:**
```bash
# Grant read access to a directory
wassette permission grant storage my-component fs://workspace/ --access read

# Grant read and write access
wassette permission grant storage my-component fs://workspace/ --access read,write

# Grant access to a specific file
wassette permission grant storage my-component fs://config/app.yaml --access read
```

**Network permissions:**
```bash
# Grant access to a specific host
wassette permission grant network my-component api.openai.com

# Grant access to a localhost service
wassette permission grant network my-component localhost:8080
```

**Environment variable permissions:**
```bash
# Grant access to an environment variable
wassette permission grant environment-variable my-component API_KEY

# Grant access to multiple variables
wassette permission grant environment-variable my-component HOME
wassette permission grant environment-variable my-component PATH
```

**Options:**
- `--access <ACCESS>`: For storage permissions, comma-separated list of access types (read, write)
- `--plugin-dir <PATH>`: Component storage directory

### `wassette permission revoke`

Remove specific permissions from a component.

**Storage permissions:**
```bash
# Revoke storage access
wassette permission revoke storage my-component fs://workspace/

# Revoke with custom plugin directory
wassette permission revoke storage my-component fs://config/ --plugin-dir /custom/components
```

**Network permissions:**
```bash
# Revoke network access
wassette permission revoke network my-component api.openai.com
```

**Environment variable permissions:**
```bash
# Revoke environment variable access
wassette permission revoke environment-variable my-component API_KEY
```

**Options:**
- `--plugin-dir <PATH>`: Component storage directory

### `wassette permission reset`

Remove all permissions for a component, resetting it to default state.

```bash
# Reset all permissions for a component
wassette permission reset my-component

# Reset with custom plugin directory
wassette permission reset my-component --plugin-dir /custom/components
```

**Options:**
- `--plugin-dir <PATH>`: Component storage directory

## Common Workflows

### Local Development

```bash
# 1. Build and load a local component
wassette component load file://./target/wasm32-wasi/debug/my-tool.wasm

# 2. Check it loaded correctly
wassette component list --output-format table

# 3. Grant necessary permissions
wassette permission grant storage my-tool fs://$(pwd)/workspace --access read,write
wassette permission grant network my-tool api.example.com

# 4. Verify permissions
wassette policy get my-tool --output-format yaml

# 5. Test via MCP server
wassette serve --stdio
```

### Component Distribution

```bash
# 1. Load component from OCI registry
wassette component load oci://ghcr.io/myorg/my-tool:v1.0.0

# 2. Configure permissions based on component needs
wassette permission grant storage my-tool fs://workspace/** --access read,write
wassette permission grant network my-tool api.myservice.com

# 3. Start server for clients
wassette serve --sse
```

### Permission Auditing

```bash
# List all components and their tool counts
wassette component list --output-format table

# Check permissions for each component
for component in $(wassette component list | jq -r '.components[].id'); do
  echo "=== $component ==="
  wassette policy get $component --output-format yaml
done
```

### Cleanup Operations

```bash
# Reset permissions for a component
wassette permission reset problematic-component

# Remove a component entirely
wassette component unload problematic-component

# List remaining components
wassette component list --output-format table
```

## Configuration

Wassette can be configured using configuration files, environment variables, and command-line options. The configuration sources are merged with the following order of precedence:

1. Command-line options (highest priority)
2. Environment variables prefixed with `WASSETTE_`
3. Configuration file (lowest priority)

### Configuration File

By default, Wassette looks for a configuration file at:
- **Linux/macOS**: `$XDG_CONFIG_HOME/wassette/config.toml` (typically `~/.config/wassette/config.toml`)
- **Windows**: `%APPDATA%\wassette\config.toml`

You can override the default configuration file location using the `WASSETTE_CONFIG_FILE` environment variable:

```bash
export WASSETTE_CONFIG_FILE=/custom/path/to/config.toml
wassette component list
```

Example configuration file (`config.toml`):

```toml
# Directory where components are stored
plugin_dir = "/opt/wassette/components"
```

### Environment Variables

- **`WASSETTE_CONFIG_FILE`**: Override the default configuration file location
- **`WASSETTE_PLUGIN_DIR`**: Override the default component storage location
- **`XDG_CONFIG_HOME`**: Base directory for configuration files (Linux/macOS)
- **`XDG_DATA_HOME`**: Base directory for data storage (Linux/macOS)

### Component Storage

By default, Wassette stores components in `$XDG_DATA_HOME/wassette/components` (typically `~/.local/share/wassette/components` on Linux/macOS). You can override this with the `--plugin-dir` option:

```bash
# Use custom storage directory
export WASSETTE_PLUGIN_DIR=/opt/wassette/components
wassette component load oci://example.com/tool:latest --plugin-dir $WASSETTE_PLUGIN_DIR
```

## Integration with MCP Clients

The CLI commands complement the MCP server functionality. You can:

1. Use CLI commands to pre-configure components and permissions
2. Start the MCP server with `wassette serve`
3. Connect MCP clients to the running server
4. Use CLI commands for administrative tasks while the server runs

**Example VS Code configuration:**
```json
{
  "name": "wassette",
  "command": "wassette",
  "args": ["serve", "--stdio"]
}
```

## Error Handling

The CLI provides clear error messages for common issues:

```bash
# Component not found
$ wassette component unload nonexistent
Error: Component 'nonexistent' not found

# Invalid path
$ wassette component load invalid://path
Error: Unsupported URI scheme 'invalid'. Use 'file://' or 'oci://'

# Permission denied
$ wassette permission grant storage my-component /restricted --access write
Error: Permission denied: cannot grant write access to /restricted
```

## Output Formats

All commands that return structured data support multiple output formats:

- **JSON** (default): Machine-readable, suitable for scripting
- **YAML**: Human-readable structured format
- **Table**: Formatted for terminal display

Use the `--output-format` or `-o` flag to specify the desired format:

```bash
wassette component list -o table
wassette policy get my-component -o yaml
```

## See Also

- [Main README](../README.md) - Installation and basic usage
- [MCP Client Setup](./mcp-clients.md) - Configuring MCP clients
- [Architecture Overview](./overview.md) - Understanding Wassette's design
- [Examples](../examples/) - Sample WebAssembly components