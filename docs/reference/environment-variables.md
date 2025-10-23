# Environment Variables

Pass environment variables to Wassette components using shell exports or config files. Components need explicit permission to access variables.

## Quick Start

```bash
export OPENWEATHER_API_KEY="your_key"
wassette serve --stdio
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
- [Docker Deployment](../deployment/docker.md) - Docker configuration
