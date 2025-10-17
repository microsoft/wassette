# Managing Permissions

Wassette uses a fine-grained permission system to control what resources WebAssembly components can access. This page explains how to work with permissions in your day-to-day use of Wassette.

## Overview

Every component in Wassette runs in a secure sandbox with **deny-by-default** permissions. This means:

- **No access by default**: Components cannot access files, networks, or environment variables unless explicitly granted
- **Per-component policies**: Each component has its own independent permission set
- **Runtime enforcement**: The WebAssembly sandbox blocks unauthorized access attempts

## Permission Types

Wassette supports four types of permissions:

### Storage Permissions

Control file system access for reading and writing files.

**Example uses:**
- Allow a component to read configuration files
- Grant write access to output directories
- Restrict access to specific workspace folders

### Network Permissions

Control outbound network access to specific hosts.

**Example uses:**
- Allow API calls to external services
- Permit access to specific domains only
- Restrict network egress for security

**Commonly Used Domains:**

When configuring network permissions for your components, you may need to grant access to commonly used development services. Below is a reference list of frequently needed domains organized by category. You should evaluate each domain and only grant access to those that your specific component requires.

**Package Registries:**
- `registry.npmjs.org`, `*.npmjs.com` - npm (Node.js packages)
- `pypi.org`, `*.pypi.org`, `files.pythonhosted.org` - PyPI (Python packages)
- `rubygems.org`, `*.rubygems.org` - RubyGems (Ruby packages)
- `crates.io`, `*.crates.io`, `static.crates.io`, `index.crates.io` - Cargo (Rust packages)
- `repo.maven.apache.org`, `repo1.maven.org`, `central.maven.org`, `search.maven.org` - Maven (Java packages)
- `nuget.org`, `*.nuget.org`, `api.nuget.org` - NuGet (.NET packages)
- `registry.yarnpkg.com` - Yarn (JavaScript packages)

**Version Control Systems:**
- `github.com`, `*.github.com`, `api.github.com`, `raw.githubusercontent.com`, `codeload.github.com` - GitHub
- `gitlab.com`, `*.gitlab.com` - GitLab
- `bitbucket.org`, `*.bitbucket.org`, `api.bitbucket.org` - Bitbucket

**Cloud Service Providers:**
- `*.amazonaws.com`, `s3.amazonaws.com`, `*.s3.amazonaws.com` - AWS
- `*.googleapis.com`, `storage.googleapis.com`, `*.google.com` - Google Cloud
- `*.azure.com`, `*.azurewebsites.net`, `*.blob.core.windows.net` - Azure
- `*.cloudflare.com`, `cloudflare.com` - Cloudflare

**Container Registries:**
- `docker.io`, `*.docker.io`, `registry-1.docker.io`, `index.docker.io` - Docker Hub
- `ghcr.io` - GitHub Container Registry
- `quay.io`, `*.quay.io` - Quay
- `gcr.io`, `*.gcr.io`, `*.pkg.dev` - Google Container Registry

**AI/ML APIs:**
- `api.openai.com`, `*.openai.com` - OpenAI
- `api.anthropic.com`, `*.anthropic.com` - Anthropic
- `api.cohere.ai`, `*.cohere.ai` - Cohere
- `huggingface.co`, `*.huggingface.co`, `cdn-lfs.huggingface.co` - Hugging Face

**Content Delivery Networks (CDNs):**
- `cdn.jsdelivr.net`, `*.jsdelivr.net` - jsDelivr
- `unpkg.com` - UNPKG
- `cdnjs.cloudflare.com` - Cloudflare CDN
- `*.fastly.net` - Fastly
- `*.akamaized.net`, `*.edgecastcdn.net` - Akamai

**Documentation and Learning:**
- `docs.rs` - Rust documentation
- `readthedocs.io`, `*.readthedocs.io`, `readthedocs.org`, `*.readthedocs.org` - Read the Docs

**Build and CI/CD:**
- `circleci.com`, `*.circleci.com` - CircleCI
- `actions.githubusercontent.com`, `objects.githubusercontent.com` - GitHub Actions

> **Security Note**: Only grant network access to domains that your component actually needs. Review each domain permission request carefully to maintain a secure sandbox environment.

### Environment Variable Permissions

Control access to environment variables.

**Example uses:**
- Provide API keys to components
- Share configuration via environment
- Control access to sensitive credentials

### Memory Permissions

Set memory limits for components (future capability).

