<div align="center">
  <h1 align="center">Wassette</h1>
</div>

Instantly discover and run MCP tools using natural language—all within secure, isolated WebAssembly sandboxes. Wassette replaces risky native tool execution with a controlled, sandboxed environment, letting you define exactly what each tool can access—without the hassle of manually configuring every MCP tool.

- 🔧 **Dynamic Loading**: Load WebAssembly components on-demand from OCI registries, URLs, or local files - no restart required.
- 🔒 **Secure Sandboxes**: Each tool runs in isolated WebAssembly environments with capability-based policies controlling file/network access.
- 🎯 **Runtime Introspection**: Automatically discover tool capabilities and exported functions without manual configuration.
- 🧩 **Composable Tools**: Mix and match components from different sources in real-time - build workflows dynamically.
- 🚀 **Developer-Friendly**: Write functions that compile to WASM components, not entire servers - focus on logic, not infrastructure.
- ⚡ **Hot-Swap Tools**: Load, unload, and replace components without downtime - perfect for experimentation and development.

> 🦺 This project is in early development and actively evolving. Expect rapid iteration, breaking changes, and responsiveness to feedback. Please submit issues or reach out with questions!

To learn more about the architecture and design philosophy, see the [Architecture Design Document](docs/architecture-design.md).

## Quick Start

### Install

```bash
curl -fsSL https://raw.githubusercontent.com/microsoft/wassette/main/install.sh | bash
```

This will detect your platform and install the latest `wassette` binary to your `$PATH`.

## Setup Your Agent

Wassette works with any MCP-compatible agent. The setup is always the same: **add `wassette serve --stdio` as an MCP server**.

**👉 [Complete setup guide for all agents (Visual Studio Code, Cursor, Claude Code, Gemini CLI, etc.)](https://github.com/microsoft/wassette/blob/main/docs/mcp-clients.md)**

**Example for Visual Studio Code:**

```bash
code --add-mcp '{"name":"Wassette","command":"wassette","args":["serve","--stdio"]}'
```

## Try It

Enter the following prompts into your AI client's chat:

1. **Load the time server component:**

   ```
   Please load the time component from oci://ghcr.io/yoshuawuyts/time:latest
   ```

   When prompted, confirm loading. This downloads the time server from the OCI registry and makes it available to Wassette.

2. **Query the current time:**

   ```
   What is the current time?
   ```

   This will prompt your MCP agent (e.g., GitHub Copilot in VS Code) to call Wassette, which uses the time component to return the current time.

   ```output
   The current time July 31, 2025 at 10:30 AM UTC
   ```

**Built-in Tools for Dynamic Loading:**

- `load-component` - Load WebAssembly components from any source
- `unload-component` - Remove components from the runtime

## Examples

| Example                                    | Description                                            |
| ------------------------------------------ | ------------------------------------------------------ |
| [fetch-rs](examples/fetch-rs/)             | HTTP client for making web requests                    |
| [filesystem-rs](examples/filesystem-rs/)   | File system operations (read, write, list directories) |
| [eval-py](examples/eval-py/)               | Python code execution sandbox                          |
| [get-weather-js](examples/get-weather-js/) | Weather API client for fetching weather data           |
| [time-server-js](examples/time-server-js/) | Simple time server component                           |
| [gomodule-go](examples/gomodule-go/)       | Go module information tool                             |
| [eval-py](examples/eval-py/)               | Python code execution sandbox                          |

See the `examples/` directory for more components you can build and load dynamically.

## Contributing

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for more information on how to contribute to this project.

## License

<sup>
Licensed under the <a href="LICENSE">MIT License</a>.
</sup>

## Trademarks

This project may contain trademarks or logos for projects, products, or services. Authorized use of Microsoft trademarks or logos is subject to and must follow [Microsoft’s Trademark & Brand Guidelines](https://www.microsoft.com/en-us/legal/intellectualproperty/trademarks). Use of Microsoft trademarks or logos in modified versions of this project must not cause confusion or imply Microsoft sponsorship. Any use of third-party trademarks or logos are subject to those third-party’s policies.
