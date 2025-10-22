# Environment Variables

Pass environment variables to Wassette components using shell exports, configuration files, or Docker environment flags. Components can only access environment variables that are explicitly granted permission through the permission system.

## Quick Example

Set an API key and grant component access:

```bash
# Set the environment variable
export OPENWEATHER_API_KEY="your_api_key_here"

# Start Wassette
wassette serve --stdio

# Grant permission (in another terminal or through AI agent)
wassette permission grant environment-variable weather-tool OPENWEATHER_API_KEY
```

## Three Ways to Pass Environment Variables

### 1. Shell Export (Recommended for Development)

Export variables before starting Wassette:

```bash
# Single variable
export API_KEY="your_key_here"

# Multiple variables
export API_KEY="your_key_here"
export LOG_LEVEL="debug"
export NODE_ENV="development"

# Start Wassette
wassette serve --stdio
```

**Pros:** Simple, works for quick testing
**Cons:** Variables lost when terminal closes

### 2. Configuration File (Recommended for Production)

Add environment variables to `config.toml`:

**Linux/macOS:** `~/.config/wassette/config.toml`  
**Windows:** `%APPDATA%\wassette\config.toml`

```toml
[environment_vars]
API_KEY = "your_key_here"
LOG_LEVEL = "info"
NODE_ENV = "production"
```

Start Wassette normally:

```bash
wassette serve --stdio
```

**Pros:** Persistent, version controlled, environment-specific
**Cons:** Requires file management

### 3. Docker Environment Flags

Pass variables when running Docker containers:

```bash
docker run --rm -p 9001:9001 \
  -e OPENWEATHER_API_KEY="your_key" \
  -e RUST_LOG="debug" \
  wassette:latest
```

Or use an environment file:

```bash
# Create .env file
cat > .env << EOF
OPENWEATHER_API_KEY=your_key
RUST_LOG=debug
EOF

# Run with env file
docker run --rm -p 9001:9001 --env-file .env wassette:latest
```

**Pros:** Container-native, works with orchestration tools
**Cons:** Docker-specific

## Granting Component Access

After setting environment variables, grant component access:

**Using AI Agent:**
```
Please grant the weather-tool component access to the OPENWEATHER_API_KEY environment variable
```

**Using CLI:**
```bash
wassette permission grant environment-variable weather-tool OPENWEATHER_API_KEY
```

**Using Policy File:**
```yaml
version: "1.0"
permissions:
  environment:
    allow:
      - key: "OPENWEATHER_API_KEY"
      - key: "API_TOKEN"
```

## Common Patterns

### API Keys for Weather Services

```bash
export OPENWEATHER_API_KEY="abc123"
export BRAVE_SEARCH_API_KEY="xyz789"

wassette serve --stdio
```

### Development vs Production

**Development:**
```toml
# dev-config.toml
[environment_vars]
LOG_LEVEL = "debug"
RUST_LOG = "trace"
```

```bash
export WASSETTE_CONFIG_FILE="./dev-config.toml"
wassette serve --stdio
```

**Production:**
```toml
# prod-config.toml
[environment_vars]
LOG_LEVEL = "info"
NODE_ENV = "production"
```

### Multiple Components

```bash
# Grant same variable to multiple components
wassette permission grant environment-variable weather-tool API_KEY
wassette permission grant environment-variable search-tool API_KEY
wassette permission grant environment-variable data-tool API_KEY
```

## Security Best Practices

Keep sensitive data secure:

```bash
# ✅ Good: Use shell export
export API_KEY="secret_key"

# ✅ Good: Use config.toml with restricted permissions
chmod 600 ~/.config/wassette/config.toml

# ❌ Avoid: Hardcoding in component code
# ❌ Avoid: Committing secrets to git
# ❌ Avoid: Passing secrets in command-line arguments
```

Use secret management for production:

```bash
# Store secrets securely
wassette secret set weather-tool API_KEY "your_secret_key"
```

Restrict config.toml permissions:

```bash
chmod 600 ~/.config/wassette/config.toml
```

## Troubleshooting

**Component can't read environment variable:**

1. Check the variable is set in your shell:
   ```bash
   echo $OPENWEATHER_API_KEY
   ```

2. Verify the component has permission:
   ```bash
   wassette permission list weather-tool
   ```

3. Check the policy file exists:
   ```bash
   cat ~/.local/share/wassette/policies/weather-tool/policy.yaml
   ```

4. Restart Wassette after setting new variables

**Variable not persisting:**

Use `config.toml` instead of shell export for persistent storage.

**Docker container can't see variable:**

Ensure you pass `-e` flag or `--env-file` when running the container.

## See Also

- [Permissions Reference](./permissions.md) - Detailed permission system documentation
- [Configuration Files](./configuration-files.md) - Complete config.toml reference
- [Docker Deployment](../deployment/docker.md) - Docker-specific configuration
- [CLI Reference](./cli.md) - Command-line interface documentation
