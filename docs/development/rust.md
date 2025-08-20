# Writing Wasm Components in Rust

This guide provides comprehensive instructions for creating WebAssembly (Wasm) components from Rust that can be used as Tools for AI Agents with Wassette. By the end of this guide, you'll understand how to structure, implement, build, and deploy Rust-based Wasm components.

## Table of Contents

- [Prerequisites](#prerequisites)
- [Understanding Wasm Components](#understanding-wasm-components)
- [Project Setup](#project-setup)
- [Defining the Interface (WIT)](#defining-the-interface-wit)
- [Implementing the Component](#implementing-the-component)
- [Building the Component](#building-the-component)
- [Testing and Deployment](#testing-and-deployment)
- [Best Practices](#best-practices)
- [Common Patterns](#common-patterns)
- [Troubleshooting](#troubleshooting)
- [Additional Resources](#additional-resources)

## Prerequisites

Before you begin, ensure you have the following installed:

1. **Rust toolchain** (1.75.0 or later):
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   source ~/.cargo/env
   ```

2. **WASI Preview 2 target**:
   ```bash
   rustup target add wasm32-wasip2
   ```

3. **Basic understanding of**:
   - Rust programming language
   - WebAssembly concepts
   - Command-line tools

## Understanding Wasm Components

WebAssembly components are a standardized way to create portable, secure, and efficient modules that can run across different environments. In the context of Wassette:

- **Components** are self-contained modules with well-defined interfaces
- **WIT (WebAssembly Interface Types)** defines the component's external interface
- **WASI Preview 2** provides system interface capabilities
- **Component Model** enables composition and interoperability

### Key Benefits

- **Security**: Components run in a sandboxed environment
- **Portability**: Same component runs across different platforms
- **Performance**: Near-native execution speed
- **Composability**: Components can be combined and orchestrated

## Project Setup

### 1. Create a New Rust Project

```bash
cargo new --lib my-component
cd my-component
```

### 2. Configure Cargo.toml

Your `Cargo.toml` should be configured specifically for Wasm component development:

```toml
[package]
name = "my-component"
version = "0.1.0"
edition = "2021"
license = "MIT"

[dependencies]
wit-bindgen-rt = { version = "0.37.0", features = ["bitflags"] }
anyhow = "1.0"  # For error handling (optional but recommended)

[lib]
crate-type = ["cdylib"]

[profile.release]
codegen-units = 1
opt-level = "s"          # Optimize for size
debug = false
strip = true             # Remove debug symbols
lto = true              # Link-time optimization

[package.metadata.component]
package = "component:my-component"

[workspace]
```

#### Key Configuration Explained

- **`crate-type = ["cdylib"]`**: Creates a C-compatible dynamic library suitable for Wasm
- **`wit-bindgen-rt`**: Runtime support for WIT bindings generation
- **Profile settings**: Optimizes for small Wasm file size
- **Package metadata**: Identifies this as a component package

### 3. Create Directory Structure

```
my-component/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   └── bindings.rs  # Generated automatically
├── wit/
│   └── world.wit
└── Justfile         # Optional: build automation
```

## Defining the Interface (WIT)

The WIT (WebAssembly Interface Types) file defines your component's external interface. Create `wit/world.wit`:

### Basic Example

```wit
package component:my-component;

/// A simple component world
world my-component {
    /// Process some input and return a result
    export process: func(input: string) -> result<string, string>;
}
```

### More Complex Example

```wit
package component:my-component;

/// A component for text processing operations
world text-processor {
    /// Transform text to uppercase
    export to-uppercase: func(text: string) -> string;
    
    /// Count words in the text
    export word-count: func(text: string) -> u32;
    
    /// Search for a pattern and return matches
    export search: func(text: string, pattern: string) -> result<list<string>, string>;
    
    /// Analyze text and return statistics
    export analyze: func(text: string) -> text-stats;
}

/// Statistics about a text
record text-stats {
    character-count: u32,
    word-count: u32,
    line-count: u32,
    average-word-length: f32,
}
```

### WIT Types Reference

| WIT Type | Rust Equivalent | Description |
|----------|-----------------|-------------|
| `bool` | `bool` | Boolean value |
| `u8`, `u16`, `u32`, `u64` | `u8`, `u16`, `u32`, `u64` | Unsigned integers |
| `s8`, `s16`, `s32`, `s64` | `i8`, `i16`, `i32`, `i64` | Signed integers |
| `f32`, `f64` | `f32`, `f64` | Floating point |
| `char` | `char` | Unicode character |
| `string` | `String` | UTF-8 string |
| `list<T>` | `Vec<T>` | Dynamic array |
| `option<T>` | `Option<T>` | Optional value |
| `result<T, E>` | `Result<T, E>` | Success or error |
| `record` | `struct` | Structured data |
| `variant` | `enum` | Tagged union |
| `tuple<T, U>` | `(T, U)` | Fixed-size collection |

## Implementing the Component

### 1. Generate Bindings

The WIT bindings need to be generated from your WIT file before building. Install and use `wit-bindgen`:

```bash
# Install wit-bindgen CLI tool
cargo install wit-bindgen-cli --version 0.37.0

# Generate Rust bindings from your WIT file
wit-bindgen rust wit/ --out-dir src/ --runtime-path wit_bindgen_rt --async none

# This creates a generated file (e.g., src/your_component.rs)
# Rename it to bindings.rs for consistency
mv src/your_component.rs src/bindings.rs
```

**Note**: The generated file name will match your component name (with underscores). You can rename it to `bindings.rs` to match the import in your `lib.rs`.

### 2. Basic Implementation Structure

Create or update `src/lib.rs`:

```rust
// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#[allow(warnings)]
mod bindings;

use bindings::Guest;

struct Component;

impl Guest for Component {
    fn process(input: String) -> Result<String, String> {
        // Your implementation here
        match input.trim() {
            "" => Err("Input cannot be empty".to_string()),
            text => Ok(format!("Processed: {}", text.to_uppercase())),
        }
    }
}

// Export the component
bindings::export!(Component with_types_in bindings);
```

### 3. Advanced Implementation Example

For the more complex text processor example:

```rust
// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#[allow(warnings)]
mod bindings;

use bindings::{Guest, TextStats};

struct Component;

impl Guest for Component {
    fn to_uppercase(text: String) -> String {
        text.to_uppercase()
    }

    fn word_count(text: String) -> u32 {
        text.split_whitespace().count() as u32
    }

    fn search(text: String, pattern: String) -> Result<Vec<String>, String> {
        if pattern.is_empty() {
            return Err("Pattern cannot be empty".to_string());
        }

        let matches: Vec<String> = text
            .lines()
            .filter(|line| line.contains(&pattern))
            .map(|line| line.to_string())
            .collect();

        Ok(matches)
    }

    fn analyze(text: String) -> TextStats {
        let character_count = text.chars().count() as u32;
        let words: Vec<&str> = text.split_whitespace().collect();
        let word_count = words.len() as u32;
        let line_count = text.lines().count() as u32;
        
        let average_word_length = if word_count > 0 {
            words.iter().map(|w| w.len()).sum::<usize>() as f32 / word_count as f32
        } else {
            0.0
        };

        TextStats {
            character_count,
            word_count,
            line_count,
            average_word_length,
        }
    }
}

bindings::export!(Component with_types_in bindings);
```

### 4. Error Handling Best Practices

```rust
use anyhow::{Context, Result as AnyhowResult};

impl Guest for Component {
    fn process_file(path: String) -> Result<String, String> {
        // Internal function with rich error handling
        fn process_internal(path: &str) -> AnyhowResult<String> {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read file: {}", path))?;
            
            let processed = content
                .lines()
                .map(|line| line.trim())
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            
            Ok(processed)
        }

        // Convert rich errors to simple strings for WIT interface
        process_internal(&path).map_err(|e| e.to_string())
    }
}
```

## Building the Component

### 1. Generate Bindings First

Before building, you need to generate Rust bindings from your WIT file:

```bash
# Install wit-bindgen CLI tool (only needed once)
cargo install wit-bindgen-cli --version 0.37.0

# Generate Rust bindings from your WIT file
wit-bindgen rust wit/ --out-dir src/ --runtime-path wit_bindgen_rt --async none

# Rename the generated file to bindings.rs for consistency
# (The generated filename will match your component name with underscores)
mv src/my_component.rs src/bindings.rs
```

**Important**: The generated file name will match your component name (with underscores replacing hyphens). For example, if your component is named `hello-component`, the generated file will be `src/hello_component.rs`.

### 2. Basic Build

After generating bindings, build the component:

```bash
cargo build --target wasm32-wasip2 --release
```

The resulting Wasm file will be located at:
```
target/wasm32-wasip2/release/my_component.wasm
```

### 3. Complete Example Walkthrough

Here's a complete step-by-step example to create a working component:

```bash
# Create new project
cargo new --lib hello-component
cd hello-component

# Create the WIT directory and interface
mkdir wit
cat > wit/world.wit << 'EOF'
package component:hello-component;

world hello-component {
    export greet: func(name: string) -> string;
}
EOF

# Configure Cargo.toml
cat > Cargo.toml << 'EOF'
[package]
name = "hello-component"
version = "0.1.0"
edition = "2021"
license = "MIT"

[dependencies]
wit-bindgen-rt = { version = "0.37.0", features = ["bitflags"] }

[lib]
crate-type = ["cdylib"]

[profile.release]
codegen-units = 1
opt-level = "s"
debug = false
strip = true
lto = true

[package.metadata.component]
package = "component:hello-component"

[workspace]
EOF

# Install the WASI target
rustup target add wasm32-wasip2

# Install wit-bindgen (if not already installed)
cargo install wit-bindgen-cli --version 0.37.0

# Generate bindings
wit-bindgen rust wit/ --out-dir src/ --runtime-path wit_bindgen_rt --async none
mv src/hello_component.rs src/bindings.rs

# Implement the component
cat > src/lib.rs << 'EOF'
#[allow(warnings)]
mod bindings;

use bindings::Guest;

struct Component;

impl Guest for Component {
    fn greet(name: String) -> String {
        format!("Hello, {}!", name)
    }
}

bindings::export!(Component with_types_in bindings);
EOF

# Build the component
cargo build --target wasm32-wasip2 --release

# Check the result
ls -la target/wasm32-wasip2/release/hello_component.wasm
```

### 4. Automated Build with Justfile

Create a `Justfile` for build automation:

```just
install-wasi-target:
    rustup target add wasm32-wasip2

install-bindgen:
    cargo install wit-bindgen-cli --version 0.37.0

generate-bindings: install-bindgen
    wit-bindgen rust wit/ --out-dir src/ --runtime-path wit_bindgen_rt --async none
    # Rename generated file to bindings.rs (adjust component name as needed)
    @COMPONENT_NAME=$(grep '^name = ' Cargo.toml | sed 's/name = "\(.*\)"/\1/' | tr '-' '_'); \
     if [ -f "src/$${COMPONENT_NAME}.rs" ]; then mv "src/$${COMPONENT_NAME}.rs" src/bindings.rs; fi

build mode="debug": install-wasi-target generate-bindings
    cargo build --target wasm32-wasip2 {{ if mode == "release" { "--release" } else { "" } }}

clean:
    cargo clean
    rm -f src/bindings.rs

# Check the component output
inspect: build
    ls -la target/wasm32-wasip2/debug/*.wasm

# Build optimized release version
release: (build "release")
```

Then run:
```bash
just build      # Build debug version
just release    # Build release version
```

### 5. Build Script Automation

For more complex projects, create a `build.sh` script:

```bash
#!/bin/bash
set -e

echo "Building Wasm component..."

# Ensure target is installed
rustup target add wasm32-wasip2

# Install wit-bindgen if not present
if ! command -v wit-bindgen &> /dev/null; then
    echo "Installing wit-bindgen..."
    cargo install wit-bindgen-cli --version 0.37.0
fi

# Generate bindings
echo "Generating bindings..."
wit-bindgen rust wit/ --out-dir src/ --runtime-path wit_bindgen_rt --async none

# Find and rename the generated file to bindings.rs
COMPONENT_NAME=$(grep '^name = ' Cargo.toml | sed 's/name = "\(.*\)"/\1/' | tr '-' '_')
GENERATED_FILE="src/${COMPONENT_NAME}.rs"
if [[ -f "$GENERATED_FILE" && "$GENERATED_FILE" != "src/bindings.rs" ]]; then
    mv "$GENERATED_FILE" src/bindings.rs
    echo "Renamed $GENERATED_FILE to src/bindings.rs"
fi

# Build the component
cargo build --target wasm32-wasip2 --release

# Check the result
WASM_FILE="target/wasm32-wasip2/release/${COMPONENT_NAME}.wasm"

if [[ -f "$WASM_FILE" ]]; then
    echo "✅ Component built successfully: $WASM_FILE"
    echo "📊 File size: $(du -h "$WASM_FILE" | cut -f1)"
else
    echo "❌ Build failed: $WASM_FILE not found"
    exit 1
fi
```

## Testing and Deployment

### 1. Unit Testing

You can write standard Rust unit tests for your component logic:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_word_count() {
        assert_eq!(Component::word_count("hello world".to_string()), 2);
        assert_eq!(Component::word_count("".to_string()), 0);
        assert_eq!(Component::word_count("   ".to_string()), 0);
    }

    #[test]
    fn test_search() {
        let text = "line 1\nmatching line\nline 3".to_string();
        let result = Component::search(text, "matching".to_string()).unwrap();
        assert_eq!(result, vec!["matching line"]);
    }

    #[test]
    fn test_error_handling() {
        let result = Component::search("text".to_string(), "".to_string());
        assert!(result.is_err());
    }
}
```

Run tests with:
```bash
cargo test
```

### 2. Integration Testing with Wassette

Test your component with Wassette locally:

```bash
# Build your component
cargo build --target wasm32-wasip2 --release

# Start Wassette with your component
wassette serve --sse --plugin-dir .
```

### 3. Component Inspection

You can inspect your built component using Wassette's component2json tool:

```bash
# Build the component2json tool
cargo build --bin component2json

# Inspect your component
./target/debug/component2json target/wasm32-wasip2/release/my_component.wasm
```

This will show you the component's interface schema and help debug issues.

## Best Practices

### 1. Interface Design

- **Keep interfaces simple**: Start with basic functions and evolve
- **Use descriptive names**: Function and parameter names should be self-documenting
- **Handle errors gracefully**: Always return meaningful error messages
- **Design for composition**: Consider how your component might be used with others

### 2. Performance Optimization

```toml
# In Cargo.toml - optimize for size
[profile.release]
opt-level = "s"        # Optimize for size, not speed
lto = true            # Link-time optimization
codegen-units = 1     # Better optimization
panic = "abort"       # Smaller code size
strip = true          # Remove debug symbols
```

### 3. Memory Management

```rust
// Prefer string slices when possible
fn process_text(text: &str) -> String {
    text.trim().to_uppercase()
}

// But WIT interfaces require owned strings
impl Guest for Component {
    fn process(input: String) -> Result<String, String> {
        Ok(process_text(&input))
    }
}
```

### 4. Error Handling Patterns

```rust
// Define custom error types for better UX
#[derive(Debug)]
enum ComponentError {
    InvalidInput(String),
    ProcessingFailed(String),
    ConfigurationError(String),
}

impl std::fmt::Display for ComponentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComponentError::InvalidInput(msg) => write!(f, "Invalid input: {}", msg),
            ComponentError::ProcessingFailed(msg) => write!(f, "Processing failed: {}", msg),
            ComponentError::ConfigurationError(msg) => write!(f, "Configuration error: {}", msg),
        }
    }
}

impl std::error::Error for ComponentError {}

// Convert to string for WIT interface
impl From<ComponentError> for String {
    fn from(error: ComponentError) -> Self {
        error.to_string()
    }
}
```

### 5. Logging and Debugging

For components that need logging capabilities:

```rust
// Use the log crate for consistent logging
use log::{info, warn, error, debug};

impl Guest for Component {
    fn process(input: String) -> Result<String, String> {
        debug!("Processing input of length: {}", input.len());
        
        if input.is_empty() {
            warn!("Received empty input");
            return Err("Input cannot be empty".to_string());
        }
        
        info!("Successfully processed input");
        Ok(input.to_uppercase())
    }
}
```

## Common Patterns

### 1. File Processing Component

```rust
// wit/world.wit
package component:file-processor;

world file-processor {
    export read-file: func(path: string) -> result<string, string>;
    export write-file: func(path: string, content: string) -> result<unit, string>;
    export list-files: func(directory: string) -> result<list<string>, string>;
}
```

```rust
// src/lib.rs
use std::fs;
use std::path::Path;

impl Guest for Component {
    fn read_file(path: String) -> Result<String, String> {
        fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read file '{}': {}", path, e))
    }

    fn write_file(path: String, content: String) -> Result<(), String> {
        fs::write(&path, content)
            .map_err(|e| format!("Failed to write file '{}': {}", path, e))
    }

    fn list_files(directory: String) -> Result<Vec<String>, String> {
        let dir_path = Path::new(&directory);
        
        if !dir_path.is_dir() {
            return Err(format!("'{}' is not a directory", directory));
        }

        let mut files = Vec::new();
        
        for entry in fs::read_dir(dir_path)
            .map_err(|e| format!("Failed to read directory '{}': {}", directory, e))? 
        {
            let entry = entry
                .map_err(|e| format!("Failed to read directory entry: {}", e))?;
            
            if let Some(filename) = entry.file_name().to_str() {
                files.push(filename.to_string());
            }
        }
        
        files.sort();
        Ok(files)
    }
}
```

### 2. HTTP Client Component

```rust
// Cargo.toml dependencies
[dependencies]
wit-bindgen-rt = { version = "0.37.0", features = ["bitflags"] }
spin-sdk = "3.0"
spin-executor = "3.0"
serde_json = "1.0"
```

```rust
// wit/world.wit
package component:http-client;

world http-client {
    export fetch: func(url: string) -> result<string, string>;
    export post: func(url: string, body: string) -> result<string, string>;
}
```

```rust
// src/lib.rs
use spin_sdk::http::{send, Request, Method, Response};
use spin_executor::run;

impl Guest for Component {
    fn fetch(url: String) -> Result<String, String> {
        run(async move {
            let request = Request::get(&url);
            let response: Response = send(request).await
                .map_err(|e| format!("Request failed: {}", e))?;
            
            if response.status() >= 400 {
                return Err(format!("HTTP error: {}", response.status()));
            }
            
            Ok(String::from_utf8_lossy(response.body()).into_owned())
        })
    }

    fn post(url: String, body: String) -> Result<String, String> {
        run(async move {
            let request = Request::builder()
                .method(Method::Post)
                .uri(&url)
                .header("content-type", "application/json")
                .body(body.into_bytes())
                .build();
            
            let response: Response = send(request).await
                .map_err(|e| format!("Request failed: {}", e))?;
            
            if response.status() >= 400 {
                return Err(format!("HTTP error: {}", response.status()));
            }
            
            Ok(String::from_utf8_lossy(response.body()).into_owned())
        })
    }
}
```

### 3. Configuration-Driven Component

```rust
// wit/world.wit
package component:configurable;

record config {
    timeout-seconds: u32,
    max-retries: u32,
    base-url: string,
}

world configurable {
    export set-config: func(config: config) -> result<unit, string>;
    export get-status: func() -> string;
    export process: func(data: string) -> result<string, string>;
}
```

```rust
// src/lib.rs
use std::sync::{Arc, Mutex};
use std::collections::HashMap;

// Global configuration storage
static CONFIG: Mutex<Option<Config>> = Mutex::new(None);

#[derive(Clone, Debug)]
struct Config {
    timeout_seconds: u32,
    max_retries: u32,
    base_url: String,
}

impl Guest for Component {
    fn set_config(config: bindings::Config) -> Result<(), String> {
        let internal_config = Config {
            timeout_seconds: config.timeout_seconds,
            max_retries: config.max_retries,
            base_url: config.base_url,
        };
        
        *CONFIG.lock().unwrap() = Some(internal_config);
        Ok(())
    }

    fn get_status() -> String {
        match CONFIG.lock().unwrap().as_ref() {
            Some(config) => format!("Configured with timeout={}s, retries={}, url={}", 
                config.timeout_seconds, config.max_retries, config.base_url),
            None => "Not configured".to_string(),
        }
    }

    fn process(data: String) -> Result<String, String> {
        let config = CONFIG.lock().unwrap()
            .clone()
            .ok_or("Component not configured")?;
        
        // Use configuration in processing
        Ok(format!("Processed '{}' with config: {:?}", data, config))
    }
}
```


## Additional Resources

### Official Documentation

- [WebAssembly Component Model](https://component-model.bytecodealliance.org/)
- [Rust Language Support](https://component-model.bytecodealliance.org/language-support/rust.html)
- [WIT Specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)
- [WASI Preview 2](https://github.com/WebAssembly/WASI/blob/main/legacy/preview2/README.md)

### Tools and Libraries

- [wit-bindgen](https://github.com/bytecodealliance/wit-bindgen) - Generate bindings from WIT
- [cargo-component](https://github.com/bytecodealliance/cargo-component) - Cargo extension for components
- [wasmtime](https://github.com/bytecodealliance/wasmtime) - WebAssembly runtime
- [wasm-tools](https://github.com/bytecodealliance/wasm-tools) - WebAssembly binary toolkit

### Examples in This Repository

- [`examples/fetch-rs/`](../../examples/fetch-rs/) - HTTP client component with HTML/JSON processing
- [`examples/filesystem-rs/`](../../examples/filesystem-rs/) - File system operations component

### Community Resources

- [Bytecode Alliance](https://bytecodealliance.org/) - Organization behind the Component Model
- [WebAssembly Working Group](https://www.w3.org/wasm/) - W3C WebAssembly standards
- [WASI Discussion](https://github.com/WebAssembly/WASI/discussions) - WASI community discussions

---

*This guide is part of the Wassette documentation. For questions or contributions, please see the [main repository](https://github.com/microsoft/wassette).*