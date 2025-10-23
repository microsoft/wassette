# Building Wasm Components with Rust

This cookbook guide shows you how to build WebAssembly components using Rust that work with Wassette.

## Quick Start

### Prerequisites
- Rust toolchain (1.75.0 or later)
- WASI Preview 2 target

### Install Tools
```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Add WASI target
rustup target add wasm32-wasip2

# Install wit-bindgen (optional, for manual binding generation)
cargo install wit-bindgen-cli --version 0.37.0
```

## Step-by-Step Guide

### 1. Create Your Project

```bash
cargo new --lib my-component
cd my-component
```

### 2. Configure Cargo.toml

```toml
[package]
name = "my-component"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = { version = "0.37.0", default-features = false }

[profile.release]
opt-level = "s"
lto = true
strip = true
```

### 3. Define Your Interface (WIT)

Create `wit/world.wit`:

```wit
package local:my-component;

world calculator {
    export add: func(a: s32, b: s32) -> s32;
    export divide: func(a: f64, b: f64) -> result<f64, string>;
}
```

### 4. Generate Bindings

```bash
wit-bindgen rust wit/ --out-dir src/ --runtime-path wit_bindgen_rt --async none
```

### 5. Implement Your Component

Create/update `src/lib.rs`:

```rust
// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

mod bindings;

use bindings::exports::local::my_component::calculator::Guest;

struct Component;

impl Guest for Component {
    fn add(a: i32, b: i32) -> i32 {
        a + b
    }
    
    fn divide(a: f64, b: f64) -> Result<f64, String> {
        if b == 0.0 {
            Err("Division by zero".to_string())
        } else {
            Ok(a / b)
        }
    }
}

bindings::export!(Component with_types_in bindings);
```

### 6. Build Your Component

```bash
# Debug build
cargo build --target wasm32-wasip2

# Release build (recommended)
cargo build --target wasm32-wasip2 --release

# Output: target/wasm32-wasip2/release/my_component.wasm
```

### 7. Inject WIT Documentation

To make your component's documentation available to AI agents, inject the WIT documentation into the compiled WASM binary:

```bash
# Install wit-docs-inject (if not already installed)
cargo install --git https://github.com/Mossaka/wit-docs-inject

# Inject documentation into your component
wit-docs-inject --component target/wasm32-wasip2/release/my_component.wasm \
                --wit-dir wit/ \
                --inplace
```

This embeds the documentation from your WIT files as a `package-docs` custom section in the WASM binary. When Wassette loads your component, it extracts this documentation and uses it to describe your tools to AI agents.

For more information, see the [Documenting WIT Interfaces](./documenting-wit.md) guide.

### 8. Test Your Component

```bash
wassette serve --sse --plugin-dir target/wasm32-wasip2/release/
```

## Complete Examples

### Simple Calculator

**wit/world.wit:**
```wit
package local:calculator;

world calculator {
    export add: func(a: s32, b: s32) -> s32;
    export subtract: func(a: s32, b: s32) -> s32;
    export multiply: func(a: s32, b: s32) -> s32;
    export divide: func(a: s32, b: s32) -> result<s32, string>;
}
```

**src/lib.rs:**
```rust
mod bindings;

use bindings::exports::local::calculator::calculator::Guest;

struct Calculator;

impl Guest for Calculator {
    fn add(a: i32, b: i32) -> i32 {
        a.saturating_add(b)
    }
    
    fn subtract(a: i32, b: i32) -> i32 {
        a.saturating_sub(b)
    }
    
    fn multiply(a: i32, b: i32) -> i32 {
        a.saturating_mul(b)
    }
    
    fn divide(a: i32, b: i32) -> Result<i32, String> {
        if b == 0 {
            Err("Division by zero".to_string())
        } else {
            Ok(a / b)
        }
    }
}

bindings::export!(Calculator with_types_in bindings);
```

### HTTP Client

**wit/world.wit:**
```wit
package local:http-client;

world fetch {
    import wasi:http/outgoing-handler@0.2.0;
    
    export fetch-url: func(url: string) -> result<string, string>;
}
```

**src/lib.rs:**
```rust
mod bindings;

use bindings::exports::local::http_client::fetch::Guest;
use bindings::wasi::http::outgoing_handler;
use bindings::wasi::http::types::{Method, Scheme};

struct Fetch;

impl Guest for Fetch {
    fn fetch_url(url: String) -> Result<String, String> {
        // Parse URL and create request
        let request = outgoing_handler::OutgoingRequest::new(
            Method::Get,
            Some(&url),
            Scheme::Https,
            None,
        );
        
        // Send request
        match outgoing_handler::handle(request, None) {
            Ok(response) => {
                // Read response body
                Ok("Response received".to_string())
            }
            Err(e) => Err(format!("HTTP error: {:?}", e)),
        }
    }
}

bindings::export!(Fetch with_types_in bindings);
```

### File System Operations

**wit/world.wit:**
```wit
package local:filesystem;

world file-ops {
    import wasi:filesystem/types@0.2.0;
    
    export read-file: func(path: string) -> result<string, string>;
    export write-file: func(path: string, content: string) -> result<_, string>;
}
```

