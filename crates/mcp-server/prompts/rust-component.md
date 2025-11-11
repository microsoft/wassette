# Building a Rust WebAssembly Component for Wassette

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

Would you like me to help you implement any specific functionality for your component?
