<div align="center">
  <h1 align="center">Wassette</h1>
  <p><b>A security-oriented runtime that runs WebAssembly Components via MCP</b></p>
  
  <!-- <a href="https://discord.gg/microsoft-open-source">
    <img src="https://dcbadge.limes.pink/api/server/microsoft-open-source" alt="Discord" style="height: 25px;">
  </a> -->

[Getting started][setup guide] | [FAQ] | [Releases] | [Contributing] | [Discord]

</div>

## Why Wassette?

- **Convenience**: Wassette makes it easy to extend AI agents with new tools,
  all without ever having to leave the chat window.
- **Reusability**: Wasm Components built for Wassette are generic and reusable;
  there is nothing MCP-specific about them.
- **Security**: Wassette is built on the Wasmtime security sandbox, providing
  browser-grade isolation of tools.

## Installation

For Linux (including Windows Subsystem for Linux) and macOS, you can install Wassette using the provided install script:

```bash
curl -fsSL https://raw.githubusercontent.com/microsoft/wassette/main/install.sh | bash
```

This will detect your platform and install the latest `wassette` binary to your `$PATH`. 

We provide a [Homebrew formula for macOS and Linux](./docs/homebrew.md).

For Windows users, we provide a [WinGet package](./docs/winget.md).

And [Nix flakes for reproducible environments](./docs/nix.md).

You can also download the latest release from the [GitHub Releases page][Releases] and add it to your `$PATH`.

## Using Wassette

With Wassette installed, the next step is to register it with your agent of
choice. We have a complete [complete setup guide][setup guide] for all agents
here, including Cursor, Claude Code, and Gemini CLI. 

Add the Wassette MCP Server to GitHub Copilot in Visual Studio Code by clicking the **Install in VS Code** or **Install in VS Code Insiders** badge below:

[![Install in VS Code](https://img.shields.io/badge/VS_Code-Install_Server-0098FF?style=flat-square&logo=visualstudiocode&logoColor=white)](https://vscode.dev/redirect?url=vscode:mcp/install?%7B%22name%22%3A%22wassette%22%2C%22gallery%22%3Afalse%2C%22command%22%3A%22wassette%22%2C%22args%22%3A%5B%22serve%22%2C%22--stdio%22%5D%7D) [![Install in VS Code Insiders](https://img.shields.io/badge/VS_Code_Insiders-Install_Server-24bfa5?style=flat-square&logo=visualstudiocode&logoColor=white)](https://vscode.dev/redirect?url=vscode-insiders:mcp/install?%7B%22name%22%3A%22wassette%22%2C%22gallery%22%3Afalse%2C%22command%22%3A%22wassette%22%2C%22args%22%3A%5B%22serve%22%2C%22--stdio%22%5D%7D)

Alternatively, you can add the Wassete MCP server to VS Code from the command line using the `code` command in a bash/zsh or PowerShell terminal:

### bash/zsh

```bash
code --add-mcp '{"name":"Wassette","command":"wassette","args":["serve","--stdio"]}'
```

### PowerShell

```powershell
 code --% --add-mcp "{\"name\":\"wassette\",\"command\":\"wassette\",\"args\":[\"serve\",\"--stdio\"]}"
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

## Demo

https://github.com/user-attachments/assets/8e5a371c-ac72-406d-859c-03833ee83963

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
| [eval-py](examples/eval-py/)               | Python code execution sandbox                          |
| [fetch-rs](examples/fetch-rs/)             | HTTP API client for fetching and converting web content |
| [filesystem-rs](examples/filesystem-rs/)   | File system operations (read, write, list directories) |
| [get-weather-js](examples/get-weather-js/) | Weather API client for fetching weather data           |
| [gomodule-go](examples/gomodule-go/)       | Go module information tool                             |
| [time-server-js](examples/time-server-js/) | JavaScript-based time server component                |

## Community Components

The Wassette community has built amazing components that you can use in your projects:

- **[QR Code Generator](https://github.com/attackordie/qr-code-webassembly)** - Generate QR codes from text using a WebAssembly component by @attackordie

## Discord

You can join us via the `#wassette` channel on the [Microsoft Open Source Discord](https://discord.gg/microsoft-open-source):

[![Microsoft Open Source Discord](https://dcbadge.limes.pink/api/server/microsoft-open-source)](https://discord.gg/microsoft-open-source)

## Contributing

Please see [CONTRIBUTING.md][Contributing] for more information on how to contribute to this project.

## License

This project is licensed under the <a href="LICENSE">MIT License</a>.

## Trademarks

This project may contain trademarks or logos for projects, products, or services. Authorized use of Microsoft trademarks or logos is subject to and must follow [Microsoft’s Trademark & Brand Guidelines](https://www.microsoft.com/en-us/legal/intellectualproperty/trademarks). Use of Microsoft trademarks or logos in modified versions of this project must not cause confusion or imply Microsoft sponsorship. Any use of third-party trademarks or logos are subject to those third-party’s policies.

[setup guide]: https://github.com/microsoft/wassette/blob/main/docs/mcp-clients.md
[FAQ]: docs/faq.md
[Contributing]: CONTRIBUTING.md
[Releases]: https://github.com/microsoft/wassette/releases
[Discord]: https://discord.gg/microsoft-open-source

## Contributors

Thanks to all contributors who are helping shape Wassette into something great.

<a href="https://github.com/microsoft/wassette/graphs/contributors">
  <img src="https://contrib.rocks/image?repo=microsoft/wassette" />
</a>
