# Developer Guide: Getting Started

Quick guide for contributing to Wassette.

## Table of Contents

- [Prerequisites](#prerequisites)
- [Getting the Source Code](#getting-the-source-code)
- [Building Wassette](#building-wassette)
- [Running Tests](#running-tests)
- [Code Formatting and Linting](#code-formatting-and-linting)
- [Running the Development Server](#running-the-development-server)
- [Building Documentation](#building-documentation)
- [Development Workflow](#development-workflow)
- [Agent Skills](#agent-skills)
- [CI/CD and Docker](#cicd-and-docker)
- [Project Structure](#project-structure)
- [Contributing](#contributing)

## Prerequisites

**Required:**

```bash
# Install Rust (1.97.1+)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Install nightly for formatting
rustup install nightly

# Add WASI target
rustup target add wasm32-wasip2

# Install Just (macOS)
brew install just

# Install Just (Linux or other)
cargo install just
```

**Optional:**

```bash
# For building docs
cargo install mdbook mdbook-mermaid

# For debugging (Node.js from nodejs.org)
```

## Getting the Source Code

```bash
git clone https://github.com/microsoft/wassette.git
cd wassette
```

## Building Wassette

```bash
# View all available commands
just --list

# Debug build
just build

# Release build
just build release

# Build example components
just build-examples
just build-examples release
```

## Running Tests

```bash
# Run all tests
just test

# Build test components separately
just build-test-components
just clean-test-components

# Run specific tests
cargo test --workspace
cargo test -p wassette
cargo test test_name
cargo test -- --nocapture
```

## Code Formatting and Linting

```bash
# Format code (required before commit)
cargo +nightly fmt

# Lint
cargo clippy --workspace
cargo clippy --workspace --fix

# Add copyright headers
./scripts/copyright.sh
```

## Running the Development Server

```bash
# Start the Streamable HTTP server (127.0.0.1:9001/mcp)
just run

# Custom log level (error, warn, info, debug, trace)
just run RUST_LOG='debug'

# Run with example components
just run-filesystem
just run-fetch-rs
just run-get-weather  # Requires OPENWEATHER_API_KEY

# Debug with MCP Inspector
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/mcp --transport http
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/mcp --transport http --method tools/list
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/mcp --transport http --method tools/call --tool-name tool-name --tool-arg param=value
```

Validate server-facing changes with the MCP Inspector before committing: list
the tools and call the ones whose behavior changed. Capture the output when it
helps reviewers understand the change.

## Building Documentation

```bash
# Build docs
just docs-build

# Serve with live reload
just docs-watch

# Serve and open in browser
just docs-serve
```

Docs available at `http://localhost:3000/overview.html`. Navigate directly to specific pages when developing locally.

## Development Workflow

```bash
# 1. Create branch
git checkout -b feature/your-feature-name

# 2. Make changes, then:
cargo +nightly fmt
cargo clippy --workspace
just build
just test

# 3. Commit and push
git add .
git commit -m "Your descriptive commit message"
git push origin feature/your-feature-name

# 4. Create Pull Request on GitHub
#    - Use a clear, user-facing title (it becomes the release note entry)
```

**Best Practices:**
- Single responsibility per function/struct
- DRY (Don't Repeat Yourself)
- Clear, descriptive names
- Add unit tests for public functions
- Keep it simple
- Write idiomatic Rust (passes `cargo clippy`)
- Use `anyhow` for error handling
- Use `Arc`/`Mutex` for thread safety
- Prefer `&str` over `String` when possible

## Agent Skills

The repository ships focused **agent skills** under [`.agents/skills/`](https://github.com/microsoft/wassette/tree/main/.agents/skills) that capture common development workflows for AI agents and are useful reading for contributors. Each skill is a self-contained `SKILL.md`; agents that support skills invoke them by name, otherwise read the file directly.

| Skill | Use it to |
| ----- | --------- |
| `build-and-test` | Build the workspace and example components, and run the test suite |
| `rust-code-style` | Write idiomatic Rust and run `fmt`, `clippy`, and `machete` |
| `copyright-headers` | Add the required Microsoft copyright header to Rust files |
| `mcp-inspector-testing` | Run the server and validate changes with the MCP Inspector |
| `documentation` | Build, serve, and write the mdBook documentation |
| `pull-request` | Write a concise, focused pull request description |
| `ci-local` | Reproduce CI locally with the Docker-based recipes |

## CI/CD and Docker

```bash
# Run CI locally with Docker
just ci-local

# Build and test (no Docker)
just ci-build-test
just ci-build-test-ghcr

# Docker commands
just ci-cache-info
just ci-clean
```

## Project Structure

```
wassette/
├── crates/                 # All crates live here
│   ├── wassette-mcp-server/ # Main MCP server binary (src/, build.rs, tests/)
│   ├── component2json/    # Component to JSON converter
│   ├── mcp-server/        # MCP server implementation
│   ├── policy/            # Policy management
│   └── wassette/          # Core Wassette library
├── examples/               # Example WebAssembly components
├── docs/                   # Documentation (mdBook)
└── Cargo.toml             # Workspace configuration
```

**Key Crates:**
- `wassette-mcp-server`: Main MCP server binary
- `wassette`: Core library with component loading
- `component2json`: Component schema converter
- `mcp-server`: MCP protocol implementation
- `policy`: Permission management

## Contributing

Before contributing:
1. Read [CONTRIBUTING.md](../../CONTRIBUTING.md)
2. Check [GitHub Issues](https://github.com/microsoft/wassette/issues)
3. Join [Discord](https://discord.gg/microsoft-open-source) (#wassette channel)
4. Follow the development workflow above
5. Ensure tests pass
6. Update docs if needed

CLA required for contributions. This project follows the [Microsoft Open Source Code of Conduct](https://opensource.microsoft.com/codeofconduct/).

## Additional Resources

- [Architecture](../design/architecture.md)
- [Permission System](../design/permission-system.md)
- [Component Schemas](../design/component2json-structured-output.md)
- [CLI Reference](../reference/cli.md)
- [FAQ](../faq.md)
- [Installation Guide](../installation.md)
- [MCP Clients Setup](../mcp-clients.md)

## Quick Reference

```bash
# Development
just build              # Debug build
just build release      # Release build
just test               # Run tests
just run                # Start MCP server
cargo +nightly fmt      # Format
cargo clippy            # Lint

# Documentation
just docs-serve         # Serve docs locally
just docs-build         # Build docs

# CI/Docker
just ci-local           # Run CI locally

# Utilities
./scripts/copyright.sh  # Add copyright headers
just clean              # Clean artifacts
```

**Environment Variables:**
- `RUST_LOG`: Log level (`info`, `debug`, `trace`)
- `OPENWEATHER_API_KEY`: For weather example
- `GITHUB_TOKEN`: For CI/GHCR tests

## Getting Help

- [GitHub Issues](https://github.com/microsoft/wassette/issues)
- [GitHub Discussions](https://github.com/microsoft/wassette/discussions)
- [Discord](https://discord.gg/microsoft-open-source) (#wassette channel)

## License

MIT License. See [LICENSE](../../LICENSE) for details.