**src/lib.rs:**
```rust
mod bindings;

use bindings::exports::local::filesystem::file_ops::Guest;
use std::fs;

struct FileOps;

impl Guest for FileOps {
    fn read_file(path: String) -> Result<String, String> {
        fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {}", path, e))
    }
    
    fn write_file(path: String, content: String) -> Result<(), String> {
        fs::write(&path, content)
            .map_err(|e| format!("Failed to write {}: {}", path, e))
    }
}

bindings::export!(FileOps with_types_in bindings);
```

## Error Handling

Rust components use `Result<T, E>` for error handling:

```rust
// Success
Ok(value)

// Error
Err("Error message".to_string())

// Using the ? operator
fn process_data(input: String) -> Result<String, String> {
    let parsed = parse_input(&input)?;
    let result = transform(parsed)?;
    Ok(result)
}
```

## Build Automation with Justfile

Create `Justfile` for easy building:

```just
install-wasi-target:
    rustup target add wasm32-wasip2

install-bindgen:
    cargo install wit-bindgen-cli --version 0.37.0

generate-bindings: install-bindgen
    wit-bindgen rust wit/ --out-dir src/ --runtime-path wit_bindgen_rt --async none
    @COMPONENT_NAME=$(grep '^name = ' Cargo.toml | sed 's/name = "\(.*\)"/\1/' | tr '-' '_'); \
     if [ -f "src/$${COMPONENT_NAME}.rs" ]; then mv "src/$${COMPONENT_NAME}.rs" src/bindings.rs; fi

build mode="debug": install-wasi-target generate-bindings
    cargo build --target wasm32-wasip2 {{ if mode == "release" { "--release" } else { "" } }}

clean:
    cargo clean
    rm -f src/bindings.rs

test:
    cargo test

all: build
```

Usage:
```bash
just build           # Debug build
just build release   # Release build
just clean          # Clean build artifacts
```

## Best Practices

1. **Use strong typing** - Leverage Rust's type system for safety
2. **Handle errors properly** - Always use `Result<T, E>` for fallible operations
3. **Optimize for size** - Use `opt-level = "s"` and enable LTO in release builds
4. **Avoid unwrap/panic** - Return errors instead of panicking
5. **Use saturating operations** - Prevent integer overflow with `saturating_add`, etc.

## Common Patterns

### String Processing
```rust
fn process_text(input: String) -> Result<String, String> {
    if input.is_empty() {
        return Err("Input cannot be empty".to_string());
    }
    
    let processed = input.to_uppercase();
    Ok(processed)
}
```

### JSON Handling (with serde)
```rust
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Data {
    name: String,
    value: i32,
}

fn parse_json(json: String) -> Result<String, String> {
    let data: Data = serde_json::from_str(&json)
        .map_err(|e| format!("JSON parse error: {}", e))?;
    
    // Process data
    let result = Data {
        name: data.name.to_uppercase(),
        value: data.value * 2,
    };
    
    serde_json::to_string(&result)
        .map_err(|e| format!("JSON serialize error: {}", e))
}
```

### Stateful Components
```rust
use std::sync::Mutex;

static STATE: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn add_item(item: String) -> Result<(), String> {
    let mut state = STATE.lock()
        .map_err(|e| format!("Lock error: {}", e))?;
    state.push(item);
    Ok(())
}

fn get_items() -> Result<Vec<String>, String> {
    let state = STATE.lock()
        .map_err(|e| format!("Lock error: {}", e))?;
    Ok(state.clone())
}
```

## Troubleshooting

### Build Errors
- Ensure `wasm32-wasip2` target is installed
- Check that WIT bindings are up to date
- Verify `wit-bindgen` version matches dependencies

### Linker Errors
- Make sure `crate-type = ["cdylib"]` is set in Cargo.toml
- Check that all imports are properly declared in WIT

### Runtime Errors
- Review WASI permissions in policy configuration
- Check for panics or unwraps in your code
- Validate input/output types match WIT interface

## Full Documentation

For complete details, including advanced topics and more examples, see the [Rust Development Guide](../development/rust.md).

## Working Examples

See these complete working examples in the repository:
- [filesystem-rs](https://github.com/microsoft/wassette/tree/main/examples/filesystem-rs) - File system operations
- [fetch-rs](https://github.com/microsoft/wassette/tree/main/examples/fetch-rs) - HTTP client

## Next Steps

- Review the [complete Rust guide](../development/rust.md)
- Check out [working examples](https://github.com/microsoft/wassette/tree/main/examples)
- Learn about [wit-bindgen](https://github.com/bytecodealliance/wit-bindgen)
- Read the [FAQ](../faq.md)

## Additional Resources

- [WebAssembly Component Model](https://component-model.bytecodealliance.org/)
- [Rust Language Support](https://component-model.bytecodealliance.org/language-support/rust.html)
- [WIT Specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)
- [WASI Preview 2](https://github.com/WebAssembly/WASI/blob/main/legacy/preview2/README.md)
