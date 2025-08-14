# Development Setup

This guide helps developers set up their environment for contributing to Wassette.

## Prerequisites

- **Rust toolchain**: Install the latest stable Rust from [rustup.rs](https://rustup.rs/)
- **Git**: For version control
- **Just**: (Optional) Task runner for simplified commands - [install guide](https://github.com/casey/just#installation)

## Getting Started

1. **Clone the repository**:
   ```bash
   git clone https://github.com/microsoft/wassette.git
   cd wassette
   ```

2. **Build the project**:
   ```bash
   # Using cargo
   cargo build
   
   # Or using just
   just build
   ```

3. **Run tests**:
   ```bash
   # Using cargo
   cargo test --workspace
   
   # Or using just
   just test
   ```

## Development Workflow

### Building and Testing

```bash
# Build in debug mode
just build

# Build in release mode
just build release

# Run all tests
just test

# Build examples
just build-examples

# Clean build artifacts
just clean
```

### Running the Server

```bash
# Start the MCP server (listens on 127.0.0.1:9001/sse)
just run

# Run with specific plugin directory
just run-filesystem
just run-fetch-rs
just run-get-weather  # Requires OPENWEATHER_API_KEY environment variable
```

### Debugging and Testing

You can use the MCP inspector to interact with the running server:

```bash
# Connect to the server
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse

# List available tools
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/list

# Call a tool
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/call --tool-name remotetool --tool-arg param=value
```

## Coding Standards

### Rust Best Practices

- **Single responsibility principle**: Ensure each function and struct has a single, well-defined purpose
- **DRY principle**: Avoid code duplication by extracting common logic into reusable functions
- **Descriptive naming**: Use clear, descriptive names for functions, variables, and types
- **Error handling**: Use `anyhow` for error handling to provide context and stack traces
- **Idiomatic Rust**: Write code that passes `cargo clippy` warnings
- **Testing**: Include unit tests for all public functions and modules
- **Simplicity**: Favor straightforward solutions that are easy to understand and maintain

### Architecture Guidelines

- Use traits to define shared behavior and generics for reusable, type-safe components
- Design APIs to be extensible
- Use stdlib primitives like `Arc` and `Mutex` for thread safety and shared state
- Choose appropriate data types (e.g., `&str` over `String`) for performance and memory efficiency
- Manage dependencies carefully in `Cargo.toml`

## Project Structure

```
wassette/
├── src/                    # Main application source
├── crates/                 # Workspace crates
│   ├── wassette/          # Core library
│   ├── mcp-server/        # MCP server implementation
│   ├── policy/            # Policy engine
│   └── component2json/    # Component introspection tool
├── docs/                  # Documentation (this book)
├── examples/              # Example WebAssembly components
└── tests/                 # Integration tests
```

## Contributing

See the main [CONTRIBUTING.md](https://github.com/microsoft/wassette/blob/main/CONTRIBUTING.md) file for information about:

- Contributor License Agreement (CLA)
- Code of Conduct
- Pull request process

## Getting Help

- Join the `#wassette` channel on [Microsoft Open Source Discord](https://discord.gg/microsoft-open-source)
- Check existing [GitHub issues](https://github.com/microsoft/wassette/issues)
- Review the [architecture documentation](./design/architecture.md) for design details