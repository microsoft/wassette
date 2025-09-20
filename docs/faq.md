# Frequently Asked Questions (FAQ)

## General Questions

### What is Wassette?

Wassette is a secure, open-source Model Context Protocol (MCP) server that leverages WebAssembly (Wasm) to provide a trusted execution environment for untrusted tools. It enables safe execution of third-party MCP tools without compromising the host system by using WebAssembly's sandboxed execution environment and fine-grained security policies.

> **Note**: The name "Wassette" is a portmanteau of "Wasm" and "Cassette" (referring to magnetic tape storage), and is pronounced "Wass-ette".

### Is Wassette a MCP server?

Yes, Wassette is itself a **local MCP server**. 

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
- **JavaScript** ([time-server-js](https://github.com/microsoft/wassette/tree/main/examples/time-server-js), [get-weather-js](https://github.com/microsoft/wassette/tree/main/examples/get-weather-js))
- **Python** ([eval-py](https://github.com/microsoft/wassette/tree/main/examples/eval-py))
- **Rust** ([fetch-rs](https://github.com/microsoft/wassette/tree/main/examples/fetch-rs), [filesystem-rs](https://github.com/microsoft/wassette/tree/main/examples/filesystem-rs))
- **Go** ([gomodule-go](https://github.com/microsoft/wassette/tree/main/examples/gomodule-go))

### Can I use existing WebAssembly modules with Wassette?

Wassette specifically requires WebAssembly **Components** (not just modules) that follow the Component Model. Existing Wasm modules would need to be adapted to use the Component Model's interface system.

### How do I create a Wasm component?

1. **Define your interface** using WebAssembly Interface Types (WIT)
2. **Implement the functionality** in your preferred supported language
3. **Compile to a Component** using appropriate tooling for your language
4. **Test with Wassette** by loading the component

See the [examples directory](https://github.com/microsoft/wassette/tree/main/examples) for complete working examples in different languages.

### Do I need to rewrite existing MCP servers?

Yes, existing MCP servers would need to be rewritten to target wasip2 (WebAssembly Components). This is a significant paradigm shift from writing servers to writing functions that compile to Wasm Components. However, the security benefits and flexibility of the Component Model make this worthwhile.

The project is exploring AI tools to help port existing MCP servers to Wasm, which should reduce the migration effort.

## Security and Permissions

### How does Wassette's security model work?

Wassette implements a **capability-based security** model with:

- **Sandbox isolation**: All tools run in WebAssembly's secure sandbox
- **Explicit permissions**: Components must declare what resources they need access to
- **Allow/deny lists**: Fine-grained control over file system paths, network endpoints, etc.
- **Principle of least privilege**: Components only get the permissions they explicitly need

Compared to running tools directly with an MCP SDK, Wassette enforces sandboxing and permissions **at runtime**. This prevents tools from inheriting host-level privileges and reduces the risk of data exfiltration or privilege escalation.

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

This policy permits read/write access to a `workspace` directory, read-only access to a specific config file, and network egress only to `api.openai.com`. All other filesystem and network access is denied and will be blcoked by the sandbox.

### Can I grant permissions at runtime?

Yes, Wassette provides built-in tools for dynamic permission management:
- `grant-storage-permission`: Grant file system access
- `grant-network-permission`: Grant network access  
- `grant-environment-variable-permission`: Grant environment variable access

You can also revoke previously granted permissions with the corresponding `revoke-*` tools. 

### What happens if a component tries to access unauthorized resources?

The WebAssembly sandbox will block the access attempt. Wassette enforces permissions at the runtime level, so unauthorized access attempts are prevented rather than just logged.

### Why not just use the Python or TypeScript SDK to build a server?

You can and many developers do. SDKs let you register and run tools directly from server code. 

The difference is **how tools execute**:
- **SDKs only:** Tools run with the same privileges as the host process
- **SDKs + Wassette:** Each tool runs in an isolated sandbox with deny-by-default, auditable permissions

Wassette is especially valuable in enterprise or multii-tenant environments, or when running untrusted/community tools, where stronger runtime safeguards are required. 

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

## What’s a practical use case?
One example is the `fetch` tool. With Wassette, you can write a policy that restricts the tool to only contact a specific API endpoint, such as `weather.com`. This means that even if the tool is compromised, it cannot exfiltrate data from your internal APIs or file systems. It is strictly limited to the network host you approved.

This makes it safe to:

- Run untrusted or community-contributed tools.  
- Allow third-party extensions in enterprise environments without exposing sensitive systems.  
- Confidently deploy MCP agents in multi-tenant or regulated environments.

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
- **Documentation**: Browse the [docs directory](https://github.com/microsoft/wassette/tree/main/docs) for detailed guides
- **Examples**: Review [working examples](https://github.com/microsoft/wassette/tree/main/examples) for common patterns

### How can I contribute to Wassette?

See the [Contributing Guide](https://github.com/microsoft/wassette/blob/main/CONTRIBUTING.md) for information on:
- Setting up the development environment
- Submitting bug reports and feature requests
- Contributing code and documentation
- Following the project's coding standards

### Where can I find more examples?

The [examples directory](https://github.com/microsoft/wassette/tree/main/examples) contains working examples in multiple languages:
- [Time server (JavaScript)](https://github.com/microsoft/wassette/tree/main/examples/time-server-js)
- [Weather API (JavaScript)](https://github.com/microsoft/wassette/tree/main/examples/get-weather-js)
- [File system operations (Rust)](https://github.com/microsoft/wassette/tree/main/examples/filesystem-rs)
- [HTTP client (Rust)](https://github.com/microsoft/wassette/tree/main/examples/fetch-rs)
- [Code execution (Python)](https://github.com/microsoft/wassette/tree/main/examples/eval-py)
- [Go module info (Go)](https://github.com/microsoft/wassette/tree/main/examples/gomodule-go)
