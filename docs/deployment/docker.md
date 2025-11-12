# Running Wassette in Docker

This guide explains how to run Wassette in a Docker container for enhanced security isolation. Containerizing Wassette provides an additional layer of defense by isolating the runtime environment from your host system.

## Why Use Docker with Wassette?

Running Wassette in Docker provides several benefits:

- **Enhanced Security**: Docker containers provide an additional isolation layer on top of Wassette's WebAssembly sandbox
- **Reproducible Environment**: Ensures consistent runtime behavior across different systems
- **Easy Deployment**: Simplifies deployment to production environments
- **Resource Control**: Allows fine-grained control over CPU, memory, and network resources

## Prerequisites

- Docker installed on your system ([Install Docker](https://docs.docker.com/get-docker/))
- Basic familiarity with Docker commands

## Quick Start

### Build the Docker Image

From the Wassette repository root:

```bash
docker build -t wassette:latest .
```

This builds a multi-stage Docker image that:
1. Compiles Wassette from source in a Rust build environment
2. Creates a minimal runtime image with only necessary dependencies
3. Runs as a non-root user for enhanced security

### Run with Streamable HTTP Transport (Default)

The Docker image defaults to streamable-http transport:

```bash
docker run --rm -p 9001:9001 wassette:latest
```

Then connect to `http://localhost:9001` from your MCP client.

### Run with Stdio Transport

For use with MCP clients that expect stdio, override the default command:

```bash
docker run -i --rm wassette:latest wassette serve --stdio
```

### Run with SSE Transport

For SSE transport, override the default command:

```bash
docker run --rm -p 9001:9001 wassette:latest wassette serve --sse
```

Then connect to `http://localhost:9001/sse` from your MCP client.

## Mounting Components

To use custom WebAssembly components with Wassette in Docker, you need to mount the component directory:

### Mount a Local Component Directory

```bash
# Mount your local components directory
docker run -i --rm \
  -v /path/to/your/components:/home/wassette/.local/share/wassette/components:ro \
  wassette:latest
```

**Important**: Use `:ro` (read-only) for the component directory when possible to prevent accidental modifications.

### Example: Running with Filesystem Component

```bash
# Build example components first (on host)
cd examples/filesystem-rs
cargo build --release --target wasm32-wasip2

# Run Wassette with the example component mounted (streamable-http transport)
docker run --rm -p 9001:9001 \
  -v $(pwd)/examples/filesystem-rs/target/wasm32-wasip2/release:/home/wassette/.local/share/wassette/components:ro \
  wassette:latest

# For stdio transport, override the default:
# docker run -i --rm \
#   -v $(pwd)/examples/filesystem-rs/target/wasm32-wasip2/release:/home/wassette/.local/share/wassette/components:ro \
#   wassette:latest wassette serve --stdio
```

### Example: Running with Multiple Component Directories

You can mount multiple component directories using multiple `-v` flags:

```bash
docker run --rm -p 9001:9001 \
  -v /path/to/components1:/home/wassette/.local/share/wassette/components:ro \
  -v /path/to/data:/data:rw \
  wassette:latest
```

## Mounting Secrets

If your components require secrets (API keys, credentials, etc.), mount the secrets directory:

```bash
docker run --rm -p 9001:9001 \
  -v /path/to/secrets:/home/wassette/.config/wassette/secrets:ro \
  -v /path/to/components:/home/wassette/.local/share/wassette/components:ro \
  wassette:latest
```

**Security Note**: Always mount secrets as read-only (`:ro`) and ensure proper file permissions.

## Configuration

### Using Environment Variables

Pass environment variables to the container:

```bash
docker run --rm -p 8080:8080 \
  -e PORT=8080 \
  -e BIND_HOST=0.0.0.0 \
  -e RUST_LOG=debug \
  -e OPENWEATHER_API_KEY=your_api_key \
  wassette:latest
```

**Twelve-Factor App Compliance**: Wassette supports `PORT` and `BIND_HOST` environment variables for flexible port binding. The Docker image defaults to `BIND_HOST=0.0.0.0` to allow external connections.

#### Server Configuration Variables

These environment variables set the default bind address when not specified via CLI (`--bind-address`) or config file:

- **PORT**: Port number to listen on (default: 9001)
- **BIND_HOST**: Host address to bind to (default: 127.0.0.1; Docker image overrides to 0.0.0.0)

**Precedence:** CLI > Config file > PORT/BIND_HOST > Defaults (127.0.0.1:9001)

Example with custom port:

```bash
docker run --rm -p 3000:3000 \
  -e PORT=3000 \
  wassette:latest
```

See the [Environment Variables reference](../reference/environment-variables.md) for comprehensive examples and best practices.

### Using a Configuration File

Mount a custom configuration file:

```bash
docker run --rm -p 9001:9001 \
  -v /path/to/config.toml:/home/wassette/.config/wassette/config.toml:ro \
  wassette:latest
```

Example `config.toml`:

```toml
# Directory where components are stored
component_dir = "/home/wassette/.local/share/wassette/components"

# Environment variables to be made available to components
[environment_vars]
API_KEY = "your_api_key"
LOG_LEVEL = "info"
```

## Docker Compose

For more complex setups, use Docker Compose:

```yaml
# docker-compose.yml
version: '3.8'

services:
  wassette:
    build: .
    image: wassette:latest
    ports:
      - "9001:9001"
    volumes:
      # Mount component directory (read-only)
      - ./components:/home/wassette/.local/share/wassette/components:ro
      # Mount secrets directory (read-only)
      - ./secrets:/home/wassette/.config/wassette/secrets:ro
      # Mount config file (optional)
      - ./config.toml:/home/wassette/.config/wassette/config.toml:ro
    environment:
      - RUST_LOG=info
    # Default is streamable-http, but you can override:
    # command: ["wassette", "serve", "--sse"]
    # command: ["wassette", "serve", "--stdio"]
    # Security: Run with limited resources
    deploy:
      resources:
        limits:
          cpus: '1.0'
          memory: 512M
```

Run with:

```bash
docker-compose up
```

## Security Best Practices

### 1. Run as Non-Root User

The Dockerfile already configures Wassette to run as a non-root user (`wassette:1000`). Never run as root:

```bash
# Good: Uses default non-root user
docker run --rm -p 9001:9001 wassette:latest

# Bad: Don't do this!
# docker run -i --rm --user root wassette:latest
```

### 2. Use Read-Only Mounts

Mount component and secret directories as read-only when possible:

```bash
docker run --rm -p 9001:9001 \
  -v /path/to/components:/home/wassette/.local/share/wassette/components:ro \
  wassette:latest
```

### 3. Limit Container Resources

Prevent resource exhaustion by setting limits:

```bash
docker run --rm -p 9001:9001 \
  --memory="512m" \
  --cpus="1.0" \
  --pids-limit=100 \
  wassette:latest
```

### 4. Use Read-Only Root Filesystem

For maximum security, run with a read-only root filesystem:

```bash
docker run --rm -p 9001:9001 \
  --read-only \
  --tmpfs /tmp:rw,noexec,nosuid,size=50m \
  -v /path/to/components:/home/wassette/.local/share/wassette/components:ro \
  wassette:latest
```

### 5. Drop Unnecessary Capabilities

Drop Linux capabilities that Wassette doesn't need:

```bash
docker run --rm -p 9001:9001 \
  --cap-drop=ALL \
  --security-opt=no-new-privileges:true \
  wassette:latest
```

### 6. Enable Security Profiles

Use AppArmor or SELinux for additional security:

```bash
# With AppArmor
docker run --rm -p 9001:9001 \
  --security-opt apparmor=docker-default \
  wassette:latest

# With SELinux
docker run --rm -p 9001:9001 \
  --security-opt label=type:container_runtime_t \
  wassette:latest
```

## Advanced Usage

### Multi-Stage Build with Custom Base

If you need a custom base image:

```dockerfile
FROM rust:1.90-bookworm AS builder
# ... build stage ...

FROM your-custom-base:latest
# ... runtime stage ...
```

### Health Checks

Wassette provides health and readiness endpoints when running with StreamableHttp transport:

- **`/health`**: Returns 200 OK if server is running
- **`/ready`**: Returns JSON with readiness status
- **`/info`**: Returns version and build information

Add health checks in Docker Compose:

```yaml
# docker-compose.yml
services:
  wassette:
    # ... other config ...
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:9001/health"]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 40s
```

Or with Docker CLI:

```bash
docker run --rm -p 9001:9001 \
  --health-cmd="curl -f http://localhost:9001/health || exit 1" \
  --health-interval=30s \
  --health-timeout=10s \
  --health-retries=3 \
  wassette:latest
```

**Note**: Health endpoints are only available with `--streamable-http` transport (the default for the Docker image). SSE transport (`--sse`) is designed solely for event streaming and does not expose standard HTTP endpoints like `/health`.

### Persistent Component Storage

For persistent component storage across container restarts:

```bash
# Create a named volume
docker volume create wassette-components

# Use the volume
docker run --rm -p 9001:9001 \
  -v wassette-components:/home/wassette/.local/share/wassette/components \
  wassette:latest
```

## Troubleshooting

### Permission Denied Errors

If you encounter permission errors when mounting volumes:

```bash
# Check the ownership of your mounted directories
ls -la /path/to/components

# Ensure the wassette user (UID 1000) can read the files
sudo chown -R 1000:1000 /path/to/components
```

### Container Cannot Access Components

Verify the mount path matches Wassette's expected directory:

```bash
# Check inside the container
docker run -i --rm \
  -v /path/to/components:/home/wassette/.local/share/wassette/components:ro \
  wassette:latest sh -c "ls -la /home/wassette/.local/share/wassette/components"
```

### Network Connectivity Issues

When using HTTP/SSE transport, ensure the port is properly exposed:

```bash
# Check if the port is listening
docker run -d --name wassette-test -p 9001:9001 wassette:latest wassette serve --sse
docker logs wassette-test
curl http://localhost:9001/sse
docker rm -f wassette-test
```

## Building from Pre-Built Binaries

For faster builds, you can create a Dockerfile that uses pre-built Wassette binaries:

```dockerfile
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3 \
        curl && \
    rm -rf /var/lib/apt/lists/*

# Download and install Wassette binary
ARG WASSETTE_VERSION=latest
RUN curl -fsSL https://github.com/microsoft/wassette/releases/download/${WASSETTE_VERSION}/wassette-linux-x86_64 -o /usr/local/bin/wassette && \
    chmod +x /usr/local/bin/wassette

# Create non-root user and directories
RUN useradd -m -u 1000 -s /bin/bash wassette && \
    mkdir -p /home/wassette/.local/share/wassette/components && \
    mkdir -p /home/wassette/.config/wassette/secrets && \
    chown -R wassette:wassette /home/wassette

ENV HOME=/home/wassette
ENV XDG_DATA_HOME=/home/wassette/.local/share
ENV XDG_CONFIG_HOME=/home/wassette/.config

USER wassette
WORKDIR /home/wassette

EXPOSE 9001

CMD ["wassette", "serve", "--stdio"]
```

This approach is faster as it doesn't require compiling from source.

## Next Steps

- Learn about [Wassette's permission system](../reference/permissions.md)
- Explore [component examples](https://github.com/microsoft/wassette/tree/main/examples)
- Read the [CLI reference](../reference/cli.md) for all available commands
- Check the [FAQ](../faq.md) for common questions

## Resources

- [Docker Documentation](https://docs.docker.com/)
- [Docker Security Best Practices](https://docs.docker.com/engine/security/)
- [Wassette GitHub Repository](https://github.com/microsoft/wassette)
