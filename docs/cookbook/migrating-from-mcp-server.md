# Migrating from JavaScript MCP Servers to Wassette Components

This guide helps you migrate an existing JavaScript-based MCP server to a Wassette WebAssembly component. This migration provides better security through sandboxing, improved portability, and fine-grained permission control.

## Overview

Traditional MCP servers run as standalone Node.js processes that communicate via stdio or HTTP. Wassette components run as sandboxed WebAssembly modules with explicit capability declarations. This guide walks you through the migration process step by step.

## Key Differences

### Traditional MCP Server
```javascript
// Traditional MCP server structure
import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';

const server = new Server({
  name: 'my-tool',
  version: '1.0.0',
}, {
  capabilities: {
    tools: {}
  }
});

// Register tools
server.setRequestHandler(ListToolsRequestSchema, async () => {
  return {
    tools: [{
      name: "myTool",
      description: "Does something useful",
      inputSchema: { /* ... */ }
    }]
  };
});

server.setRequestHandler(CallToolRequestSchema, async (request) => {
  // Tool implementation
  return { content: [{ type: "text", text: result }] };
});

// Start server
const transport = new StdioServerTransport();
await server.connect(transport);
```

### Wassette Component
```javascript
// Wassette component - just the business logic
export async function myTool(input) {
  // Tool implementation - same logic!
  return result;
}
```

**Key benefits of the Wassette approach:**
- **Simpler**: No server boilerplate, just implement your business logic
- **Secure**: Sandboxed execution with explicit permissions
- **Portable**: Runs anywhere WebAssembly is supported
- **Efficient**: Lower overhead than full Node.js process

## Migration Steps

### Step 1: Analyze Your MCP Server

First, identify what your MCP server does:

1. **List your tools**: What tools does your server expose?
2. **Identify dependencies**: What external resources does it use? (files, network, etc.)
3. **Note I/O patterns**: Does it read files, make HTTP requests, use environment variables?
4. **Check for MCP-specific code**: Most of this will be removed

Example analysis of a weather MCP server:
```javascript
// Original MCP server
server.setRequestHandler(CallToolRequestSchema, async (request) => {
  if (request.params.name === "get_weather") {
    const city = request.params.arguments.city;
    const apiKey = process.env.WEATHER_API_KEY;
    
    // Make HTTP request
    const response = await fetch(`https://api.weather.com/${city}`);
    const data = await response.json();
    
    return {
      content: [{
        type: "text",
        text: JSON.stringify(data)
      }]
    };
  }
});
```

**Analysis:**
- ✅ One tool: `get_weather`
- ✅ Network access needed: `api.weather.com`
- ✅ Environment variable: `WEATHER_API_KEY`
- ✅ Async/await usage (supported)

### Step 2: Create Component Structure

Create a new directory for your component:

```bash
mkdir my-component
cd my-component
npm init -y
```

Update `package.json`:
```json
{
  "type": "module",
  "dependencies": {
    "@bytecodealliance/componentize-js": "^0.18.1",
    "@bytecodealliance/jco": "^1.11.1"
  },
  "scripts": {
    "build": "jco componentize -w ./wit main.js -o component.wasm"
  }
}
```

Install dependencies:
```bash
npm install
```

### Step 3: Define WIT Interface

Create `wit/world.wit` to define your component's interface. This replaces the MCP tool schema.

**Original MCP tool schema:**
```javascript
{
  name: "get_weather",
  description: "Get weather for a city",
  inputSchema: {
    type: "object",
    properties: {
      city: { type: "string", description: "City name" }
    },
    required: ["city"]
  }
}
```

**Equivalent WIT interface:**
```wit
package local:weather;

/// Get weather information for a city
world weather-component {
    /// Fetch current weather for the specified city
    /// Returns temperature in Celsius as a string
    export get-weather: func(city: string) -> result<string, string>;
}
```

**WIT Type Mapping:**

| MCP Schema Type | WIT Type | JavaScript Type |
|----------------|----------|----------------|
| `string` | `string` | `string` |
| `number` | `f64` or `s32` | `number` |
| `integer` | `s32`, `s64`, `u32`, `u64` | `number` |
| `boolean` | `bool` | `boolean` |
| `array` | `list<T>` | `Array` |
| `object` | `record { ... }` | `object` |
| N/A | `result<T, E>` | `{ tag: "ok", val: T }` or `{ tag: "err", val: E }` |

### Step 4: Migrate Business Logic

Extract the core logic from your MCP server, removing all MCP-specific code:

**Before (MCP Server):**
```javascript
import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { CallToolRequestSchema } from '@modelcontextprotocol/sdk/types.js';

