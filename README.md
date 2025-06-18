# weld-mcp-server

Weld is a secure and open source MCP server that runs on top of WebAssembly (Wasm). It is designed to securely execute untrusted tools by embedding a Wasm runtime and applying capability-based policies to control access to system resources. It uses the sandboxing and abstraction provided by the Wasm [Component Model](https://github.com/WebAssembly/component-model)  to ensure tools can be executed safely and easily without compromising the host system.

Please read the rest of the README for more background, but the TL;DR is this:

`weld` essentially acts as a *virtual MCP server*, with built-in observability, resource quotas, and handles infrastructure complexity automatically.

## Getting Started

Weld is under heavy development and doesn't currently have a tagged release. We will be working on adding releases soon, but for now you'll have to build it from source.

### Building

You will need a recent Rust toolchain and [just](https://github.com/casey/just) installed. If you don't want to install `just`, take a look at the [Justfile](./Justfile) to see the commands you need to run.

```bash
just run
```

<!-- TODO: We should push fetch-rs to GHCR under the github org so we can walk through a whole "fetch and install this for me" here -->

##  🚩 Purpose

### Problem Statement

A popular use scenario for MCP today is to run the server as a standalone process on a machine that talks to clients over stdio or a socket. Running these servers natively poses security risks, such as "my mcp-client calls the tool to read a file, and for some reason the tool writes to a file on my machine", "the tool opens a socket to a remote machine and sends data to it", or "I downloaded this open source mcp-server, but it has a vulnerability that allows an attacker to execute arbitrary code on my machine". This is the same problem as running untrusted code on your machine, but mcp-clients make them much easier to install on your machine and run.

### Who is the target audience?

1. *Developers* who want to focus on writing the business logic for MCP tools, instead of worrying about the infrastructure.

2. *DevOps engineers* who want the tools to be able to run everywhere and have a great observability story and tools are secured by design.

3. *Users* who want to run a trusted mcp-server on their machine that is proven to securely execute untrusted tools.

4. *Platform providers* who want to provide a serverless experience for their users.

### What are the current solutions?

1. Package and distribute the server as Docker images. This is perhaps the most common way to run MCP servers securely today, because it works with existing tooling and infrastructure and requires no changes to the server code. One could argue that containers are not a secure boundary, but they are a good starting point. The harder problem is how to apply security policies to the container like "how do I know what HTTP domain is this tool calling to?". [The Docker MCP Catalog](https://docs.docker.com/ai/mcp-catalog-and-toolkit/catalog/) runs each MCP server as a container - providing isolation and portability.
2. Running binaries directly using `npx` or `uvx`. This is a simple way to run MCP servers (and often the default way MCP servers document how to use it), but it is not secure. It is easy to run a tool that has a vulnerability or malicious code that can read/write files on your machine, open sockets, or even execute arbitrary code.
3. Centralized MCP server that runs WebAssembly-based tools locally (think tools like [mcp.run](https://mcp.run)). This has the advantage of running tools in tiny sandboxes which incur less memory overhead than containers. However, most of these tools still require custom ABIs and libraries and are not compatible with each other.

### So why does this exist?

We wanted to build an entirely open source tool that enables developers to define tools via the Component Model, which means they are easy to reuse and compose in addition to running with low memory requirements and in a secure sandbox. They also let anyone see exactly what features the tool is requesting and allows a server to fulfill those requests in a secure way. This is a significant improvement over the current state of MCP servers, which are either arbitrary code or require custom ABIs and libraries, and are not compatible with each other.

So what is this project aiming to be?

1. One centralized open-source mcp-server, written in a memory safe, high performance language that embeds a WebAssembly runtime (e.g. [Wasmtime](https://github.com/bytecodealliance/wasmtime) or [hyperlight-wasm](https://github.com/hyperlight-dev/hyperlight-wasm)), acting as a minimal trusted computing base (TCB).
2. `weld` will implement allow/deny lists for file paths, network endpoints, and system calls using capability-based policy like [policy-mcp-rs](https://github.com/semcp/policy-mcp-rs).
3. Untrusted tool code will be distributed as WebAssembly OCI artifacts in OCI registries, and be loaded into the trusted layer upon signature verification. Each tool will have a discrete set of capabilities. For example, tool A needs to read `./data`; not network; tool B needs read/write to `/assets` and outbound HTTP only to `api.company.com:443`.

### What about the developer experience?

Developers will write MCP tools as normal functions that can be compiled to WebAssembly Components, instead of developing servers. This is a significant paradigm shift and offers a completely different experience than writing MCP servers as it currently stands. We are fully aware that current MCP server code would need to be rewritten for retargeting to Wasm but the security benefits and flexibility of the Component Model are worth it.

If you are interested in learning more about what programming language supports WebAssembly, you can check out [this page](https://developer.fermyon.com/wasm-languages/webassembly-language-support).

## Contributing

We welcome contributions to Weld! Please check through our issue queue for any open issues and comment if you'd like to work on one. If you have an idea for a new feature or improvement, please open an issue to discuss it first as things are moving quickly and we may already be working on it or have something in flight!

Any code should be pushed to a fork of this repository and a pull request opened against the `main` branch. Maintainers will then review your code as soon as possible and provide feedback. We will try to be responsive, but please be patient as we are a small team and may not be able to respond immediately.