**Example uses:**
- Prevent resource exhaustion
- Enforce quotas in multi-tenant environments

## Granting Permissions

The recommended way to grant permissions is through your AI agent when running Wassette as an MCP server. You can also use CLI commands for direct management, or define permissions in policy files.

### Using MCP Built-in Tools (Recommended)

When running Wassette as an MCP server, simply ask your AI agent to grant permissions in natural language:

```text
Please grant storage read and write permissions to the weather-tool for fs://workspace/
```

The agent will automatically use the appropriate built-in tool to apply the permission.

**More examples:**

```text
Grant network access to api.weather.com for the weather-tool component
```

```text
Allow the weather-tool to access the API_KEY environment variable
```

**Available MCP tools:**
- `grant-storage-permission`: Grant file system access
- `grant-network-permission`: Grant network access
- `grant-environment-variable-permission`: Grant environment variable access

The agent understands permission requests and selects the right tool, so you don't need to worry about command syntax.

> **Note**: After granting environment variable permissions, the server must be able to see those environment variables. You can provide them by:
> - Using `wassette secret set <component-id> <key> <value>` to inject secrets
> - Running the server with the necessary environment variables already set

### Using CLI Commands

For direct management or scripting, use the `wassette permission grant` command:

**Grant storage access:**
```bash
# Read-only access to a directory
wassette permission grant storage weather-tool fs://workspace/ --access read

# Read and write access
wassette permission grant storage weather-tool fs://workspace/ --access read,write

# Access to a specific file
wassette permission grant storage weather-tool fs://config/app.yaml --access read
```

**Grant network access:**
```bash
# Allow access to a specific host
wassette permission grant network weather-tool api.weather.com

# Allow localhost access
wassette permission grant network weather-tool localhost:8080
```

**Grant environment variable access:**
```bash
# Grant access to an environment variable
wassette permission grant environment-variable weather-tool API_KEY

# Grant access to multiple variables
wassette permission grant environment-variable weather-tool HOME
wassette permission grant environment-variable weather-tool PATH
```

### Using Policy Files

Policy files store permissions for components in YAML format. These files are typically managed automatically by Wassette when you use the built-in tools or CLI commands rather than being manually written.

When you grant permissions through MCP built-in tools or CLI commands, Wassette creates and updates a `policy.yaml` file alongside your component:

```yaml
version: "1.0"
description: "Weather tool permissions"
permissions:
  storage:
    allow:
      - uri: "fs://workspace/**"
        access: ["read", "write"]
      - uri: "fs://config/app.yaml"
        access: ["read"]
  network:
    allow:
      - host: "api.weather.com"
      - host: "api.openweathermap.org"
  environment:
    allow:
      - key: "API_KEY"
      - key: "WEATHER_API_TOKEN"
```

**Policy file structure:**
- `version`: Policy format version (currently "1.0")
- `description`: Human-readable description
- `permissions`: Permission declarations organized by type
  - `storage.allow`: List of file system URIs and access types
  - `network.allow`: List of allowed hosts
  - `environment.allow`: List of environment variable keys

**Network permission options:**
- `host: "example.com"`: Allow access to a specific host
- `host: "*.example.com"`: Allow access to all subdomains of example.com

