# Migrating from JavaScript MCP Servers to Wassette Components

Traditional MCP servers run as standalone Node.js processes with full system access. Wassette components run as sandboxed WebAssembly modules with explicit permissions. The key difference is that you write just the business logic—no server boilerplate, better security, and cleaner code.

## Migration Example

Here's how to convert a weather MCP server to a Wassette component:

### Before: Traditional MCP Server

**package.json:**
```json
{
  "dependencies": {
    "@modelcontextprotocol/sdk": "^0.5.0"
  }
}
```

**index.js:**
**index.js:**
```javascript
import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';

const server = new Server({
  name: 'weather-server',
  version: '1.0.0'
}, {
  capabilities: { tools: {} }
});

server.setRequestHandler(ListToolsRequestSchema, async () => ({
  tools: [{
    name: 'get_weather',
    description: 'Get current weather for a city',
    inputSchema: {
      type: 'object',
      properties: {
        city: { type: 'string', description: 'City name' }
      },
      required: ['city']
    }
  }]
}));

server.setRequestHandler(CallToolRequestSchema, async (request) => {
  const { city } = request.params.arguments;
  const apiKey = process.env.WEATHER_API_KEY;
  
  const response = await fetch(
    `https://api.openweathermap.org/data/2.5/weather?q=${city}&appid=${apiKey}`
  );
  const data = await response.json();
  
  return {
    content: [{
      type: 'text',
      text: `Temperature: ${data.main.temp}°C`
    }]
  };
});

const transport = new StdioServerTransport();
await server.connect(transport);
```

**Total:** ~60 lines of boilerplate + business logic.

### After: Wassette Component

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

**wit/world.wit:**
```wit
package local:weather;

world weather-component {
    import wasi:config/store@0.2.0-draft;
    
    /// Get current weather for a city
    export get-weather: func(city: string) -> result<string, string>;
}
```

> **Note**: You'll need the WASI config WIT definitions. Copy them from the [get-weather-js example](https://github.com/microsoft/wassette/tree/main/examples/get-weather-js/wit/deps) or download from the WASI repository.

**weather.js:**
```javascript
import { get } from "wasi:config/store@0.2.0-draft";

export async function getWeather(city) {
  const apiKey = await get("WEATHER_API_KEY");
  if (!apiKey) {
    throw "WEATHER_API_KEY not configured";
  }
  
  const response = await fetch(
    `https://api.openweathermap.org/data/2.5/weather?q=${city}&appid=${apiKey}`
  );
  const data = await response.json();
  
  return `Temperature: ${data.main.temp}°C`;
}
```

**Total:** ~20 lines of business logic.

## Key Changes

1. **No MCP SDK** - Just export your functions directly
2. **Environment variables** - Replace `process.env` with WASI config store (`wasi:config/store`)
3. **Error handling** - Throw errors or return result types instead of MCP response objects
4. **WIT interface** - Define your API in WIT instead of MCP tool schemas
5. **Build** - Run `npm run build` to create the `.wasm` component

## Migration Steps

1. Extract your tool's business logic (the actual work it does)
2. Create a WIT file defining your function signatures
   - Include `import` statements for any WASI interfaces you use (e.g., `wasi:config/store`)
   - Copy required WIT dependencies to `wit/deps/` (see examples for reference)
3. Update environment variable access to use WASI config store
4. Export your functions directly from your JS file
5. Build with `jco componentize`

## Next Steps

- See the [JavaScript/TypeScript guide](./javascript.md) for more details on building components
- Review [example components](https://github.com/microsoft/wassette/tree/main/examples) for working code
- Check the [Permissions reference](../reference/permissions.md) for security configuration

