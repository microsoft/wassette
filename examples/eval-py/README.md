# Python Evaluation Example

This example demonstrates how to evaluate a Python expression using a Wassette component.

For more information on installing Wassette, please see the [installation instructions](https://github.com/microsoft/wassette?tab=readme-ov-file#installation).

## Building

This example uses `componentize-py` to build the component with an additional documentation injection step:

```bash
# Generate bindings and build the component
just build

# From repository root: inject WIT documentation into the component
just inject-docs examples/eval-py/eval.wasm examples/eval-py/wit
```

The documentation injection embeds the WIT interface documentation into the WASM binary, making it available to AI agents when they discover this tool. See [`wit/world.wit`](wit/world.wit) for the documented interface.

For more information about documenting components, see the [Documenting WIT Interfaces](../../docs/cookbook/documenting-wit.md) guide.

## Usage

To use this component, load it from the OCI registry and provide a Python expression to evaluate.

**Load the component:**
```
Please load the component from oci://ghcr.io/microsoft/eval-py:latest
```

**Evaluate an expression:**
```
Please evaluate the python expression '2 + 2'
```

The source code for this example can be found in [`main.py`](main.py).