See the [Network Permissions](#network-permissions) section above for a comprehensive list of commonly used domains you may need to grant access to.

While you can manually create or edit policy files for distributing components with predefined permissions, for most use cases, granting permissions through the AI agent or CLI commands is simpler and less error-prone.

## Revoking Permissions

Remove previously granted permissions using the `wassette permission revoke` command:

**Revoke storage access:**
```bash
wassette permission revoke storage weather-tool fs://workspace/
```

**Revoke network access:**
```bash
wassette permission revoke network weather-tool api.weather.com
```

**Revoke environment variable access:**
```bash
wassette permission revoke environment-variable weather-tool API_KEY
```

### Reset All Permissions

To remove all permissions for a component:

```bash
wassette permission reset weather-tool
```

This returns the component to its default deny-all state.

## Checking Permissions

View the current permissions for a component:

**Using CLI:**
```bash
# Get policy in JSON format
wassette policy get weather-tool

# Get policy in YAML format
wassette policy get weather-tool --output-format yaml
```

**Using MCP:**
```text
What are the current permissions for weather-tool?
```

The agent will use the `get-policy` tool to retrieve the information.

## Common Permission Patterns

### Development Environment

Grant broad permissions for local development:

```yaml
version: "1.0"
description: "Development permissions"
permissions:
  storage:
    allow:
      - uri: "fs://$(pwd)/workspace/**"
        access: ["read", "write"]
      - uri: "fs://$(pwd)/config/**"
        access: ["read"]
  network:
    allow:
      - host: "localhost"
      - host: "127.0.0.1"
      - host: "*.local"
  environment:
    allow:
      - key: "HOME"
      - key: "USER"
      - key: "PWD"
```

### Production Environment

Restrict permissions to minimum required:

```yaml
version: "1.0"
description: "Production permissions"
permissions:
  storage:
    allow:
      - uri: "fs:///app/data/**"
        access: ["read"]
      - uri: "fs:///app/cache/**"
        access: ["read", "write"]
  network:
    allow:
      - host: "api.production-service.com"
  environment:
    allow:
      - key: "API_KEY"
```

### Untrusted Components

Minimal permissions for third-party components:

```yaml
version: "1.0"
description: "Restricted third-party component"
permissions:
  storage:
    allow:
      - uri: "fs:///tmp/component-cache/**"
        access: ["read", "write"]
  network:
    allow:
      - host: "api.trusted-vendor.com"
  # No environment variable access
```

## Security Best Practices

### Principle of Least Privilege

Only grant the minimum permissions needed:

✅ **Good:**
```yaml
permissions:
  storage:
    allow:
      - uri: "fs:///app/config/settings.yaml"
        access: ["read"]
```

❌ **Too permissive:**
```yaml
permissions:
  storage:
    allow:
      - uri: "fs:///**"
        access: ["read", "write"]
```

### Use Specific Paths

Avoid wildcards when possible:

✅ **Good:**
```yaml
permissions:
  network:
    allow:
      - host: "api.example.com"
      - host: "cdn.example.com"
```

❌ **Too broad:**
```yaml
permissions:
  network:
    allow:
      - host: "*.example.com"
```

### Audit Regularly

Review component permissions periodically:

```bash
# List all components
wassette component list

# Check permissions for each
for component in $(wassette component list | jq -r '.components[].id'); do
  echo "=== $component ==="
  wassette policy get $component --output-format yaml
done
```

### Test Before Production

Validate permissions in a safe environment:

1. Load component in test environment
2. Grant minimal permissions
3. Test functionality
4. Add permissions incrementally as needed
5. Document final permission set

## Troubleshooting

### Component Cannot Access Files

**Symptom:** Component fails when trying to read or write files.

**Solution:**
1. Check current permissions: `wassette policy get <component-id>`
2. Verify the file path matches the policy URI
3. Ensure access level includes required operations (read/write)
4. Grant missing permissions: `wassette permission grant storage <component-id> fs://path --access read,write`

### Network Requests Failing

**Symptom:** Component cannot make network requests.

**Solution:**
1. Check current permissions: `wassette policy get <component-id>`
2. Verify the host is in the allow list
3. Check for typos in host names
4. Grant missing permissions: `wassette permission grant network <component-id> api.example.com`

### Environment Variables Not Available

**Symptom:** Component cannot read environment variables.

**Solution:**
1. Check current permissions: `wassette policy get <component-id>`
2. Verify the variable key is in the allow list
3. Ensure the environment variable is set in your shell
4. Grant missing permissions: `wassette permission grant environment-variable <component-id> VAR_NAME`

### Permission Changes Not Taking Effect

**Solution:**
- Restart the Wassette server after modifying policy files
- Use runtime permission commands for immediate effect
- Verify changes with `wassette policy get <component-id>`

## What Happens When Access is Denied?

When a component tries to access an unauthorized resource:

1. **The WebAssembly sandbox blocks the attempt** - No unauthorized access occurs
2. **The operation fails** - The component receives an error
3. **No security exceptions are raised** - This is expected behavior
4. **Logs record the attempt** - Check logs with `RUST_LOG=debug`

This deny-by-default behavior ensures components cannot exceed their granted capabilities.

## Next Steps

- **[CLI Reference](./cli.md)**: Complete CLI command documentation
- **[FAQ](../faq.md)**: Common questions about security and permissions
- **[Permission System Design](../design/permission-system.md)**: Technical architecture details

## Additional Resources

- [WebAssembly Component Model](https://github.com/WebAssembly/component-model)
- [Capability-Based Security](https://en.wikipedia.org/wiki/Capability-based_security)
- [Wasmtime Security](https://docs.wasmtime.dev/security.html)
