# Release Process

This document describes the process for releasing new versions of the Wassette project.

## Release.yml overview

The release process is automated using GitHub Actions, specifically the [`release.yml`](.github/workflows/release.yml) workflow. You run it manually from the Actions tab (or with `gh workflow run release.yml -f version=X.Y.Z`) and it creates the `vX.Y.Z` tag as it publishes the release, so there is no separate tagging workflow. The workflow uses a matrix to compile `wassette` for different platforms on native runners and uses `sccache` to speed up compilation. The compiled binaries are then uploaded as release assets.

### Release Notes

The release workflow generates release notes automatically. When the release is
created, it calls GitHub's [automatically generated release
notes](https://docs.github.com/en/repositories/releasing-projects-on-github/automatically-generated-release-notes)
API, which groups the pull requests merged since the previous tag into the
categories defined in [`.github/release.yml`](.github/release.yml). There is no
`CHANGELOG.md` file to maintain: write clear pull request titles and apply
category labels (`enhancement`, `bug`, `documentation`, `security`,
`breaking-change`, or `skip-changelog`) and the release notes take care of
themselves.

## Release Versioning

Wassette uses semantic versioning. All releases follow the format `vX.Y.Z`, where X is the major version, Y is the minor version, and Z is the patch version.

## Tagging Strategy

- All release tags are prefixed with v, e.g., v0.10.0.
- Tags are created automatically by the release workflow when it publishes the release; you never push a tag by hand.
- The tag points at the `main` commit that carries the matching `Cargo.toml` version bump.
- Patch releases increment the Z portion, e.g., v0.6.1 → v0.6.2.
- Minor releases increment the Y portion, e.g., v0.9.0 → v0.10.0.

## Steps to Cut a Release

The release process is now largely automated through GitHub Actions workflows and uses a release branch strategy to prevent blocking development on main. Follow these steps:

The workflows use `GITHUB_TOKEN`; no separate `RELEASE_TOKEN` secret is
required. Enable **Allow GitHub Actions to create and approve pull requests**
in the repository Actions settings.

1. **Prepare the release**: Trigger the `prepare-release` workflow to create a PR that bumps the version.

   1. Go to the [Actions tab](https://github.com/microsoft/wassette/actions/workflows/prepare-release.yml)
   1. Click "Run workflow"
   1. Enter the new version number without the `v` prefix (e.g., `0.4.0` or `0.4.0-rc1`)
   1. Click "Run workflow"

   Alternatively, dispatch it with the project Justfile:

   ```bash
   just prepare-release 0.4.0-rc1
   ```

   This will automatically:
   - Create a release branch such as `release/vX.Y.Z` or `release/vX.Y.Z-rc1`
   - Update the version in `Cargo.toml`
   - Update `Cargo.lock`
   - Create a pull request to merge the release branch into main

1. **Review and merge the version bump PR**: The workflow will create a pull request with the version changes. Review and merge this PR into the main branch.

   **Important**: The release branch can be deleted after the version bump PR is merged.

1. **Run the release workflow**: Once the version bump PR is merged to `main`, trigger the `release` workflow to build and publish the release.

   1. Go to the [Actions tab](https://github.com/microsoft/wassette/actions/workflows/release.yml)
   1. Click "Run workflow"
   1. Enter the same version you prepared, without the `v` prefix (e.g., `0.4.0` or `0.4.0-rc1`)
   1. Click "Run workflow"

   Alternatively, dispatch it from the command line:

   ```bash
   gh workflow run release.yml -f version=0.4.0
   ```

   The workflow validates that the version matches `Cargo.toml`, then creates the `v0.4.0` tag as part of publishing the release.

1. **Monitor the release workflow**: The `release.yml` run:
   - Builds binaries for all platforms (Linux, macOS, Windows; AMD64 and ARM64)
   - Generates release notes automatically from the pull requests merged since the previous tag
   - Creates a draft GitHub release, uploads and verifies all compiled binaries, then publishes the immutable release (creating the `vX.Y.Z` tag) with the generated notes
   - Publishes the example components with the release version and `latest` tags
   - Publishes versioned documentation for the release
   - Monitor the workflow progress in the [Actions tab](https://github.com/microsoft/wassette/actions)

   If a run fails after the version bump is merged, simply dispatch `release.yml` again with the same version.

1. **Package manifests are updated automatically**: After the release is published, the `update-package-manifests` workflow will automatically:
   - Download all release assets
   - Compute SHA256 checksums
   - Update `Formula/wassette.rb` (Homebrew)
   - Update `winget/Microsoft.Wassette.yaml` (WinGet)
   - Create a pull request with these updates

   Simply review and merge the automatically created PR to complete the release process.

## Dry Run / Test Releases

The release process supports dry run or test releases for validating the build and release process without triggering package manifest updates. This is useful for:
- Testing the release workflow with pre-release versions
- Creating release candidates for testing
- Publishing test builds for validation before the official release

### How to Create a Dry Run Release

To publish a versioned prerelease, run the Prepare Release workflow with a
hyphen suffix such as `0.4.0-rc1`, merge its version bump PR, then dispatch the
release workflow with the same version.

To test the current package version without changing `Cargo.toml`, dispatch the
release workflow with a hyphen suffix on the current version (e.g., `-test1`,
`-rc1`, `-alpha`, `-beta`); the release job accepts it because the base version
still matches `Cargo.toml`:

```bash
gh workflow run release.yml -f version=0.3.4-test1
```

When the version contains a hyphen, the release workflow builds binaries for all platforms and creates a GitHub release marked as "Pre-release". It does not update package manifests, publish example components, or deploy versioned documentation.

### Dry Run Version Examples

Common prerelease version patterns:
- `0.3.4-test1`, `0.3.4-test2` - Test releases
- `0.4.0-rc1`, `0.4.0-rc2` - Release candidates
- `0.4.0-alpha`, `0.4.0-beta` - Pre-release versions
- `0.3.4-hotfix-test` - Testing a hotfix

### Deleting Dry Run Releases

After validation, you can delete the dry run release and its tag:

```bash
# Delete the GitHub release and its tag in one step
gh release delete v0.3.4-test1 --cleanup-tag --yes
```

## Release Branch Strategy

The release process uses a dedicated release branch to keep the version bump off `main` until it is reviewed:

1. **Release branch creation**: When the `prepare-release` workflow is triggered, it creates a branch named `release/vX.Y.Z` (e.g., `release/v0.4.0`) containing the `Cargo.toml` and `Cargo.lock` version bump.

2. **Version bump PR**: The branch is opened as a pull request against `main`. After it is merged, you dispatch the release workflow to build, tag, and publish the release.

3. **Branch cleanup**: Once the version bump PR is merged, the release branch has served its purpose and can be safely deleted.

This strategy ensures that:
- Development can continue on `main` without interruption during the release process
- The version bump is reviewed through a normal pull request

## Manual Release Process (If Automation Fails)

If the automated workflows fail, you can drive the release by hand:

1. **Bump the version manually** (if the `prepare-release` workflow fails):
   ```bash
   # Update Cargo.toml
   sed -i 's/version = "OLD_VERSION"/version = "NEW_VERSION"/' Cargo.toml

   # Update Cargo.lock
   cargo update -p wassette-mcp-server --precise NEW_VERSION

   # Commit on a release branch and open a PR to main
   git checkout -b release/vNEW_VERSION
   git add Cargo.toml Cargo.lock
   git commit -m "chore(release): bump version to NEW_VERSION"
   git push origin release/vNEW_VERSION
   ```

1. **Create the release manually** (if the `release` workflow fails): after the
   version bump is on `main`, build the binaries you need, then create the
   release and tag in a single step. This creates the `v<version>` tag on the
   current `main` commit:
   ```bash
   git checkout main
   git pull origin main

   gh release create v<version> \
     --target main \
     --title v<version> \
     --generate-notes \
     ./release-assets/*
   ```

1. **If the release workflow did not dispatch package updates, update package manifests manually**:
   
   1. Go to the [Actions tab](https://github.com/microsoft/wassette/actions/workflows/update-package-manifests.yml)
   1. Click "Run workflow"
   1. Enter the release tag name (e.g., `v0.4.0`)
   1. Click "Run workflow"
   1. The workflow will automatically create a PR with the updated manifests

## Releasing Example Component Images

Example WebAssembly components are automatically published to the GitHub Container Registry (GHCR) as OCI artifacts. This allows users to load examples directly from `oci://ghcr.io/microsoft/<example-name>:latest`.

### Automatic Publishing on Main Branch

The [`examples.yml`](.github/workflows/examples.yml) workflow automatically publishes example components when:
- Changes to files in the `examples/**` directory are pushed to the `main` branch
- A pull request targeting the `main` branch modifies files in the `examples/**` directory (build only, no publish)
- The release workflow dispatches it with a version tag after publishing the binaries

**Published examples include:**
- `eval-py` - Python expression evaluator
- `fetch-rs` - HTTP fetch example in Rust
- `filesystem-rs` - Filesystem operations in Rust
- `get-weather-js` - Weather API example in JavaScript using OpenWeather API
- `gomodule-go` - Go module information tool
- `memory-js` - Knowledge graph memory server in JavaScript
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
wassette component load oci://ghcr.io/microsoft/fetch-rs:0.4.0
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
