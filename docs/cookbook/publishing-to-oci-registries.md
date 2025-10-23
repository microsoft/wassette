# Publishing Wasm Components to OCI Registries

Publish your WebAssembly components to OCI registries like GitHub Container Registry (GHCR) for easy distribution. Once published, load components with: `wassette component load oci://ghcr.io/user/component:latest`

## Prerequisites

- A built `.wasm` component (see [JavaScript](./javascript.md), [Python](./python.md), [Rust](./rust.md), or [Go](./go.md) guides)
- Access to an OCI registry (GHCR, Docker Hub, Azure Container Registry, etc.)
- Authentication credentials for your registry

## Method 1: Local Publishing with wkg CLI

Install the `wkg` tool:

```bash
cargo install wkg
# or faster: cargo binstall wkg -y
```

Authenticate to GHCR:

```bash
# Create a GitHub PAT with 'write:packages' scope at https://github.com/settings/tokens/new
echo $GITHUB_TOKEN | docker login ghcr.io -u USERNAME --password-stdin
```

Publish your component:

```bash
# Basic publish
wkg oci push ghcr.io/your-username/component-name:v1.0.0 component.wasm

# With metadata annotations
wkg oci push ghcr.io/your-username/component-name:v1.0.0 component.wasm \
  --annotation "org.opencontainers.image.description"="Component description" \
  --annotation "org.opencontainers.image.source"="https://github.com/your-username/repo" \
  --annotation "org.opencontainers.image.version"="1.0.0" \
  --annotation "org.opencontainers.image.licenses"="MIT"
```

## Method 2: Automated Publishing with GitHub Actions

Create `.github/workflows/publish-component.yml`:

```yaml
name: Publish Component
on:
  push:
    tags: [ "v*" ]

jobs:
  publish:
    runs-on: ubuntu-latest
    permissions:
      packages: write
      id-token: write
    steps:
      - uses: actions/checkout@v4
      - name: Build component
        run: npm install && npm run build  # Adjust for your language
      - name: Log in to GHCR
        uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}
      - name: Publish component
        uses: bytecodealliance/wkg-github-action@v5
        with:
          file: dist/component.wasm
          oci-reference-without-tag: ghcr.io/${{ github.repository_owner }}/component
          version: ${{ github.ref_name }}
```

See [Wassette's workflow](https://github.com/microsoft/wassette/blob/main/.github/workflows/examples.yml) for a complete example.

## Versioning Strategy

```bash
wkg oci push ghcr.io/user/component:latest component.wasm       # Latest stable
wkg oci push ghcr.io/user/component:v1.0.0 component.wasm       # Semantic version
wkg oci push ghcr.io/user/component:abc1234 component.wasm      # Commit SHA
wkg oci push ghcr.io/user/component:v1.0.0-beta.1 component.wasm # Pre-release
```

## Best Practices

- Always tag with specific versions, not just `latest`
- Sign components with Cosign for security
- Use CI/CD for consistent builds
- Add OCI annotations for discoverability
- Follow semantic versioning (MAJOR.MINOR.PATCH)

## Resources

- [OCI Distribution Spec](https://github.com/opencontainers/distribution-spec)
- [GHCR Documentation](https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry)
- [Cosign Documentation](https://docs.sigstore.dev/cosign/overview/)
