# Filesystem Example (Rust)

This example demonstrates how to interact with the filesystem using a Wassette component written in Rust. It provides production-ready filesystem operations with comprehensive error handling and security features.

For more information on installing Wassette, please see the [installation instructions](https://github.com/microsoft/wassette?tab=readme-ov-file#installation).

## Features

This filesystem component provides the following operations:

### Read Operations
- **list-directory**: Get a detailed listing of files and directories
- **read-file**: Read the complete contents of a file
- **search-file**: Recursively search for files matching a pattern
- **get-file-info**: Retrieve detailed metadata (size, type, permissions, timestamps)
- **file-exists**: Check if a file or directory exists
- **get-directory-tree**: Get a recursive tree view of directory structure

### Write Operations (requires write permission in policy)
- **write-file**: Write content to a file (creates or overwrites)
- **create-directory**: Create a new directory (creates parents if needed)
- **move-path**: Move or rename files and directories
- **delete-file**: Delete a file (with safety checks)
- **delete-directory**: Delete an empty directory

## Usage

To use this component, load it from the OCI registry and interact with the filesystem.

**Load the component:**
```
Please load the component from oci://ghcr.io/microsoft/filesystem-rs:latest
```

**Read operations:**
```
Please read the file examples/filesystem-rs/README.md
Please list the directory examples/filesystem-rs
Please search for files matching "README" in examples
Please get information about examples/filesystem-rs/Cargo.toml
Please check if the file examples/filesystem-rs/policy.yaml exists
Please show me a tree view of examples/filesystem-rs with depth 2
```

**Write operations (require write permission in policy):**
```
Please write "Hello World" to /tmp/test.txt
Please create a directory at /tmp/my-new-folder
Please move /tmp/test.txt to /tmp/backup/test.txt
Please delete the file /tmp/test.txt
Please delete the directory /tmp/my-new-folder
```

## Policy

By default, WebAssembly (Wasm) components do not have any access to the host machine. The `policy.yaml` file is used to explicitly define what paths and permissions are made available to the component through the WebAssembly System Interface (WASI). This ensures that the component can only access the resources that are explicitly allowed.

### Read-Only Example

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

### Read-Write Example

To enable write operations, add write access to specific directories:

```yaml
version: "1.0"
description: "Permission policy for filesystem access in wassette"
permissions:
  storage:
    allow:
      # Read-only access to most directories
      - uri: "fs:///Users/USERNAME/github/wassette"
        access: ["read"]
      # Read and write access to a specific directory
      - uri: "fs:///Users/USERNAME/tmp"
        access: ["read", "write"]
```

## Security Features

- **Path validation**: All paths support tilde (`~`) expansion for home directory
- **Permission enforcement**: Write operations require explicit write access in policy
- **Safety checks**: 
  - Delete operations verify file/directory type
  - Move operations create parent directories if needed
  - Directory deletion only works on empty directories
- **Descriptive errors**: Clear error messages with context and suggestions

## Implementation

The source code for this example can be found in [`src/lib.rs`](src/lib.rs). The component is implemented in Rust and compiled to WebAssembly using the `wasm32-wasip2` target.

