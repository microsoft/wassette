# Fetch Example (Rust)

This example demonstrates how to fetch content from a URL using a Wassette component written in Rust.

For more information on installing Wassette, please see the [installation instructions](https://github.com/microsoft/wassette?tab=readme-ov-file#installation).

## Building

This example uses the standard Rust build process with an additional documentation injection step:

```bash
# Build the component
cargo build --target wasm32-wasip2 --release

# From repository root: inject WIT documentation into the component
just inject-docs examples/fetch-rs/target/wasm32-wasip2/release/fetch_rs.wasm examples/fetch-rs/wit
```

The documentation injection embeds the WIT interface documentation into the WASM binary, making it available to AI agents when they discover this tool. See [`wit/world.wit`](wit/world.wit) for the documented interface.

For more information about documenting components, see the [Documenting WIT Interfaces](../../docs/cookbook/documenting-wit.md) guide.

## Usage

To use this component, load it from the OCI registry and provide a URL to fetch.

**Load the component:**

```
Please load the component from oci://ghcr.io/microsoft/fetch-rs:latest
```

**Fetch content:**

```
Please fetch the content of https://example.com
```

## Policy

By default, WebAssembly (Wasm) components do not have any access to the host machine or network. The `policy.yaml` file is used to explicitly define what network resources are made available to the component. This ensures that the component can only access the resources that are explicitly allowed.

Example:

```yaml
version: "1.0"
description: "Permission policy for fetch-rs example in wassette"
permissions:
  network:
    allow:
      - host: "https://rss.nytimes.com/"
```

The source code for this example can be found in [`src/lib.rs`](src/lib.rs).
