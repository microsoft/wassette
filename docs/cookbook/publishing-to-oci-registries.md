# Publishing Wasm Components to OCI Registries

This guide explains how to publish your WebAssembly components to OCI (Open Container Initiative) registries, making them easily accessible for loading in Wassette and other component-based systems. You'll learn how to publish to GitHub Container Registry (GHCR) using the `wkg` tool and GitHub Actions.

## What is OCI and Why Use It?

OCI (Open Container Initiative) registries provide a standardized way to store and distribute container images and artifacts. Publishing Wasm components as OCI artifacts offers several benefits:

**Benefits of OCI Storage:**
- **Easy Distribution**: Components can be pulled by anyone with access to the registry, just like Docker images
- **Version Management**: Support for semantic versioning and tagging (e.g., `latest`, `v1.0.0`)
- **Integrity Verification**: Built-in support for cryptographic signing and verification
- **Standard Tooling**: Use familiar container registry tools and workflows
- **Access Control**: Leverage existing authentication and authorization mechanisms

**Loading from OCI Registries:**

Once published, Wassette can load components directly from OCI registries:

```bash
# Load the latest version
wassette component load oci://ghcr.io/microsoft/time-server-js:latest

# Load a specific version
wassette component load oci://ghcr.io/microsoft/fetch-rs:v0.4.0
```

## Prerequisites

Before publishing components, ensure you have:

1. **A built Wasm component** (`.wasm` file) - See the language-specific guides:
   - [JavaScript/TypeScript](./javascript.md)
   - [Python](./python.md)
   - [Rust](./rust.md)
   - [Go](./go.md)

2. **Access to an OCI registry** - This guide focuses on GitHub Container Registry (GHCR), but the process is similar for other registries:
   - GitHub Container Registry (ghcr.io) - free for public repositories
   - Docker Hub (docker.io)
   - Azure Container Registry (azurecr.io)
   - Any OCI-compliant registry

3. **Authentication credentials** for your chosen registry

## Method 1: Publishing with wkg CLI (Local Development)

The `wkg` (WebAssembly Package) tool provides a command-line interface for publishing Wasm components to OCI registries.

### Installing wkg

Install the `wkg` tool using Cargo:

```bash
# Install using cargo
cargo install wkg

# Or install using cargo-binstall (faster)
cargo binstall wkg -y
```

### Authenticating to GHCR

Before publishing, authenticate to the GitHub Container Registry:

```bash
# Create a GitHub Personal Access Token with 'write:packages' scope
# Visit: https://github.com/settings/tokens/new

# Login to GHCR using docker (wkg uses docker credentials)
echo $GITHUB_TOKEN | docker login ghcr.io -u USERNAME --password-stdin

# Alternative: Use the GitHub CLI
gh auth token | docker login ghcr.io -u USERNAME --password-stdin
```

### Publishing Your Component

Use the `wkg oci push` command to publish your component:

```bash
# Basic publish command
wkg oci push ghcr.io/your-username/component-name:latest component.wasm

# Publish with version tag
wkg oci push ghcr.io/your-username/component-name:v1.0.0 component.wasm

# Publish with annotations (metadata)
wkg oci push ghcr.io/your-username/component-name:v1.0.0 component.wasm \
  --annotation "org.opencontainers.image.description"="My component description" \
  --annotation "org.opencontainers.image.source"="https://github.com/your-username/repo" \
  --annotation "org.opencontainers.image.version"="1.0.0" \
  --annotation "org.opencontainers.image.licenses"="MIT"
```

**Command Breakdown:**
- **OCI Reference**: `ghcr.io/your-username/component-name:tag`
  - `ghcr.io` - Registry hostname
  - `your-username` - GitHub username or organization
  - `component-name` - Name of your component
  - `tag` - Version tag (e.g., `latest`, `v1.0.0`)
- **Component File**: Path to your `.wasm` file
- **Annotations**: Optional metadata following OCI image spec

### Example: Publishing a Time Server Component

Let's walk through a complete example using a JavaScript component:

```bash
# 1. Build the component (from your component directory)
cd examples/time-server-js
npm install
npm run build  # Creates time-server-js.wasm

# 2. Authenticate to GHCR
echo $GITHUB_TOKEN | docker login ghcr.io -u myusername --password-stdin

# 3. Publish with version and metadata
wkg oci push ghcr.io/myusername/time-server-js:v1.0.0 time-server-js.wasm \
  --annotation "org.opencontainers.image.description"="A time server component" \
  --annotation "org.opencontainers.image.source"="https://github.com/myusername/my-components" \
  --annotation "org.opencontainers.image.version"="1.0.0" \
  --annotation "org.opencontainers.image.licenses"="MIT"

# 4. Also tag as latest
wkg oci push ghcr.io/myusername/time-server-js:latest time-server-js.wasm
```

