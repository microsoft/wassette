# Time Server Example (.NET)

This example provides the same `get-current-time` tool as the JavaScript time
server, implemented as a .NET 10 WebAssembly component.

## Building

```bash
dotnet build -c Release
```

The build uses `componentize-dotnet` and NativeAOT to produce
`bin/Release/net10.0/wasi-wasm/native/time-server-dotnet.wasm`.

From the repository root, inject the WIT documentation before loading it:

```bash
just inject-docs examples/time-server-dotnet/bin/Release/net10.0/wasi-wasm/native/time-server-dotnet.wasm examples/time-server-dotnet/wit
```

## Usage

Load the component from a local path and ask:

```text
Please load the component from file:///path/to/time-server-dotnet.wasm
What is the current time?
```

The component imports the WASI wall clock through the generated .NET bindings;
it does not read the host clock through a .NET static or ambient runtime API.
