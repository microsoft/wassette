# Filesystem Example (.NET)

This is the .NET 10 equivalent of `filesystem-rs`, with the same read/write
tool names and a restrictive storage policy. The WIT world explicitly imports
WASI filesystem preopens and types; .NET `System.IO` calls are compiled for
WASI and remain subject to the runtime's preopen and policy boundary.

## Building

```bash
dotnet build -c Release
```

Inject the documented WIT interface before loading the component:

```bash
just inject-docs examples/filesystem-dotnet/bin/Release/net10.0/wasi-wasm/native/filesystem-dotnet.wasm examples/filesystem-dotnet/wit
```

The checked-in policy grants read/write access to `/tmp` as a portable
development fixture. Production policies should use a narrower path.
