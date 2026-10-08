# Go Module Example (.NET)

This is the .NET 10 equivalent of `gomodule-go`. It queries the public Go
module proxy using explicit WASI HTTP imports and preserves both exported tool
names.

Its policy permits requests only to `https://proxy.golang.org/`; it does not
grant filesystem access or require secrets.

```bash
dotnet build -c Release
just inject-docs examples/gomodule-dotnet/bin/Release/net10.0/wasi-wasm/native/gomodule-dotnet.wasm examples/gomodule-dotnet/wit
```
