---
name: pull-request
description: Write concise, focused pull request descriptions for Wassette — at most three sentences on the what and why, with an explicit note on any breaking public-API change and how users adapt. Use when opening or editing a Wassette pull request.
allowed-tools: Bash, Read, Write, Edit, Glob, Grep
---

# pull-request skill

Keep Wassette pull request descriptions concise and focused.

## Rules

- Describe the change in **at most three sentences**.
- Focus on the *what* and *why*, not implementation details.
- If the PR breaks a public-facing API, use one or two sentences to state what
  breaks and how users should adapt.

## Example

```
This PR adds instrumentation to the MCP server runtime. It enables performance
monitoring and debugging of tool execution. The changes are backward compatible
with existing configurations.
```

## Breaking-change example

```
This PR refactors the component registry API to support versioning. The
ComponentRegistry::register() method now requires a version parameter. Existing
code should pass a version string as the second argument.
```

## Before opening

- Use a clear, user-facing title because GitHub generates release notes from
  merged pull request titles.
- Most contributions require a Contributor License Agreement; the CLA bot will
  tell you if one is needed. See `CONTRIBUTING.md`.
