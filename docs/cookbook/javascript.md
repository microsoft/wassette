# Building Wasm Components with JavaScript/TypeScript

This cookbook guide shows you how to build WebAssembly components using JavaScript or TypeScript that work with Wassette.

## Quick Start

### Prerequisites
- Node.js (version 18 or later)
- npm or yarn package manager

### Install Tools
```bash
npm install -g @bytecodealliance/jco
```

## Step-by-Step Guide

### 1. Create Your Project

```bash
mkdir my-component
cd my-component
npm init -y
```

### 2. Install Dependencies

Add the following to your `package.json`:

```json
{
  "type": "module",
  "dependencies": {
    "@bytecodealliance/componentize-js": "^0.18.1",
    "@bytecodealliance/jco": "^1.11.1"
  },
  "scripts": {
    "build:component": "jco componentize -w ./wit main.js -o component.wasm"
  }
}
```

```bash
npm install
```

### 3. Define Your Interface (WIT)

Create `wit/world.wit`:

```wit
package local:my-component;

interface calculator {
    add: func(a: s32, b: s32) -> s32;
    divide: func(a: f64, b: f64) -> result<f64, string>;
}

world calculator-component {
    export calculator;
}
```

### 4. Implement Your Component

Create `main.js`:

```javascript
export const calculator = {
    add(a, b) {
        return a + b;
    },
    
    divide(a, b) {
        if (b === 0) {
            return { tag: "err", val: "Division by zero" };
        }
        return { tag: "ok", val: a / b };
    }
};
```

### 5. Build Your Component

```bash
# Basic build
jco componentize main.js --wit ./wit -o component.wasm

# Build with WASI dependencies
jco componentize main.js --wit ./wit -d http -d random -d stdio -o component.wasm
```

Common WASI dependencies:
- `http` - HTTP client capabilities
- `random` - Random number generation
- `stdio` - Standard input/output
- `filesystem` - File system access
- `clocks` - Time and clock access

### 6. Inject WIT Documentation

To make your component's documentation available to AI agents, inject the WIT documentation into the compiled WASM binary:

```bash
# Install wit-docs-inject (if not already installed)
cargo install --git https://github.com/Mossaka/wit-docs-inject

# Inject documentation into your component
wit-docs-inject --component component.wasm \
                --wit-dir wit/ \
                --inplace
```

This embeds the documentation from your WIT files as a `package-docs` custom section in the WASM binary. When Wassette loads your component, it extracts this documentation and uses it to describe your tools to AI agents.

For more information, see the [Documenting WIT Interfaces](./documenting-wit.md) guide.

### 7. Test Your Component

```bash
wassette serve --sse --plugin-dir .
```

## Complete Examples

### Simple Time Server

**wit/world.wit:**
```wit
package local:time-server;

world time-server {
    export get-current-time: func() -> string;
}
```

**main.js:**
```javascript
export function getCurrentTime() {
    return new Date().toISOString();
}
```

### HTTP Weather Service

**wit/world.wit:**
```wit
package local:weather;

world weather-service {
    export get-weather: func(location: string) -> result<string, string>;
}
```

**main.js:**
```javascript
import { fetch } from 'wasi:http/outgoing-handler';

export async function getWeather(location) {
    try {
        const response = await fetch(`https://api.weather.com/${location}`);
        const data = await response.json();
        return { tag: "ok", val: JSON.stringify(data) };
    } catch (error) {
        return { tag: "err", val: error.message };
    }
}
```

## Error Handling

JavaScript components use WIT's `result` type for error handling:

```javascript
// Success
return { tag: "ok", val: resultValue };

// Error
return { tag: "err", val: "Error message" };
```

## Using WASI Interfaces

### HTTP Client
```javascript
import { fetch } from 'wasi:http/outgoing-handler';

const response = await fetch('https://api.example.com/data');
```

### Random Numbers
```javascript
import { getRandomBytes } from 'wasi:random/random';

const bytes = getRandomBytes(16);
```

### File System
```javascript
import { read, write } from 'wasi:filesystem/types';

