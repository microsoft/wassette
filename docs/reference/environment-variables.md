# Environment Variables

Pass environment variables to Wassette components using shell exports, config files, or Docker flags. Components need explicit permission to access variables.

## Quick Start

```bash
export OPENWEATHER_API_KEY="your_key"
wassette serve --stdio
wassette permission grant environment-variable weather-tool OPENWEATHER_API_KEY
```

## Three Methods

**Shell Export (Development)**

```bash
export API_KEY="your_key"
wassette serve --stdio
```

**Config File (Production)** - `~/.config/wassette/config.toml`:

```toml
[environment_vars]
API_KEY = "your_key"
```

**Docker**

```bash
docker run -e API_KEY="your_key" wassette:latest
# Or with env file
docker run --env-file .env wassette:latest
```

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

## Security

```bash
# ✅ Good
export API_KEY="secret"
chmod 600 ~/.config/wassette/config.toml
wassette secret set weather-tool API_KEY "secret"

# ❌ Avoid: Hardcoding, committing to git, command-line args
```

## Troubleshooting

**Can't read variable:** Check `echo $VAR_NAME`, verify `wassette permission list component-id`, restart Wassette.

**Not persisting:** Use `config.toml` instead of shell export.

**Docker can't see:** Pass `-e` flag or `--env-file`.

## See Also

- [Permissions](./permissions.md) - Permission system details
- [Configuration Files](./configuration-files.md) - Complete config.toml reference  
- [Docker Deployment](../deployment/docker.md) - Docker configuration
