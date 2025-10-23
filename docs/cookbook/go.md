# Building Wasm Components with Go

This cookbook guide shows you how to build WebAssembly components using Go and TinyGo that work with Wassette.

## Quick Start

### Prerequisites
- Go (version 1.19 through 1.23)
- TinyGo (version 0.32 or later)

### Install Tools
```bash
# Install Go from https://golang.org/dl/

# Install TinyGo from https://tinygo.org/getting-started/install/
# On macOS with Homebrew:
brew tap tinygo-org/tools
brew install tinygo

# On Linux:
wget https://github.com/tinygo-org/tinygo/releases/download/v0.32.0/tinygo_0.32.0_amd64.deb
sudo dpkg -i tinygo_0.32.0_amd64.deb
```

## Step-by-Step Guide

### 1. Create Your Project

```bash
mkdir my-component
cd my-component
go mod init example.com/my-component
```

### 2. Define Your Interface (WIT)

Create `wit/world.wit`:

```wit
package local:my-component;

world my-component {
    export greet: func(name: string) -> string;
    export calculate: func(a: s32, b: s32) -> s32;
}
```

### 3. Generate Go Bindings

```bash
go run go.bytecodealliance.org/cmd/wit-bindgen-go@v0.6.2 generate ./wit --out gen
```

This creates Go bindings in the `gen/` directory.

### 4. Implement Your Component

Create `main.go`:

```go
package main

import (
    "fmt"
    
    "example.com/my-component/gen"
)

// Component implementation
type Component struct{}

func (c Component) Greet(name string) string {
    return fmt.Sprintf("Hello, %s!", name)
}

func (c Component) Calculate(a, b int32) int32 {
    return a + b
}

func init() {
    // Export the component
    gen.SetExports(Component{})
}

func main() {}
```

### 5. Build Your Component

```bash
tinygo build -o component.wasm -target wasip2 --wit-package ./wit --wit-world my-component main.go
```

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

### Module Information Service

**wit/world.wit:**
```wit
package local:gomodule-server;

world gomodule-server {
    export get-module-info: func(module-path: string) -> result<string, string>;
}
```

**main.go:**
```go
package main

import (
    "encoding/json"
    "fmt"
    "net/http"
    "time"
    
    "example.com/gomodule-server/gen"
)

type ModuleInfo struct {
    Path    string `json:"path"`
    Version string `json:"version"`
    Time    string `json:"time"`
}

type Component struct{}

func (c Component) GetModuleInfo(modulePath string) (string, error) {
    // Fetch module information from pkg.go.dev
    url := fmt.Sprintf("https://proxy.golang.org/%s/@latest", modulePath)
    
    resp, err := http.Get(url)
    if err != nil {
        return "", fmt.Errorf("failed to fetch: %v", err)
    }
    defer resp.Body.Close()
    
    var info ModuleInfo
    if err := json.NewDecoder(resp.Body).Decode(&info); err != nil {
        return "", fmt.Errorf("failed to parse: %v", err)
    }
    
    result, err := json.Marshal(info)
    if err != nil {
        return "", fmt.Errorf("failed to marshal: %v", err)
    }
    
    return string(result), nil
}

func init() {
    gen.SetExports(Component{})
}

func main() {}
```

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

**main.go:**
```go
package main

import (
    "fmt"
    
    "example.com/calculator/gen"
)

type Calculator struct{}

func (c Calculator) Add(a, b int32) int32 {
    return a + b
}

func (c Calculator) Subtract(a, b int32) int32 {
    return a - b
}

func (c Calculator) Multiply(a, b int32) int32 {
    return a * b
}

func (c Calculator) Divide(a, b int32) (int32, error) {
    if b == 0 {
        return 0, fmt.Errorf("division by zero")
    }
    return a / b, nil
}

func init() {
    gen.SetExports(Calculator{})
}

func main() {}
```

### Text Processing

**wit/world.wit:**
```wit
package local:text-processor;

world processor {
    export process-text: func(input: string, operation: string) -> result<string, string>;
}
```

**main.go:**
```go
package main

import (
    "fmt"
    "strings"
    
    "example.com/text-processor/gen"
)

type Processor struct{}

func (p Processor) ProcessText(input, operation string) (string, error) {
    switch operation {
    case "uppercase":
        return strings.ToUpper(input), nil
    case "lowercase":
        return strings.ToLower(input), nil
    case "reverse":
        runes := []rune(input)
        for i, j := 0, len(runes)-1; i < j; i, j = i+1, j-1 {
            runes[i], runes[j] = runes[j], runes[i]
        }
        return string(runes), nil
    default:
        return "", fmt.Errorf("unknown operation: %s", operation)
    }
}

func init() {
    gen.SetExports(Processor{})
}

func main() {}
```

## Error Handling

Go components use the standard Go error type for WIT's `result`:

```go
// Success
return result, nil

// Error
return "", fmt.Errorf("error message: %v", err)

// Or with a zero value for the result
return 0, fmt.Errorf("calculation failed")
```

## Working with WIT Types