server.setRequestHandler(CallToolRequestSchema, async (request) => {
  if (request.params.name === "get_weather") {
    const { city } = request.params.arguments;
    const apiKey = process.env.WEATHER_API_KEY;
    
    try {
      const response = await fetch(
        `https://api.openweathermap.org/data/2.5/weather?q=${city}&appid=${apiKey}`
      );
      const data = await response.json();
      const temp = data.main.temp;
      
      return {
        content: [{
          type: "text",
          text: `Temperature in ${city}: ${temp}°C`
        }]
      };
    } catch (error) {
      return {
        content: [{
          type: "text",
          text: `Error: ${error.message}`
        }],
        isError: true
      };
    }
  }
});
```

**After (Wassette Component):**
```javascript
// main.js
import { get } from "wasi:config/store@0.2.0-draft";

export async function getWeather(city) {
  // Get API key from configuration (replaces process.env)
  const apiKey = await get("WEATHER_API_KEY");
  if (!apiKey) {
    throw "WEATHER_API_KEY not configured";
  }
  
  try {
    // Same fetch logic - WebAssembly supports it!
    const response = await fetch(
      `https://api.openweathermap.org/data/2.5/weather?q=${city}&appid=${apiKey}`
    );
    
    if (!response.ok) {
      throw `HTTP error ${response.status}`;
    }
    
    const data = await response.json();
    const temp = data.main.temp;
    
    // Return just the result - no MCP wrapping
    return `Temperature in ${city}: ${temp}°C`;
  } catch (error) {
    // Throw errors - they'll be caught by the runtime
    throw error.message || String(error);
  }
}
```

### Step 5: Adapt Common Patterns

#### Environment Variables
**MCP Server:**
```javascript
const apiKey = process.env.API_KEY;
const debug = process.env.DEBUG === 'true';
```

**Wassette Component:**
```javascript
import { get } from "wasi:config/store@0.2.0-draft";

const apiKey = await get("API_KEY");
const debugStr = await get("DEBUG");
const debug = debugStr === "true";
```

#### HTTP Requests
**MCP Server:**
```javascript
import fetch from 'node-fetch'; // or built-in fetch

const response = await fetch(url);
```

**Wassette Component:**
```javascript
// Use global fetch - it's built into WASI
const response = await fetch(url);

// Or import explicitly
import { fetch } from 'wasi:http/outgoing-handler';
```

#### File System Access
**MCP Server:**
```javascript
import fs from 'fs/promises';

const content = await fs.readFile('./config.json', 'utf-8');
await fs.writeFile('./output.txt', data);
```

**Wassette Component:**
```javascript
// Note: File system access requires explicit permissions
// and WASI filesystem interfaces

// For simple cases, consider using configuration store instead:
import { get } from "wasi:config/store@0.2.0-draft";
const config = await get("CONFIG_JSON");
```

#### Error Handling
**MCP Server:**
```javascript
try {
  const result = await doSomething();
  return {
    content: [{ type: "text", text: result }]
  };
} catch (error) {
  return {
    content: [{ type: "text", text: error.message }],
    isError: true
  };
}
```

**Wassette Component:**
```javascript
// For functions that return result<T, string>
try {
  const result = await doSomething();
  return result; // No wrapping needed
} catch (error) {
  throw error.message || String(error);
}

// Or use explicit result type:
export async function myTool(input) {
  try {
    const result = await doSomething();
    return { tag: "ok", val: result };
  } catch (error) {
    return { tag: "err", val: error.message };
  }
}
```

### Step 6: Build Your Component

Build the component with required WASI dependencies:

```bash
# Basic build
npm run build

# Or with specific WASI dependencies
jco componentize main.js --wit ./wit -d http -d random -d clocks -o component.wasm
```

**Common WASI dependencies:**
- `http` - HTTP client (for fetch)
- `random` - Random number generation
- `clocks` - Time and date functions
- `stdio` - Standard input/output
- `filesystem` - File system access (requires permissions)

### Step 7: Configure Permissions

Create a `policy.yaml` file to define what your component can access:

```yaml
version: "1.0"
description: "Weather service component permissions"
permissions:
  network:
    allow:
      - host: "api.openweathermap.org"
        protocols: ["https"]
  
  config:
    allow:
      - key: "WEATHER_API_KEY"
        access: ["read"]
```

Without explicit permissions, your component runs fully sandboxed with no external access.

### Step 8: Test Your Component

Test locally with Wassette:

```bash
# Load component with policy
wassette serve --stdio --plugin-dir .

# Set configuration
# (via MCP client or CLI)
```

For more testing options, see the [CLI reference](../reference/cli.md).

## Complete Migration Example

Let's migrate a complete MCP server that provides weather information.

### Original MCP Server

**Directory structure:**
```
weather-mcp-server/
├── package.json
├── index.js
└── .env
```

**package.json:**
```json
{
  "name": "weather-mcp-server",
  "version": "1.0.0",
  "type": "module",
  "dependencies": {
    "@modelcontextprotocol/sdk": "^0.5.0"
  },
  "scripts": {
    "start": "node index.js"
  }
}
```

**index.js:**
```javascript
import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
} from '@modelcontextprotocol/sdk/types.js';

