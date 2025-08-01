<div align="center">
  <h1 align="center">Wassette</h1>
  <p>A security-oriented runtime that runs WebAssembly Components via MCP</p>
  
  <!-- [![](https://dcbadge.limes.pink/api/server/microsoft-open-source)](https://discord.gg/microsoft-open-source) -->
</div>

Wassette extends AI agents by dynamically loading and configuring MCP tools using natural language prompts. It does this by pulling Wasm components from OCI registries or local files, and running them as MCP tools in isolated processes with precisely scoped permissions.

> 🦺 This project is in early development and actively evolving. Expect rapid iteration, breaking changes, and responsiveness to feedback. Please submit issues or reach out with questions!

To learn more about the architecture and design philosophy, see the [Architecture Design Document](docs/architecture-design.md).

## Installation

For Linux and macOS, you can install Wassette using the provided install script:

```bash
curl -fsSL https://raw.githubusercontent.com/microsoft/wassette/main/install.sh | bash
```

This will detect your platform and install the latest `wassette` binary to your `$PATH`. For Windows, you can download the latest release from the [GitHub Releases page](https://github.com/microsoft/wassette/releases).

## Using Wassette

With Wassette installed, the next step is to register it with your agent of
choice. We have a complete [complete setup guide][setup guide] for all agents
here, including Cursor, Claude Code, and Gemini CLI. However to get started with
Visual Studio Code, just run the following command:

```bash
code --add-mcp '{"name":"Wassette","command":"wassette","args":["serve","--stdio"]}'
```

Now that your agent knows about Wassette, we are ready to load Wasm Components. To teach your agent to tell the time, we can ask it to load a time component:

```text
Please load the time component from oci://ghcr.io/yoshuawuyts/time:latest
```

Now that the time component is loaded, we can ask your agent to tell you the current time:

```text
What is the current time?
```

The agent will respond with the current time, which is fetched from the time component running in a secure WebAssembly sandbox:

```output
The current time July 31, 2025 at 10:30 AM UTC
```

Congratulations! You've just run your first Wasm Component and taught your agent how to tell time!

## Building for Wassette

Wasm Components provide fully typed interfaces defined using WebAssembly
Interface Types (WIT). Wassette can take any Wasm Component and load it as an
MCP tool by inspecting the types it exposes. Take for example the following WIT
definition for a time server:

```wit
package local:time-server;

world time-server {
    export get-current-time: func() -> string;
}
```

You'll notice that this interface doesn't mention MCP at all; it is just a
regular library interface that exports a function. That means there is no such
thing as a "Wassette-specific Wasm Component". Wassette is able to load any Wasm
Component and expose its functions as MCP tools. Just like Components built for Wassette can be re-used by other Wasm runtimes.

See the [`examples/`](./examples/) directory for a complete list of examples. Here is a
selection of examples written in different languages:

| Example                                    | Description                                            |
| ------------------------------------------ | ------------------------------------------------------ |
| [filesystem-rs](examples/filesystem-rs/)   | File system operations (read, write, list directories) |
| [eval-py](examples/eval-py/)               | Python code execution sandbox                          |
| [get-weather-js](examples/get-weather-js/) | Weather API client for fetching weather data           |
| [gomodule-go](examples/gomodule-go/)       | Go module information tool                             |

## Contributing

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for more information on how to contribute to this project.

## License

This project is icensed under the <a href="LICENSE">MIT License</a>.

## Trademarks

This project may contain trademarks or logos for projects, products, or services. Authorized use of Microsoft trademarks or logos is subject to and must follow [Microsoft’s Trademark & Brand Guidelines](https://www.microsoft.com/en-us/legal/intellectualproperty/trademarks). Use of Microsoft trademarks or logos in modified versions of this project must not cause confusion or imply Microsoft sponsorship. Any use of third-party trademarks or logos are subject to those third-party’s policies.

[setup guide]: https://github.com/microsoft/wassette/blob/main/docs/mcp-clients.md
