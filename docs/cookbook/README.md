# Cookbook: Building Wasm Components for Wassette

Welcome to the Wassette Cookbook! This section provides practical guides and recipes for building WebAssembly (Wasm) components that work with Wassette from various programming languages.

## What You'll Learn

The cookbook guides will walk you through:

- Setting up your development environment for each language
- Understanding WebAssembly Interface Types (WIT)
- Creating component interfaces
- Implementing component logic
- Building and testing your components
- Best practices and common patterns

## Available Language Guides

Choose the programming language you want to use to build your Wasm component:

### [JavaScript/TypeScript](./javascript.md)
Build Wasm components using JavaScript or TypeScript with the Bytecode Alliance's `jco` tooling. Perfect for developers familiar with Node.js ecosystem.

**Key highlights:**
- Use familiar JavaScript/TypeScript syntax
- Leverage npm packages and existing JavaScript libraries
- Quick build times with `jco componentize`
- Examples: time server, weather API, data processing

### [Python](./python.md)
Create Wasm components using Python with `componentize-py`. Ideal for data processing, scripting, and AI/ML workflows.

**Key highlights:**
- Write components in pure Python
- Use the `uv` package manager for fast builds
- Access Python's rich ecosystem
- Examples: calculator, code execution, data analysis

### [Rust](./rust.md)
Build high-performance Wasm components with Rust. Best for performance-critical tools and system-level programming.

**Key highlights:**
- Near-native performance
- Strong type safety and memory safety
- Extensive WebAssembly tooling support
- Examples: file system operations, HTTP clients

### [Go](./go.md)
Develop Wasm components using Go and TinyGo. Great for developers who prefer Go's simplicity and concurrency features.

**Key highlights:**
- Familiar Go syntax and idioms
- Good performance characteristics
- Growing WebAssembly support
- Examples: module information service

## Getting Started

If you're new to WebAssembly components, we recommend:

1. **Start with the language you know best** - Each guide is self-contained and provides all the necessary context
2. **Review the [Architecture documentation](../design/architecture.md)** - Understand how Wassette works with Wasm components
3. **Check out the [Examples](https://github.com/microsoft/wassette/tree/main/examples)** - See working implementations in action
4. **Read the [FAQ](../faq.md)** - Find answers to common questions

## Prerequisites

All guides assume basic familiarity with:

- Command-line tools and terminals
- Your chosen programming language
- Basic WebAssembly concepts (though we explain them in each guide)

## Common Concepts Across All Languages

Regardless of which language you choose, you'll work with:

### WIT (WebAssembly Interface Types)
WIT is an Interface Definition Language (IDL) that defines how your component interacts with Wassette and other systems. All guides show you how to write WIT interfaces.

Example WIT interface:
```wit
package local:my-tool;

world my-component {
    export process: func(input: string) -> result<string, string>;
}
```

### Component Model
The WebAssembly Component Model provides a standard way to create portable, composable, and secure modules. Your components will follow this model regardless of the source language.

### WASI (WebAssembly System Interface)
WASI provides a standard interface for WebAssembly components to access system capabilities like file I/O, networking, and random number generation. Each guide explains which WASI features are available.

## Testing Your Components

Once you've built a component, you can test it with Wassette:

```bash
# Load and test your component
wassette serve --sse --plugin-dir /path/to/your/component

# Or load it explicitly
wassette load file:///path/to/your/component.wasm
```

For more details on testing, see the individual language guides.

## Contributing Your Components

Have you built a useful component? Consider contributing it to the [Wassette examples](https://github.com/microsoft/wassette/tree/main/examples)! See our [Contributing Guide](https://github.com/microsoft/wassette/blob/main/CONTRIBUTING.md) for details.

## Next Steps

Pick a language guide above and start building your first Wasm component! Each guide provides step-by-step instructions with working examples.

## Additional Resources

- [WebAssembly Component Model](https://component-model.bytecodealliance.org/)
- [WIT Specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)
- [WASI Preview 2](https://github.com/WebAssembly/WASI/blob/main/legacy/preview2/README.md)
- [Model Context Protocol (MCP)](https://github.com/modelcontextprotocol/specification)
