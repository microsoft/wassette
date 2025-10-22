# Migration Notes: MCP Memory Server to Wasm Component

This document provides technical notes on the migration of the MCP memory server from a TypeScript MCP server to a JavaScript WebAssembly component.

## Source Material

- **Original Server**: https://github.com/modelcontextprotocol/servers/blob/main/src/memory/index.ts
- **Target Platform**: Wassette (WebAssembly Component Model)
- **Migration Date**: October 2024

## Architecture Changes

### From MCP Server to Wasm Component

| Aspect | Original | Migrated |
|--------|----------|----------|
| **Runtime** | Node.js | Wasmtime (via Wassette) |
| **Communication** | stdio/SSE with MCP SDK | Direct function exports |
| **Type System** | TypeScript interfaces | WIT (WebAssembly Interface Types) |
| **Persistence** | File system (JSONL) | In-memory arrays |
| **Dependencies** | @modelcontextprotocol/sdk | None (pure JavaScript) |
| **Security Model** | OS-level permissions | Wasm sandbox + capabilities |

## Technical Decisions

### 1. Persistence Strategy

**Original**: JSONL file-based storage with automatic migration from JSON format
```typescript
const MEMORY_FILE_PATH: string;
await fs.writeFile(MEMORY_FILE_PATH, lines.join("\n"));
```

**Migrated**: In-memory storage using module-level variables
```javascript
let entities = [];
let relations = [];
```

**Rationale**: 
- WebAssembly Component Model doesn't include file I/O in base specification
- WASI filesystem interfaces could be added but increase complexity
- In-memory approach keeps the component simple and portable
- State persists for component instance lifetime
- Host (Wassette) could implement snapshots for true persistence

### 2. Field Naming

**Challenge**: WIT reserves certain keywords that were field names in the original

**Original**:
```typescript
interface Relation {
    from: string;
    to: string;
    relationType: string;
}
```

**Issue**: `from` is a reserved keyword in WIT syntax

**Solution**: Renamed to avoid conflicts
```wit
record relation {
    from-entity: string,
    to-entity: string,
    relation-type: string,
}
```

**Impact**: All JavaScript code updated to use new field names consistently

### 3. Error Handling

**Original**: JavaScript exceptions and promises
```typescript
async addObservations(observations) {
    const entity = graph.entities.find(e => e.name === o.entityName);
    if (!entity) {
        throw new Error(`Entity with name ${o.entityName} not found`);
    }
}
```

**Migrated**: WIT result types
```javascript
function addObservations(observations) {
    try {
        const entity = findEntity(input.entityName);
        if (!entity) {
            return { tag: "err", val: `Entity with name ${input.entityName} not found` };
        }
        return { tag: "ok", val: results };
    } catch (error) {
        return { tag: "err", val: `Failed to add observations: ${error.message}` };
    }
}
```

**Benefits**:
- Explicit success/failure handling
- Type-safe error propagation
- Consistent with Component Model patterns

### 4. Async Function Support

**Original**: Extensive use of async/await with file I/O
```typescript
async createEntities(entities: Entity[]): Promise<Entity[]> {
    const graph = await this.loadGraph();
    // ... logic ...
    await this.saveGraph(graph);
    return newEntities;
}
```

**Migrated**: Synchronous operations (though async syntax still works)
```javascript
function createEntities(newEntities) {
    // No await needed for in-memory operations
    const created = [];
    // ... logic ...
    return { tag: "ok", val: created };
}
```

**Note**: JavaScript components compiled with jco support async functions, but they're not needed for this in-memory implementation.

## WIT Interface Design

### Record Types

All records are defined within the interface scope:

```wit
interface knowledge-graph-ops {
    record entity { ... }
    record relation { ... }
    // ... other records ...
    
    // Functions using these records
    create-entities: func(...) -> result<...>;
}
```

### Function Signatures

Functions follow a consistent pattern:

