# CHANGELOG Synchronization Workflow

This document provides a visual overview of how the CHANGELOG synchronization works in the release pipeline.

## Workflow Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                      BEFORE RELEASE                              │
│                                                                   │
│  Developer prepares CHANGELOG.md:                                │
│                                                                   │
│  ## [Unreleased]                                                 │
│                                                                   │
│  ### Added                                                       │
│  - New feature A                                                 │
│  - New feature B                                                 │
│                                                                   │
│  ### Fixed                                                       │
│  - Bug fix C                                                     │
│                                                                   │
│  ## [v0.3.0] - 2025-10-03                                        │
│  ...                                                             │
│                                                                   │
│  [Unreleased]: .../compare/v0.3.0...HEAD                         │
│  [v0.3.0]: .../compare/v0.2.0...v0.3.0                           │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│                     RELEASE WORKFLOW                             │
│                                                                   │
│  1. Tag pushed (v0.4.0)                                          │
│  2. Build binaries for all platforms                             │
│  3. Extract CHANGELOG content:                                   │
│     └→ scripts/extract-changelog.sh v0.4.0                       │
│                                                                   │
│  4. Create GitHub Release:                                       │
│     ├─ Title: v0.4.0                                            │
│     ├─ Body: CHANGELOG content from [Unreleased]                │
│     └─ Assets: Platform binaries                                 │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│                POST-RELEASE AUTOMATION                           │
│                                                                   │
│  1. Get previous version (v0.3.0)                                │
│  2. Update CHANGELOG.md:                                         │
│     └→ scripts/update-changelog-post-release.sh v0.4.0 v0.3.0   │
│                                                                   │
│  3. Commit and push to main:                                     │
│     └→ "chore(release): update CHANGELOG for v0.4.0"            │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│                      AFTER RELEASE                               │
│                                                                   │
│  Updated CHANGELOG.md:                                           │
│                                                                   │
│  ## [Unreleased]                                                 │
│                                                                   │
│  ## [v0.4.0] - 2025-10-16                                        │
│                                                                   │
│  ### Added                                                       │
│  - New feature A                                                 │
│  - New feature B                                                 │
│                                                                   │
│  ### Fixed                                                       │
│  - Bug fix C                                                     │
│                                                                   │
│  ## [v0.3.0] - 2025-10-03                                        │
│  ...                                                             │
│                                                                   │
│  [Unreleased]: .../compare/v0.4.0...HEAD                         │
│  [v0.4.0]: .../compare/v0.3.0...v0.4.0                           │
│  [v0.3.0]: .../compare/v0.2.0...v0.3.0                           │
└─────────────────────────────────────────────────────────────────┘
```

## Key Features

### 1. Single Source of Truth
- CHANGELOG.md is the only place to maintain release notes
- No need to duplicate content in GitHub release UI

### 2. Automatic Updates
- [Unreleased] section → [vX.Y.Z] with release date
- New empty [Unreleased] section added
- Version comparison links updated automatically

### 3. Keep a Changelog Format
- Follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format
- Maintains consistent structure
- Clear categorization (Added, Changed, Deprecated, Removed, Fixed, Security)

## Scripts

### extract-changelog.sh
```bash
./scripts/extract-changelog.sh v0.4.0
```
**Output:** Changelog content for v0.4.0 (without header)

### update-changelog-post-release.sh
```bash
./scripts/update-changelog-post-release.sh v0.4.0 v0.3.0
```
**Actions:**
- Convert [Unreleased] to [v0.4.0] with today's date
- Add new empty [Unreleased] section
- Update comparison links

## Testing

Run integration tests:
```bash
./scripts/test-changelog-scripts.sh
```

## Manual Release Process

If automation fails, manually:

1. Extract changelog content:
   ```bash
   ./scripts/extract-changelog.sh v0.4.0 > release-notes.md
   ```

2. Create GitHub release with the content

3. Update CHANGELOG:
   ```bash
   ./scripts/update-changelog-post-release.sh v0.4.0 v0.3.0
   git add CHANGELOG.md
   git commit -m "chore(release): update CHANGELOG for v0.4.0"
   git push
   ```

## Troubleshooting

### Issue: Previous version not found
**Symptom:** Update CHANGELOG job skips with "Could not find previous tag"
**Solution:** This is expected for the first release. The CHANGELOG won't be updated automatically.

### Issue: CHANGELOG content empty in release notes
**Symptom:** Release notes don't show CHANGELOG content
**Solution:** Ensure [Unreleased] section exists and has content in CHANGELOG.md before release.

### Issue: Merge conflicts in CHANGELOG.md
**Symptom:** Push to main fails due to conflicts
**Solution:** This shouldn't happen in normal workflow. If it does, manually resolve and push.
