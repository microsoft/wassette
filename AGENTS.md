# AI Agent Development Guide for Wassette

This guide provides comprehensive instructions for AI agents (including GitHub Copilot, Claude Code, Cursor, and others) working on the Wassette project.

## Project Overview

Wassette is a **Model Context Protocol (MCP) server** implementation that runs Tools as **WebAssembly (Wasm) components** using the Wasmtime engine. It provides a security-oriented runtime that enables AI agents to execute tools with browser-grade isolation.

### Key Features
- **Convenience**: Easy extension of AI agents with new tools via MCP
- **Reusability**: Generic Wasm Components with no MCP-specific dependencies
- **Security**: Built on Wasmtime security sandbox with fine-grained permissions

### Architecture
Wassette connects MCP clients (VS Code, Claude Code, Cursor, Gemini CLI, etc.) to WebAssembly Components. For detailed architecture information, see `docs/design/architecture.md`.

## Development Environment Setup

### Prerequisites
- **Rust**: Latest stable version (nightly required for formatting)
- **Cargo**: Rust's package manager
- **Just**: Command runner for development tasks
- **mdbook**: For building documentation (optional)
- **Node.js**: For running MCP inspector (debugging)

### Building the Project

```bash
# Build in debug mode (default)
just build

# Build in release mode
just build release

# Build example components
just build-examples

# Clean build artifacts
just clean
```

### Running Tests

```bash
# Run all tests (automatically builds test components)
just test

# Clean test component artifacts
just clean-test-components

# Pre-build test components separately
just build-test-components
```

Tests include both unit tests and documentation tests. The test suite automatically builds required WebAssembly components before running.

## Code Style and Best Practices

### Rust Code Guidelines