```wit
/// Operation description
operation-name: func(input: type) -> result<output-type, string>;
```

This provides:
- Clear success/failure semantics
- String error messages for debugging
- Type-safe input validation

## Build Process

### Tools Required

1. **jco** - JavaScript Component Tools from Bytecode Alliance
2. **wit-docs-inject** - Embeds WIT documentation into component
3. **npm** - Package management

### Build Steps

```bash
# Install dependencies
npm install

# Compile JavaScript to Wasm Component
jco componentize ./memory.js --wit ./wit -o ./memory.wasm

# Inject documentation
wit-docs-inject --component memory.wasm --wit-dir wit/ --inplace
```

### Build Output

- `memory.wasm` - Compiled WebAssembly component (~12MB)
- `memory.cwasm` - Cached precompiled component (fast loading)
- `memory.metadata.json` - Tool schemas for MCP integration

## Performance Characteristics

### First Load
- **Compilation Time**: ~3-4 minutes (JavaScript components are larger)
- **Result**: Precompiled component cached for future use

### Cached Load
- **Load Time**: ~33ms
- **Memory**: ~12MB component + runtime overhead

### Operation Performance
- All operations execute in-memory without I/O
- Linear time complexity for most operations
- Search is O(n*m) where n=entities, m=avg observations per entity

## Testing

### Unit Testing
Currently none - the component is tested through Wassette integration.

### Integration Testing
1. Build the component: `just build`
2. Start Wassette: `just run-memory`
3. Verify load: Check logs for "component loaded component_id=memory"
4. Test with MCP Inspector or AI agent

### Manual Testing Examples

**Create entities:**
```json
{
  "entities": [
    {"name": "Alice", "entityType": "person", "observations": ["Engineer"]},
    {"name": "Acme", "entityType": "company", "observations": ["Tech startup"]}
  ]
}
```

**Create relations:**
```json
{
  "relations": [
    {"fromEntity": "Alice", "toEntity": "Acme", "relationType": "works-for"}
  ]
}
```

## Limitations and Future Work

### Current Limitations

1. **No Persistence**: State lost on component restart
2. **No File Migration**: Can't import existing memory.json/jsonl files
3. **Memory Bounds**: Large graphs limited by Wasm linear memory (2GB default)
4. **No Transactions**: Operations aren't atomic across multiple calls

### Potential Enhancements

1. **Add WASI Filesystem Support**
   - Import `wasi:filesystem` interfaces
   - Implement file-based persistence
   - Maintain backward compatibility with JSONL format

2. **Optimize Memory Usage**
   - Use more compact data structures
   - Implement lazy loading for large graphs
   - Add memory limits to policy

3. **Enhanced Query Capabilities**
   - Graph traversal functions
   - Complex query language
   - Aggregation operations

4. **Export/Import Operations**
   - Serialize graph to JSON/JSONL
   - Import from various formats
   - Snapshot/restore functionality

## Migration Checklist for Similar Projects

- [ ] Identify all file I/O operations
- [ ] Decide on persistence strategy (in-memory vs WASI filesystem)
- [ ] Map TypeScript types to WIT types
- [ ] Check for reserved keyword conflicts
- [ ] Convert async/await patterns
- [ ] Implement result types for error handling
- [ ] Update all field references consistently
- [ ] Create comprehensive documentation
- [ ] Test with actual workloads
- [ ] Plan for data migration (if needed)

## Resources

- [Original MCP Memory Server](https://github.com/modelcontextprotocol/servers/blob/main/src/memory/index.ts)
- [WebAssembly Component Model](https://component-model.bytecodealliance.org/)
- [WIT Specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)
- [JavaScript Component Tools (jco)](https://github.com/bytecodealliance/jco)
- [Wassette Documentation](https://microsoft.github.io/wassette/)

## Contributors

This migration was performed as a demonstration of converting MCP servers to Wassette components. The goal was to document the process and identify patterns for future migrations.
