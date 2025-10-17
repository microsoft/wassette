# Documenting WIT Interfaces

Documentation in your WIT files is automatically extracted and embedded into your compiled Wasm components. AI agents use this documentation to understand what your tools do and when to use them.

## How It Works

Wassette uses [`wit-docs-inject`](https://github.com/Mossaka/wit-docs-inject) to automatically extract documentation from your WIT files and embed them as a `package-docs` custom section in the WASM binary. This happens during the build process - you just need to write the documentation.

### The Build Process

The documentation injection happens in two stages:

1. **Build your component**: First, compile your component using the standard toolchain for your language (Rust, JavaScript, Python, or Go)
2. **Inject documentation**: Run `wit-docs-inject` to extract WIT documentation and embed it into the compiled WASM binary

This two-stage process is automated in the Wassette repository's build system. Here's how it works:

#### Automated Build Integration

The root [`Justfile`](https://github.com/microsoft/wassette/blob/main/Justfile) orchestrates the build and injection process:

```just
# Install wit-docs-inject if not present
ensure-wit-docs-inject:
    if ! command -v wit-docs-inject &> /dev/null; then
        cargo install --git https://github.com/Mossaka/wit-docs-inject
    fi

# Inject documentation into a component
inject-docs wasm_path wit_dir:
    wit-docs-inject --component {{ wasm_path }} --wit-dir {{ wit_dir }} --inplace

# Build examples with documentation injection
build-examples mode="debug":
    # 1. Build all example components
    (cd examples/fetch-rs && just build mode)
    (cd examples/get-weather-js && just build)
    # ... other examples ...
    
    # 2. Inject documentation into each built component
    just inject-docs examples/fetch-rs/target/wasm32-wasip2/{{ mode }}/fetch_rs.wasm examples/fetch-rs/wit
    just inject-docs examples/get-weather-js/weather.wasm examples/get-weather-js/wit
    # ... other examples ...
```

#### Manual Documentation Injection

If you're building components outside of the Wassette build system, you can inject documentation manually:

```bash
# Install wit-docs-inject
cargo install --git https://github.com/Mossaka/wit-docs-inject

# Build your component first (example for Rust)
cargo build --target wasm32-wasip2 --release

# Inject documentation into the compiled component
wit-docs-inject --component target/wasm32-wasip2/release/my_component.wasm \
                --wit-dir wit/ \
                --inplace
```

The `--inplace` flag modifies the WASM file directly. Without it, `wit-docs-inject` creates a new file.

### How Documentation Translates to Tool Descriptions

When Wassette loads a component with embedded documentation, it extracts the `package-docs` custom section from the WASM binary and parses the documentation to associate it with exported functions. The system then generates MCP tool schemas using the documentation as tool descriptions and exposes these tools to AI agents through the Model Context Protocol.

Your WIT documentation comments (`///`) become the descriptions that AI agents see when discovering and selecting tools to use.

## Basic Syntax

Use `///` for documentation comments:

```wit
package local:my-component;

world my-component {
    /// Fetch data from a URL and return the response body.
    ///
    /// Returns an error if the request fails or the URL is invalid.
    export fetch: func(url: string) -> result<string, string>;
}
```

## Documenting Types

```wit
/// Statistics about analyzed text
record text-stats {
    /// Total number of characters
    character-count: u32,

    /// Total number of words
    word-count: u32,
}

/// Processing status
variant status {
    /// Waiting to be processed
    pending,

    /// Currently processing
    processing(u32),

    /// Completed successfully
    completed(string),

    /// Failed with error
    failed(string),
}
```

## Verifying Documentation

After building and injecting documentation, verify it's properly embedded:

```bash
# For Wassette examples - build with automatic doc injection
just build-examples release

# Or manually for your own component:
# 1. Build your component
just build release

# 2. Inject documentation
just inject-docs target/wasm32-wasip2/release/my_component.wasm wit/

# 3. Inspect the component to verify docs are embedded
./target/debug/component2json target/wasm32-wasip2/release/my_component.wasm
```

You should see output indicating the documentation is embedded:
```
Found package docs!
fetch, Some("Fetch data from a URL and return the response body")
```

### Viewing Embedded Documentation

You can also use `wit-docs-view` (installed alongside `wit-docs-inject`) to view the embedded documentation:

```bash
wit-docs-view target/wasm32-wasip2/release/my_component.wasm
```

## Impact on AI Agents

**Without documentation:**
```json
{
  "name": "process",
  "description": "Auto-generated schema for function 'process'"
}
```

**With documentation:**
```json
{
  "name": "process",
  "description": "Process text input by normalizing whitespace and converting to uppercase.\n\nReturns an error if the input is empty after normalization."
}
```

The documentation helps AI agents understand when and how to use your tools effectively.

## Complete Example Workflow

Here's a complete example using the `fetch-rs` example from the Wassette repository:

### 1. Write WIT Documentation

The WIT file (`examples/fetch-rs/wit/world.wit`) contains:

```wit
package component:fetch-rs;

/// An example world for the component to target.
world fetch {
    /// Fetch data from a URL and return the response body as a String
    export fetch: func(url: string) -> result<string, string>;
}
```

### 2. Build the Component

```bash
cd examples/fetch-rs
cargo build --target wasm32-wasip2 --release
```

This creates `target/wasm32-wasip2/release/fetch_rs.wasm` - but without embedded documentation yet.

### 3. Inject Documentation

From the repository root:

```bash
# Ensure wit-docs-inject is installed
just ensure-wit-docs-inject

# Inject documentation
just inject-docs examples/fetch-rs/target/wasm32-wasip2/release/fetch_rs.wasm examples/fetch-rs/wit
```

This embeds the WIT documentation into the WASM binary as a `package-docs` custom section.

### 4. Verify the Result

```bash
# View embedded documentation
cargo run --bin component2json -- examples/fetch-rs/target/wasm32-wasip2/release/fetch_rs.wasm
```

Output:
```
Found package docs!
fetch, Some("Fetch data from a URL and return the response body as a String")
```

### 5. Load in Wassette

When you load this component in Wassette:

```bash
wassette serve --sse --plugin-dir examples/fetch-rs
```

The AI agent sees a tool with the description from your WIT documentation:

```json
{
  "name": "fetch",
  "description": "Fetch data from a URL and return the response body as a String",
  "inputSchema": {
    "type": "object",
    "properties": {
      "url": { "type": "string" }
    }
  }
}
```

## Language-Specific Guides

For implementation details in your language:

- [Rust Guide](./rust.md)
- [Go Guide](./go.md)
- [Python Guide](./python.md)
- [JavaScript/TypeScript Guide](./javascript.md)

## Resources

- [WIT Specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)
- [wit-docs-inject Tool](https://github.com/Mossaka/wit-docs-inject)
- [WebAssembly Component Model](https://component-model.bytecodealliance.org/)