### Verifying the Published Component

After publishing, verify your component is accessible:

```bash
# Test loading the component with Wassette
wassette component load oci://ghcr.io/myusername/time-server-js:latest

# Or use docker to inspect the artifact
docker pull ghcr.io/myusername/time-server-js:latest
```

## Method 2: Automated Publishing with GitHub Actions

For production workflows, automate component publishing using GitHub Actions. This ensures consistent publishing and integrates with your CI/CD pipeline.

### Setting Up the Workflow

Create a workflow file at `.github/workflows/publish-component.yml`:

```yaml
name: Publish Component

on:
  push:
    branches: [ "main" ]
    tags: [ "v*" ]
  workflow_dispatch:
    inputs:
      tag:
        description: 'Tag for the component'
        required: true
        default: 'latest'

env:
  REGISTRY: ghcr.io

jobs:
  build-and-publish:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      packages: write      # Required for GHCR push
      id-token: write      # Required for Cosign signing
    
    steps:
      # Checkout repository
      - name: Checkout code
        uses: actions/checkout@v4
      
      # Set up your build environment (example for JavaScript)
      - name: Set up Node.js
        uses: actions/setup-node@v4
        with:
          node-version: '20'
      
      # Build the component
      - name: Build component
        run: |
          npm install
          npm run build
      
      # Log in to GitHub Container Registry
      - name: Log in to GHCR
        uses: docker/login-action@v3
        with:
          registry: ${{ env.REGISTRY }}
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}
      
      # Determine version tag
      - name: Determine tag
        id: meta
        run: |
          if [[ $GITHUB_REF == refs/tags/* ]]; then
            VERSION=${GITHUB_REF#refs/tags/}
            echo "tag=$VERSION" >> $GITHUB_OUTPUT
          elif [[ "${{ github.event_name }}" == "workflow_dispatch" ]]; then
            echo "tag=${{ github.event.inputs.tag }}" >> $GITHUB_OUTPUT
          else
            echo "tag=$GITHUB_SHA" >> $GITHUB_OUTPUT
          fi
      
      # Publish component using wkg-github-action
      - name: Publish component
        id: publish
        uses: bytecodealliance/wkg-github-action@v5
        with:
          file: dist/my-component.wasm
          oci-reference-without-tag: ghcr.io/${{ github.repository_owner }}/my-component
          version: ${{ steps.meta.outputs.tag }}
          description: "My awesome Wasm component"
          source: ${{ github.server_url }}/${{ github.repository }}
          licenses: "MIT"
      
      # Optional: Sign the component with Cosign
      - name: Install Cosign
        uses: sigstore/cosign-installer@v4
      
      - name: Sign container image
        run: |
          cosign sign --yes ghcr.io/${{ github.repository_owner }}/my-component@${{ steps.publish.outputs.digest }}
```

### Workflow Explanation

**Key Components:**

1. **Triggers**: The workflow runs on:
   - Pushes to `main` branch
   - Version tags (e.g., `v1.0.0`)
   - Manual workflow dispatch

2. **Permissions**: Required GitHub token permissions:
   - `contents: read` - Read repository contents
   - `packages: write` - Push to GHCR
   - `id-token: write` - Sign with Cosign (optional)

3. **Build Steps**: Customize based on your component's language and build process

4. **wkg-github-action**: Handles the OCI push with proper annotations

5. **Cosign Signing** (Optional): Cryptographically signs the component for verification

### Publishing Multiple Components

If you have multiple components in a monorepo, use a matrix strategy:

```yaml
jobs:
  publish:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        component:
          - name: time-server-js
            file: time-server-js.wasm
            path: examples/time-server-js
          - name: fetch-rs
            file: fetch-rs.wasm
            path: examples/fetch-rs
    
    steps:
      - uses: actions/checkout@v4
      
      # Build step depends on component type
      - name: Build component
        working-directory: ${{ matrix.component.path }}
        run: |
          # Your build command here
          just build
      
      - name: Publish ${{ matrix.component.name }}
        uses: bytecodealliance/wkg-github-action@v5
        with:
          file: bin/${{ matrix.component.file }}
          oci-reference-without-tag: ghcr.io/${{ github.repository_owner }}/${{ matrix.component.name }}
          version: ${{ github.sha }}
```

