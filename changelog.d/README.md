# Changelog Fragments

This directory contains changelog fragments for pending changes that will be included in the next release. We use the [towncrier](https://towncrier.readthedocs.io/) format for managing changelog fragments.

## Format

Each fragment file follows the towncrier naming convention: `<pr_number>.<type>`

The file can have any extension (`.md`, `.rst`, `.txt`) or no extension. We use `.md` for consistency.

Where `<type>` is one of:
- `feature` - New features
- `bugfix` - Bug fixes
- `doc` - Documentation improvements
- `removal` - Deprecations or removal of public API
- `misc` - Miscellaneous changes not of interest to users

## Example

For PR #1234 that adds a new feature:

**File**: `1234.feature.md`

**Content**:
```markdown
Added support for new component loading feature.
```

## Automated Generation

Changelog fragments are automatically created by the agentic workflow when PRs are opened or reopened. The workflow analyzes the PR title and description to determine the appropriate change type and creates the fragment file.

## Manual Creation

You can also manually create fragment files if needed. Just follow the naming convention and write a concise description of the change.

## Processing

During release preparation, use towncrier to build the changelog:

```bash
towncrier build --version X.Y.Z
```

This will consolidate all fragment files into the CHANGELOG.md file and remove the fragments from this directory.

## Example Files

This directory contains example fragment files (starting with `.example`) that demonstrate the format:
- `.example.feature.md` - Example of a feature addition
- `.example.bugfix.md` - Example of a bug fix

These example files should be ignored by towncrier (they start with `.` so they won't be picked up).
