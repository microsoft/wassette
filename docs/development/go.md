# Authoring Wasm Components with Go

This guide explains how to create WebAssembly (Wasm) components using Go and TinyGo that can be used as Tools for AI Agents with Wassette.

## Prerequisites

Before you begin, ensure you have the following tools installed:

### Required Tools

1. **Go** (version 1.19 through 1.23)
   ```bash
   # Download from https://golang.org/dl/
   # Note: TinyGo currently supports Go 1.19-1.23, not newer versions
   go version
   ```

2. **TinyGo** (version 0.32 or later)
   ```bash
   # Install TinyGo from https://tinygo.org/getting-started/install/
   tinygo version
   ```

3. **wit-bindgen-go** (for generating Go bindings from WIT files)
   ```bash
   # This will be installed automatically during the build process
   go run go.bytecodealliance.org/cmd/wit-bindgen-go@v0.6.2 version
   ```

### Optional but Recommended

- **wkg** (Wasm Component Tooling): For managing WIT dependencies

## Understanding WIT (WebAssembly Interface Types)

WIT (WebAssembly Interface Types) is an IDL (Interface Definition Language) for defining interfaces between WebAssembly components and their hosts. For detailed information about WIT, see the [WIT specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md).

When creating a Wasm component with Go, you'll need to:

1. Define your component's interface in a `.wit` file
2. Generate Go bindings from the WIT file
3. Implement the interface in your Go code

### Example WIT Interface

Here's an example WIT file (`wit/world.wit`) for a simple module information service:

```wit
package local:gomodule-server;

interface gomodule {
    /// Get the latest version of multiple Go modules
    /// Returns JSON string with module -> version mapping
    get-latest-versions: func(module-names: string) -> result<string, string>;
    
    /// Get detailed information about multiple Go modules  
    /// Returns JSON string with module information array
    get-module-info: func(module-names: string) -> result<string, string>;
}

world gomodule-server {
    include wasi:cli/imports@0.2.0;
    import wasi:http/outgoing-handler@0.2.0;
    
    export gomodule;
}
```

**Note**: If your WIT World needs to import from other WIT packages, you will need to use the `wkg wit fetch` command to fetch the WIT packages from registries.

Key concepts:
- **Package**: Defines the namespace for your component
- **Interface**: Groups related functions together
- **World**: Defines what your component imports and exports
- **WASI imports**: Standard WebAssembly System Interface capabilities (CLI, HTTP, filesystem, etc.)

## Project Structure

A typical Go Wasm component project structure looks like this:

```
my-component/
├── wit/
│   ├── world.wit           # Main WIT interface definition
│   ├── deps/               # WIT dependencies (auto-managed)
│   └── wkg.lock           # Dependency lock file
├── gen/                    # Generated Go bindings (auto-generated)
├── main.go                # Your component implementation
├── go.mod                 # Go module definition
└── go.sum                 # Go dependency checksums
```

## Step-by-Step Component Creation

### 1. Initialize Your Project

```bash
# Create project directory
mkdir my-component && cd my-component

# Initialize Go module
go mod init my-component

# Create wit directory and main WIT file
mkdir wit
cat > wit/world.wit << 'EOF'
package local:my-component;

interface my-interface {
    // Define your functions here
    process-data: func(input: string) -> result<string, string>;
}

world my-component {
    include wasi:cli/imports@0.2.0;
    export my-interface;
}
EOF
```

### 2. Add Required Dependencies

Add these dependencies to your `go.mod`:

```go
module my-component

go 1.23

require (
    go.bytecodealliance.org/cm v0.2.2
    github.com/ydnar/wasi-http-go v0.0.0-20250620060720-9877ebcf27b5 // if you need HTTP
)
```

### 3. Generate Go Bindings

Generate the bindings using the wit-bindgen-go tool:

```bash
go run go.bytecodealliance.org/cmd/wit-bindgen-go@v0.6.2 generate -o gen ./wit
```

The bindings files (.go files) will be output to the `gen` folder. You can examine that folder to understand the generated Go code and types that correspond to your WIT interface definitions.

### 4. Implement Your Component

Create `main.go`:

```go
package main

import (
    "encoding/json"
    "fmt"
    
    // Import the generated bindings
    "my-component/gen/local/my-component/my-interface"
    
    "go.bytecodealliance.org/cm"
)

func init() {
    // Register your function implementations
    myinterface.Exports.ProcessData = processData
}

// Define result types using the cm package
type ProcessDataResult = cm.Result[string, string, string]

// Implement your function
func processData(input string) ProcessDataResult {
    // Your implementation logic here
    result := fmt.Sprintf("Processed: %s", input)
    
    // Return success
    return cm.OK[ProcessDataResult](result)
    
    // For errors, return:
    // return cm.Err[ProcessDataResult]("error message")
}

func main() {
    // Required but can be empty for components
}
```

### 5. Build the Component

```bash
tinygo build -o component.wasm -target wasip2 --wit-package ./wit --wit-world my-component main.go
```

## Working with HTTP Requests

If your component needs to make HTTP requests, use the `wasi-http-go` library:

