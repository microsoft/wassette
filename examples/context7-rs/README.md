# Context7 Example (Rust)

This example demonstrates how to use the Context7 API to search for libraries and fetch documentation using a Wassette component written in Rust.

For more information on installing Wassette, please see the [installation instructions](https://github.com/microsoft/wassette?tab=readme-ov-file#installation).

## Usage

To use this component, you will need an API key from [Context7](https://context7.com). Set the API key as an environment variable:

```bash
export CONTEXT7_API_KEY="your_api_key_here"
```

See the [Environment Variables reference](../../docs/reference/environment-variables.md) for alternative methods including config files and Docker.

Then, load the component from the local file system.

**Load the component:**

```
Please load the component from file:///path/to/mcp-wasmtime/examples/context7-rs/target/wasm32-wasip2/debug/context7.wasm
```

**Search for libraries:**

```
resolve library id for "react"
```

**Get library documentation:**

```
get library docs for "library-id" with topic "hooks" and 15000 tokens
```

## Local Wassette workflow

From the repository root:

```bash
just build-examples release
STORE="$(mktemp -d)"
export CONTEXT7_API_KEY="<developer-key>"

wassette component load \
  "file://$PWD/bin/context7-rs.wasm" \
  --component-dir "$STORE"

wassette permission grant network \
  context7-rs context7.com \
  --component-dir "$STORE"

wassette permission grant environment-variable \
  context7-rs CONTEXT7_API_KEY \
  --component-dir "$STORE"

wassette run --component-dir "$STORE" --disable-builtin-tools
```

The checked-in [`policy.yaml`](policy.yaml) documents both required grants but
is not automatically attached by a local file load. See the
[examples permission workflow](../README.md).

## Policy

By default, WebAssembly (Wasm) components do not have any access to the host machine or network. The `policy.yaml` file is used to explicitly define what network resources and environment variables are made available to the component. This ensures that the component can only access the resources that are explicitly allowed.

Example:

```yaml
version: "1.0"
description: "Permission policy for context7 component"
permissions:
  network:
    allow:
      - host: "context7.com"
  environment:
    allow:
      - key: "CONTEXT7_API_KEY"
```

## Available Functions

The component exports two main functions:

1. **resolve-library-id**: Searches for libraries by name and returns Context7-compatible library IDs
   - Input: library name (string)
   - Output: search response with results, success flag, and optional error

2. **get-library-docs**: Fetches documentation for a specific library using its Context7-compatible ID
   - Input: library ID (string), optional topic (string), optional token count (u32)
   - Output: docs response with content, success flag, and optional error

The source code for this example can be found in [`src/lib.rs`](src/lib.rs).
