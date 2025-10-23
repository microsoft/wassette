# Go Module Example (Go)

This example demonstrates how to get information about Go modules using a Wassette component written in Go.

For more information on installing Wassette, please see the [installation instructions](https://github.com/microsoft/wassette?tab=readme-ov-file#installation).

## Building

This example uses TinyGo to build the component with an additional documentation injection step:

```bash
# Generate bindings and build the component
just build

# From repository root: inject WIT documentation into the component
just inject-docs examples/gomodule-go/gomodule.wasm examples/gomodule-go/wit
```

The documentation injection embeds the WIT interface documentation into the WASM binary, making it available to AI agents when they discover this tool. See [`wit/world.wit`](wit/world.wit) for the documented interface.

For more information about documenting components, see the [Documenting WIT Interfaces](../../docs/cookbook/documenting-wit.md) guide.

## Usage

To use this component, load it from the OCI registry and provide a Go module path.

**Load the component:**
```
Please load the component from oci://ghcr.io/microsoft/gomodule-go:latest
```

**Get module information:**
```
get the latest versions for the go module urfave/cli
```

The source code for this example can be found in [`main.go`](main.go).
