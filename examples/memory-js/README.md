# Memory Server Example (JavaScript)

This example demonstrates a knowledge graph memory server implemented as a Wassette WebAssembly component. It is a migration of the [MCP memory server](https://github.com/modelcontextprotocol/servers/blob/main/src/memory/index.ts) from TypeScript to a JavaScript Wasm Component.

For more information on installing Wassette, please see the [installation instructions](https://github.com/microsoft/wassette?tab=readme-ov-file#installation).

## Overview

The memory server provides a knowledge graph storage system that allows AI agents to:
- Create and manage entities with observations
- Define relationships between entities
- Search through the knowledge graph
- Query specific nodes and their connections

This implementation uses in-memory storage where the knowledge graph persists for the lifetime of the component instance.

## Usage

Load the component from the OCI registry and interact with the knowledge graph:

**Load the component:**
```
Please load the component from oci://ghcr.io/microsoft/memory-js:latest
```

**Create entities:**
```
Create an entity named Alice of type person with observation "Software engineer"
```

**Create relations:**
```
Create a relation from Alice to Acme Corp with type works-for
```

**Search nodes:**
```
Search for nodes containing "engineer"
```

**Read the graph:**
```
Show me the entire knowledge graph
```

## Building

This example uses `jco` to build the component with an additional documentation injection step:

```bash
# Build the component
npm install
npm run build

# From repository root: inject WIT documentation into the component
just inject-docs examples/memory-js/memory.wasm examples/memory-js/wit
```

The documentation injection embeds the WIT interface documentation into the WASM binary, making it available to AI agents when they discover this tool. See [`wit/world.wit`](wit/world.wit) for the documented interface.

For more information about documenting components, see the [Documenting WIT Interfaces](../../docs/cookbook/documenting-wit.md) guide.

## Migration Notes

This component was migrated from the original MCP memory server. Key changes:
- **Persistence**: Changed from file-based JSONL to in-memory storage for simplicity
- **Field names**: Renamed `from`/`to` to `from-entity`/`to-entity` to avoid WIT reserved keywords
- **Error handling**: Converted JavaScript exceptions to WIT `result<T, string>` types
- **State management**: Uses module-level variables instead of file I/O

For detailed migration insights and patterns for converting MCP servers to WebAssembly components, see the [JavaScript Migration Guide](../../docs/cookbook/javascript.md#migrating-mcp-servers).

The source code for this example can be found in [`memory.js`](memory.js).
