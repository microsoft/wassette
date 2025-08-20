# Frequently Asked Questions (FAQ)

## General Questions

### What is Wassette?

Wassette is a secure, open-source Model Context Protocol (MCP) server that leverages WebAssembly (Wasm) to provide a trusted execution environment for untrusted tools. It enables safe execution of third-party MCP tools without compromising the host system by using WebAssembly's sandboxed execution environment and fine-grained security policies.

> **Note**: The name "Wassette" is a portmanteau of "Wasm" and "Cassette" (referring to magnetic tape storage), and is pronounced "Wass-ette".

### How is Wassette different from other MCP servers?

Traditional MCP servers run with the same privileges as the host process, creating security risks. Wassette addresses this by:

- **Sandboxed execution**: Tools run in WebAssembly's secure sandbox, not directly on the host
- **Fine-grained permissions**: Explicit control over file system, network, and system resource access
- **Component-based architecture**: Uses the standardized WebAssembly Component Model for tool interoperability
- **Centralized security**: Single trusted computing base instead of multiple potentially vulnerable servers

### What are WebAssembly Components?

WebAssembly Components are a standardized way to build portable, secure, and interoperable software modules. Unlike traditional WebAssembly modules, Components use the [WebAssembly Component Model](https://github.com/WebAssembly/component-model) which provides:

- **Standardized interfaces** defined using WebAssembly Interface Types (WIT)
- **Language interoperability** - components can be written in any language that compiles to Wasm
- **Composability** - components can be combined and reused across different environments

## Language and Development

### What programming languages are supported?

Wassette supports tools written in any language that can compile to WebAssembly Components. For current language support, see the [WebAssembly Language Support Guide](https://developer.fermyon.com/wasm-languages/webassembly-language-support).

The project includes examples in several popular languages:
- **JavaScript** ([time-server-js](../examples/time-server-js/), [get-weather-js](../examples/get-weather-js/))
- **Python** ([eval-py](../examples/eval-py/))
- **Rust** ([fetch-rs](../examples/fetch-rs/), [filesystem-rs](../examples/filesystem-rs/))
- **Go** ([gomodule-go](../examples/gomodule-go/))

### Can I use existing WebAssembly modules with Wassette?

Wassette specifically requires WebAssembly **Components** (not just modules) that follow the Component Model. Existing Wasm modules would need to be adapted to use the Component Model's interface system.

### How do I create a Wasm component?

1. **Define your interface** using WebAssembly Interface Types (WIT)
2. **Implement the functionality** in your preferred supported language
3. **Compile to a Component** using appropriate tooling for your language
4. **Test with Wassette** by loading the component

See the [examples directory](../examples/) for complete working examples in different languages.

### Do I need to rewrite existing MCP servers?

Yes, existing MCP servers would need to be rewritten to target `wasip2` (WebAssembly Components). This is a significant paradigm shift from writing servers to writing functions that compile to Wasm Components. However, the security benefits and flexibility of the Component Model make this worthwhile.

The project is exploring AI tools to help port existing MCP servers to Wasm, which should reduce the migration effort.

## Security and Permissions

### How does Wassette's security model work?

Wassette implements a **capability-based security** model with:

- **Sandbox isolation**: All tools run in WebAssembly's secure sandbox
- **Explicit permissions**: Components must declare what resources they need access to
- **Allow/deny lists**: Fine-grained control over file system paths, network endpoints, etc.
- **Principle of least privilege**: Components only get the permissions they explicitly need

### What is a policy file?

A policy file (`policy.yaml`) defines what permissions a component has. Example:

```yaml
version: "1.0"
description: "Permission policy for filesystem access"
permissions:
  storage:
    allow:
      - uri: "fs://workspace/**"
        access: ["read", "write"]
      - uri: "fs://config/app.yaml"
        access: ["read"]
  network:
    allow:
      - host: "api.openai.com"
```

### Can I grant permissions at runtime?

Yes, Wassette provides built-in tools for dynamic permission management:
- `grant-storage-permission`: Grant file system access
- `grant-network-permission`: Grant network access  
- `grant-environment-variable-permission`: Grant environment variable access

### What happens if a component tries to access unauthorized resources?

The WebAssembly sandbox will block the access attempt. Wassette enforces permissions at the runtime level, so unauthorized access attempts are prevented rather than just logged.

## Installation and Setup

### What platforms does Wassette support?

Wassette supports:
- **Linux** (including Windows Subsystem for Linux)
- **macOS** 
- **Windows** (via WinGet package)

### How do I install Wassette?

**Linux/macOS:**
```bash
curl -fsSL https://raw.githubusercontent.com/microsoft/wassette/main/install.sh | bash
```

**macOS (Homebrew):**
See the [Homebrew installation guide](./homebrew.md)

**Windows (WinGet):**
See the [WinGet installation guide](./winget.md)

**Nix:**
See the [Nix installation guide](./nix.md)

### How do I configure Wassette with my AI agent?

Wassette works with any MCP-compatible AI agent. See the [MCP clients setup guide](./mcp-clients.md) for specific instructions for:
- Visual Studio Code
- Cursor
- Claude Code
- Gemini CLI

## Usage and Troubleshooting

### How do I load a component in Wassette?

You can load components from OCI registries or local files:

```text
Please load the component from oci://ghcr.io/microsoft/time-server-js:latest
```

Or for local files:
```text
Please load the component from ./path/to/component.wasm
```

### What built-in tools does Wassette provide?

Wassette includes several built-in management tools:
- `load-component`: Load WebAssembly components
- `unload-component`: Unload components
- `list-components`: List loaded components
- `get-policy`: Get policy information
- `grant-storage-permission`: Grant storage access
- `grant-network-permission`: Grant network access
- `grant-environment-variable-permission`: Grant environment variable access
- `revoke-storage-permission`: Revoke storage access permissions
- `revoke-network-permission`: Revoke network access permissions
- `revoke-environment-variable-permission`: Revoke environment variable access permissions
- `reset-permission`: Reset all permissions for a component

### How do I debug component issues?

1. **Check the logs**: Run Wassette with `RUST_LOG=debug` for detailed logging
2. **Verify permissions**: Ensure your policy file grants necessary permissions
3. **Test component separately**: Validate that your component works outside Wassette
4. **Check the interface**: Ensure your WIT interface matches what Wassette expects

### Are there performance implications of using WebAssembly?

WebAssembly Components in Wassette have:
- **Lower memory overhead** compared to containers
- **Fast startup times** due to efficient Wasm instantiation
- **Near-native performance** for CPU-intensive tasks
- **Minimal runtime overhead** thanks to Wasmtime's optimizations

### Can I use Wassette in production?

Wassette is actively developed and used by Microsoft. However, as with any software, you should:
- Test thoroughly in your specific environment
- Review the security model for your use case
- Keep up with updates and security patches
- Consider your specific requirements for stability and support

## Getting Help

### Where can I get support?

- **GitHub Issues**: [Report bugs or request features](https://github.com/microsoft/wassette/issues)
- **Discord**: Join the `#wassette` channel on [Microsoft Open Source Discord](https://discord.gg/microsoft-open-source)
- **Documentation**: Check the [docs directory](../docs/) for detailed guides
- **Examples**: Review [working examples](../examples/) for common patterns

### How can I contribute to Wassette?

See the [Contributing Guide](../CONTRIBUTING.md) for information on:
- Setting up the development environment
- Submitting bug reports and feature requests
- Contributing code and documentation
- Following the project's coding standards

### Where can I find more examples?

The [examples directory](../examples/) contains working examples in multiple languages:
- [Time server (JavaScript)](../examples/time-server-js/)
- [Weather API (JavaScript)](../examples/get-weather-js/)
- [File system operations (Rust)](../examples/filesystem-rs/)
- [HTTP client (Rust)](../examples/fetch-rs/)
- [Code execution (Python)](../examples/eval-py/)
- [Go module info (Go)](../examples/gomodule-go/)