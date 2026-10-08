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

### GitHub and Go module examples

The [`github-dotnet`](../../examples/github-dotnet/README.md) example is a
read-only .NET port of the GitHub component. It exposes repository metadata,
file contents, branches, issues, and the authenticated-user profile through
explicit `wasi:http/outgoing-handler` and `wasi:cli/environment` imports. Its
policy allows only `api.github.com` and the `GITHUB_TOKEN` environment key;
provide the token at runtime and never commit it.

The [`gomodule-dotnet`](../../examples/gomodule-dotnet/README.md) example
preserves the Go module component's latest-version and metadata operations. It
uses the same generated HTTP bindings to query only
`https://proxy.golang.org/`, with no filesystem, secret, or arbitrary network
access.

Both examples keep their WIT dependency contracts under `wit/deps` and
implement the generated export interfaces. Build, inject documentation,
validate, and inspect either component before loading it:

```bash
dotnet build examples/github-dotnet/github-dotnet.csproj -c Release
just inject-docs \
  examples/github-dotnet/bin/Release/net10.0/wasi-wasm/native/github-dotnet.wasm \
  examples/github-dotnet/wit
wasm-tools validate \
  examples/github-dotnet/bin/Release/net10.0/wasi-wasm/native/github-dotnet.wasm
wasm-tools component wit \
  examples/github-dotnet/bin/Release/net10.0/wasi-wasm/native/github-dotnet.wasm
```

### Weather examples

The [`get-weather-dotnet`](../../examples/get-weather-dotnet/README.md)
example reads `OPENWEATHER_API_KEY` through the generated
`wasi:cli/environment` bindings, geocodes a city, and returns its current
temperature from OpenWeather. Its policy grants only the OpenWeather host and
that one environment key; never commit a real API key.

The [`get-open-meteo-weather-dotnet`](../../examples/get-open-meteo-weather-dotnet/README.md)
example uses the same `wasi:http/outgoing-handler` capability to geocode a city
and query Open-Meteo without credentials. Its policy allows only the two
Open-Meteo hosts needed for those requests.

Both examples keep the HTTP WIT dependencies under `wit/deps`, implement the
generated export interface, and use explicit HTTP imports rather than
`HttpClient`. Build, inject the WIT documentation, validate, and inspect a
component before loading it:

```bash
dotnet build examples/get-open-meteo-weather-dotnet/get-open-meteo-weather-dotnet.csproj -c Release
just inject-docs \
  examples/get-open-meteo-weather-dotnet/bin/Release/net10.0/wasi-wasm/native/get-open-meteo-weather-dotnet.wasm \
  examples/get-open-meteo-weather-dotnet/wit
wasm-tools validate \
  examples/get-open-meteo-weather-dotnet/bin/Release/net10.0/wasi-wasm/native/get-open-meteo-weather-dotnet.wasm
wasm-tools component wit \
  examples/get-open-meteo-weather-dotnet/bin/Release/net10.0/wasi-wasm/native/get-open-meteo-weather-dotnet.wasm
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

### Evaluation and memory examples

The [`eval-dotnet`](../../examples/eval-dotnet/README.md) example keeps the
`eval` and `exec` tool names from `eval-py` while using a small deterministic
expression engine. It supports arithmetic, quoted strings, assignments, and
`print(...)` statements without embedding a dynamic language runtime or
requesting any host capabilities. Unsupported syntax returns a WIT error.

The [`memory-dotnet`](../../examples/memory-dotnet/README.md) example keeps
the nine knowledge-graph operations from `memory-js`. Its state is held in
managed memory for the lifetime of the component instance, and its empty
policy grants no filesystem or network access. Both examples use only their
local WIT world, so no `wit/deps` directory is required.

### Capability-focused API examples

The [`brave-search-dotnet`](../../examples/brave-search-dotnet/README.md)
example imports `wasi:http/outgoing-handler` and
`wasi:cli/environment` to call Brave Search with a policy-controlled API key.
Its policy allows only `api.search.brave.com` and
`BRAVE_SEARCH_API_KEY`.

The [`arxiv-dotnet`](../../examples/arxiv-dotnet/README.md) example uses the
same explicit HTTP import to search, download, and inspect arXiv papers. Its
policy limits requests to the two arXiv hosts required by those operations.

The [`context7-dotnet`](../../examples/context7-dotnet/README.md) example
combines explicit HTTP and environment imports to resolve libraries and fetch
documentation. It allows only `context7.com` and `CONTEXT7_API_KEY`; the
preview API is built and validated in CI but is not published until its
response contract stabilizes.

Each example keeps its WIT dependency contracts under `wit/deps`, implements
the generated export interface, and avoids `HttpClient` so network access
remains visible to component policy. Build, inject the WIT documentation,
validate, and inspect a component before loading it:

```bash
dotnet build examples/brave-search-dotnet/brave-search-dotnet.csproj -c Release
just inject-docs \
  examples/brave-search-dotnet/bin/Release/net10.0/wasi-wasm/native/brave-search-dotnet.wasm \
  examples/brave-search-dotnet/wit
wasm-tools validate \
  examples/brave-search-dotnet/bin/Release/net10.0/wasi-wasm/native/brave-search-dotnet.wasm
wasm-tools component wit \
  examples/brave-search-dotnet/bin/Release/net10.0/wasi-wasm/native/brave-search-dotnet.wasm
```

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
