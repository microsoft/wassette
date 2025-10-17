<div align="center">
  <h1 align="center">Wassette</h1>
  <p><b>A security-oriented runtime that runs WebAssembly Components via MCP</b></p>
  
  <!-- <a href="https://discord.gg/microsoft-open-source">
    <img src="https://dcbadge.limes.pink/api/server/microsoft-open-source" alt="Discord" style="height: 25px;">
  </a> -->

[Getting started][setup guide] | [FAQ] | [Documentation] | [Releases] | [Contributing] | [Discord]
</div>

## Why Wassette?

- **Convenience**: Wassette makes it easy to extend AI agents with new tools,
  all without ever having to leave the chat window.
- **Reusability**: Wasm Components are generic and reusable;
  there is nothing MCP-specific about them.
- **Security**: Wassette is built on the Wasmtime security sandbox, providing
  browser-grade isolation of tools.

## Architecture

![An architecture diagram showing the relationship between Wassette, MCP Clients, and Wasm Components](./assets/architecture.png)

## Installation

**Quick start:** For Linux/macOS, use our one-liner install script:

```bash
curl -fsSL https://raw.githubusercontent.com/microsoft/wassette/main/install.sh | bash
```

**For complete installation instructions** for all platforms (including Windows, Homebrew, Nix, Docker, and more), see our **[Installation Guide](https://microsoft.github.io/wassette/latest/installation.html)**.

## Using Wassette

With Wassette installed, the next step is to register it with your agent of choice. We have a [complete setup guide][setup guide] for configuring Wassette with popular AI agents, including GitHub Copilot, Cursor, Claude Code, and Gemini CLI.

Once configured, you can start loading WebAssembly components. To teach your agent to tell the time, ask it to load a time component:

```text
Please load the time component from oci://ghcr.io/microsoft/time-server-js:latest
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
[FAQ]: https://microsoft.github.io/wassette/faq.html
[Documentation]: https://microsoft.github.io/wassette
[Contributing]: CONTRIBUTING.md
[Releases]: https://github.com/microsoft/wassette/releases
[Discord]: https://discord.gg/microsoft-open-source

## Contributors

Thanks to all contributors who are helping shape Wassette into something great.

<a href="https://github.com/microsoft/wassette/graphs/contributors">
  <img src="https://contrib.rocks/image?repo=microsoft/wassette" />
</a>
