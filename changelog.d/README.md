# Changelog Fragments

This directory contains changelog fragments for pending changes that will be included in the next release.

## Format

Each fragment file follows the naming convention: `<pr_number>.<change_type>.md`

Where `<change_type>` is one of:
- `added` - New features
- `changed` - Changes in existing functionality
- `deprecated` - Soon-to-be removed features
- `removed` - Now removed features
- `fixed` - Bug fixes
- `security` - Security vulnerability fixes

## Example

For PR #1234 that adds a new feature:

**File**: `1234.added.md`

**Content**:
```markdown
Added support for new component loading feature
```

## Automated Generation

Changelog fragments are automatically created by the agentic workflow when PRs are opened or reopened. The workflow analyzes the PR title and description to determine the appropriate change type and creates the fragment file.

## Manual Creation

You can also manually create fragment files if needed. Just follow the naming convention and write a concise description of the change.

## Processing

During release preparation, all fragment files in this directory will be consolidated into the main CHANGELOG.md file and removed from this directory.

## Example Files

This directory contains example fragment files (starting with `.example`) that demonstrate the format:
- `.example.added.md` - Example of a feature addition
- `.example.fixed.md` - Example of a bug fix

These example files should be ignored by any consolidation scripts.
