# Evaluation Example (.NET)

This example keeps the `eval` and `exec` tool names from `eval-py` while using
a deterministic, NativeAOT-safe C# expression engine. It supports arithmetic,
quoted strings, assignments, and `print(...)` statements without embedding a
dynamic Python runtime.

## Building

```bash
dotnet build -c Release
just inject-docs examples/eval-dotnet/bin/Release/net10.0/wasi-wasm/native/eval-dotnet.wasm examples/eval-dotnet/wit
```

## Usage

```text
Please load the component from file:///path/to/eval-dotnet.wasm
Please evaluate the expression 2 + 2
Please execute the statements x = 5
print(x * 2)
```

The .NET port intentionally does not claim Python-language parity. Unsupported
syntax returns a WIT error instead of falling back to reflection or a dynamic
interpreter.
