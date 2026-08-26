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
- [CI Checks](#ci-checks)
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

### Testing terminal MCP clients

After authenticating Copilot CLI, Claude Code, and Codex CLI separately, run
the end-to-end client harness:

```bash
just test-mcp-clients

# Verify that the harness rejects calls it was instructed not to make
just test-mcp-clients-negative
```

The harness prefers `COPILOT_GITHUB_TOKEN` for Copilot CLI authentication. As
a local convenience, you can instead set `WASSETTE_CLIENTS_COPILOT_TOKEN_FILE` to a readable
token file; otherwise, Copilot CLI uses its normal configured authentication.
The harness is deliberately not wired into CI because it requires three
separately authenticated vendor CLIs and spends model tokens on every run.

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

# Serve only protocol revision 2026-07-28 and later (no session lifecycle),
# and reply with plain JSON instead of a request-scoped event stream
cargo run --bin wassette -- serve --streamable-http --legacy-sessions=false --json-response

# Debug with MCP Inspector
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/mcp --transport http
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/mcp --transport http --method tools/list
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/mcp --transport http --method tools/call --tool-name tool-name --tool-arg param=value
```

Both flags are additive and default to today's behaviour: `--legacy-sessions`
defaults to `true` and `--json-response` to `false`. They can also be set with
`WASSETTE_LEGACY_SESSIONS` and `WASSETTE_JSON_RESPONSE`, or in `config.toml` as
`legacy_sessions` and `json_response`. Clients that negotiate protocol revision
`2026-07-28` or later are served statelessly regardless of these settings; see
the [CLI reference](../reference/cli.md) and the
[operations guide](../deployment/operations.md).

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

## CI Checks

```bash
# Build and test
just ci-build-test
just ci-build-test-ghcr
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

# CI
just ci-build-test      # Run the build and test checks

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
