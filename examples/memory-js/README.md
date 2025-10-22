# Memory Server Example (JavaScript)

This example demonstrates a knowledge graph memory server implemented as a Wassette WebAssembly component. It is a migration of the [MCP memory server](https://github.com/modelcontextprotocol/servers/blob/main/src/memory/index.ts) from TypeScript to a JavaScript Wasm Component.

## Overview

The memory server provides a persistent knowledge graph storage system that allows AI agents to:
- Create and manage entities with observations
- Define relationships between entities
- Search through the knowledge graph
- Query specific nodes and their connections

This implementation uses an in-memory storage approach where the knowledge graph persists for the lifetime of the component instance.

## Migration Journey

### 1. Understanding the Original Implementation

The original [MCP memory server](https://github.com/modelcontextprotocol/servers/blob/main/src/memory/index.ts) was built as a TypeScript MCP server using:
- File-based persistence (JSONL format)
- Node.js file system operations
- MCP SDK for server/client communication
- Standard input/output transport

Key features:
- 9 tool operations for managing a knowledge graph
- Entities with names, types, and observations
- Relations between entities with types
- Full-text search capabilities
- Backward compatibility with migration from JSON to JSONL

### 2. Adapting to WebAssembly Components

#### Challenge 1: WIT Interface Design

**Finding**: WebAssembly Interface Types (WIT) requires careful consideration of data structures.

**Solution**: 
- Defined record types within the interface scope
- Used kebab-case naming for fields (e.g., `entity-type`, `from-entity`)
- Avoided reserved keywords like `from` and `to` (renamed to `from-entity` and `to-entity`)

#### Challenge 2: Persistence Model

**Finding**: The original implementation used file I/O for persistence, which isn't directly available in the base Wasm component model.

**Solution**: 
- Changed to in-memory storage (arrays instead of file operations)
- State persists across function calls within the same component instance
- For true persistence, the host environment (Wassette) would need to manage serialization

**Trade-off**: This design prioritizes simplicity and compatibility with the component model. For production use cases requiring persistence across restarts, additional WASI interfaces or host-provided storage would be needed.

#### Challenge 3: Error Handling

**Finding**: JavaScript/TypeScript error handling differs from WIT's result types.

**Solution**:
- Converted to `result<T, string>` pattern
- Used `{ tag: "ok", val: ... }` for success
- Used `{ tag: "err", val: "error message" }` for failures
- Wrapped all operations in try-catch blocks

#### Challenge 4: Type Mapping

**Finding**: WIT types need explicit mapping to JavaScript types.

**Solution**:
- `record` → JavaScript objects with matching property names
- `list<T>` → JavaScript arrays
- `string` → JavaScript strings
- Maintained camelCase in JavaScript while using kebab-case in WIT

### 3. Implementation Differences

| Aspect | Original MCP Server | Wasm Component |
|--------|---------------------|----------------|
| **Persistence** | File-based (JSONL) | In-memory |
| **Communication** | MCP protocol | Function exports |
| **Transport** | stdio/SSE | Direct function calls |
| **Dependencies** | Node.js, MCP SDK | None (pure JavaScript) |
| **State Management** | File system | Module-level variables |
| **Error Handling** | Exceptions | Result types |

### 4. Benefits of the Wasm Component Approach

1. **Security**: Runs in Wasmtime's security sandbox with explicit permissions
2. **Portability**: Can run anywhere Wassette is supported
3. **Simplicity**: No external dependencies beyond jco tooling
4. **Performance**: No file I/O overhead for operations
5. **Isolation**: Each component instance has isolated state
6. **Reusability**: Generic Wasm component with no MCP-specific code

### 5. Limitations and Considerations

1. **No Persistence**: State is lost when the component instance terminates
2. **No File Migration**: Cannot migrate existing memory.json/jsonl files
3. **Memory Bounds**: Large knowledge graphs are limited by Wasm linear memory
4. **No Async File I/O**: All operations are synchronous (though async functions work)

### 6. Key Learnings

1. **WIT Design Patterns**:
   - Keep record definitions inside interfaces
   - Use descriptive, non-reserved field names
   - Document thoroughly for AI agent discovery

2. **State Management**:
   - Module-level variables work well for in-memory storage
   - JavaScript's mutable data structures adapt easily to component model

3. **Error Handling**:
   - Result types provide clear success/failure semantics
   - String error messages are simple but effective

4. **Testing Strategy**:
   - Build incrementally and test after each change
   - Use `jco` for quick validation of WIT interface correctness
   - Test with Wassette's MCP inspector for end-to-end validation

## Usage

### Building the Component

```bash
just build
# Or manually:
npm install
npm run build
```

This creates `memory.wasm` with embedded WIT documentation.

### Loading in Wassette

```bash
# Start Wassette with the memory component
wassette serve --sse --plugin-dir ./examples/memory-js
```

### Using with MCP Inspector

```bash
# Connect to Wassette
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse
```

### Example Operations

**Create entities:**
```json
{
  "entities": [
    {
      "name": "Alice",
      "entityType": "person",
      "observations": ["Works as a software engineer", "Likes hiking"]
    },
    {
      "name": "Acme Corp",
      "entityType": "organization",
      "observations": ["Tech company", "Founded in 2020"]
    }
  ]
}
```

**Create relations:**
```json
{
  "relations": [
    {
      "fromEntity": "Alice",
      "toEntity": "Acme Corp",
      "relationType": "works-for"
    }
  ]
}
```

**Search nodes:**
```json
{
  "query": "software"
}
```

**Read entire graph:**
```json
{}
```

## Architecture

```
┌─────────────────────────────────────┐
│         AI Agent / Client           │
└───────────────┬─────────────────────┘
                │ MCP Protocol
┌───────────────▼─────────────────────┐
│            Wassette                  │
│  (MCP Server + Wasm Runtime)        │
└───────────────┬─────────────────────┘
                │ Component Calls
┌───────────────▼─────────────────────┐
│      memory.wasm Component          │
│                                      │
│  ┌────────────────────────────────┐ │
│  │  In-Memory Storage             │ │
│  │  - entities: []                │ │
│  │  - relations: []               │ │
│  └────────────────────────────────┘ │
│                                      │
│  ┌────────────────────────────────┐ │
│  │  Operations                     │ │
│  │  - createEntities              │ │
│  │  - createRelations             │ │
│  │  - addObservations             │ │
│  │  - deleteEntities              │ │
│  │  - deleteObservations          │ │
│  │  - deleteRelations             │ │
│  │  - readGraph                   │ │
│  │  - searchNodes                 │ │
│  │  - openNodes                   │ │
│  └────────────────────────────────┘ │
└─────────────────────────────────────┘
```

## For Production Use

To add persistence to this component:

1. **Option 1: Host-Side Persistence**
   - Wassette could snapshot component state periodically
   - Use component import/export for serialization

2. **Option 2: WASI Filesystem**
   - Add `wasi:filesystem` imports to WIT
   - Implement file-based storage similar to original
   - Requires filesystem permissions in policy.yaml

3. **Option 3: External Storage**
   - Add HTTP client capabilities
   - Connect to external database/storage service
   - Requires network permissions

## Related Resources

- [Original MCP Memory Server](https://github.com/modelcontextprotocol/servers/blob/main/src/memory/index.ts)
- [JavaScript Component Guide](../../docs/development/javascript.md)
- [JavaScript Cookbook](../../docs/cookbook/javascript.md)
- [Wassette Documentation](https://microsoft.github.io/wassette/)
- [Component Model](https://component-model.bytecodealliance.org/)
- [WIT Specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)