### Real-World Example: Wassette's Publishing Workflow

The Wassette project uses an automated workflow to publish example components. You can view the complete implementation in [`.github/workflows/examples.yml`](https://github.com/microsoft/wassette/blob/main/.github/workflows/examples.yml).

**Key features of Wassette's approach:**
- Builds multiple components with different languages (Rust, Go, JavaScript, Python)
- Publishes on every push to `main` that modifies `examples/**`
- Tags components with both commit SHA and release version
- Signs all published images with Cosign
- Uses a matrix strategy for parallel publishing

## Advanced Topics

### Managing Versions and Tags

Follow semantic versioning for your components:

```bash
# Development/testing (commit-based)
wkg oci push ghcr.io/user/component:abc1234 component.wasm

# Pre-release versions
wkg oci push ghcr.io/user/component:v1.0.0-beta.1 component.wasm

# Stable releases
wkg oci push ghcr.io/user/component:v1.0.0 component.wasm
wkg oci push ghcr.io/user/component:latest component.wasm
```

**Tagging Strategy:**
- `latest` - Most recent stable release
- `v1.0.0` - Specific semantic version
- `abc1234` - Commit SHA for exact reproducibility
- `v1.0.0-beta.1` - Pre-release versions

### Component Signing and Verification

Signing components provides authenticity and integrity verification:

**Signing with Cosign:**

```bash
# Sign after publishing
cosign sign ghcr.io/user/component@sha256:digest

# Verify a signed component
cosign verify ghcr.io/user/component:latest
```

**In GitHub Actions:**

```yaml
- name: Sign container image
  run: |
    cosign sign --yes ghcr.io/${{ github.repository_owner }}/component@${{ steps.publish.outputs.digest }}
```

### Private Registries

For private components, adjust registry URL and authentication:

```bash
# Authenticate to private registry
docker login your-registry.azurecr.io -u username -p password

# Publish to private registry
wkg oci push your-registry.azurecr.io/namespace/component:v1.0.0 component.wasm
```

### Registry Permissions

**GitHub Container Registry:**
- Public packages are readable by anyone
- Private packages require authentication
- Configure package visibility in GitHub repository settings

**Access Control:**
```bash
# Make a package public (GitHub CLI)
gh api -X PATCH /user/packages/container/component-name \
  -f visibility=public
```

## Troubleshooting

### Authentication Issues

**Problem**: `unauthorized: authentication required`

**Solution**:
```bash
# Verify token has correct permissions
# For GHCR: Need 'write:packages' and 'read:packages' scopes

# Re-authenticate
docker logout ghcr.io
echo $NEW_TOKEN | docker login ghcr.io -u username --password-stdin
```

### Push Failures

**Problem**: `insufficient_scope: authorization failed`

**Solution**:
- Check GitHub token has `packages: write` permission
- Verify repository/organization access
- Ensure package visibility settings allow publishing

### Version Conflicts

**Problem**: `tag already exists`

**Solution**:
```bash
# Tags are immutable in most registries
# Use a different tag or version number
wkg oci push ghcr.io/user/component:v1.0.1 component.wasm  # Use v1.0.1 instead of v1.0.0
```

## Best Practices

1. **Version Everything**: Always tag with specific versions, not just `latest`

2. **Automate Publishing**: Use CI/CD to ensure consistent, reproducible builds

3. **Sign Your Components**: Add cryptographic signatures for security

4. **Document Dependencies**: Include README and WIT documentation in your component

5. **Test Before Publishing**: Verify components work locally before publishing

6. **Use Annotations**: Add metadata to make components discoverable and documented

7. **Follow Semantic Versioning**: Use clear version numbers (MAJOR.MINOR.PATCH)

8. **Implement Access Controls**: Set appropriate public/private visibility

## Next Steps

Now that you can publish components to OCI registries:

- **Share Your Components**: Make them available to the community
- **Integrate with CI/CD**: Automate your publishing workflow
- **Explore Component Composition**: Combine multiple components
- **Monitor Usage**: Track downloads and versions

## Resources

- [OCI Distribution Specification](https://github.com/opencontainers/distribution-spec)
- [GitHub Container Registry Documentation](https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry)
- [Cosign Documentation](https://docs.sigstore.dev/cosign/overview/)
- [Wassette Examples Workflow](https://github.com/microsoft/wassette/blob/main/.github/workflows/examples.yml)
- [WebAssembly Component Model](https://component-model.bytecodealliance.org/)
