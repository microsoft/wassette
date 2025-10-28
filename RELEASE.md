# Release Process

This document describes the process for releasing new versions of the Wassette project.

## Release.yml overview

The release process is automated using GitHub Actions, specifically the [`release.yml`](.github/workflows/release.yml) workflow. This workflow is triggered when a new tag is pushed to the repository. Once triggered, the workflow uses a matrix to compile `wassette` for different platforms on native runners and uses `sccache` to speed up the compilation process by caching previous builds. The compiled binaries are then uploaded as artifacts to the release.

### CHANGELOG Synchronization

The release workflow automatically synchronizes with the `CHANGELOG.md` file:

1. **Release Notes from CHANGELOG**: The workflow extracts the changelog content for the specific version being released and uses it as the release notes on GitHub. This ensures consistency between the CHANGELOG and release notes.

2. **Post-Release CHANGELOG Update**: After the release is published, a separate job automatically updates the CHANGELOG:
   - Converts the `[Unreleased]` section to the new version with the release date
   - Adds a new empty `[Unreleased]` section at the top
   - Updates the comparison links to point to the correct version ranges
   - Commits and pushes these changes back to the main branch

This automation eliminates the need to manually maintain release notes separately from the CHANGELOG.

## Release Versioning

Wassette uses semantic versioning. All releases follow the format `vX.Y.Z`, where X is the major version, Y is the minor version, and Z is the patch version.

## Tagging Strategy

- All release tags are prefixed with v, e.g., v0.10.0.
- Tags are created on the default branch (typically main), or on a release branch when applicable.
- Patch releases increment the Z portion, e.g., v0.6.1 → v0.6.2.
- Minor releases increment the Y portion, e.g., v0.9.0 → v0.10.0.

## Steps to Cut a Release

The release process is now largely automated through GitHub Actions workflows and uses a release branch strategy to prevent blocking development on main. Follow these steps:

