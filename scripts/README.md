# CHANGELOG Automation Scripts

Scripts for automating CHANGELOG management during releases.

## Scripts

### changelog_utils.py

Python utility for extracting and updating CHANGELOG.md content.

**Extract changelog content:**
```bash
python3 scripts/changelog_utils.py extract <version> [changelog_path]
```

**Update changelog post-release:**
```bash
python3 scripts/changelog_utils.py update <new_version> <prev_version> [changelog_path]
```

**Examples:**
```bash
# Extract v0.4.0 content
python3 scripts/changelog_utils.py extract v0.4.0

# Update CHANGELOG after v0.4.0 release
python3 scripts/changelog_utils.py update v0.4.0 v0.3.0

# Use custom changelog path
python3 scripts/changelog_utils.py extract v0.4.0 docs/CHANGELOG.md
```

### test_changelog_utils.py

Unit tests for changelog_utils module.

**Run tests:**
```bash
cd scripts
python3 test_changelog_utils.py
```

## Release Workflow Integration

The `.github/workflows/release.yml` workflow uses these scripts to:

1. **During Release:** Extract CHANGELOG content for GitHub release notes
2. **Post-Release:** Update CHANGELOG.md automatically (converts `[Unreleased]` to versioned section, adds new `[Unreleased]`, updates comparison links)

## Manual Testing

```bash
# Test extraction
python3 scripts/changelog_utils.py extract v0.3.0

# Test update (backup first!)
cp CHANGELOG.md CHANGELOG.md.backup
python3 scripts/changelog_utils.py update v0.4.0 v0.3.0
git diff CHANGELOG.md
mv CHANGELOG.md.backup CHANGELOG.md  # Restore if needed
```
