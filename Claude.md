# Claude Code Development Guide for Wassette

Welcome, Claude Code! This guide provides specific information for working on the Wassette project.

## Quick Start

For **complete development guidelines**, please refer to **[AGENTS.md](./AGENTS.md)**, which contains comprehensive instructions for:
- Project overview and architecture
- Development environment setup
- Code style and best practices
- Building, testing, and debugging
- Documentation guidelines
- Changelog management
- Contributing guidelines

## Claude Code Specific Information

### Using Wassette with Claude Code

If you're testing Wassette integration with Claude Code, you can install it as an MCP server:

```bash
# Install Claude Code (requires Node.js 18+)
npm install -g @anthropic-ai/claude-code

# Add Wassette MCP server
claude mcp add -- wassette wassette serve --stdio

# Verify installation
claude mcp list

# Remove if needed
claude mcp remove wassette
```

For more details, see [docs/mcp-clients.md](./docs/mcp-clients.md).

### Key Development Workflows

#### 1. Building and Testing
```bash
just build          # Build the project
just test           # Run all tests
cargo clippy        # Check for issues
cargo +nightly fmt  # Format code
```

#### 2. Running the Server for Debugging
```bash
just run            # Start MCP server on 127.0.0.1:9001/sse
just run RUST_LOG='debug'  # With debug logging
```

Then connect with MCP Inspector:
```bash
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse
```

#### 3. Documentation
```bash
just docs-serve     # Build and serve docs at http://localhost:3000
```

### Important Reminders

1. **Copyright Headers**: All Rust files must have the Microsoft copyright header. Run `./scripts/copyright.sh` to add them automatically.

2. **Format Code**: Always run `cargo +nightly fmt` before committing.

3. **Update Changelog**: Add entries to `CHANGELOG.md` under the `[Unreleased]` section for any code changes.

4. **Test Your Changes**: Run `just test` to ensure all tests pass.

### Project Architecture

Wassette is an MCP server that executes WebAssembly components with security isolation:

```
MCP Clients          Wassette          Wasm Components
(VS Code,      <-->  (MCP Server) <--> (Sandboxed Tools)
Claude Code,         (Wasmtime)
Cursor, etc.)
```

Key components:
- **MCP Server**: Implements Model Context Protocol
- **Wasmtime Engine**: Provides WebAssembly runtime with security sandbox
- **Permission System**: Fine-grained control over component capabilities
- **Component Registry**: Manages loaded WebAssembly components

For detailed architecture, see `docs/design/architecture.md`.

### Common Tasks

#### Adding a New Feature
1. Review existing code structure in `src/` and `crates/`
2. Write tests first (TDD approach)
3. Implement the feature
4. Run tests: `just test`
5. Format: `cargo +nightly fmt`
6. Lint: `cargo clippy`
7. Update `CHANGELOG.md`
8. Ensure copyright headers: `./scripts/copyright.sh`

#### Fixing a Bug
1. Add a test that reproduces the bug
2. Fix the issue
3. Verify all tests pass: `just test`
4. Update `CHANGELOG.md` under `### Fixed`
5. Format and lint code

#### Updating Documentation
1. Edit files in `docs/`
2. Preview changes: `just docs-serve`
3. For visual changes, capture screenshots using Playwright
4. Update `CHANGELOG.md` only if the change significantly impacts user experience

### File Organization

- `src/`: Main application code
- `crates/`: Additional library crates
- `examples/`: WebAssembly component examples in various languages
- `docs/`: mdBook documentation
- `tests/`: Integration tests
- `.github/instructions/`: AI agent instruction files

### Getting Help

- Review the comprehensive guide: **[AGENTS.md](./AGENTS.md)**
- Check documentation: `docs/` directory
- FAQs: `docs/faq.md`
- Architecture details: `docs/design/`
- Contributing guidelines: `CONTRIBUTING.md`

## Next Steps

1. Read **[AGENTS.md](./AGENTS.md)** for complete development guidelines
2. Explore the `docs/` directory for user-facing documentation
3. Review `examples/` to understand WebAssembly component development
4. Check `.github/instructions/` for specific guidance on different file types

Happy coding! 🚀
