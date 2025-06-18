# Published Examples

This repository automatically publishes WebAssembly example components as OCI artifacts to the GitHub Container Registry.

## Available Examples

All examples are published to `ghcr.io/semcp/` and are automatically built and signed with each release.

### ghcr.io/semcp/fetch-rs
A Rust-based HTTP client component that can fetch content from URLs.

**Usage:**
```bash
# Pull the latest version
wkg oci pull ghcr.io/semcp/fetch-rs:latest

# Or pull a specific version
wkg oci pull ghcr.io/semcp/fetch-rs:v0.1.0
```

### ghcr.io/semcp/filesystem
A filesystem access component for reading and writing files.

**Usage:**
```bash
wkg oci pull ghcr.io/semcp/filesystem:latest
```

### ghcr.io/semcp/get-weather
A weather information component that fetches weather data.

**Usage:**
```bash
wkg oci pull ghcr.io/semcp/get-weather:latest
```

### ghcr.io/semcp/time-server-js
A JavaScript-based time server component.

**Usage:**
```bash
wkg oci pull ghcr.io/semcp/time-server-js:latest
```

## Tags

- `latest` - Latest stable release
- `main` - Latest build from main branch
- `v*` - Specific version tags (e.g., `v0.1.0`)

## Signatures

All published artifacts are signed using [cosign](https://docs.sigstore.dev/cosign/overview/) with keyless signing. You can verify signatures with:

```bash
cosign verify ghcr.io/semcp/fetch-rs:latest \
  --certificate-identity-regexp="https://github.com/semcp/weld-mcp-server/.*" \
  --certificate-oidc-issuer="https://token.actions.githubusercontent.com"
```

## Automated Publishing

Examples are automatically published by the GitHub Actions workflow in `.github/workflows/publish-examples.yml` on:

- Pushes to the `main` branch (tagged as `main`)
- Release tags starting with `v` (tagged with version and `latest`)
- Manual workflow dispatch

The workflow:
1. Builds all examples using `just build-examples release`
2. Publishes each component to `ghcr.io/semcp/{component-name}`
3. Signs all artifacts with cosign
4. For releases, also tags as `latest`