1. **Prepare the CHANGELOG**: Before creating a release, ensure that the `[Unreleased]` section in `CHANGELOG.md` contains all the changes for the upcoming release. Follow the [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format with sections for:
   - `Added` - new features
   - `Changed` - changes in existing functionality
   - `Deprecated` - soon-to-be removed features
   - `Removed` - now removed features
   - `Fixed` - bug fixes
   - `Security` - vulnerability fixes

   The release workflow will automatically use this content for the GitHub release notes.

1. **Prepare the release**: Trigger the `prepare-release` workflow to create a PR that bumps the version.

   1. Go to the [Actions tab](https://github.com/microsoft/wassette/actions/workflows/prepare-release.yml)
   1. Click "Run workflow"
   1. Enter the new version number (without `v` prefix, e.g., `0.4.0`)
   1. Click "Run workflow"

   This will automatically:
   - Create a release branch `release/vX.Y.Z`
   - Update the version in `Cargo.toml`
   - Update `Cargo.lock`
   - Create a pull request to merge the release branch into main

1. **Review and merge the version bump PR**: The workflow will create a pull request with the version changes. Review and merge this PR into the main branch.

   **Important**: The release branch (`release/vX.Y.Z`) is preserved after merging and will be used during the release process.

1. **Create and push a release tag**: Once the version bump PR is merged:

   ```bash
   # Checkout the main branch and pull the latest changes
   git checkout main
   git pull origin main

   # Create a new tag (e.g., v0.4.0)
   git tag -s v<version> -m "Release v<version>"
   
   # Push the tag
   git push origin v<version>
   ```

1. **Monitor the release workflow**: Once the tag is pushed, the `release.yml` workflow will be triggered automatically:
   - Builds binaries for all platforms (Linux, macOS, Windows; AMD64 and ARM64)
   - Extracts the changelog content for the version from `CHANGELOG.md`
   - Creates a GitHub release with all compiled binaries and the changelog content as release notes
   - Automatically updates `CHANGELOG.md` on the release branch:
     - Converts `[Unreleased]` section to the new version with release date
     - Adds a new empty `[Unreleased]` section
     - Updates version comparison links
   - Creates a PR to merge the release branch back to main with the updated CHANGELOG
   - Monitor the workflow progress in the [Actions tab](https://github.com/microsoft/wassette/actions)

1. **Merge the CHANGELOG update PR**: After the release workflow completes, a new PR will be created to merge the release branch back to main with the updated CHANGELOG. Review and merge this PR.

1. **Package manifests are updated automatically**: After the release is published, the `update-package-manifests` workflow will automatically:
   - Download all release assets
   - Compute SHA256 checksums
   - Update `Formula/wassette.rb` (Homebrew)
   - Update `winget/Microsoft.Wassette.yaml` (WinGet)
   - Create a pull request with these updates

   Simply review and merge the automatically created PR to complete the release process.

## Release Branch Strategy

The release process uses a dedicated release branch strategy to prevent blocking development:

1. **Release branch creation**: When the `prepare-release` workflow is triggered, it creates a branch named `release/vX.Y.Z` (e.g., `release/v0.4.0`)

2. **Release branch preservation**: Unlike typical feature branches, the release branch is not deleted after the initial version bump PR is merged. It remains available for the entire release process.

3. **Release isolation**: All release-related activities (building binaries, updating CHANGELOG) happen on or reference the release branch, ensuring that ongoing development on `main` is not blocked or interrupted.

4. **CHANGELOG updates**: After the release is published, the CHANGELOG is updated on the release branch (not on main), and a PR is created to merge these changes back to main.

5. **Branch cleanup**: After the CHANGELOG update PR is merged, the release branch can be safely deleted as it has served its purpose.

This strategy ensures that:
- Development can continue on `main` without interruption during the release process
- Release activities are isolated to dedicated branches
- All release-related changes are properly tracked and reviewed through PRs

### Manual Release Process (If Automation Fails)

If the automated workflows fail, you can follow the manual process:

1. **Update the version manually**:
   ```bash
   # Update Cargo.toml
   sed -i 's/version = "OLD_VERSION"/version = "NEW_VERSION"/' Cargo.toml
   
   # Update Cargo.lock
   cargo update -p wassette-mcp-server
   
   # Commit and push
   git add Cargo.toml Cargo.lock
   git commit -m "chore(release): bump version to NEW_VERSION"
   git push origin <branch_name>
   ```

1. **After release is published, update package manifests manually**:
   - Download checksums from the GitHub release page
   - Update `Formula/wassette.rb` with new version and checksums
   - Update `winget/Microsoft.Wassette.yaml` with new version, release date, and checksums
   - Create a PR with these changes

## Releasing Example Component Images

Example WebAssembly components are automatically published to the GitHub Container Registry (GHCR) as OCI artifacts. This allows users to load examples directly from `oci://ghcr.io/microsoft/<example-name>:latest`.

### Automatic Publishing on Main Branch

The [`examples.yml`](.github/workflows/examples.yml) workflow automatically publishes example components when:
- Changes to files in the `examples/**` directory are pushed to the `main` branch
- A pull request targeting the `main` branch modifies files in the `examples/**` directory (build only, no publish)

**Published examples include:**
- `eval-py` - Python expression evaluator
- `fetch-rs` - HTTP fetch example in Rust
- `filesystem-rs` - Filesystem operations in Rust
- `get-weather-js` - Weather API example in JavaScript using OpenWeather API
- `gomodule-go` - Go module information tool
- `time-server-js` - Time server example in JavaScript

**Additional examples in repository (not yet published to OCI registry):**
- `brave-search-rs` - Web search using Brave Search API
- `context7-rs` - Search libraries and fetch documentation via Context7 API
- `get-open-meteo-weather-js` - Weather data via Open-Meteo API (no API key required)

**What the workflow does:**
1. Builds all example components using `just build-examples`
2. Publishes each component to `ghcr.io/microsoft/<component-name>`
3. Tags each component with both:
   - The commit SHA (e.g., `abc1234`)
   - The `latest` tag for main branch pushes
4. Signs all published images using Cosign

### Manual Release of Example Components

To manually publish examples with a specific version tag:

1. **Navigate to the Actions tab**:
   - Go to [Publish Examples workflow](https://github.com/microsoft/wassette/actions/workflows/examples.yml)
   - Click "Run workflow"

2. **Configure the workflow run**:
   - Select the branch (typically `main`)
   - Enter a custom tag (e.g., `v0.4.0`) or leave as default `latest`
   - Click "Run workflow"

3. **Monitor the workflow**:
   - The workflow will build all examples
   - Publish them to GHCR with both the commit SHA and your specified tag
   - Sign all published images

### Using Published Examples

Users can load published examples using the Wassette CLI:

```bash
# Load the latest version
wassette component load oci://ghcr.io/microsoft/fetch-rs:latest

# Load a specific version
wassette component load oci://ghcr.io/microsoft/fetch-rs:v0.4.0
```

### Building Examples Locally

To build examples locally for testing before release:

```bash
# Build all examples in debug mode
just build-examples

# Build all examples in release mode
just build-examples release

# Build a specific example (e.g., fetch-rs)
cd examples/fetch-rs && just build release
```

Each example directory contains:
- A `Justfile` with build commands
- A `README.md` with usage instructions
- Source code and WIT interface definitions

### Adding New Examples

When adding a new example to be published:

1. Create the example in the `examples/` directory
2. Add build instructions to the root `Justfile` in the `build-examples` recipe
3. Add the component to the matrix in `.github/workflows/examples.yml`:
   ```yaml
   - name: my-new-example
     file: my-new-example.wasm
   ```
4. Update this documentation to include the new example in the published list