1. **Single Responsibility Principle**: Each function and struct should have a single, well-defined purpose
2. **DRY (Don't Repeat Yourself)**: Extract common logic into reusable functions or modules
3. **Descriptive Naming**: Use clear, descriptive names for functions, variables, and types
4. **Unit Tests**: Include tests for all public functions and modules to verify correctness and handle edge cases
5. **Keep It Simple**: Avoid unnecessary complexity; favor straightforward solutions
6. **Dependency Management**: Use `Cargo.toml` carefully; avoid unnecessary dependencies
7. **Error Handling**: Use `anyhow` for error handling to provide context and stack traces
8. **Idiomatic Rust**: Write code that passes `cargo clippy` warnings
9. **Traits and Generics**: Use traits for shared behavior and generics for reusable, type-safe components
10. **Thread Safety**: Use stdlib primitives like `Arc` and `Mutex` for shared state
11. **Performance**: Choose appropriate data types like `&str` over `String` when appropriate

### Code Formatting

**ALWAYS** run the formatter before committing:

```bash
cargo +nightly fmt
```

### Linting

Run Clippy to catch common mistakes and non-idiomatic code:

```bash
cargo clippy --workspace
```

## Copyright Headers

**All Rust files (`.rs`) must include the Microsoft copyright header** at the top of the file.

### Required Format

```rust
// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.
```

The header should be at the very beginning of the file, followed by a blank line before any other content.

### Automated Application

Run the automated script to add headers to all relevant files:

```bash
./scripts/copyright.sh
```

This script is idempotent - it won't add duplicate headers if they already exist.

### Verification

Check if a file has the correct copyright header:

```bash
grep -q "Copyright (c) Microsoft Corporation" your_file.rs
```

## Debugging

### Running the MCP Server

Start the Wassette MCP server for development and debugging:

```bash
# Start server with SSE transport (listens on 127.0.0.1:9001/sse)
just run

# Start with custom log level
just run RUST_LOG='debug'

# Run with example plugins
just run-filesystem
just run-get-weather  # Requires OPENWEATHER_API_KEY environment variable
just run-fetch-rs
```

### Using MCP Inspector

Connect to the running server using the MCP Inspector:

```bash
# Connect to remote MCP server (default SSE transport)
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse

# List available tools
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/list

# Call a specific tool
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/call --tool-name remotetool --tool-arg param=value
```

## Documentation

### Building Documentation

The project uses [mdBook](https://rust-lang.github.io/mdBook/) for documentation:

```bash
# Build documentation to docs/book/
just docs-build

# Serve with auto-reload at http://localhost:3000
just docs-watch

# Serve and open in browser
just docs-serve
```

Alternatively, use mdBook directly:

```bash
cd docs
mdbook serve        # Serve with live reload
mdbook build        # Build static HTML
```

### Documentation Structure

The documentation uses a multi-version setup:
- **Local development**: Navigate to `http://localhost:3000/overview.html`
- **Production**: `https://microsoft.github.io/wassette/latest/` or `/v0.3.0/` for releases

### Visual Documentation Changes

When making documentation changes that affect visual presentation:
- Use Playwright to capture before/after screenshots
- Include screenshots in progress reports
- This helps reviewers understand the visual impact

## Contributing

### Contributor License Agreement

Most contributions require you to agree to a Contributor License Agreement (CLA). When you submit a pull request, a CLA-bot will determine if you need to provide a CLA. See [CONTRIBUTING.md](./CONTRIBUTING.md) for details.

### Code of Conduct

This project has adopted the [Microsoft Open Source Code of Conduct](https://opensource.microsoft.com/codeofconduct/). For more information, see the [Code of Conduct FAQ](https://opensource.microsoft.com/codeofconduct/faq/) or contact [opencode@microsoft.com](mailto:opencode@microsoft.com).

## CI/CD and Docker

### Running CI Locally

Test your changes in the same environment as CI:

```bash
# Run CI tests locally with Docker
just ci-local

# Build and test (without Docker)
just ci-build-test

# Build and test including GHCR (GitHub Container Registry) tests
just ci-build-test-ghcr
```

### Docker Commands

```bash
# View Docker cache information
just ci-cache-info

# Clean Docker images and cache
just ci-clean
```

## Project Structure

```
wassette/
├── src/                    # Main source code
├── crates/                 # Additional crates
├── examples/               # Example WebAssembly components
│   ├── fetch-rs/          # Rust example
│   ├── filesystem-rs/     # Rust filesystem example
│   ├── get-weather-js/    # JavaScript example
│   ├── time-server-js/    # JavaScript time example
│   ├── eval-py/           # Python example
│   └── gomodule-go/       # Go example
├── docs/                   # Documentation source (mdBook)
├── tests/                  # Integration tests
├── scripts/                # Utility scripts
├── .github/               # GitHub workflows and instructions
│   └── instructions/      # AI agent instruction files
└── Justfile               # Development commands
```

## Additional Resources

- **Architecture**: `docs/design/architecture.md`
- **Permission System**: `docs/design/permission-system.md`
- **Component Schemas**: `docs/design/component2json-structured-output.md`
- **CLI Reference**: `docs/reference/cli.md`
- **FAQ**: `docs/faq.md`
- **Installation Guide**: `docs/installation.md`
- **MCP Clients Setup**: `docs/mcp-clients.md`

## Quick Reference

### Common Commands

```bash
# Development
just build              # Build project
just test               # Run tests
just run                # Start MCP server
cargo +nightly fmt      # Format code
cargo clippy            # Run linter

# Documentation
just docs-serve         # View docs locally

# CI/Docker
just ci-local           # Run CI locally

# Utilities
./scripts/copyright.sh  # Add copyright headers
```

### Environment Variables

- `RUST_LOG`: Set log level (e.g., `info`, `debug`, `trace`)
- `OPENWEATHER_API_KEY`: Required for weather example
- `GITHUB_TOKEN`: For CI and GHCR tests

## Support

- **Issues**: [GitHub Issues](https://github.com/microsoft/wassette/issues)
- **Discussions**: [GitHub Discussions](https://github.com/microsoft/wassette/discussions)
- **Discord**: See README.md for Discord invite

## License

This project is licensed under the MIT License. See [LICENSE](./LICENSE) for details.