const server = new Server(
  {
    name: 'weather-server',
    version: '1.0.0',
  },
  {
    capabilities: {
      tools: {},
    },
  }
);

server.setRequestHandler(ListToolsRequestSchema, async () => {
  return {
    tools: [
      {
        name: 'get_weather',
        description: 'Get current weather for a city',
        inputSchema: {
          type: 'object',
          properties: {
            city: {
              type: 'string',
              description: 'City name',
            },
          },
          required: ['city'],
        },
      },
    ],
  };
});

server.setRequestHandler(CallToolRequestSchema, async (request) => {
  if (request.params.name !== 'get_weather') {
    throw new Error(`Unknown tool: ${request.params.name}`);
  }

  const { city } = request.params.arguments;
  const apiKey = process.env.OPENWEATHER_API_KEY;

  try {
    const geoResponse = await fetch(
      `https://api.openweathermap.org/geo/1.0/direct?q=${city}&limit=1&appid=${apiKey}`
    );
    const geoData = await geoResponse.json();
    const { lat, lon } = geoData[0];

    const weatherResponse = await fetch(
      `https://api.openweathermap.org/data/2.5/weather?lat=${lat}&lon=${lon}&appid=${apiKey}&units=metric`
    );
    const weatherData = await weatherResponse.json();
    const temp = weatherData.main.temp;

    return {
      content: [
        {
          type: 'text',
          text: `Temperature in ${city}: ${temp}°C`,
        },
      ],
    };
  } catch (error) {
    return {
      content: [
        {
          type: 'text',
          text: `Error: ${error.message}`,
        },
      ],
      isError: true,
    };
  }
});

async function main() {
  const transport = new StdioServerTransport();
  await server.connect(transport);
}

main().catch(console.error);
```

**.env:**
```
OPENWEATHER_API_KEY=your_api_key_here
```

### Migrated Wassette Component

**Directory structure:**
```
weather-component/
├── package.json
├── weather.js
├── policy.yaml
└── wit/
    └── world.wit
```

**package.json:**
```json
{
  "type": "module",
  "dependencies": {
    "@bytecodealliance/componentize-js": "^0.18.1",
    "@bytecodealliance/jco": "^1.11.1"
  },
  "scripts": {
    "build": "jco componentize -w ./wit weather.js -o weather.wasm"
  }
}
```

**weather.js:**
```javascript
import { get } from "wasi:config/store@0.2.0-draft";

export async function getWeather(city) {
  const apiKey = await get("OPENWEATHER_API_KEY");
  if (!apiKey) {
    throw "OPENWEATHER_API_KEY not configured";
  }

  try {
    const geoResponse = await fetch(
      `https://api.openweathermap.org/geo/1.0/direct?q=${city}&limit=1&appid=${apiKey}`
    );
    
    if (!geoResponse.ok) {
      throw `Failed to fetch geo data: ${geoResponse.status}`;
    }
    
    const geoData = await geoResponse.json();
    const { lat, lon } = geoData[0];

    const weatherResponse = await fetch(
      `https://api.openweathermap.org/data/2.5/weather?lat=${lat}&lon=${lon}&appid=${apiKey}&units=metric`
    );
    
    if (!weatherResponse.ok) {
      throw `Failed to fetch weather data: ${weatherResponse.status}`;
    }
    
    const weatherData = await weatherResponse.json();
    const temp = weatherData.main.temp;

    return `Temperature in ${city}: ${temp}°C`;
  } catch (error) {
    throw error.message || String(error);
  }
}
```

**wit/world.wit:**
```wit
package local:weather;

/// Weather information service
world weather-component {
    /// Get current weather temperature for a city
    ///
    /// # Parameters
    /// * `city` - Name of the city to get weather for
    ///
    /// # Returns
    /// Temperature in Celsius as a formatted string
    ///
    /// # Errors
    /// Returns error if API key is missing, city not found, or API request fails
    export get-weather: func(city: string) -> result<string, string>;
}
```

**policy.yaml:**
```yaml
version: "1.0"
description: "Weather service permissions"
permissions:
  network:
    allow:
      - host: "api.openweathermap.org"
        protocols: ["https"]
  
  config:
    allow:
      - key: "OPENWEATHER_API_KEY"
        access: ["read"]
```

**Build and run:**
```bash
# Install dependencies
npm install

# Build component
npm run build

