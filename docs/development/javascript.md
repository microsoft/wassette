# Authoring WebAssembly Components from JavaScript/TypeScript

This guide provides detailed instructions on how to create WebAssembly (Wasm) Components using JavaScript or TypeScript that can be used as Tools for AI Agents with Wassette. WebAssembly Components are portable, secure, and language-agnostic modules that can be executed across different environments.

## Table of Contents

1. [Prerequisites](#prerequisites)
2. [Understanding WebAssembly Components](#understanding-webassembly-components)
3. [Project Setup](#project-setup)
4. [WIT (WebAssembly Interface Types)](#wit-webassembly-interface-types)
5. [Writing Component Code](#writing-component-code)
6. [Building Components](#building-components)
7. [WASI Interfaces and Imports](#wasi-interfaces-and-imports)
8. [Testing and Debugging](#testing-and-debugging)
9. [Best Practices](#best-practices)
10. [Examples](#examples)

## Prerequisites

Before you begin, ensure you have the following tools installed:

- **Node.js** (version 18 or later)
- **npm** or **yarn** package manager
- **jco** - The JavaScript Component Tools from the Bytecode Alliance

### Installing jco

Install the JavaScript Component Tools globally:

```bash
npm install -g @bytecodealliance/jco
```

Alternatively, you can install the required dependencies locally in your project:

```bash
npm install @bytecodealliance/jco @bytecodealliance/componentize-js
```

## Understanding WebAssembly Components

WebAssembly Components are a higher-level abstraction built on top of WebAssembly modules. They provide:

- **Interface Definition**: Components define clear interfaces using WIT (WebAssembly Interface Types)
- **Composition**: Components can import and export functionality from other components
- **Security**: Components run in a sandboxed environment with explicit permissions
- **Portability**: Components can run across different hosts and platforms

### Key Concepts

- **WIT (WebAssembly Interface Types)**: An IDL for defining component interfaces
- **World**: A complete description of a component's imports and exports
- **Package**: A collection of interfaces and worlds that can be versioned
- **Instance**: A runtime instantiation of a component

## Project Setup

### 1. Initialize Your Project

Create a new directory for your component and initialize a Node.js project:

```bash
mkdir my-component
cd my-component
npm init -y
```

### 2. Install Dependencies

Add the required dependencies to your `package.json`:

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

Install the dependencies:

```bash
npm install
```

### 3. Project Structure

A typical JavaScript component project has the following structure:

```
my-component/
├── package.json
├── main.js          # Your component's JavaScript code
├── wit/             # WIT interface definitions
│   ├── world.wit    # Main world definition
│   └── deps/        # External interface dependencies
└── README.md
```

## WIT (WebAssembly Interface Types)

WIT is an Interface Definition Language (IDL) used to define the interfaces of WebAssembly components. It specifies what functions a component exports and what dependencies it imports.

### Basic WIT Syntax

#### Defining an Interface

```wit
// Define a package
package local:my-component;

// Define an interface
interface calculator {
    add: func(a: s32, b: s32) -> s32;
    divide: func(a: f64, b: f64) -> result<f64, string>;
}
```

#### Defining a World

A world describes the complete interface of a component:

```wit
package local:my-component;

interface calculator {
    add: func(a: s32, b: s32) -> s32;
    subtract: func(a: s32, b: s32) -> s32;
}

world my-component {
    export calculator;
}
```

#### Types in WIT

WIT supports various data types:

| WIT Type | Description | JavaScript Equivalent |
|----------|-------------|----------------------|
| `bool` | Boolean value | `boolean` |
| `s8`, `s16`, `s32`, `s64` | Signed integers | `number` or `bigint` |
| `u8`, `u16`, `u32`, `u64` | Unsigned integers | `number` or `bigint` |
| `float32`, `float64` | Floating point | `number` |
| `char` | Unicode code point | `string` (single character) |
| `string` | UTF-8 string | `string` |
| `list<T>` | Array of type T | `Array<T>` |
| `record { ... }` | Struct-like type | Object with named fields |
| `variant { ... }` | Tagged union | Object with tag field |
| `option<T>` | Optional value | `T | undefined` |
| `result<T, E>` | Success/Error type | `{ tag: "ok", val: T } | { tag: "err", val: E }` |

### Complex Types Examples

#### Records

```wit
record person {
    name: string,
    age: u32,
    email: option<string>,
}

interface people {
    create-person: func(name: string, age: u32) -> person;
}
```

#### Variants

```wit
variant status {
    loading,
    success(string),
    error(string),
}

interface api {
    get-status: func() -> status;
}
```

#### Results

```wit
interface math {
    divide: func(a: f64, b: f64) -> result<f64, string>;
}
```

## Writing Component Code

### Basic Component Structure

Here's how to structure your JavaScript code to match a WIT interface:

**wit/world.wit:**
```wit
package local:example;

interface calculator {
    add: func(a: s32, b: s32) -> s32;
    multiply: func(a: f64, b: f64) -> f64;
}

world calculator-component {
    export calculator;
}
```

**main.js:**
```javascript
// Export functions that match the WIT interface
export const calculator = {
    add(a, b) {
        return a + b;
    },
    
    multiply(a, b) {
        return a * b;
    }
};
```

### Handling Complex Types

#### Working with Records

**WIT:**
```wit
record user {
    id: u32,
    name: string,
    active: bool,
}

interface users {
    create-user: func(name: string) -> user;
    get-user: func(id: u32) -> option<user>;
}
```

**JavaScript:**
```javascript
let nextId = 1;
const users = new Map();

export const users = {
    createUser(name) {
        const user = {
            id: nextId++,
            name,
            active: true
        };
        users.set(user.id, user);
        return user;
    },
    
    getUser(id) {
        return users.get(id); // Returns undefined if not found
    }
};
```

#### Working with Results

**WIT:**
```wit
interface math {
    divide: func(dividend: f64, divisor: f64) -> result<f64, string>;
}
```

**JavaScript:**
```javascript
export const math = {
    divide(dividend, divisor) {
        if (divisor === 0) {
            // Return error result
            return { tag: "err", val: "Division by zero" };
        }
        // Return success result
        return { tag: "ok", val: dividend / divisor };
    }
};
```

### Async Functions

Components can export async functions, which is useful for I/O operations:

```javascript
export const api = {
    async fetchData(url) {
        try {
            const response = await fetch(url);
            const data = await response.json();
            return { tag: "ok", val: JSON.stringify(data) };
        } catch (error) {
            return { tag: "err", val: error.message };
        }
    }
};
```

## Building Components

### Using jco to Build Components

The primary tool for building JavaScript components is `jco` (JavaScript Component Tools).

#### Basic Build Command

```bash
# Build a component from JavaScript
jco componentize main.js --wit ./wit -o component.wasm
```

#### Build with Dependencies

If your component uses WASI interfaces or other dependencies:

```bash
# Build with specific WASI dependencies
jco componentize main.js --wit ./wit -d http -d random -d stdio -o component.wasm
```

Common WASI dependencies:
- `http` - HTTP client capabilities
- `random` - Random number generation
- `stdio` - Standard input/output
- `filesystem` - File system access
- `clocks` - Time and clock access

#### Using npm Scripts

Add build scripts to your `package.json`:

```json
{
  "scripts": {
    "build": "jco componentize main.js --wit ./wit -o component.wasm",
    "build:deps": "jco componentize main.js --wit ./wit -d http -d random -o component.wasm",
    "transpile": "jco transpile component.wasm -o transpiled"
  }
}
```

Then build with:

```bash
npm run build
```

## WASI Interfaces and Imports

WebAssembly System Interface (WASI) provides standardized APIs for system operations. JavaScript components can import WASI interfaces to access host capabilities.

### Common WASI Imports

#### Configuration Store

Access environment variables and configuration:

**WIT:**
```wit
world my-component {
    import wasi:config/store@0.2.0-draft;
    export process-config: func() -> result<string, string>;
}
```

**JavaScript:**
```javascript
import { get } from "wasi:config/store@0.2.0-draft";

export async function processConfig() {
    try {
        const apiKey = await get("API_KEY");
        if (!apiKey) {
            return { tag: "err", val: "API_KEY not configured" };
        }
        return { tag: "ok", val: `Using API key: ${apiKey.substring(0, 8)}...` };
    } catch (error) {
        return { tag: "err", val: error.toString() };
    }
}
```

#### HTTP Client

Make HTTP requests:

**JavaScript:**
```javascript
export async function fetchWeather(city) {
    try {
        const response = await fetch(`https://api.weather.com/v1/current?q=${city}`);
        const data = await response.json();
        return { tag: "ok", val: JSON.stringify(data) };
    } catch (error) {
        return { tag: "err", val: error.message };
    }
}
```

#### File System Access

When the filesystem WASI interface is available:

```javascript
import { readFile, writeFile } from "wasi:filesystem/preopens@0.2.0-draft";

export async function processFile(filename) {
    try {
        const content = await readFile(filename);
        const processed = content.toUpperCase();
        await writeFile(`${filename}.processed`, processed);
        return { tag: "ok", val: "File processed successfully" };
    } catch (error) {
        return { tag: "err", val: error.message };
    }
}
```

### Working with Dependencies

Create a `wit/deps` directory for external interface definitions. For example, to use WASI configuration:

1. Create `wit/deps/wasi-config-0.2.0-draft/package.wit`:

```wit
package wasi:config@0.2.0-draft;

interface store {
  variant config-error {
    upstream(string),
    io(string),
  }

  get: func(key: string) -> result<option<string>, config-error>;
  get-all: func() -> result<list<tuple<string, string>>, config-error>;
}

world imports {
  import store;
}
```

2. Import in your main WIT file:

```wit
package local:my-component;

world my-component {
    import wasi:config/store@0.2.0-draft;
    export process: func() -> result<string, string>;
}
```

## Testing and Debugging

### Local Testing

1. **Build your component:**
   ```bash
   npm run build
   ```

2. **Test with Wassette:**
   ```bash
   # Start Wassette with your component directory
   wassette serve --sse --plugin-dir .
   ```

3. **Use the MCP Inspector to test:**
   ```bash
   npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse
   ```

### Debugging Techniques

#### Console Logging

Use console.log for debugging (output appears in Wassette logs):

```javascript
export const debug = {
    logMessage(message) {
        console.log(`[DEBUG] ${new Date().toISOString()}: ${message}`);
        return `Logged: ${message}`;
    }
};
```

#### Error Handling

Always use proper error handling:

```javascript
export const api = {
    async safeOperation(input) {
        try {
            // Your operation here
            const result = await someAsyncOperation(input);
            return { tag: "ok", val: result };
        } catch (error) {
            console.error("Operation failed:", error);
            return { 
                tag: "err", 
                val: `Operation failed: ${error.message}` 
            };
        }
    }
};
```

#### Testing with TypeScript

For better type safety, you can write your component in TypeScript:

**main.ts:**
```typescript
interface User {
    id: number;
    name: string;
    email?: string;
}

interface UserService {
    createUser(name: string, email?: string): User;
    getUser(id: number): User | undefined;
}

let nextId = 1;
const users = new Map<number, User>();

export const userService: UserService = {
    createUser(name: string, email?: string): User {
        const user: User = {
            id: nextId++,
            name,
            email
        };
        users.set(user.id, user);
        return user;
    },
    
    getUser(id: number): User | undefined {
        return users.get(id);
    }
};
```

Build TypeScript components:

```bash
# Compile TypeScript first
npx tsc main.ts --target es2022 --module es2022
# Then componentize
jco componentize main.js --wit ./wit -o component.wasm
```

## Best Practices

### 1. Interface Design

- **Keep interfaces simple**: Start with simple function signatures and evolve as needed
- **Use appropriate types**: Choose the most specific WIT types for your data
- **Handle errors gracefully**: Use `result<T, E>` types for operations that can fail
- **Document your interfaces**: Add comments to your WIT files

### 2. Error Handling

```javascript
// Good: Consistent error handling
export const api = {
    async fetchData(url) {
        try {
            const response = await fetch(url);
            if (!response.ok) {
                return { 
                    tag: "err", 
                    val: `HTTP ${response.status}: ${response.statusText}` 
                };
            }
            const data = await response.text();
            return { tag: "ok", val: data };
        } catch (error) {
            return { 
                tag: "err", 
                val: `Network error: ${error.message}` 
            };
        }
    }
};
```

### 3. Security

- **Minimal permissions**: Only request the permissions your component actually needs
- **Validate inputs**: Always validate and sanitize inputs from the host
- **Secure secrets**: Use the configuration store for API keys and sensitive data

### 4. Performance

- **Avoid blocking operations**: Use async/await for I/O operations
- **Cache when appropriate**: Cache expensive computations or API calls
- **Minimize memory usage**: Be mindful of memory allocation in long-running components

### 5. Testing

- **Unit test your logic**: Test your component functions independently
- **Integration testing**: Test the complete component with Wassette
- **Error path testing**: Test error conditions and edge cases

## Examples

### Example 1: Simple Calculator

**wit/world.wit:**
```wit
package local:calculator;

interface math {
    add: func(a: f64, b: f64) -> f64;
    subtract: func(a: f64, b: f64) -> f64;
    multiply: func(a: f64, b: f64) -> f64;
    divide: func(a: f64, b: f64) -> result<f64, string>;
}

world calculator {
    export math;
}
```

**main.js:**
```javascript
export const math = {
    add(a, b) {
        return a + b;
    },
    
    subtract(a, b) {
        return a - b;
    },
    
    multiply(a, b) {
        return a * b;
    },
    
    divide(a, b) {
        if (b === 0) {
            return { tag: "err", val: "Division by zero" };
        }
        return { tag: "ok", val: a / b };
    }
};
```

### Example 2: Time Server (Based on time-server-js example)

**wit/world.wit:**
```wit
package local:time-server;

interface time {
    get-current-time: func() -> string;
}

world time-server {
    export time;
}
```

**main.js:**
```javascript
async function getCurrentTime() {
    return new Date().toISOString();
}

export const time = {
    getCurrentTime
};
```

**package.json:**
```json
{
  "name": "time-server",
  "type": "module",
  "scripts": {
    "build": "jco componentize main.js --wit ./wit -o time.wasm"
  },
  "dependencies": {
    "@bytecodealliance/componentize-js": "^0.18.1",
    "@bytecodealliance/jco": "^1.11.1"
  }
}
```

### Example 3: Weather Service (Based on get-weather-js example)

**wit/world.wit:**
```wit
package local:weather;

world weather-service {
    import wasi:config/store@0.2.0-draft;
    export get-weather: func(city: string) -> result<string, string>;
}
```

**main.js:**
```javascript
import { get } from "wasi:config/store@0.2.0-draft";

export async function getWeather(city) {
    try {
        const apiKey = await get("OPENWEATHER_API_KEY");
        if (!apiKey) {
            return { tag: "err", val: "API key not configured" };
        }
        
        const response = await fetch(
            `https://api.openweathermap.org/data/2.5/weather?q=${city}&appid=${apiKey}&units=metric`
        );
        
        if (!response.ok) {
            return { tag: "err", val: `Weather API error: ${response.status}` };
        }
        
        const data = await response.json();
        const temp = data.main.temp;
        
        return { tag: "ok", val: `${temp}°C` };
    } catch (error) {
        return { tag: "err", val: `Error: ${error.message}` };
    }
}
```

### Example 4: Data Processing with File System

**wit/world.wit:**
```wit
package local:processor;

record processing-result {
    lines-processed: u32,
    errors: list<string>,
}

interface processor {
    process-file: func(input-path: string, output-path: string) -> result<processing-result, string>;
}

world file-processor {
    export processor;
}
```

**main.js:**
```javascript
import { readFile, writeFile } from "node:fs/promises";

export const processor = {
    async processFile(inputPath, outputPath) {
        try {
            const content = await readFile(inputPath, 'utf8');
            const lines = content.split('\n');
            const errors = [];
            let processedLines = 0;
            
            const processedContent = lines.map((line, index) => {
                try {
                    // Example processing: convert to uppercase
                    processedLines++;
                    return line.toUpperCase();
                } catch (error) {
                    errors.push(`Line ${index + 1}: ${error.message}`);
                    return line; // Keep original on error
                }
            }).join('\n');
            
            await writeFile(outputPath, processedContent);
            
            const result = {
                linesProcessed: processedLines,
                errors
            };
            
            return { tag: "ok", val: result };
        } catch (error) {
            return { tag: "err", val: `File processing failed: ${error.message}` };
        }
    }
};
```

## Conclusion

WebAssembly Components provide a powerful way to create portable, secure, and efficient tools. By following this guide, you can:

1. Create well-structured JavaScript/TypeScript components
2. Define clear interfaces using WIT
3. Handle complex data types and error conditions
4. Configure appropriate security policies
5. Test and debug your components effectively

For more information, refer to:
- [WebAssembly Component Model](https://component-model.bytecodealliance.org/)
- [WIT Specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)
- [jco Documentation](https://bytecodealliance.github.io/jco/)
- [WASI Preview 2](https://github.com/WebAssembly/WASI/tree/main/wasip2)

Happy component building!