### Type Mappings
```go
// WIT type -> Go type
// s32, s64 -> int32, int64
// u32, u64 -> uint32, uint64
// f32, f64 -> float32, float64
// string -> string
// bool -> bool
// list<T> -> []T
// option<T> -> *T
// result<T, E> -> (T, error)
// record -> struct
```

### Complex Types
```go
// WIT record
// record person {
//     name: string,
//     age: u32,
// }

type Person struct {
    Name string
    Age  uint32
}

func processPerson(p Person) string {
    return fmt.Sprintf("%s is %d years old", p.Name, p.Age)
}
```

## Best Practices

1. **Use proper error handling** - Always check and return errors appropriately
2. **Keep components small** - TinyGo produces smaller binaries with focused code
3. **Avoid unsupported features** - Some Go standard library features may not work with TinyGo
4. **Test thoroughly** - Validate your component works with Wassette before deployment
5. **Document interfaces** - Use WIT comments to document your API

## Common Patterns

### JSON Processing
```go
import (
    "encoding/json"
    "fmt"
)

type Data struct {
    Name  string `json:"name"`
    Value int    `json:"value"`
}

func processJSON(jsonStr string) (string, error) {
    var data Data
    if err := json.Unmarshal([]byte(jsonStr), &data); err != nil {
        return "", fmt.Errorf("invalid JSON: %v", err)
    }
    
    // Process data
    data.Value *= 2
    
    result, err := json.Marshal(data)
    if err != nil {
        return "", fmt.Errorf("marshal error: %v", err)
    }
    
    return string(result), nil
}
```

### HTTP Client
```go
import (
    "fmt"
    "io"
    "net/http"
)

func fetchURL(url string) (string, error) {
    resp, err := http.Get(url)
    if err != nil {
        return "", fmt.Errorf("request failed: %v", err)
    }
    defer resp.Body.Close()
    
    if resp.StatusCode != http.StatusOK {
        return "", fmt.Errorf("bad status: %s", resp.Status)
    }
    
    body, err := io.ReadAll(resp.Body)
    if err != nil {
        return "", fmt.Errorf("read failed: %v", err)
    }
    
    return string(body), nil
}
```

### String Validation
```go
import (
    "fmt"
    "strings"
)

func validateInput(input string) (string, error) {
    if strings.TrimSpace(input) == "" {
        return "", fmt.Errorf("input cannot be empty")
    }
    
    if len(input) > 1000 {
        return "", fmt.Errorf("input too long (max 1000 chars)")
    }
    
    return strings.TrimSpace(input), nil
}
```

## Build Configuration

### Using Justfile

Create `Justfile` for build automation:

```just
# Generate Go bindings from WIT files
generate:
    go run go.bytecodealliance.org/cmd/wit-bindgen-go@v0.6.2 generate ./wit --out gen

# Build the component
build: generate
    tinygo build -o component.wasm -target wasip2 --wit-package ./wit --wit-world my-component main.go

# Build with optimizations
build-release: generate
    tinygo build -o component.wasm -target wasip2 --wit-package ./wit --wit-world my-component -opt=2 main.go

# Clean build artifacts
clean:
    rm -rf gen/
    rm -f component.wasm

# Test the component
test: build
    wassette serve --sse --plugin-dir .
```

Usage:
```bash
just build          # Build component
just build-release  # Build with optimizations
just clean         # Clean artifacts
```

## Troubleshooting

### Build Errors

**TinyGo version issues:**
- Ensure TinyGo supports your Go version (currently 1.19-1.23)
- Update TinyGo to the latest version

**WIT binding errors:**
- Regenerate bindings after WIT changes
- Check that wit-bindgen-go version is compatible

**Import errors:**
- Some Go packages may not work with TinyGo
- Use TinyGo-compatible alternatives or implement manually

### Runtime Errors

**Component not loading:**
- Verify WIT interface matches implementation
- Check that all exported functions are implemented
- Review Wassette logs for details

**Performance issues:**
- Use `-opt=2` flag for optimized builds
- Profile your code to find bottlenecks
- Consider using Rust for performance-critical components

## TinyGo Limitations

Some Go features are not available in TinyGo:

- Some reflection features
- Full goroutine support (limited)
- Some standard library packages
- CGO

See [TinyGo language support](https://tinygo.org/docs/reference/lang-support/) for details.

## Full Documentation

For complete details, including advanced topics and more examples, see the [Go Development Guide](../development/go.md).

## Working Examples

See this complete working example in the repository:
- [gomodule-go](https://github.com/microsoft/wassette/tree/main/examples/gomodule-go) - Go module information service

## Next Steps

- Review the [complete Go guide](../development/go.md)
- Check out [working examples](https://github.com/microsoft/wassette/tree/main/examples)
- Learn about [TinyGo](https://tinygo.org/)
- Read the [FAQ](../faq.md)

## Additional Resources

- [TinyGo Documentation](https://tinygo.org/docs/)
- [wit-bindgen-go](https://github.com/bytecodealliance/wit-bindgen-go)
- [WebAssembly Component Model](https://component-model.bytecodealliance.org/)
- [WIT Specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)
