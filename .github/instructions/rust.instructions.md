---
applyTo: "**/*.rs"
---

# Project overview

This project is a ModelContextProtocol (MCP) server implementation that runs Tools as WebAssembly (Wasm) components using the Wasmtime engine.

View the design/architecture.md for a high-level overview of the architecture and docs for detailed documentations.

## Best practices

- Single responsibility principle: ensure each function and struct has a single, well-defined purpose.
- DRY (Don't Repeat Yourself): avoid code duplication by extracting common logic into reusable functions or modules.
- Descriptive naming: use clear, descriptive names for functions, variables, and types to improve code readability.
- Include unit tests for all public functions and modules to verify correctness and handle edge cases.
- Keep it simple: avoid unnecessary complexity in code and design. Favor straightforward solutions that are easy to understand and maintain.
- Manage dependencies carefully: use `Cargo.toml` to manage dependencies and keep them up-to-date. Avoid unnecessary dependencies that bloat the project.
- Idiomatic error handling: Use `anyhow` for error handling to provide context and stack traces.
- Write idiomatic Rust code that passes `cargo clippy` warnings.
- Use traits to define shared behavior and generics to create reusable, type-safe components. Design the API to be extensible
- Use stdlib primitives like `Arc` and `Mutex` for thread safety and shared state.
- Choose appropriate data types like `&str` over `String` for performance and memory efficiency.

## Debugging


You can use commands in the Justfile to start the wassette mcp server (`just run`) and to run the tests (`just test`). `just run` will start the server that listens to "127.0.0.1:9001/sse". 

Then you can use `npx @modelcontextprotocol/inspector` to connect to the server and inspect the state of the MCP server.

The following is a list of sample CLI commands you can use to interact with the MCP server:

```bash
# Connect to a remote MCP server (default is SSE transport)
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse

# List tools from a remote server
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/list

# Call a tool on a remote server
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse --method tools/call --tool-name remotetool --tool-arg param=value
```