const content = await read('/path/to/file');
await write('/path/to/file', data);
```

## Best Practices

1. **Use clear interface definitions** - Make your WIT interfaces descriptive and well-documented
2. **Handle errors properly** - Always use `result<T, string>` for operations that can fail
3. **Keep components focused** - Each component should do one thing well
4. **Test thoroughly** - Validate your component works before deploying
5. **Document your interfaces** - Use WIT comments to explain your API

## Common Patterns

### Async Operations
```javascript
export async function processData(input) {
    const result = await fetchExternalData(input);
    return result;
}
```

### Type Conversions
```javascript
// WIT types map to JavaScript as follows:
// s32, s64, u32, u64 -> Number
// f32, f64 -> Number
// string -> String
// bool -> Boolean
// list<T> -> Array
// record -> Object
```

### Configuration
```javascript
export const config = {
    timeout: 5000,
    retries: 3
};
```

## Migrating MCP Servers

When migrating existing MCP servers to WebAssembly components, consider these key patterns learned from converting the [MCP memory server](https://github.com/modelcontextprotocol/servers/blob/main/src/memory/index.ts):

### Reserved Keywords

WIT has reserved keywords that may conflict with common field names. For example, `from` and `to` are reserved in WIT syntax.

**Solution**: Use descriptive alternatives:
```wit
// Instead of:
// from: string,
// to: string,

// Use:
from-entity: string,
to-entity: string,
```

### Persistence Strategy

MCP servers often use file-based persistence. The base WebAssembly Component Model doesn't include file I/O.

**Options**:
1. **In-memory storage**: Simplest approach, state persists per component instance
2. **WASI filesystem**: Add `wasi:filesystem` imports for file-based storage
3. **External storage**: Use HTTP client to connect to databases or storage services

For the memory server example, in-memory storage was chosen for simplicity:
```javascript
// Module-level variables persist across function calls
let entities = [];
let relations = [];
```

### Error Handling Patterns

Convert JavaScript exceptions to WIT result types:

**MCP Server** (exceptions):
```javascript
async function addObservation(entityName, observation) {
    const entity = entities.find(e => e.name === entityName);
    if (!entity) {
        throw new Error(`Entity ${entityName} not found`);
    }
    entity.observations.push(observation);
}
```

**Wasm Component** (result types):
```javascript
function addObservation(entityName, observation) {
    try {
        const entity = entities.find(e => e.name === entityName);
        if (!entity) {
            return { tag: "err", val: `Entity ${entityName} not found` };
        }
        entity.observations.push(observation);
        return { tag: "ok", val: entity };
    } catch (error) {
        return { tag: "err", val: error.message };
    }
}
```

### Type Mapping

Map TypeScript interfaces to WIT records within interface scope:

**TypeScript**:
```typescript
interface Entity {
    name: string;
    entityType: string;
    observations: string[];
}
```

**WIT**:
```wit
interface knowledge-graph-ops {
    record entity {
        name: string,
        entity-type: string,
        observations: list<string>,
    }
}
```

**JavaScript** (matches WIT kebab-case):
```javascript
export const knowledgeGraphOps = {
    createEntity(name, entityType, observations) {
        return {
            name,
            entityType,  // camelCase in JS
            observations
        };
    }
};
```

Note: Use kebab-case in WIT, camelCase in JavaScript. The jco tooling handles the conversion.

### State Management

Use module-level variables for component state:

```javascript
// State persists across function calls
let knowledgeGraph = {
    entities: [],
    relations: []
};

export const operations = {
    addEntity(entity) {
        knowledgeGraph.entities.push(entity);
        return { tag: "ok", val: entity };
    },
    
    getGraph() {
        return { tag: "ok", val: knowledgeGraph };
    }
};
```

## Troubleshooting

### Build Errors
- Ensure Node.js version is 18 or later
- Check that WIT interface matches your exports
- Verify all dependencies are installed
- Watch for reserved keywords in WIT field names

### Runtime Errors
- Check WASI permission configuration
- Validate input/output types match WIT interface
- Review Wassette logs for details
- Ensure result types use correct `{tag, val}` format

## Full Documentation

For complete details, including advanced topics, WASI interfaces, and more examples, see the [JavaScript/TypeScript Development Guide](../development/javascript.md).

## Working Examples

See these complete working examples in the repository:
- [time-server-js](https://github.com/microsoft/wassette/tree/main/examples/time-server-js) - Simple time server
- [get-weather-js](https://github.com/microsoft/wassette/tree/main/examples/get-weather-js) - Weather API client
- [get-open-meteo-weather-js](https://github.com/microsoft/wassette/tree/main/examples/get-open-meteo-weather-js) - Open-Meteo weather service
- [memory-js](https://github.com/microsoft/wassette/tree/main/examples/memory-js) - Knowledge graph memory server

## Next Steps

- Review the [complete JavaScript guide](../development/javascript.md)
- Check out [working examples](https://github.com/microsoft/wassette/tree/main/examples)
- Learn about [Wassette's architecture](../design/architecture.md)
- Read the [FAQ](../faq.md)