# Test with Wassette
wassette serve --stdio --plugin-dir .
```

## Common Migration Challenges

### 1. Multiple Tools in One Server

**MCP Server with multiple tools:**
```javascript
server.setRequestHandler(ListToolsRequestSchema, async () => {
  return {
    tools: [
      { name: 'tool1', /* ... */ },
      { name: 'tool2', /* ... */ },
      { name: 'tool3', /* ... */ }
    ]
  };
});
```

**Wassette approach:**

**Option A: Multiple exports (recommended)**
```javascript
// main.js
export async function tool1(input) { /* ... */ }
export async function tool2(input) { /* ... */ }
export async function tool3(input) { /* ... */ }
```

```wit
world my-tools {
    export tool1: func(input: string) -> string;
    export tool2: func(input: string) -> string;
    export tool3: func(input: string) -> string;
}
```

**Option B: Separate components**
Create three separate components, each with its own directory, WIT file, and implementation.

### 2. Streaming Responses

MCP servers can stream responses. WebAssembly components currently don't support streaming in the same way, but you can:

1. **Return complete results**: Most use cases work fine with complete responses
2. **Use chunked patterns**: Call the tool multiple times with continuation tokens
3. **Buffer and return**: Collect streaming data and return it all at once

### 3. State Management

**MCP Server with state:**
```javascript
let cache = {};

server.setRequestHandler(CallToolRequestSchema, async (request) => {
  if (request.params.name === 'get_cached') {
    return cache[request.params.arguments.key];
  }
});
```

**Wassette approach:**

Components are stateless between invocations. For state:
1. **Use external storage**: Database, Redis, etc.
2. **Pass state in requests**: Include previous state in input
3. **Use MCP client-side state**: Let the client maintain state

### 4. Dynamic Tool Discovery

MCP servers can dynamically list tools. In Wassette, tools are defined statically in WIT at compile time. If you need dynamic behavior:

1. **Create separate components** for different tool sets
2. **Use parameters** to control behavior within a tool
3. **Version your components** for different capabilities

## Migration Checklist

Use this checklist to track your migration:

- [ ] Analyzed MCP server structure and identified all tools
- [ ] Listed all external dependencies (network, files, env vars)
- [ ] Created new component directory structure
- [ ] Installed Wassette tooling (`jco`, `componentize-js`)
- [ ] Created WIT interface definition
- [ ] Migrated business logic (removed MCP SDK code)
- [ ] Replaced `process.env` with WASI config store
- [ ] Adapted HTTP requests to WASI fetch
- [ ] Adapted file access to WASI filesystem (if needed)
- [ ] Updated error handling to use result types or exceptions
- [ ] Created `policy.yaml` with required permissions
- [ ] Built component successfully
- [ ] Tested component with Wassette
- [ ] Verified permissions work as expected
- [ ] Documented any behavior changes
- [ ] Updated deployment configuration

## Testing Your Migration

1. **Unit test business logic:**
```bash
# Your logic works the same in Node.js
node --test weather.test.js
```

2. **Build the component:**
```bash
npm run build
```

3. **Test with Wassette:**
```bash
wassette serve --stdio --plugin-dir .
```

4. **Verify permissions:**
```bash
# Check that unauthorized access is blocked
# Try accessing a different domain, reading a different config key, etc.
```

## Performance Considerations

- **Cold start**: WebAssembly components have very fast cold starts (milliseconds)
- **Memory**: Components use less memory than full Node.js processes
- **Network**: WASI HTTP has similar performance to Node.js fetch
- **Computation**: Near-native performance for compute-intensive tasks

## Security Benefits

After migration, you get:

1. **Sandboxing**: Component can only access explicitly permitted resources
2. **Least privilege**: Grant only the minimum permissions needed
3. **Auditability**: Policy files make permissions explicit and reviewable
4. **Isolation**: Multiple components can't interfere with each other
5. **Portability**: Same security model across all platforms

## Next Steps

- **Read the complete [JavaScript guide](./javascript.md)** for more details
- **Explore [working examples](https://github.com/microsoft/wassette/tree/main/examples)** in the repository
- **Review [permission system](../design/permission-system.md)** documentation
- **Check [FAQ](../faq.md)** for common questions
- **Join the community** for support and best practices

## Getting Help

If you encounter issues during migration:

1. Check the [FAQ](../faq.md) for common problems
2. Review [example components](https://github.com/microsoft/wassette/tree/main/examples)
3. Search [GitHub issues](https://github.com/microsoft/wassette/issues)
4. Ask in [GitHub Discussions](https://github.com/microsoft/wassette/discussions)

## Additional Resources

- [WebAssembly Component Model](https://component-model.bytecodealliance.org/)
- [WASI Preview 2](https://github.com/WebAssembly/WASI/blob/main/legacy/preview2/README.md)
- [Model Context Protocol Specification](https://github.com/modelcontextprotocol/specification)
- [Bytecode Alliance jco](https://github.com/bytecodealliance/jco)
