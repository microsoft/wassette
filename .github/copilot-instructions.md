# GitHub Copilot Instructions for Wassette

This file provides specific instructions for GitHub Copilot when working on the Wassette project.

## Pull Request Descriptions

**Keep PR descriptions concise and focused:**
- Describe your changes in **at most 3 sentences**
- Focus on the what and why, not implementation details
- If the PR breaks public-facing APIs, use one or two sentences to describe what is broken and how users should adapt

**Example of a good PR description:**
```
This PR adds instrumentation to the MCP server runtime. It enables performance monitoring and debugging of tool execution. The changes are backward compatible with existing configurations.
```

**Example for breaking changes:**
```
This PR refactors the component registry API to support versioning. The `ComponentRegistry::register()` method now requires a version parameter. Existing code should be updated to pass a version string as the second argument.
```

## Additional Guidelines

For comprehensive development guidelines, see:
- **[Developer Guide](../docs/development/getting-started.md)** - Complete development guide, including agent skills
- **[.github/instructions/rust.instructions.md](.github/instructions/rust.instructions.md)** - Rust-specific instructions
- **[.github/instructions/docs.instructions.md](.github/instructions/docs.instructions.md)** - Documentation guidelines
