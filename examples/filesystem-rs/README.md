# Filesystem Example (Rust)

This example demonstrates how to interact with the filesystem using a Wassette component written in Rust.

For more information on installing Wassette, please see the [installation instructions](https://github.com/microsoft/wassette?tab=readme-ov-file#installation).

## Usage

To use this component, load it from the OCI registry and provide a file path to read.

**Load the component:**
```
Please load the component from oci://ghcr.io/microsoft/filesystem:latest
```

**Get file content:**
```
Please get the content of the file examples/filesystem-rs/README.md
```

## Policy

By default, WebAssembly (Wasm) components do not have any access to the host machine. The `policy.yaml` file is used to explicitly define what paths and permissions are made available to the component through the WebAssembly System Interface (WASI). This ensures that the component can only access the resources that are explicitly allowed.

Example:

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

The source code for this example can be found in [`src/lib.rs`](src/lib.rs).
