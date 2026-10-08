# .NET

Build a WASI 0.2 WebAssembly component with .NET 10, C#, and
[componentize-dotnet](https://github.com/bytecodealliance/componentize-dotnet).
The workflow is the .NET equivalent of `jco componentize`,
`componentize-py`, and the Rust WIT/component tooling: WIT describes the
component boundary, `wit-bindgen` generates C# bindings, and NativeAOT emits a
trimmed component.

## Prerequisites

- .NET 10 SDK
- `wasm-tools` on `PATH` for validation and inspection
- Rust and `wit-docs-inject` when working from this repository

The repository pins the componentize-dotnet SDK in `examples/Directory.Build.props`
and uses the experimental .NET package feed in `examples/nuget.config`. The
first build downloads the NativeAOT-LLVM and component tooling assets.

## Project layout

Use a traditional SDK-style project for an exported component:

```text
my-component/
  my-component.csproj
  ComponentExportsImpl.cs
  wit/world.wit
```

The project must select the WIT world explicitly:

```xml
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Library</OutputType>
    <IsComponentizeDotNet>true</IsComponentizeDotNet>
    <TargetName>my-component</TargetName>
  </PropertyGroup>
  <ItemGroup>
    <Wit Remove="**\*.wit" />
    <Wit Include="wit" World="my-component" />
  </ItemGroup>
</Project>
```

The `World` value is the exact WIT `world` identifier. Without it,
componentize-dotnet cannot know whether the generated bindings are imports or
exports. The generated C# files live under `obj/` by default and should not be
checked in.

## WIT and generated C#

WIT is the stable interface between Wassette and the implementation. Keep
tool-facing names and descriptions in the WIT file:

```wit
package example:hello;

interface greeting {
    /// Greet a person by name.
    greet: func(name: string) -> string;
}

world hello {
    export greeting;
}
```

For exported interfaces, implement the generated `I...Exports` interface in the
namespace shown by the generated source. Export methods are static so the
component can be linked without a managed object graph. Prefer explicit
generated WASI imports for capabilities:

- `wasi:http/outgoing-handler` for network requests
- `wasi:clocks/wall-clock` for time
- `wasi:cli/environment` or `wasi:config/store` for configuration

Do not replace the network, clock, or configuration imports with `HttpClient`,
`DateTime.UtcNow`, or `Environment.GetEnvironmentVariable`; those APIs bypass
the component's documented capability boundary and are not the intended
policy-controlled surface.

### HTTP fetch example

The [`fetch-dotnet`](../../examples/fetch-dotnet/README.md) example exposes a
credential-free `fetch` tool by importing
`wasi:http/outgoing-handler@0.2.1`. Its generated bindings provide
`OutgoingRequest`, `IOutgoingHandlerImports.Handle`, and the response stream
used by `FetchExportsImpl.cs`; the component does not use `HttpClient` or
secret-backed configuration.

Keep the HTTP WIT dependencies under `wit/deps` with the component. The
component's `policy.yaml` grants only the example host:

```yaml
permissions:
  network:
    allow:
      - host: "https://example.com/"
```

Build and inspect it from the repository root:

```bash
dotnet build examples/fetch-dotnet/fetch-dotnet.csproj -c Release
just inject-docs \
  examples/fetch-dotnet/bin/Release/net10.0/wasi-wasm/native/fetch-dotnet.wasm \
  examples/fetch-dotnet/wit
wasm-tools validate \
  examples/fetch-dotnet/bin/Release/net10.0/wasi-wasm/native/fetch-dotnet.wasm
wasm-tools component wit \
  examples/fetch-dotnet/bin/Release/net10.0/wasi-wasm/native/fetch-dotnet.wasm
```

Filesystem access is the deliberate exception. The
[`filesystem-dotnet`](../../examples/filesystem-dotnet/README.md) example
imports `wasi:filesystem/preopens` and `wasi:filesystem/types`; its
`System.IO` calls are backed by those WASI interfaces and remain restricted to
the host directories exposed as WASI preopens by Wassette policy. A policy
grant such as `fs:///tmp` is required before the component can read or write
that location; it does not grant arbitrary host filesystem access. Use
`System.IO` only for this WASI-backed filesystem scenario, and do not assume
that host-specific paths outside the configured preopens are available.

Use source-generated `System.Text.Json` metadata for JSON payloads:

```csharp
[JsonSerializable(typeof(ApiResponse))]
internal partial class JsonContext : JsonSerializerContext
{
}
```

Avoid reflection, dynamic loading, and libraries that require untrimmed
runtime code. NativeAOT warnings are usually a sign that an API needs an
explicit source-generated or static alternative.

## Build and inspect

Build a component in Release mode:

```bash
dotnet build -c Release
```

The NativeAOT component is normally written to
`bin/Release/net10.0/wasi-wasm/native/<target-name>.wasm`. Validate and inspect
the output before using it:

```bash
wasm-tools validate bin/Release/net10.0/wasi-wasm/native/my-component.wasm
wasm-tools component wit bin/Release/net10.0/wasi-wasm/native/my-component.wasm
```

Inject the WIT comments that Wassette exposes to MCP clients:

```bash
just inject-docs \
  examples/my-component-dotnet/bin/Release/net10.0/wasi-wasm/native/my-component-dotnet.wasm \
  examples/my-component-dotnet/wit
```

Injection must happen after the component has been built and before the
artifact is copied to `bin/` or published to an OCI registry.

## Policies and local Wassette use

WIT imports declare which capabilities a component can request. Wassette
policies decide which concrete hosts, paths, and environment keys are granted.
Keep a `policy.yaml` beside each component and grant only the endpoints or
directories needed by that example. A missing grant is expected to fail
explicitly; do not add an allow-all fallback.

After building and injecting documentation:

```bash
wassette serve --streamable-http --component-dir examples/my-component-dotnet
```

Use the MCP Inspector or an MCP client to discover and call the exported tool.
Runtime tests should be run only when the local Wassette binary and policy
fixtures are available; `wasm-tools validate` and `component wit` are useful
lower-level checks but do not prove runtime policy behavior.

## File-based apps are not exported components

Do not use .NET file-based apps (`dotnet run file.cs`) for these examples.
File-based apps are convenient for scripts, but the current componentize-dotnet
workflow needs traditional `.csproj`/MSBuild metadata to select the WIT world,
run binding generation, configure NativeAOT-LLVM, and produce a library
component. Each example therefore has an independent `.csproj`, even when its
implementation is small.

## Current preview limitations

componentize-dotnet is still a preview toolchain. WIT binding shapes and
NativeAOT/WASI support can change between SDK releases, and the package's
native tooling download is platform-specific. Keep the package version pinned,
validate the component in CI, and document any intentionally narrower behavior
than the corresponding JavaScript, Python, Rust, or Go example. Do not publish
an artifact that was not built, validated, and documentation-injected by the
workflow.
