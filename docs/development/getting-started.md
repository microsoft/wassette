# Developer Guide: Getting Started

This guide provides comprehensive instructions for developers who want to contribute to Wassette or build it from source. By the end of this guide, you'll understand how to set up your development environment, build the project, run tests, and contribute effectively.

## Table of Contents

- [Prerequisites](#prerequisites)
- [Getting the Source Code](#getting-the-source-code)
- [Building Wassette](#building-wassette)
- [Running Tests](#running-tests)
- [Code Formatting and Linting](#code-formatting-and-linting)
- [Running the Development Server](#running-the-development-server)
- [Building Documentation](#building-documentation)
- [Development Workflow](#development-workflow)
- [CI/CD and Docker](#cicd-and-docker)
- [Project Structure](#project-structure)
- [Contributing](#contributing)

## Prerequisites

Before you begin, ensure you have the following installed on your system:

### Required Tools

1. **Rust toolchain** (1.75.0 or later):
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   source ~/.cargo/env
   ```

2. **Rust nightly** (for formatting):
   ```bash
   rustup install nightly
   ```

3. **WASI Preview 2 target**:
   ```bash
   rustup target add wasm32-wasip2
   ```

4. **Just** (command runner):
   - **macOS (Homebrew)**:
     ```bash
     brew install just
     ```
   - **Linux**:
     ```bash
     cargo install just
     ```
   - **Other platforms**: See [Just Installation Guide](https://github.com/casey/just#installation)

### Optional Tools

5. **mdBook** (for building documentation):
   ```bash
   cargo install mdbook
   cargo install mdbook-mermaid
   ```

6. **Node.js** (for running MCP Inspector for debugging):
   - Download from [nodejs.org](https://nodejs.org/) or use your package manager

## Getting the Source Code

Clone the repository:

```bash
git clone https://github.com/microsoft/wassette.git
cd wassette
```

## Building Wassette

Wassette uses [Just](https://github.com/casey/just) as a command runner for development tasks. You can view all available commands by running:

```bash
just --list
```

### Build in Debug Mode

```bash
just build
```

This will:
- Build the workspace in debug mode
- Create a `bin` directory
- Copy the `wassette` binary to `bin/`

### Build in Release Mode

```bash
just build release
```

### Build Example Components

To build all example WebAssembly components:

```bash
just build-examples
```

For release builds of examples:

```bash
just build-examples release
```

## Running Tests

Wassette has a comprehensive test suite that includes both unit tests and documentation tests.

### Run All Tests

```bash
just test
```

This command will:
1. Clean any existing test component artifacts
2. Build test components (fetch-rs and filesystem-rs)
3. Inject documentation into the compiled components
4. Run all unit tests
5. Run all documentation tests

### Pre-build Test Components

If you want to build test components separately:

```bash
just build-test-components
```

### Clean Test Components

To remove test component artifacts:

```bash
just clean-test-components
```

### Run Specific Tests

You can also run tests directly with Cargo:

```bash
# Run all workspace tests
cargo test --workspace

# Run tests for a specific crate
cargo test -p wassette

# Run a specific test
cargo test test_name

# Run tests with verbose output
cargo test -- --nocapture
```

## Code Formatting and Linting

### Format Code

**ALWAYS** run the formatter before committing code:

```bash
cargo +nightly fmt
```

The project uses nightly Rust for formatting to access the latest formatting features.

### Run Linter

Check for common mistakes and non-idiomatic code:

```bash
cargo clippy --workspace
```

To automatically fix some lint warnings:

```bash
cargo clippy --workspace --fix
```

### Copyright Headers

All Rust files must include the Microsoft copyright header. Run the automated script to add headers:

```bash
./scripts/copyright.sh
```

This script is idempotent and won't add duplicate headers.

## Running the Development Server

### Basic Server

Start the Wassette MCP server with SSE transport:

```bash
just run
```

This starts the server listening on `127.0.0.1:9001/sse` with `info` level logging.

### Custom Log Level

```bash
just run RUST_LOG='debug'
```

Available log levels: `error`, `warn`, `info`, `debug`, `trace`

### Run with Example Plugins

```bash
# Filesystem example
just run-filesystem

# Fetch example
just run-fetch-rs

# Weather example (requires OPENWEATHER_API_KEY environment variable)
just run-get-weather
```

### Debugging with MCP Inspector

Once the server is running, use the MCP Inspector to interact with it:

```bash
# Connect to the server
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse

# List available tools
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/list

# Call a tool
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/call --tool-name tool-name --tool-arg param=value
```

## Building Documentation

Wassette uses [mdBook](https://rust-lang.github.io/mdBook/) for documentation.

### Build Documentation

```bash
just docs-build
```

This builds the documentation to `docs/book/`.

### Serve Documentation Locally

```bash
# Serve with live reload
just docs-watch

# Serve and open in browser
just docs-serve
```

The documentation will be available at `http://localhost:3000/overview.html`.

**Note**: When developing locally, navigate directly to specific pages (e.g., `/overview.html`). The version picker dropdown is designed for the production multi-version setup and won't work in local development.

### Documentation Structure

The documentation uses a multi-version setup:
- **Local development**: `http://localhost:3000/overview.html`
- **Production**: `https://microsoft.github.io/wassette/latest/` or `/v0.3.0/` for releases

## Development Workflow

### Making Changes

1. **Create a branch** for your changes:
   ```bash
   git checkout -b feature/your-feature-name
   ```

2. **Make your changes** following the coding standards

3. **Format your code**:
   ```bash
   cargo +nightly fmt
   ```

4. **Run linter**:
   ```bash
   cargo clippy --workspace
   ```

5. **Build the project**:
   ```bash
   just build
   ```

6. **Run tests**:
   ```bash
   just test
   ```

7. **Update CHANGELOG.md** (for non-trivial changes):
   - Add entries under the `[Unreleased]` section
   - Follow [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format
   - Categorize changes: Added, Changed, Deprecated, Removed, Fixed, Security

8. **Commit your changes**:
   ```bash
   git add .
   git commit -m "Your descriptive commit message"
   ```

9. **Push your branch**:
   ```bash
   git push origin feature/your-feature-name
   ```

10. **Create a Pull Request** on GitHub

### Best Practices

- **Single Responsibility Principle**: Each function and struct should have a single, well-defined purpose
- **DRY (Don't Repeat Yourself)**: Avoid code duplication by extracting common logic
- **Descriptive Naming**: Use clear, descriptive names for functions, variables, and types
- **Include Tests**: Add unit tests for all public functions and modules
- **Keep It Simple**: Favor straightforward solutions that are easy to understand
- **Idiomatic Rust**: Write code that passes `cargo clippy` warnings
- **Error Handling**: Use `anyhow` for error handling to provide context and stack traces
- **Thread Safety**: Use stdlib primitives like `Arc` and `Mutex` for shared state
- **Performance**: Choose appropriate data types (e.g., `&str` over `String` when possible)

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
│   ├── component2json/    # Component to JSON converter
│   ├── mcp-server/        # MCP server implementation
│   ├── policy/            # Policy management
│   └── wassette/          # Core Wassette library
├── examples/               # Example WebAssembly components
│   ├── fetch-rs/          # Rust fetch example
│   ├── filesystem-rs/     # Rust filesystem example
│   ├── get-weather-js/    # JavaScript weather example
│   ├── time-server-js/    # JavaScript time example
│   ├── eval-py/           # Python eval example
│   └── gomodule-go/       # Go module example
├── docs/                   # Documentation source (mdBook)
│   ├── cookbook/          # Component building guides
│   ├── deployment/        # Deployment guides
│   ├── design/            # Design documents
│   ├── development/       # Development guides
│   └── reference/         # Reference documentation
├── tests/                  # Integration tests
├── scripts/                # Utility scripts
├── .github/               # GitHub workflows and instructions
├── Justfile               # Development commands
├── Cargo.toml             # Workspace configuration
└── README.md              # Project overview
```

### Key Crates

- **`wassette-mcp-server`**: The main MCP server binary
- **`wassette`**: Core library with component loading and execution
- **`component2json`**: Utilities for converting component schemas to JSON
- **`mcp-server`**: MCP protocol implementation
- **`policy`**: Permission and policy management

## Contributing

We welcome contributions! Before contributing:

1. **Read [CONTRIBUTING.md](../../CONTRIBUTING.md)** for general guidelines
2. **Check the [GitHub Issues](https://github.com/microsoft/wassette/issues)** for existing issues or create a new one
3. **Join the Discord** for discussions: [Microsoft Open Source Discord](https://discord.gg/microsoft-open-source) (#wassette channel)
4. **Follow the development workflow** outlined above
5. **Ensure all tests pass** before submitting a PR
6. **Update documentation** if your changes affect user-facing functionality

### Contributor License Agreement

Most contributions require you to agree to a Contributor License Agreement (CLA) declaring that you have the right to grant us the rights to use your contribution. When you submit a pull request, a CLA-bot will automatically determine whether you need to provide a CLA.

### Code of Conduct

This project has adopted the [Microsoft Open Source Code of Conduct](https://opensource.microsoft.com/codeofconduct/). For more information, see the [Code of Conduct FAQ](https://opensource.microsoft.com/codeofconduct/faq/) or contact [opencode@microsoft.com](mailto:opencode@microsoft.com) with any additional questions or comments.

## Additional Resources

- **Architecture**: [docs/design/architecture.md](../design/architecture.md)
- **Permission System**: [docs/design/permission-system.md](../design/permission-system.md)
- **Component Schemas**: [docs/design/component2json-structured-output.md](../design/component2json-structured-output.md)
- **CLI Reference**: [docs/reference/cli.md](../reference/cli.md)
- **FAQ**: [docs/faq.md](../faq.md)
- **Installation Guide**: [docs/installation.md](../installation.md)
- **MCP Clients Setup**: [docs/mcp-clients.md](../mcp-clients.md)

## Quick Reference

### Common Commands

```bash
# Development
just build              # Build project in debug mode
just build release      # Build project in release mode
just test               # Run all tests
just run                # Start MCP server
cargo +nightly fmt      # Format code
cargo clippy            # Run linter

# Documentation
just docs-serve         # Build and serve docs locally
just docs-build         # Build docs to docs/book/

# CI/Docker
just ci-local           # Run CI locally with Docker

# Utilities
./scripts/copyright.sh  # Add copyright headers
just clean              # Clean build artifacts
```

### Environment Variables

- `RUST_LOG`: Set log level (e.g., `info`, `debug`, `trace`)
- `OPENWEATHER_API_KEY`: Required for weather example
- `GITHUB_TOKEN`: For CI and GHCR tests

## Getting Help

- **Issues**: [GitHub Issues](https://github.com/microsoft/wassette/issues)
- **Discussions**: [GitHub Discussions](https://github.com/microsoft/wassette/discussions)
- **Discord**: [Microsoft Open Source Discord](https://discord.gg/microsoft-open-source) (#wassette channel)

## License

This project is licensed under the MIT License. See [LICENSE](../../LICENSE) for details.
