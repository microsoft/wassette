# arXiv Research Component (.NET)

This is the .NET 10 equivalent of `arxiv-rs`. It preserves the
`search-papers`, `download-paper`, and `read-paper` tools and uses the explicit
WASI HTTP import. Search and metadata responses retain the arXiv API XML so
callers can choose their own rendering.

```bash
dotnet build -c Release
just inject-docs examples/arxiv-dotnet/bin/Release/net10.0/wasi-wasm/native/arxiv-dotnet.wasm examples/arxiv-dotnet/wit
```
