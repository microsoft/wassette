# What is Wassette?

## Overview

Wassette is a secure, open-source Model Context Protocol (MCP) server that leverages WebAssembly (Wasm) to provide a trusted execution environment for untrusted tools. MCP is a standard how LLMs access and share data with external tools. By embedding a WebAssembly runtime and applying fine-grained security policies, Wassette enables safe execution of third-party MCP tools without compromising the host system.

### Key Features

Wassette provides the following key features:

- **Sandboxed tools** using the WebAssembly Component Model
- **Fine-grained permissions** for file system, network, and system resources
- **Developer-friendly approach** that simplifies tool development by focusing on business logic rather than infrastructure complexity

> **Note**: The name "Wassette" is a portmanteau of "Wasm" and "Cassette" (referring to magnetic tape storage), and is pronounced "Wass-ette".

## Problem Statement

The current landscape of MCP server deployment presents significant security challenges. Today's common deployment patterns include standalone processes communicating via stdio or sockets, direct binary execution using package managers like `npx` or `uvx`, and container-based isolation providing basic security boundaries.

These approaches expose users to various security risks including unrestricted file system access where tools can read and write arbitrary files, network vulnerabilities through uncontrolled outbound connections to external services, code execution risks from malicious or vulnerable tools, and limited visibility making it difficult to monitor and audit tool behavior.

The fundamental issue is that current MCP servers run with the same privileges as the host process, creating an unacceptable attack surface for untrusted code execution.

## Target Audience

Wassette serves four primary user groups:

- **Application Developers** who want to focus on business logic implementation with reduced infrastructure complexity and simplified deployment
- **DevOps Engineers** who benefit from platform-agnostic deployment capabilities, comprehensive observability and monitoring, and security-by-design architecture
- **End Users** who gain a trusted execution environment for third-party tools with transparent security policies and protection against malicious or vulnerable tools
- **Platform Providers** who can leverage Wassette's serverless-ready architecture, consistent runtime environment, and scalable multi-tenant capabilities

## Current Solutions Analysis

1. **Container-based isolation**. This is perhaps the most common way to run MCP servers securely today, because it works with existing tooling and infrastructure and requires no changes to the server code. One could argue that containers are not a secure boundary, but they are a good starting point. The harder problem is how to apply security policies to the container like "how do I know what HTTP domain is this tool calling to?". [The Docker MCP Catalog](https://docs.docker.com/ai/mcp-catalog-and-toolkit/catalog/) runs each MCP server as a container - providing isolation and portability.
2. **Direct binary execution**. Running binaries directly using `npx` or `uvx`. This is a simple way to run MCP servers (and often the default way MCP servers document how to use it), but it is not secure. It is easy to run a tool that has a vulnerability or malicious code that can read/write files on your machine, open sockets, or even execute arbitrary code.
3. **WebAssembly platforms**. Centralized MCP server that runs WebAssembly-based tools locally (think tools like [mcp.run](https://mcp.run)). This has the advantage of running tools in tiny sandboxes which incur less memory overhead than containers. However, most of these tools still require custom ABIs and libraries and are not compatible with each other.

## Wassette Solution

### Design Philosophy

Wassette addresses the security and interoperability challenges of current MCP deployments by leveraging the [WebAssembly Component Model](https://github.com/WebAssembly/component-model). This approach provides strong security boundaries through WebAssembly's sandboxed execution environment, capability-based access control with fine-grained permission management, tool interoperability via standardized component interfaces, transparent security through explicit capability declarations, and low resource overhead with efficient memory usage compared to containers.

### Architecture Goals

Wassette implements a **centralized trusted computing base (TCB)** through a single, open-source MCP server implementation built with memory-safe, high-performance runtimes like [Wasmtime](https://github.com/bytecodealliance/wasmtime), maintaining a minimal attack surface through reduced complexity.

The system enforces **capability-based security** with allow/deny lists for file system paths, network endpoint access control, system call restrictions, and a policy engine similar to [policy-mcp-rs](https://github.com/microsoft/policy-mcp-rs).

For **secure distribution**, WebAssembly components are distributed as OCI artifacts with cryptographic signature verification, registry-based tool distribution, and granular capability declarations per tool.

### Example Permission Policy

```yaml
version: "1.0"
description: "An example policy"
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

## Developer Experience

Developers will write MCP tools as functions that can be compiled to WebAssembly Components, instead of developing servers. This is a significant paradigm shift and offers a completely different experience than writing MCP servers as it currently stands. We are fully aware that current MCP server code would need to be rewritten for retargeting to Wasm but the security benefits and flexibility of the Component Model are worth it.

We are exploring AI tools that make porting existing MCP servers to Wasm easier, removing the biggest barrier to adoption.

### Language Support

Wassette supports tools written in any language that can compile to WebAssembly Components. For current language support, see the [WebAssembly Language Support Guide](https://developer.fermyon.com/wasm-languages/webassembly-language-support).

Wassette provides examples in JavaScript and Python, which are the most popular languages for MCP server development, see [examples](../examples/).
