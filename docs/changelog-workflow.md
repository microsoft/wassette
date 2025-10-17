# CHANGELOG Synchronization Workflow

Visual overview of automatic CHANGELOG synchronization during releases.

## Workflow

```
┌─────────────────────────────────────────────────────────────────┐
│                      BEFORE RELEASE                              │
│                                                                   │
│  ## [Unreleased]                                                 │
│  ### Added                                                       │
│  - New feature A                                                 │
│  - New feature B                                                 │
│  ### Fixed                                                       │
│  - Bug fix C                                                     │
│                                                                   │
│  [Unreleased]: .../compare/v0.3.0...HEAD                         │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│                     RELEASE WORKFLOW                             │
│                                                                   │
│  1. Tag pushed (v0.4.0)                                          │
│  2. Build binaries                                               │
│  3. Extract CHANGELOG content                                    │
│  4. Create GitHub Release with CHANGELOG as release notes        │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│                POST-RELEASE AUTOMATION                           │
│                                                                   │
│  1. Verify release job succeeded                                 │
│  2. Get previous version                                         │
│  3. Update CHANGELOG.md:                                         │
│     - Convert [Unreleased] → [v0.4.0] with date                 │
│     - Add new empty [Unreleased] section                         │
│     - Update comparison links                                    │
│  4. Commit and push to main                                      │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│                      AFTER RELEASE                               │
│                                                                   │
│  ## [Unreleased]                                                 │
│                                                                   │
│  ## [v0.4.0] - 2025-10-16                                        │
│  ### Added                                                       │
│  - New feature A                                                 │
│  - New feature B                                                 │
│  ### Fixed                                                       │
│  - Bug fix C                                                     │
│                                                                   │
│  [Unreleased]: .../compare/v0.4.0...HEAD                         │
│  [v0.4.0]: .../compare/v0.3.0...v0.4.0                           │
└─────────────────────────────────────────────────────────────────┘
```

## Key Features

- **Single Source:** CHANGELOG.md is the only place to maintain release notes
- **Automatic Updates:** Post-release automation handles all CHANGELOG updates
- **Consistent Format:** Follows Keep a Changelog format throughout

## Scripts

**Extract content:**
```bash
python3 scripts/changelog_utils.py extract v0.4.0
```

**Update post-release:**
```bash
python3 scripts/changelog_utils.py update v0.4.0 v0.3.0
```

**Run tests:**
```bash
python3 scripts/test_changelog_utils.py
```

## Manual Release Process

If automation fails:

1. Extract changelog: `python3 scripts/changelog_utils.py extract v0.4.0 > notes.md`
2. Create GitHub release with content from notes.md
3. Update CHANGELOG: `python3 scripts/changelog_utils.py update v0.4.0 v0.3.0`
4. Commit and push changes

## Troubleshooting

**Previous version not found:** Expected for first release. CHANGELOG won't be updated automatically.

**Empty release notes:** Ensure `[Unreleased]` section has content before release.

**Merge conflicts:** Shouldn't happen in normal flow. Manually resolve if it occurs.
