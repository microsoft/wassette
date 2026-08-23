# Environment Variables

Pass environment variables to Wassette components using shell exports or config files. Components need explicit permission to access variables.

## Server Configuration

Wassette supports the following environment variables for server configuration (following the [twelve-factor app](https://12factor.net/) methodology):

### PORT
Sets the port number for Streamable HTTP when `bind_address` is not specified via CLI or config file.

```bash
PORT=8080 wassette serve --streamable-http
```

Default: `9001`

**Precedence:** CLI (`--bind-address`) > Config file (`bind_address`) > PORT/BIND_HOST > Default (127.0.0.1:9001)

### BIND_HOST
Sets the host address to bind to for HTTP-based transports when `bind_address` is not specified via CLI or config file.

```bash
BIND_HOST=0.0.0.0 wassette serve --streamable-http
```

Default: `127.0.0.1` (localhost only)

**Note:** In Docker containers, use `BIND_HOST=0.0.0.0` to allow external connections.

**Precedence:** CLI (`--bind-address`) > Config file (`bind_address`) > PORT/BIND_HOST > Default (127.0.0.1:9001)

### WASSETTE_CONFIG_FILE
Path to custom configuration file.

```bash
WASSETTE_CONFIG_FILE=/path/to/config.toml wassette serve
```

Default: `$XDG_CONFIG_HOME/wassette/config.toml`

### WASSETTE_ALLOWED_HOSTS
Comma-separated `Host` header values accepted on the `/mcp` endpoint.

```bash
WASSETTE_ALLOWED_HOSTS=wassette.internal,wassette.example.com:9001 \
  wassette serve --streamable-http --bind-address 0.0.0.0:9001
```

Default: unset, which accepts loopback (`localhost`, `127.0.0.1`, `::1`) only.

Requests whose `Host` is not listed are rejected with `403` before MCP dispatch, which
is what prevents DNS rebinding against a locally running server. This is independent of
the bind address: binding to `0.0.0.0` does not by itself make a server reachable as
`http://wassette:9001/mcp`. Entries may be a bare hostname, which matches any port, or
`host:port`, which must match exactly. Empty entries are ignored, and a value that is
empty after trimming leaves the loopback default in place rather than disabling the
check.

A configured list **replaces** the loopback default rather than extending it, so include
loopback explicitly if local clients must keep working:

```bash
WASSETTE_ALLOWED_HOSTS=wassette.internal,localhost,127.0.0.1 \
  wassette serve --streamable-http --bind-address 0.0.0.0:9001
```

**Precedence:** CLI (`--allowed-host`) > `WASSETTE_ALLOWED_HOSTS` > Config file (`allowed_hosts`) > Default (loopback only)

## Component Environment Variables

### Quick Start

```bash
export OPENWEATHER_API_KEY="your_key"
wassette run
wassette permission grant environment-variable weather-tool OPENWEATHER_API_KEY
```

## Recommended Method

Use `wassette secret set` to securely pass environment variables to components:

```bash
wassette secret set weather-tool API_KEY "your_secret_key"
```

This stores the secret securely and makes it available to the component when granted permission.

## Grant Access

```bash
wassette permission grant environment-variable weather-tool API_KEY
```

Or in policy file:

```yaml
version: "1.0"
permissions:
  environment:
    allow:
      - key: "API_KEY"
```

## See Also

- [Permissions](./permissions.md) - Permission system details
- [Configuration Files](./configuration-files.md) - Complete config.toml reference  
