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

## Testing Changes Before Commit

**CRITICAL**: Always test your changes with the MCP Inspector before committing. This ensures that:
- The server starts correctly with your changes
- Tools are properly exposed via MCP
- Tool calls work as expected
- No regressions have been introduced

### Testing Workflow

For **every change** you make to Wassette, follow this workflow:

1. **Build your changes**: `just build`
2. **Start the Wassette server**: `just run` (or use a specific example like `just run-fetch-rs`)
3. **Test with MCP Inspector**: Use inspector commands to verify functionality
4. **Show the output**: Include inspector output in your commit/PR description

This workflow applies to all changes: bug fixes, new features, refactoring, documentation updates that include code changes, etc.

## Debugging

### Running the MCP Server

Start the Wassette MCP server for development and debugging:

```bash
# Start server with SSE transport (listens on 127.0.0.1:9001/sse)
just run

# Start with custom log level
just run RUST_LOG='debug'

# Run with example components
just run-filesystem
just run-get-weather  # Requires OPENWEATHER_API_KEY environment variable
just run-fetch-rs
just run-memory
```

### Testing with MCP Inspector

The MCP Inspector is your primary tool for testing and validating changes. Always use it before committing.

#### Basic Inspector Usage

Connect to the running Wassette server and interact with it:

```bash
# Connect to local Wassette server (default SSE transport)
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse

# List available tools (always run this first to see what's available)
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/list

# List available resources
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method resources/list

# List available prompts
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method prompts/list
```

#### Calling Tools

Test tool functionality by calling them with various arguments:

```bash
# Call a tool with simple arguments
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/call --tool-name mytool --tool-arg param=value

# Call a tool with multiple arguments
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/call --tool-name mytool --tool-arg key1=value1 --tool-arg key2=value2

# Call a tool with JSON arguments (for complex parameters)
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/call --tool-name mytool --tool-arg 'options={"format": "json", "max_tokens": 100}'
```

#### Testing with Different Transports

Wassette supports multiple transport protocols. Test with both:

```bash
# SSE transport (default, recommended for development)
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse

# Streamable HTTP transport
just run-streamable  # Start server with streamable HTTP transport
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001 --transport http --method tools/list
```

#### Testing with Custom Headers

If testing authentication or custom headers:

```bash
# Add custom headers to requests
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --transport http --method tools/list --header "X-API-Key: your-api-key"
```

#### Testing with Configuration Files

For complex setups, use configuration files:

```bash
# Use a config file to specify server and settings
npx @modelcontextprotocol/inspector --cli --config path/to/config.json --server myserver
```

### Practical Testing Examples

#### Example 1: Testing the Fetch Component

```bash
# Terminal 1: Start Wassette with fetch-rs component
just run-fetch-rs

# Terminal 2: Test the component
# List available tools
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/list

# Call the fetch tool
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/call --tool-name fetch --tool-arg url=https://api.github.com/repos/microsoft/wassette
```

#### Example 2: Testing the Filesystem Component

```bash
# Terminal 1: Start Wassette with filesystem-rs component
just run-filesystem

# Terminal 2: Test filesystem operations
# List tools
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/list

# Test reading a file
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/call --tool-name read_file --tool-arg path=/tmp/test.txt

# Test listing directory
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/call --tool-name list_directory --tool-arg path=/tmp
```

#### Example 3: Testing After Code Changes

```bash
# 1. Make your code changes
vim src/my_file.rs

# 2. Rebuild
just build

# 3. Start server (choose appropriate example or use default)
just run-fetch-rs

# 4. Verify tools are available
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/list

# 5. Test specific functionality you changed
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/call --tool-name your-tool --tool-arg test=value

# 6. Include output in your commit message or PR description
```

### Capturing and Sharing Inspector Output

Always capture the inspector output to demonstrate that your changes work:

```bash
# Save inspector output to a file
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/list > inspector-output.txt

# Or capture a full testing session
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/call --tool-name mytool --tool-arg test=value 2>&1 | tee test-results.txt
```

Include this output in:
- Your commit messages for significant changes
- Pull request descriptions
- Progress reports using the `report_progress` tool
- Issue comments when demonstrating fixes

### Troubleshooting with Inspector

If tools aren't working as expected:

1. **Verify server is running**: Check that `just run` or equivalent is running without errors
2. **List tools**: Run `tools/list` to see what tools are actually available
3. **Check logs**: Look at server logs (RUST_LOG=debug for detailed logs)
4. **Test incrementally**: Start with simple tool calls, then add complexity
5. **Compare with working examples**: Test known-good examples like `fetch-rs` first

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

# Testing with Inspector (ALWAYS do this before committing)
just run                # Terminal 1: Start server
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/list    # Terminal 2: List tools
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/call --tool-name TOOL_NAME --tool-arg key=value    # Test a tool

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
