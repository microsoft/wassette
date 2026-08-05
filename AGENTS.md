# Agent Guide for Wassette

Wassette is an MCP server that runs tools as WebAssembly components with
Wasmtime. This file is the entry point for coding agents and contributors; the
linked documentation and skills are the canonical sources for detailed
guidance.

## Start here

- [Contributing](CONTRIBUTING.md): contribution requirements and release-note
  policy.
- [Developer guide](docs/development/getting-started.md): prerequisites,
  repository layout, build, test, lint, documentation, and local CI workflows.
- [Architecture](docs/design/architecture.md): system design and component
  boundaries.
- [Release process](RELEASE.md): release automation and versioning.

## Agent skills

Focused workflows live under [`.agents/skills/`](.agents/skills/):

| Skill | Use it for |
| --- | --- |
| `build-and-test` | Building the workspace, examples, and test components |
| `rust-code-style` | Rust design, formatting, Clippy, and dependency checks |
| `copyright-headers` | Applying the required headers to Rust files |
| `mcp-inspector-testing` | Testing server behavior through MCP Inspector |
| `documentation` | Writing, building, and previewing the mdBook |
| `pull-request` | Writing concise, user-facing pull requests |

Agents that support the Agent Skills standard should invoke the relevant skill.
Other agents should read its `SKILL.md` before performing that workflow.

## Repository requirements

- Prefer the repository `just` recipes over ad-hoc commands; several recipes
  build required WebAssembly components before the Rust workspace.
- Format Rust with `cargo +nightly fmt`, lint with
  `cargo clippy --workspace`, and check unused dependencies with
  `cargo machete`.
- Every Rust source file must begin with the Microsoft copyright header; use
  `./scripts/copyright.sh`.
- Validate server-facing behavior with MCP Inspector when the change affects
  MCP tools, resources, prompts, transports, or runtime behavior.
- Release notes are generated from pull request titles. Write a clear,
  user-facing title; do not create or update a `CHANGELOG.md`.

Do not duplicate these rules in tool-specific instruction files. Update the
canonical document or skill instead, then link to it from this index.
