# Release Pipeline Failure Analysis for v0.3.3

## Executive Summary

The release pipeline for v0.3.3 failed because the `CHANGELOG.md` file does not contain a section for version v0.3.3. The automated release workflow requires a corresponding changelog entry for each version being released, but the repository is in an intermediate state where:

- **Cargo.toml version**: `0.3.3` (ready for release)
- **CHANGELOG.md versions**: Only `v0.3.0`, `v0.2.0`, and `v0.1.0` (missing v0.3.3)

## Problem Details

### Error Message
```
Error: Version v0.3.3 not found in CHANGELOG.md
Error: Process completed with exit code 1.
```

### Technical Explanation

The release workflow (`.github/workflows/release.yml`) performs the following steps:

1. **Extract changelog content** (line 149-155):
   ```bash
   CHANGELOG_CONTENT=$(python3 scripts/changelog_utils.py extract "${{ github.ref_name }}")
   ```

2. **Use extracted content** for GitHub release notes (line 157-196)

The `changelog_utils.py` script searches for a section header matching the pattern:
```markdown
## [v0.3.3] - YYYY-MM-DD
```

Since this section doesn't exist in CHANGELOG.md, the extraction fails.

## Current Repository State

### CHANGELOG.md Structure
```markdown
## [Unreleased]
### Added
- [... extensive list of unreleased features ...]

## [v0.3.0] - 2025-10-03
## [v0.2.0] - 2025-08-05
## [v0.1.0] - 2025-08-05
```

### Version in Cargo.toml
```toml
version = "0.3.3"
```

## Root Cause

The repository is missing the standard release preparation step that should happen **before** creating and pushing a release tag. According to the documented release process in `RELEASE.md`, the workflow should be:

1. **Prepare CHANGELOG** - Ensure `[Unreleased]` section contains all changes for the upcoming release
2. **Trigger prepare-release workflow** - This creates a PR to bump version in Cargo.toml
3. **Merge version bump PR**
4. **Create and push release tag** - This triggers the release workflow

The current state suggests that steps 2-3 were completed (version bumped to 0.3.3), but either:
- Step 1 was incomplete (CHANGELOG not finalized), OR
- A tag was created prematurely before the release workflow could process the CHANGELOG

## Resolution Options

### Option 1: Complete the Release for v0.3.3 (Recommended)

This option prepares the changelog and completes the v0.3.3 release as intended.

**Steps:**
1. The `[Unreleased]` section contains all changes for v0.3.3
2. **DO NOT** manually convert `[Unreleased]` to `[v0.3.3]` in CHANGELOG.md
3. Create and push the v0.3.3 tag:
   ```bash
   git checkout main
   git pull origin main
   git tag -s v0.3.3 -m "Release v0.3.3"
   git push origin v0.3.3
   ```
4. The release workflow will automatically:
   - Extract the `[Unreleased]` content (it should be updated to handle this)
   - Create the GitHub release
   - Update CHANGELOG.md by converting `[Unreleased]` to `[v0.3.3]` with date
   - Add a new empty `[Unreleased]` section

**Issue:** The current implementation may not support extracting from `[Unreleased]`. This would require updating `changelog_utils.py`.

### Option 2: Fix the Automation to Support Unreleased Content

Update `changelog_utils.py` to support extracting from `[Unreleased]` when the version doesn't exist yet.

**Implementation:**
```python
def extract_changelog_content(changelog_path: Path, version: str) -> str:
    # ... existing code ...
    
    # If version not found, try to extract from [Unreleased]
    if not found:
        # Try to extract from [Unreleased] section instead
        for i, line in enumerate(lines):
            if line == '## [Unreleased]':
                found = True
                continue
            if found:
                if line.startswith('## [') or line.startswith('[Unreleased]:'):
                    break
                output_lines.append(line)
        
        if not found:
            raise ValueError(f"Version {version} not found in {changelog_path}")
    
    # ... rest of code ...
```

This allows the release workflow to extract unreleased content when creating a new release.

### Option 3: Skip v0.3.3 and Prepare for v0.3.4

If there's no urgent need for v0.3.3, increment to v0.3.4:

**Steps:**
1. Update version in Cargo.toml to `0.3.4`
2. Ensure CHANGELOG.md `[Unreleased]` section is complete
3. Follow the standard release process from RELEASE.md

**Cons:**
- Skips a version number
- Creates confusion about what happened to v0.3.3
- May have implications if v0.3.3 was already referenced elsewhere

### Option 4: Manual CHANGELOG Update (Quick Fix)

Manually update CHANGELOG.md to include v0.3.3 section:

**Steps:**
1. Edit CHANGELOG.md:
   ```markdown
   ## [Unreleased]

   ## [v0.3.3] - 2025-10-28
   ### Added
   - [move content from Unreleased section here]
   ```
2. Update comparison links at bottom:
   ```markdown
   [Unreleased]: https://github.com/microsoft/wassette/compare/v0.3.3...HEAD
   [v0.3.3]: https://github.com/microsoft/wassette/compare/v0.3.0...v0.3.3
   ```
3. Commit and push to main
4. Create and push v0.3.3 tag

**Cons:**
- Bypasses automation
- Requires manual maintenance
- Doesn't fix the underlying process issue

## Recommended Solution

**I recommend Option 2 (Fix the Automation)** combined with completing the v0.3.3 release.

### Rationale:
1. **Improves the release process**: Makes it more resilient and user-friendly
2. **Follows Keep a Changelog principles**: The `[Unreleased]` section should contain upcoming release content
3. **Maintains automation**: The post-release automation can still update the CHANGELOG
4. **Prevents future issues**: Handles the common case where changes are in `[Unreleased]` before tagging

### Implementation Plan:
1. Update `changelog_utils.py` to extract from `[Unreleased]` as a fallback
2. Add tests for this new behavior
3. Verify the fix works
4. Create and push the v0.3.3 tag to complete the release

## Process Improvement Recommendations

1. **Pre-release validation**: Add a GitHub Actions check that validates:
   - Version in Cargo.toml matches the tag being created
   - CHANGELOG.md has content for the version OR has content in `[Unreleased]`

2. **Documentation clarification**: Update RELEASE.md to explicitly state:
   - Changes should accumulate in `[Unreleased]` section
   - The release workflow automatically converts `[Unreleased]` to the versioned section
   - Manual CHANGELOG updates are not required before tagging

3. **Prepare-release workflow enhancement**: The prepare-release workflow could:
   - Validate that `[Unreleased]` section is not empty
   - Provide a warning if trying to release with empty unreleased content

## Testing Performed

1. ✅ Verified `changelog_utils.py` tests pass (8/8 tests)
2. ✅ Confirmed v0.3.3 not in CHANGELOG.md
3. ✅ Confirmed Cargo.toml version is 0.3.3
4. ✅ Verified the release workflow logic in `.github/workflows/release.yml`
5. ✅ Understood the complete automation chain

## Conclusion

The immediate issue is a mismatch between code version (0.3.3) and CHANGELOG entries (only v0.3.0). The best path forward is to:

1. **Short-term**: Update the changelog extraction logic to support extracting from `[Unreleased]`
2. **Complete v0.3.3 release**: Once the fix is in place, tag and release v0.3.3
3. **Long-term**: Enhance release validation and documentation to prevent this scenario

This approach maintains the existing content in the repository, completes the intended v0.3.3 release, and improves the release automation for future releases.
