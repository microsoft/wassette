# weld-mcp-server

Weld is a secure and open source MCP server that runs on top of WebAssembly (Wasm). It is designed to securely execute untrusted tools by embedding a Wasm runtime and applying capability-based policies to control access to system resources. It uses the sandboxing and abstraction provided by the Wasm [Component Model](https://github.com/WebAssembly/component-model) to ensure tools can be executed safely and easily without compromising the host system.

Please read the rest of the README for more background, but the TL;DR is this:

`weld` essentially acts as a _virtual MCP server_, with built-in observability, resource quotas, and handles infrastructure complexity automatically.

## 🚩 Purpose

### Problem Statement

A popular use scenario for MCP today is to run the server as a standalone process on a machine that talks to clients over stdio or a socket. Running these servers natively poses security risks, such as "my mcp-client calls the tool to read a file, and for some reason the tool writes to a file on my machine", "the tool opens a socket to a remote machine and sends data to it", or "I downloaded this open source mcp-server, but it has a vulnerability that allows an attacker to execute arbitrary code on my machine". This is the same problem as running untrusted code on your machine, but mcp-clients make them much easier to install on your machine and run.

### Who is the target audience?

1. _Developers_ who want to focus on writing the business logic for MCP tools, instead of worrying about the infrastructure.

2. _DevOps engineers_ who want the tools to be able to run everywhere and have a great observability story and tools are secured by design.

3. _Users_ who want to run a trusted mcp-server on their machine that is proven to securely execute untrusted tools.

4. _Platform providers_ who want to provide a serverless experience for their users.

### What are the current solutions?

1. Package and distribute the server as Docker images. This is perhaps the most common way to run MCP servers securely today, because it works with existing tooling and infrastructure and requires no changes to the server code. One could argue that containers are not a secure boundary, but they are a good starting point. The harder problem is how to apply security policies to the container like "how do I know what HTTP domain is this tool calling to?". [The Docker MCP Catalog](https://docs.docker.com/ai/mcp-catalog-and-toolkit/catalog/) runs each MCP server as a container - providing isolation and portability.
2. Running binaries directly using `npx` or `uvx`. This is a simple way to run MCP servers (and often the default way MCP servers document how to use it), but it is not secure. It is easy to run a tool that has a vulnerability or malicious code that can read/write files on your machine, open sockets, or even execute arbitrary code.
3. Centralized MCP server that runs WebAssembly-based tools locally (think tools like [mcp.run](https://mcp.run)). This has the advantage of running tools in tiny sandboxes which incur less memory overhead than containers. However, most of these tools still require custom ABIs and libraries and are not compatible with each other.

### So why does this exist?

We wanted to build an entirely open source tool that enables developers to define tools via the Component Model, which means they are easy to reuse and compose in addition to running with low memory requirements and in a secure sandbox. They also let anyone see exactly what features the tool is requesting and allows a server to fulfill those requests in a secure way. This is a significant improvement over the current state of MCP servers, which are either arbitrary code or require custom ABIs and libraries, and are not compatible with each other.

So what is this project aiming to be?

1. One centralized open-source mcp-server, written in a memory safe, high performance language that embeds a WebAssembly runtime (e.g. [Wasmtime](https://github.com/bytecodealliance/wasmtime) or [hyperlight-wasm](https://github.com/hyperlight-dev/hyperlight-wasm)), acting as a minimal trusted computing base (TCB).
2. `weld` will implement allow/deny lists for file paths, network endpoints, and system calls using capability-based policy like [policy-mcp-rs](https://github.com/microsoft/policy-mcp-rs).
3. Untrusted tool code will be distributed as WebAssembly OCI artifacts in OCI registries, and be loaded into the trusted layer upon signature verification. Each tool will have a discrete set of capabilities. For example, tool A needs to read `./data`; not network; tool B needs read/write to `/assets` and outbound HTTP only to `api.company.com:443`.

### What about the developer experience?

Developers will write MCP tools as normal functions that can be compiled to WebAssembly Components, instead of developing servers. This is a significant paradigm shift and offers a completely different experience than writing MCP servers as it currently stands. We are fully aware that current MCP server code would need to be rewritten for retargeting to Wasm but the security benefits and flexibility of the Component Model are worth it.

If you are interested in learning more about what programming language supports WebAssembly, you can check out [this page](https://developer.fermyon.com/wasm-languages/webassembly-language-support).

## Install

### All Platforms (Shell Script)

```bash
curl -fsSL https://raw.githubusercontent.com/microsoft/weld-mcp-server/main/install.sh | bash
```

This will detect your platform and install the latest `weld-mcp-server` binary to your `$PATH`.

## Integrate with MCP Clients

### [VSCode](https://code.visualstudio.com/docs/copilot/chat/mcp-servers) / [GitHub Copilot](https://docs.github.com/en/copilot/customizing-copilot/extending-copilot-chat-with-mcp) / [Cursor](https://docs.cursor.com/context/model-context-protocol)

Add this to your VSCode or Cursor settings:

```json
"mcp": {
  "servers": {
    "weld": {
      "type": "sse",
      "url": "http://127.0.0.1:9001/sse"
    }
  }
}
```

## Quick Start

1. **Start the weld server:**

   ```bash
   # Run the following cmd in your terminal to start the Weld MCP server
   weld-mcp-server serve --http --policy-file policy.yaml
   ```

2. **Dynamically load tools:**

   **From OCI Registry:**
   <!-- update to point to weld pkgs -->

   ```
   # Enter the following prompt into your AI client
   Load the filesystem tools from oci://ghcr.io/duffney/filesystem:latest
   ```

   **From Local File:**

   ```
   # Enter the following prompt into your AI client
   Load component from file:///path/to/my-tools.wasm
   ```

3. **Use the newly loaded tools immediately:**

   ```
   # Enter the following prompt into your AI client
   Use the read-file tool to get the contents of the Justfile at the root of this repo
   ```

   The tools are now available in your AI client's tool list - no restart required! Weld automatically detects what functions each component exports and makes them available as MCP tools.

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

See the `examples/` directory for more components you can build and load dynamically.

## Contributing

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for more information on how to contribute to this project.