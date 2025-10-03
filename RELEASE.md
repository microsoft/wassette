# Release Process

This document describes the process for releasing new versions of the Wassette project.

## Release.yml overview

The release process is automated using GitHub Actions, specifically the [`release.yml`](.github/workflows/release.yml) workflow. This workflow is triggered when a new tag is pushed to the repository. Once triggered, the workflow uses a matrix to compile `wassette` for different platforms on native runners and uses `sccache` to speed up the compilation process by caching previous builds. The compiled binaries are then uploaded as artifacts to the release.

## Release Versioning

Wassette uses semantic versioning. All releases follow the format `vX.Y.Z`, where X is the major version, Y is the minor version, and Z is the patch version.

## Tagging Strategy

- All release tags are prefixed with v, e.g., v0.10.0.
- Tags are created on the default branch (typically main), or on a release branch when applicable.
- Patch releases increment the Z portion, e.g., v0.6.1 → v0.6.2.
- Minor releases increment the Y portion, e.g., v0.9.0 → v0.10.0.

## Steps to Cut a Release

The release process is now largely automated through GitHub Actions workflows. Follow these steps:

1. **Prepare the release**: Trigger the `prepare-release` workflow to create a PR that bumps the version.

   1. Go to the [Actions tab](https://github.com/microsoft/wassette/actions/workflows/prepare-release.yml)
   1. Click "Run workflow"
   1. Enter the new version number (without `v` prefix, e.g., `0.4.0`)
   1. Click "Run workflow"

   This will automatically:
   - Update the version in `Cargo.toml`
   - Update `Cargo.lock`
   - Create a pull request with these changes

1. **Review and merge the version bump PR**: The workflow will create a pull request with the version changes. Review and merge this PR into the main branch.

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
   - Creates a GitHub release with all compiled binaries
   - Monitor the workflow progress in the [Actions tab](https://github.com/microsoft/wassette/actions)

1. **Package manifests are updated automatically**: After the release is published, the `update-package-manifests` workflow will automatically:
   - Download all release assets
   - Compute SHA256 checksums
   - Update `Formula/wassette.rb` (Homebrew)
   - Update `winget/Microsoft.Wassette.yaml` (WinGet)
   - Create a pull request with these updates

   Simply review and merge the automatically created PR to complete the release process.

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
