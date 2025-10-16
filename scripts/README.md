# CHANGELOG Automation Scripts

This directory contains scripts for automating CHANGELOG management during the release process.

## Scripts

### extract-changelog.sh

Extracts the changelog content for a specific version from `CHANGELOG.md`.

**Usage:**
```bash
./scripts/extract-changelog.sh <version>
```

**Example:**
```bash
./scripts/extract-changelog.sh v0.4.0
```

**Output:** Prints the changelog content for the specified version (without the version header).

**Use in CI:** This script is used by the release workflow to populate GitHub release notes with the appropriate changelog content.

### update-changelog-post-release.sh

Updates `CHANGELOG.md` after a release by:
1. Converting the `[Unreleased]` section to the new version with release date
2. Adding a new empty `[Unreleased]` section at the top
3. Updating version comparison links

**Usage:**
```bash
./scripts/update-changelog-post-release.sh <version> <previous-version>
```

**Example:**
```bash
./scripts/update-changelog-post-release.sh v0.4.0 v0.3.0
```

**Use in CI:** This script is used by the release workflow after creating a GitHub release to automatically update the CHANGELOG.

### test-changelog-scripts.sh

Integration test for the CHANGELOG automation scripts. Runs a complete test cycle:
1. Extract changelog for an existing version
2. Update changelog to create a new version
3. Verify the updated CHANGELOG format
4. Extract changelog for the new version

**Usage:**
```bash
./scripts/test-changelog-scripts.sh
```

## Environment Variables

Both extraction and update scripts support the `CHANGELOG_FILE` environment variable to specify a custom CHANGELOG path:

```bash
CHANGELOG_FILE=/path/to/CHANGELOG.md ./scripts/extract-changelog.sh v0.4.0
```

If not specified, defaults to `CHANGELOG.md` in the current directory.

## Release Workflow Integration

The release workflow (`.github/workflows/release.yml`) integrates these scripts:

1. **Release Job:**
   - Extracts changelog content using `extract-changelog.sh`
   - Uses the content as GitHub release notes
   - Creates release with binaries and formatted changelog

2. **Update CHANGELOG Job:**
   - Runs after successful release
   - Determines the previous version from git tags
   - Updates CHANGELOG using `update-changelog-post-release.sh`
   - Commits and pushes changes back to main branch

## Manual Testing

To test the scripts locally before a release:

```bash
# Test extraction
./scripts/extract-changelog.sh v0.3.0

# Test update (creates a backup first!)
cp CHANGELOG.md CHANGELOG.md.backup
./scripts/update-changelog-post-release.sh v0.4.0 v0.3.0
# Review changes
git diff CHANGELOG.md
# Restore if needed
mv CHANGELOG.md.backup CHANGELOG.md
```

Or use the test script:
```bash
./scripts/test-changelog-scripts.sh
```
