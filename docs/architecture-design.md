# Wassette Architecture & Design

## Overview

Wassette is a secure, open-source Model Context Protocol (MCP) server that leverages WebAssembly (Wasm) to provide a trusted execution environment for untrusted tools. By embedding a WebAssembly runtime and applying capability-based security policies, Wassette enables safe execution of third-party MCP tools without compromising the host system.

### Key Features

Wassette provides security-first sandboxed execution using the WebAssembly Component Model, capability-based access control with fine-grained permissions for file system, network, and system resources, built-in observability with comprehensive monitoring and resource quotas, and a developer-friendly approach that simplifies tool development by focusing on business logic rather than infrastructure complexity.

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

### Container-Based Isolation

**Approach**: Package MCP servers as Docker images (e.g., [Docker MCP Catalog](https://docs.docker.com/ai/mcp-catalog-and-toolkit/catalog/))

**Advantages**:

- Compatible with existing tooling and infrastructure
- No code changes required for existing servers
- Provides basic process isolation

**Limitations**:

- Containers are not considered a strong security boundary
- Limited visibility into resource access patterns
- Coarse-grained permission model
- Higher memory overhead

### Direct Binary Execution

**Approach**: Install and run MCP servers directly using package managers (`npx`, `uvx`)

**Advantages**:

- Simple installation and execution
- Low overhead
- Fast startup times

**Limitations**:

- No security isolation
- Full host system access
- Vulnerable to malicious or compromised tools
- No resource controls or monitoring

### Centralized WebAssembly Platforms

**Approach**: Cloud-based platforms like [mcp.run](https://mcp.run) running WebAssembly tools

**Advantages**:

- Sandboxed execution environment
- Lower memory overhead than containers
- Centralized management

**Limitations**:

- Requires custom ABIs and libraries
- Limited interoperability between tools
- Vendor lock-in concerns
- Network dependency for execution

## Wassette Solution

### Design Philosophy

Wassette addresses the security and interoperability challenges of current MCP deployments by leveraging the [WebAssembly Component Model](https://github.com/WebAssembly/component-model). This approach provides strong security boundaries through WebAssembly's sandboxed execution environment, capability-based access control with fine-grained permission management, tool interoperability via standardized component interfaces, transparent security through explicit capability declarations, and low resource overhead with efficient memory usage compared to containers.

### Architecture Goals

Wassette implements a **centralized trusted computing base (TCB)** through a single, open-source MCP server implementation built with memory-safe, high-performance runtimes like [Wasmtime](https://github.com/bytecodealliance/wasmtime) or [hyperlight-wasm](https://github.com/hyperlight-dev/hyperlight-wasm), maintaining a minimal attack surface through reduced complexity.

The system enforces **capability-based security** with allow/deny lists for file system paths, network endpoint access control, system call restrictions, and a policy engine similar to [policy-mcp-rs](https://github.com/microsoft/policy-mcp-rs).

For **secure distribution**, WebAssembly components are distributed as OCI artifacts with cryptographic signature verification, registry-based tool distribution, and granular capability declarations per tool.

### Example Capability Model

```yaml
# Tool A: File processor
capabilities:
  filesystem:
    read: ["./data"]
  network: none

# Tool B: API client
capabilities:
  filesystem:
    read: ["/assets"]
    write: ["/assets"]
  network:
    outbound: ["api.company.com:443"]
```

## Developer Experience

### Paradigm Shift

Wassette introduces a fundamental change in how MCP tools are developed and deployed:

**Traditional Approach**: Developers build complete MCP servers
**Wassette Approach**: Developers write individual tools as WebAssembly components

### Benefits

Wassette offers simplified development by allowing developers to focus on tool functionality rather than server infrastructure. Security is enhanced through capabilities declared explicitly at build time, while improved portability ensures tools run consistently across different environments. The system also provides better composability, enabling tools to be easily combined and reused across projects.

### Development Workflow

The development process follows five key steps: implementing business logic as standard functions, declaring required system access in the component manifest, compiling to target the WebAssembly Component Model, publishing as OCI artifacts to registries, and finally loading into the Wassette runtime with automatic security policy enforcement.

### Migration Considerations

**Important**: Existing MCP servers will require rewriting to target WebAssembly. While this represents a significant migration effort, the security benefits and improved developer experience of the Component Model justify this investment.

### Language Support

Wassette supports tools written in any language that can compile to WebAssembly Components. For current language support, see the [WebAssembly Language Support Guide](https://developer.fermyon.com/wasm-languages/webassembly-language-support).

## Next Steps

This document outlines the architectural vision for Wassette. Implementation details, API specifications, and migration guides will be developed as the project evolves.