```go
import (
    "net/http"
    wasihttp "github.com/ydnar/wasi-http-go/wasihttp"
)

func httpRequest(url string) ([]byte, error) {
    client := &http.Client{
        Transport: &wasihttp.Transport{},
    }

    req, err := http.NewRequest("GET", url, nil)
    if err != nil {
        return nil, fmt.Errorf("failed to create request: %v", err)
    }

    req.Header.Set("User-Agent", "my-component/1.0")

    resp, err := client.Do(req)
    if err != nil {
        return nil, fmt.Errorf("HTTP request failed: %v", err)
    }
    defer resp.Body.Close()

    if resp.StatusCode != http.StatusOK {
        return nil, fmt.Errorf("HTTP request failed with status: %d", resp.StatusCode)
    }

    body, err := io.ReadAll(resp.Body)
    if err != nil {
        return nil, fmt.Errorf("failed to read response body: %v", err)
    }

    return body, nil
}
```

## Error Handling

Use the `cm.Result` type for functions that can fail:

```go
import "go.bytecodealliance.org/cm"

type MyResult = cm.Result[string, string, string]

func myFunction(input string) MyResult {
    if input == "" {
        return cm.Err[MyResult]("input cannot be empty")
    }
    
    result := "processed: " + input
    return cm.OK[MyResult](result)
}
```

## Testing Your Component

### Local Testing with Wassette

1. Build your component
2. Start Wassette with your component:
   ```bash
   wassette serve --sse --plugin-dir ./
   ```
3. Connect using the MCP inspector:
   ```bash
   npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse
   ```

### Unit Testing

Create Go unit tests for your business logic:

```go
package main

import (
    "testing"
    "go.bytecodealliance.org/cm"
)

func TestProcessData(t *testing.T) {
    result := processData("test input")
    
    if result.IsErr() {
        t.Fatalf("Expected success, got error: %v", result.Err())
    }
    
    expected := "Processed: test input"
    if result.OK() != expected {
        t.Errorf("Expected %q, got %q", expected, result.OK())
    }
}
```

## Best Practices

### 1. Keep Functions Pure
- Avoid global state when possible
- Make functions deterministic and testable
- Use dependency injection for external services

### 2. Handle Errors Gracefully
```go
func safeOperation(input string) MyResult {
    if input == "" {
        return cm.Err[MyResult]("validation error: input is required")
    }
    
    // Process input...
    result, err := processInput(input)
    if err != nil {
        return cm.Err[MyResult](fmt.Sprintf("processing error: %v", err))
    }
    
    return cm.OK[MyResult](result)
}
```

### 3. Use Structured Data
```go
type ModuleInfo struct {
    Name    string `json:"name"`
    Version string `json:"version"`
    URL     string `json:"url"`
}

func getModuleInfo(name string) MyResult {
    info := ModuleInfo{
        Name:    name,
        Version: "v1.0.0",
        URL:     "https://github.com/example/repo",
    }
    
    jsonData, err := json.Marshal(info)
    if err != nil {
        return cm.Err[MyResult](fmt.Sprintf("marshal error: %v", err))
    }
    
    return cm.OK[MyResult](string(jsonData))
}
```

### 4. Optimize Binary Size
TinyGo produces smaller binaries, but you can further optimize:

```bash
# Use additional TinyGo flags for smaller binaries
tinygo build -o component.wasm \
    -target wasip2 \
    --wit-package ./wit \
    --wit-world my-component \
    -opt=2 \
    -gc=leaking \
    main.go
```

## Advanced Topics

### Custom WIT Types

Define custom types in your WIT file:

```wit
interface advanced {
    record user-info {
        name: string,
        age: u32,
        active: bool,
    }
    
    variant result-type {
        success(string),
        user-error(string),
        system-error(string),
    }
    
    process-user: func(user: user-info) -> result-type;
}
```

### Resource Management

For components that need cleanup:

```go
type ResourceManager struct {
    connections map[string]*Connection
}

func (rm *ResourceManager) cleanup() {
    for _, conn := range rm.connections {
        conn.Close()
    }
}

// Use defer for cleanup in exported functions
func processWithResources(input string) MyResult {
    rm := &ResourceManager{connections: make(map[string]*Connection)}
    defer rm.cleanup()
    
    // Use resources...
    return cm.OK[MyResult]("processed")
}
```

## Real-World Example

The [gomodule-go example](../../examples/gomodule-go/) in this repository demonstrates:
- HTTP requests to external APIs
- JSON data processing
- Error handling
- Multiple exported functions

Study this example for a complete working implementation. Some of the real world examples can also be found at [go-modules](https://github.com/bytecodealliance/go-modules).

## Resources

- [WebAssembly Component Model](https://github.com/WebAssembly/component-model)
- [WIT Specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)
- [TinyGo Documentation](https://tinygo.org/docs/)
- [wit-bindgen-go](https://github.com/bytecodealliance/wit-bindgen-go)
- [WASI HTTP](https://github.com/WebAssembly/wasi-http)

For more examples and patterns, explore the other examples in this repository and the Wassette documentation.