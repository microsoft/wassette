# Examples

Wassette includes several example WebAssembly components that demonstrate how to build tools for different use cases and programming languages.

## Available Examples

The following examples are available in the [`examples/`](https://github.com/microsoft/wassette/tree/main/examples) directory:

| Example | Language | Description |
|---------|----------|-------------|
| [eval-py](https://github.com/microsoft/wassette/tree/main/examples/eval-py) | Python | Python code execution sandbox |
| [fetch-rs](https://github.com/microsoft/wassette/tree/main/examples/fetch-rs) | Rust | HTTP API client for fetching and converting web content |
| [filesystem-rs](https://github.com/microsoft/wassette/tree/main/examples/filesystem-rs) | Rust | File system operations (read, write, list directories) |
| [get-weather-js](https://github.com/microsoft/wassette/tree/main/examples/get-weather-js) | JavaScript | Weather API client for fetching weather data |
| [gomodule-go](https://github.com/microsoft/wassette/tree/main/examples/gomodule-go) | Go | Go module information tool |
| [time-server-js](https://github.com/microsoft/wassette/tree/main/examples/time-server-js) | JavaScript | Time server component |

## Building Examples

You can build all examples using the Justfile:

```bash
# Build all examples
just build-examples

# Build in release mode
just build-examples release
```

This will compile the examples and copy the resulting WebAssembly files to the `bin/` directory.

## Running Examples

### Filesystem Example

The filesystem example demonstrates file system operations with proper permission policies:

```bash
# Start Wassette with the filesystem example
just run-filesystem
```

Example usage:
- Load the component: `Please load the component from oci://ghcr.io/microsoft/filesystem-rs:latest`
- Read a file: `Please get the content of the file examples/filesystem-rs/README.md`

### Fetch Example

The fetch example shows how to make HTTP requests and convert content:

```bash
# Start Wassette with the fetch example
just run-fetch-rs
```

### Weather Example

The weather example requires an OpenWeather API key:

```bash
# Set your API key
export OPENWEATHER_API_KEY="your_api_key_here"

# Start Wassette with the weather example
just run-get-weather
```

## Language Support

Wassette supports tools written in any language that can compile to WebAssembly Components. For current language support, see the [WebAssembly Language Support Guide](https://developer.fermyon.com/wasm-languages/webassembly-language-support).

### Key Principles

- **No MCP Dependencies**: Components don't need to know about MCP - they're just regular library interfaces
- **Reusable**: Components built for Wassette can be used by other Wasm runtimes
- **Typed Interfaces**: All interfaces are defined using WebAssembly Interface Types (WIT)

### Example WIT Definition

Here's a simple WIT definition for a time server:

```wit
package local:time-server;

world time-server {
    export get-current-time: func() -> string;
}
```

This interface exports a single function that returns the current time as a string. Wassette automatically exposes this as an MCP tool.

## Security and Permissions

All examples include permission policies that define what resources the component can access. For example, the filesystem component's `policy.yaml`:

```yaml
version: "1.0"
description: "Permission policy for filesystem access in wassette"
permissions:
  storage:
    allow:
      - uri: "fs:///Users/USERNAME/github/wassette"
        access: ["read"]
      - uri: "fs:///Users/USERNAME"
        access: ["read"]
      - uri: "fs:///"
        access: ["read"]
```

This follows the principle of least privilege - components only get the permissions they explicitly need.

## Creating Your Own Examples

To create a new example:

1. Create a new directory under `examples/`
2. Define your WIT interface
3. Implement the interface in your chosen language
4. Create a `policy.yaml` file with appropriate permissions
5. Add build instructions (typically a `Justfile`)

See the existing examples for reference implementations in different languages.