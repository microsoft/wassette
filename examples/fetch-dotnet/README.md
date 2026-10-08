# Fetch Example (.NET)

This example exposes the same `fetch` tool as `fetch-rs`, using the explicit
WASI HTTP outgoing-handler import required by the component model.

## Building

```bash
dotnet build -c Release
```

Inject the documented WIT interface before loading the component:

```bash
just inject-docs examples/fetch-dotnet/bin/Release/net10.0/wasi-wasm/native/fetch-dotnet.wasm examples/fetch-dotnet/wit
```

## Usage

```text
Please load the component from file:///path/to/fetch-dotnet.wasm
Please fetch the content of https://example.com
```

The component requires a policy grant for the destination host. HTML and JSON
are returned as text; this initial .NET port intentionally does not include the
Rust example's HTML-to-Markdown conversion.
