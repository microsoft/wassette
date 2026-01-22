// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use anyhow::Result;
use rmcp::model::{
    GetPromptRequestParam, GetPromptResult, ListPromptsResult, Prompt, PromptArgument,
    PromptMessage, PromptMessageRole,
};

const RUST_COMPONENT_TEMPLATE: &str = r#"# Building a Rust WebAssembly Component for Wassette

I'll help you build a WebAssembly component named "{component_name}" using Rust.

## Prerequisites
- Rust toolchain (1.75.0 or later)
- WASI Preview 2 target

## Step 1: Install Required Tools

First, ensure you have the necessary tools installed:

```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Add WASI target
rustup target add wasm32-wasip2

# Install wit-bindgen (optional, for manual binding generation)
cargo install wit-bindgen-cli --version 0.37.0
```

## Step 2: Create Your Project

```bash
cargo new --lib {component_name}
cd {component_name}
```

## Step 3: Configure Cargo.toml

Update your `Cargo.toml`:

```toml
[package]
name = "{component_name}"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = {{ version = "0.37.0", default-features = false }}

[profile.release]
opt-level = "s"
lto = true
strip = true
```

## Step 4: Define Your WIT Interface

Create `wit/world.wit` (see [WIT reference](https://component-model.bytecodealliance.org/design/wit.html) and [WIT by example](https://component-model.bytecodealliance.org/design/wit-example.html)):

```wit
package local:{component_name};

world {component_name} {{
    // Define your exported functions here
    export greet: func(name: string) -> string;
}}
```

## Step 5: Generate Bindings

```bash
wit-bindgen rust wit/ --out-dir src/ --runtime-path wit_bindgen_rt --async none
```

## Step 6: Implement Your Component

Create/update `src/lib.rs`:

```rust
// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

mod bindings;

use bindings::exports::local::{component_name}::{component_name}::Guest;

struct Component;

impl Guest for Component {{
    fn greet(name: String) -> String {{
        format!("Hello, {{}}!", name)
    }}
}}

bindings::export!(Component with_types_in bindings);
```

## Step 7: Build Your Component

```bash
# Debug build
cargo build --target wasm32-wasip2

# Release build (recommended)
cargo build --target wasm32-wasip2 --release

# Output: target/wasm32-wasip2/release/{component_name}.wasm
```

## Step 8: Inject WIT Documentation (Optional but Recommended)

To make your component's documentation available to AI agents:

```bash
# Install wit-docs-inject (if not already installed)
cargo install --git https://github.com/Mossaka/wit-docs-inject

# Inject documentation into your component
wit-docs-inject --component target/wasm32-wasip2/release/{component_name}.wasm \
                --wit-dir wit/ \
                --inplace
```

## Step 9: Test Your Component

```bash
# Start Wassette with your component
wassette serve --sse --plugin-dir target/wasm32-wasip2/release/

# In another terminal, use an MCP client to test
```

## Working with HTTP Requests

To make HTTP requests, import the WASI HTTP interface in your WIT:

```wit
package local:{component_name};

world {component_name} {{
    import wasi:http/outgoing-handler@0.2.0;
    
    export fetch-url: func(url: string) -> result<string, string>;
}}
```

Then use it in your Rust code:

```rust
use bindings::wasi::http::outgoing_handler;
use bindings::wasi::http::types::{{Method, Scheme, OutgoingRequest}};

impl Guest for Component {{
    fn fetch_url(url: String) -> Result<String, String> {{
        let request = OutgoingRequest::new(Method::Get, Some(&url), Scheme::Https, None);
        
        match outgoing_handler::handle(request, None) {{
            Ok(response) => Ok("Success".to_string()),
            Err(e) => Err(format!("HTTP error: {{:?}}", e)),
        }}
    }}
}}
```

## Reading Environment Variables

To access environment variables, import the WASI environment interface in your WIT:

```wit
package local:{component_name};

world {component_name} {{
    import wasi:cli/environment@0.2.0;
    
    export get-config: func() -> result<string, string>;
}}
```

Then use it in your Rust code:

```rust
use bindings::wasi::cli::environment;

impl Guest for Component {{
    fn get_config() -> Result<String, String> {{
        let env_vars = environment::get_environment();
        
        // Find a specific variable
        for (key, value) in env_vars {{
            if key == "MY_CONFIG" {{
                return Ok(value);
            }}
        }}
        
        Err("MY_CONFIG not found".to_string())
    }}
}}
```

## Best Practices

1. **Use strong typing** - Leverage Rust's type system for safety
2. **Handle errors properly** - Always use `Result<T, E>` for fallible operations
3. **Optimize for size** - Use `opt-level = "s"` and enable LTO in release builds
4. **Avoid unwrap/panic** - Return errors instead of panicking
5. **Document your WIT interface** - Add comments to explain your functions

## Additional Resources

- [Rust Cookbook Guide](https://microsoft.github.io/wassette/latest/cookbook/rust.html)
- [Example Components](https://github.com/microsoft/wassette/tree/main/examples)
- [WebAssembly Component Model](https://component-model.bytecodealliance.org/)

Would you like me to help you implement any specific functionality for your component?"#;

const JAVASCRIPT_COMPONENT_TEMPLATE: &str = r#"# Building a JavaScript WebAssembly Component for Wassette

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

Would you like me to help you implement any specific functionality for your component?"#;

/// Get the list of available prompts
pub async fn handle_prompts_list(_req: serde_json::Value) -> Result<serde_json::Value> {
    let response = ListPromptsResult {
        prompts: get_available_prompts(),
        next_cursor: None,
    };
    Ok(serde_json::to_value(response)?)
}

/// Get a specific prompt by name
pub async fn handle_prompts_get(req: serde_json::Value) -> Result<serde_json::Value> {
    let parsed_req: GetPromptRequestParam = serde_json::from_value(req)?;

    let prompt_name = parsed_req.name.as_str();
    let arguments = parsed_req.arguments.unwrap_or_default();

    let result = match prompt_name {
        "build-rust-component" => build_rust_component_prompt(arguments)?,
        "build-javascript-component" => build_javascript_component_prompt(arguments)?,
        _ => {
            return Err(anyhow::anyhow!("Unknown prompt: {}", prompt_name));
        }
    };

    Ok(serde_json::to_value(result)?)
}

/// Returns the list of available prompts
fn get_available_prompts() -> Vec<Prompt> {
    vec![
        Prompt::new(
            "build-rust-component",
            Some("Guide to building a WebAssembly component for Wassette using Rust"),
            Some(vec![PromptArgument {
                name: "component_name".to_string(),
                title: None,
                description: Some("The name of the component to build".to_string()),
                required: Some(false),
            }]),
        ),
        Prompt::new(
            "build-javascript-component",
            Some("Guide to building a WebAssembly component for Wassette using JavaScript"),
            Some(vec![PromptArgument {
                name: "component_name".to_string(),
                title: None,
                description: Some("The name of the component to build".to_string()),
                required: Some(false),
            }]),
        ),
    ]
}

/// Generate the Rust component building prompt
fn build_rust_component_prompt(
    arguments: serde_json::Map<String, serde_json::Value>,
) -> Result<GetPromptResult> {
    let component_name = arguments
        .get("component_name")
        .and_then(|v| v.as_str())
        .unwrap_or("my-component");

    let content = RUST_COMPONENT_TEMPLATE.replace("{component_name}", component_name);

    Ok(GetPromptResult {
        description: Some(format!(
            "A step-by-step guide to building a Rust WebAssembly component named '{}'",
            component_name
        )),
        messages: vec![PromptMessage::new_text(PromptMessageRole::User, content)],
    })
}

/// Generate the JavaScript component building prompt
fn build_javascript_component_prompt(
    arguments: serde_json::Map<String, serde_json::Value>,
) -> Result<GetPromptResult> {
    let component_name = arguments
        .get("component_name")
        .and_then(|v| v.as_str())
        .unwrap_or("my-component");

    let content = JAVASCRIPT_COMPONENT_TEMPLATE.replace("{component_name}", component_name);

    Ok(GetPromptResult {
        description: Some(format!(
            "A step-by-step guide to building a JavaScript WebAssembly component named '{}'",
            component_name
        )),
        messages: vec![PromptMessage::new_text(PromptMessageRole::User, content)],
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn test_handle_prompts_list() {
        let result = handle_prompts_list(json!(null)).await.unwrap();
        let list_result: ListPromptsResult = serde_json::from_value(result).unwrap();

        assert_eq!(list_result.prompts.len(), 2);
        assert_eq!(list_result.prompts[0].name, "build-rust-component");
        assert_eq!(list_result.prompts[1].name, "build-javascript-component");

        // Check that all prompts have descriptions
        for prompt in &list_result.prompts {
            assert!(prompt.description.is_some());
            assert!(prompt.arguments.is_some());
        }
    }

    #[tokio::test]
    async fn test_handle_prompts_get_rust() {
        let req = json!({
            "name": "build-rust-component",
            "arguments": {
                "component_name": "test-component"
            }
        });

        let result = handle_prompts_get(req).await.unwrap();
        let get_result: GetPromptResult = serde_json::from_value(result).unwrap();

        assert!(get_result.description.is_some());
        assert!(get_result.description.unwrap().contains("test-component"));
        assert_eq!(get_result.messages.len(), 1);
        assert_eq!(get_result.messages[0].role, PromptMessageRole::User);

        // Check content includes expected sections
        let content_text = match &get_result.messages[0].content {
            rmcp::model::PromptMessageContent::Text { text } => text,
            _ => panic!("Expected text content"),
        };
        assert!(content_text.contains("Building a Rust WebAssembly Component"));
        assert!(content_text.contains("test-component"));
        assert!(content_text.contains("cargo build"));
        assert!(content_text.contains("wasm32-wasip2"));
    }

    #[tokio::test]
    async fn test_handle_prompts_get_javascript() {
        let req = json!({
            "name": "build-javascript-component",
            "arguments": {
                "component_name": "js-tool"
            }
        });

        let result = handle_prompts_get(req).await.unwrap();
        let get_result: GetPromptResult = serde_json::from_value(result).unwrap();

        assert!(get_result.description.is_some());
        assert!(get_result.description.unwrap().contains("js-tool"));
        assert_eq!(get_result.messages.len(), 1);

        let content_text = match &get_result.messages[0].content {
            rmcp::model::PromptMessageContent::Text { text } => text,
            _ => panic!("Expected text content"),
        };
        assert!(content_text.contains("Building a JavaScript WebAssembly Component"));
        assert!(content_text.contains("js-tool"));
        assert!(content_text.contains("jco componentize"));
    }

    #[tokio::test]
    async fn test_handle_prompts_get_default_component_name() {
        let req = json!({
            "name": "build-rust-component"
        });

        let result = handle_prompts_get(req).await.unwrap();
        let get_result: GetPromptResult = serde_json::from_value(result).unwrap();

        let content_text = match &get_result.messages[0].content {
            rmcp::model::PromptMessageContent::Text { text } => text,
            _ => panic!("Expected text content"),
        };
        // Should use default "my-component" when no argument provided
        assert!(content_text.contains("my-component"));
    }

    #[tokio::test]
    async fn test_handle_prompts_get_unknown_prompt() {
        let req = json!({
            "name": "unknown-prompt"
        });

        let result = handle_prompts_get(req).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown prompt: unknown-prompt"));
    }
}
