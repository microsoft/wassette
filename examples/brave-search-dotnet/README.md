# Brave Search Component (.NET)

This is the .NET 10 equivalent of `brave-search-rs`. It preserves the
`search` tool and reads `BRAVE_SEARCH_API_KEY` through the explicit WASI
environment import.

```bash
dotnet build -c Release
just inject-docs examples/brave-search-dotnet/bin/Release/net10.0/wasi-wasm/native/brave-search-dotnet.wasm examples/brave-search-dotnet/wit
```

Provide the API key through Wassette policy/configuration; no credentials are
stored in this example.
