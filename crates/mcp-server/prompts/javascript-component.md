# Building a JavaScript WebAssembly Component for Wassette

I'll help you build a WebAssembly component named "{component_name}" using JavaScript.

## Prerequisites
- Node.js (version 18 or later)
- npm or yarn package manager

## Step 1: Install Tools

```bash
npm install -g @bytecodealliance/jco
```

## Step 2: Create Your Project

```bash
mkdir {component_name}
cd {component_name}
npm init -y
```

## Step 3: Install Dependencies

Add to your `package.json`:

```json
{{
  "type": "module",
  "dependencies": {{
    "@bytecodealliance/componentize-js": "^0.18.1",
    "@bytecodealliance/jco": "^1.11.1"
  }},
  "scripts": {{
    "build:component": "jco componentize -w ./wit main.js -o component.wasm"
  }}
}}
```

Then install:

```bash
npm install
```

## Step 4: Define Your WIT Interface

Create `wit/world.wit` (see [WIT reference](https://component-model.bytecodealliance.org/design/wit.html) and [WIT by example](https://component-model.bytecodealliance.org/design/wit-example.html)):

```wit
package local:{component_name};

interface operations {{
    greet: func(name: string) -> string;
}}

world {component_name}-component {{
    export operations;
}}
```

## Step 5: Implement Your Component

Create `main.js`:

```javascript
export const operations = {{
    greet(name) {{
        return `Hello, ${{name}}!`;
    }}
}};
```

## Step 6: Build Your Component

```bash
# Basic build
jco componentize main.js --wit ./wit -o component.wasm

# Build with WASI dependencies (if needed)
jco componentize main.js --wit ./wit -d http -d random -d stdio -o component.wasm
```

Common WASI dependencies:
- `http` - HTTP client capabilities
- `random` - Random number generation
- `stdio` - Standard input/output
- `filesystem` - File system access
- `clocks` - Time and clock access

## Step 7: Inject WIT Documentation (Optional but Recommended)

To make your component's documentation available to AI agents:

```bash
# Install wit-docs-inject (if not already installed)
cargo install --git https://github.com/Mossaka/wit-docs-inject

# Inject documentation into your component
wit-docs-inject --component component.wasm \
                --wit-dir wit/ \
                --inplace
```

## Step 8: Test Your Component

```bash
# Start Wassette with your component
wassette serve --sse --plugin-dir .

# In another terminal, use an MCP client to test
```

## Working with HTTP Requests

To make HTTP requests using the `fetch()` function, add the WASI HTTP dependency when building:

```bash
jco componentize main.js --wit ./wit -d http -o component.wasm
```

Update your WIT interface to import the HTTP handler:

```wit
package local:{component_name};

interface operations {{
    fetch-data: func(url: string) -> result<string, string>;
}}

world {component_name}-component {{
    import wasi:http/outgoing-handler@0.2.0;
    export operations;
}}
```

Then use `fetch()` in your JavaScript code:

```javascript
export const operations = {{
    async fetchData(url) {{
        try {{
            const response = await fetch(url);
            const text = await response.text();
            return {{ tag: "ok", val: text }};
        }} catch (error) {{
            return {{ tag: "err", val: error.message }};
        }}
    }}
}};
```

## Reading Environment Variables

To access environment variables, add the WASI CLI dependency:

```bash
jco componentize main.js --wit ./wit -d cli -o component.wasm
```

Update your WIT interface:

```wit
package local:{component_name};

interface operations {{
    get-config: func() -> result<string, string>;
}}

world {component_name}-component {{
    import wasi:cli/environment@0.2.0;
    export operations;
}}
```

Then read environment variables in your JavaScript code:

```javascript
import {{ getEnvironment }} from 'wasi:cli/environment@0.2.0';

export const operations = {{
    getConfig() {{
        try {{
            const env = getEnvironment();
            const config = env.find(([key]) => key === 'MY_CONFIG');
            
            if (config) {{
                return {{ tag: "ok", val: config[1] }};
            }}
            return {{ tag: "err", val: "MY_CONFIG not found" }};
        }} catch (error) {{
            return {{ tag: "err", val: error.message }};
        }}
    }}
}};
```

## Error Handling

JavaScript components use WIT's `result` type for error handling:

```javascript
export const operations = {{
    divide(a, b) {{
        if (b === 0) {{
            return {{ tag: "err", val: "Division by zero" }};
        }}
        return {{ tag: "ok", val: a / b }};
    }}
}};
```

## Best Practices

1. **Use clear interface definitions** - Make your WIT interfaces descriptive
2. **Handle errors properly** - Always use `result<T, string>` for operations that can fail
3. **Keep components focused** - Each component should do one thing well
4. **Test thoroughly** - Validate your component works before deploying
5. **Document your interfaces** - Use WIT comments to explain your API

## Additional Resources

- [JavaScript Cookbook Guide](https://microsoft.github.io/wassette/latest/cookbook/javascript.html)
- [Example Components](https://github.com/microsoft/wassette/tree/main/examples)
- [componentize-js Documentation](https://github.com/bytecodealliance/componentize-js)

Would you like me to help you implement any specific functionality for your component?
