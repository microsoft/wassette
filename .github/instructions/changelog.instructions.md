---
applyTo: "**/*"
---

# Changelog Management

Any Pull Request that makes changes to the codebase **MUST** update the `CHANGELOG.md` file following the guiding principles of [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## Guiding Principles

Changelogs are for humans, not machines:
- There should be an entry for every single version
- The same types of changes should be grouped
- Versions and sections should be linkable
- The latest version comes first
- The release date of each version is displayed
- Mention whether you follow Semantic Versioning

## Types of Changes

When updating the `CHANGELOG.md`, categorize your changes under the appropriate section in the `[Unreleased]` section:

- **Added** for new features
- **Changed** for changes in existing functionality
- **Deprecated** for soon-to-be removed features
- **Removed** for now removed features
- **Fixed** for any bug fixes
- **Security** in case of vulnerabilities

## How to Update the Changelog

1. **Always update the `[Unreleased]` section** - never modify existing version entries
2. **Add your entry under the appropriate type** (Added, Changed, etc.)
3. **Include a brief description** of what was changed
4. **Reference the Pull Request** number using the format `([#123](https://github.com/microsoft/wassette/pull/123))`
5. **Use present tense** and be concise but descriptive

## Example Entry Format

```markdown
## [Unreleased]

### Added
- Support for new component loading feature ([#123](https://github.com/microsoft/wassette/pull/123))

### Fixed
- Issue with component lifecycle management ([#124](https://github.com/microsoft/wassette/pull/124))
```

## Important Notes

- **Breaking changes** should be clearly marked with `**BREAKING CHANGE**:` prefix
- Documentation-only changes typically don't require changelog entries unless they significantly impact user experience
- Ensure your changelog entry is placed under the correct type section
- The changelog follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html)

This project's changelog format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).