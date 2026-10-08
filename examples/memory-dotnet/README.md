# Memory Server Example (.NET)

This is the .NET 10 equivalent of `memory-js`. It keeps an in-memory
knowledge graph for the lifetime of the component instance and exports the
same nine knowledge-graph operations.

```bash
dotnet build -c Release
just inject-docs examples/memory-dotnet/bin/Release/net10.0/wasi-wasm/native/memory-dotnet.wasm examples/memory-dotnet/wit
```

The component has no filesystem or network permissions. State is intentionally
ephemeral, matching the JavaScript example's documented migration behavior.
