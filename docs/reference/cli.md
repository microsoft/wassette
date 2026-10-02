# Wassette CLI Reference

The Wassette command-line interface provides comprehensive tools for managing WebAssembly components, policies, and permissions both locally and through the MCP server. This document covers all CLI functionality and usage patterns.

## Overview

Wassette offers two primary modes of operation:

1. **Server Mode**: Run as an MCP server that responds to client requests
2. **CLI Mode**: Direct command-line management of components and permissions

The CLI mode allows you to perform administrative tasks without requiring a running MCP server, making it ideal for automation, scripting, and local development workflows.

## Installation

For installation instructions, see the main [README](https://github.com/microsoft/wassette/blob/main/README.md#installation). Once installed, the `wassette` command will be available in your PATH.

## Quick Start

```bash
# Check available commands
wassette --help

# List currently loaded components
wassette component list

# Load a component from an OCI registry
wassette component load oci://ghcr.io/microsoft/time-server-js:latest

# Load a component from a local file
wassette component load file:///path/to/component.wasm

# Start the MCP server for local development (stdio transport)
wassette run

# Reconcile finished builds in the local drop directory
wassette component sync
```

## Command Structure

Wassette uses a hierarchical command structure organized around functional areas:

```
wassette
├── run            # Start MCP server with stdio transport (local development)
├── serve          # Start MCP server with HTTP transports (remote access)
├── acp            # [EXPERIMENTAL] Host an ACP agent on stdio
├── component      # Component lifecycle management
│   ├── load       # Load components
│   ├── unload     # Remove components
│   ├── list       # Show loaded components
│   ├── build      # Opt-in isolated build and install (component-generation feature)
│   └── sync       # Reconcile locally built components in the drop directory
├── inspect        # Inspect component schema (debugging)
├── registry       # Registry search and fetch
│   ├── search     # Search for components
│   └── get        # Fetch and load from registry
├── policy         # Policy information
│   └── get        # Retrieve component policies
├── permission     # Permission management
│   ├── grant      # Add permissions
│   ├── revoke     # Remove permissions
│   └── reset      # Clear all permissions
└── secret         # Secret management
    ├── list       # List component secrets
    ├── set        # Set secret values
    └── delete     # Remove secrets
```

## Server Commands

### `wassette component build` (opt-in)

Available in binaries built with the `component-generation` feature when the
private builder image is present at
`~/.local/share/wassette/builder/rust-initrd.cpio` (or the corresponding path
under `$XDG_DATA_HOME` when set). `just install` builds the
feature-enabled CLI but does not download or install the image:

```bash
just install
wassette component build request.json
wassette component build request.json --emit-source ./source
wassette component source 'example:generated' --out ./source
```

`request.json` contains the same inline Rust/WIT request as the
[`build-component` management tool](built-in-tools.md#build-component-opt-in).
The command reads a file, not stdin. Its independent adapter transport limits
are 2 MiB for encoded JSON and 256 KiB each for decoded source and WIT; these
are not builder defaults. Escaped JSON can exceed the total cap even when
decoded fields fit. Component names accept up to 512 UTF-8 bytes and worlds up
to 256 bytes without changing their spelling. The builder may impose stricter
source/WIT limits, including the shared request-plus-dependency WIT budget.
`--component-dir` selects the managed store. `--emit-source DIR`
optionally writes a local rebuildable source layout after a successful build;
it refuses a non-empty directory. The private builder image compiles the
request in isolation, and the host validates captured output before installing
through the shared component store. A direct CLI invocation is the user's
build request. Exposure is authorized by default when requested, while
rebuild is disabled by default; ACP asks the editor to approve each phase.

### `wassette component source` (opt-in)

Use `wassette component source <component-id> [--revision TOKEN] [--out DIR]
[--force] [--component-dir DIR]` to retrieve the installed revision's retained
source. Without `--out`, it prints a rebuildable generation request as JSON.
With `--out`, it writes `src/lib.rs`, `wit/world.wit`, and `request.json`; pass
the latter to `wassette component build` (in a store where the component name
is available). `--revision` requires an exact match
with the installed receipt; earlier revision bundles are removed on replacement.
A non-empty output directory requires `--force`; symlink targets are never
overwritten. Non-generated components, retired components and generated
components built without source retention have no retrievable bundle and
return an error. Only the request's build inputs are retained, not builder
configuration, policies or credentials. Authors can put arbitrary
sensitive content in source: treat the component store and exported files as
private. No MCP source-reading tool is exposed because the existing built-in
permission model does not separately authorize disclosure of these contents.

The request supplies inline source and WIT; it cannot select the builder image,
compiler configuration, or permissions. No operator JSON profile, helper
program, helper digest, or `--generation-config` option is used.
The host asks for approval before building and installing; rebuilding an
existing generated revision is disabled by default. Generated Tool components
are enabled immediately in the ACP session that built them and in every other
running or future session; use `/tools disable` to opt a session out.
ACP layers are stored but never hot-swapped into a running session.

The JSON report contains the canonical commit/receipt, private storage key,
provenance, opaque revision token, actual refresh result, and bounded preview
diagnostics. A `committed-refresh-failed` report means installation is durable
even though catalog refresh failed; do not repeat it as a new generation.
`committed-source-export-failed` means `--emit-source` could not write local
files after installation; the report still includes the committed receipt and
`component source --out` can recover them. `commit-unknown` and
`committed-recovery-required` are also command failures:
they retain the store `operation` ID and any exact observed commit receipt.
Inspect and recover that operation before continuing. These reports never
claim rollback or authorize retrying as a new generation.
Interrupting the command cancels precommit work and waits for the supervised
builder child process to stop; an accepted store transaction finishes rather
than being aborted.

Only the private local builder image at
`~/.local/share/wassette/builder/rust-initrd.cpio` is used. Wassette does not
download or distribute the image, and there is no host compiler/Cargo fallback.
The image's `rustc` compiles the request; crates that need build scripts or
procedural macros are not supported.

### `wassette component sync`

Scan the local build drop directory once, report installed, unchanged,
deferred, rejected, and conflicting sources, and refresh the tool catalog.
Run `wassette component sync --force` to retry a previously unloaded,
unchanged local build. `--local-component-dir` selects a different drop
directory; `--component-dir` selects a different managed store. The two
directories must not overlap. Discovery uses `local:<visible-filename-stem>`
without `.wasm`; embedded root `component-name` metadata is cosmetic and does
not select the ID. See [`component load`](#wassette-component-load) for the
source identity rules shared by all acquisition paths.

Pass `--link <WASM>` repeatedly to register finished builds as stable links in
the resolved drop directory before reconciling:

```bash
wassette component sync \
  --link /checkout/bin/filesystem.wasm \
  --link /checkout/components/acp-echo-provider/target/wasm32-wasip2/release/acp_echo_provider.wasm
```

Link registration is Unix-only, refuses unrelated existing files or sources,
and preserves the store's identity, owner, policy, secret, and revision checks.
Source-checkout installers can add `--adopt-explicit-local` to transfer a
matching explicit local-file installation to managed link ownership. Adoption
is explicit and restricted to the same source-derived logical ID and visible
filename; it does not migrate registry, HTTPS, generated, or differently named
file sources. The receipt and its policy and secrets remain bound to the same
exact source and storage binding; Wassette does not automatically migrate them.
Ordinary Tool components join the shared catalog and are enabled by default.
ACP discovers installed providers at session startup; providers and layers
are not hot-swapped into running sessions.

See [Local component discovery](local-components.md) for platform paths,
security checks, and the `off|startup|watch` modes.

### `wassette acp` (experimental)

Host an ACP agent from a WebAssembly provider on stdio. This command may
change or be removed; see the [ACP design](../design/acp.md) for usage and
limitations.

ACP discovers installed providers from the shared component store and starts
`--allow-shared-grants` flags are removed; delete them from editor
configuration. Layers and providers are not hot-swapped into running sessions.

Copilot's Auto-approve and the host Terminal toggle can change while a turn is
running. Auto-approve applies to subsequent native tool permission checks, not
approvals already displayed or host component-tool revision approvals. Terminal
gates new command creation immediately after application; its model-facing tool
list refreshes next round, and existing commands continue running. Model, mode
and other backend settings still require an idle session.

ACP resolves `component_dir` and `secrets_dir` through the same command-line,
`WASSETTE_*`, `config.toml`, and platform-default precedence as the rest of the
CLI. The root `wassette --component-dir` option applies when ACP's own
`--component-dir` is absent. This keeps semantic selectors stable when an editor
starts Wassette from a directory other than the repository.

Use `--local-components startup` to reconcile the local component drop directory
before starting providers, or `--local-components watch` to keep reconciling it
while ACP runs. ACP defaults to `off`. Override the drop directory with
`--local-component-dir <PATH>`. Local discovery installs validated components;
new ordinary tools appear in the shared catalog and are enabled by default.

`/install` validates and stores ordinary Tool components and ACP artifacts.
Installed tools are enabled in every session; provider/layer artifacts do not
replace the running session's provider chain. `/version` shows the Wassette
version, full commit SHA (with `-dirty` for modified builds), and UTC build
time. Wassette handles it locally even if the active provider advertises its
own `/version`; it is never forwarded. While a session is busy, these commands
follow the same busy-session restriction.

`/tools` and `/tools list` show a markdown table of the current session's
ordinary component exports and available host tools, including their exposure
status. Use `/tools disable <name>` to opt out of an ordinary export for this
editor session; tools are enabled by default. Names can be the full
`component-id/export`, a unique export, or a unique component ID. Ambiguous
names list the exact candidates. A change takes effect on the next prompt
turn, never mid-turn; an already-admitted call can finish after disabling.
Disabling does not change another session's exposure. Tool calls still require
editor approval and do not gain file or network permissions from exposure. The
store cursor refreshes running sessions when
components are installed, upgraded, or removed. New catalog revisions appear
in `/tools list`; stale references are rejected for new calls while admitted
calls keep their pinned revision. A tool generated by `build_component` or
installed with `/install` is enabled immediately in every session.
When the Copilot provider is active, its host-owned `terminal` and
`build_component` tools are listed if available: configure the former through
the ACP terminal option; generation requires the `component-generation`
feature and the private image at its expected path, not `/tools enable`.

### `wassette run`

Start the Wassette MCP server with stdio transport for local development and testing. This is the recommended mode for MCP clients.

**Basic usage:**
```bash
# Start server with stdio transport
wassette run

# Use with specific configuration directory
wassette run --component-dir /custom/components
```

**Options:**
- `--component-dir <PATH>`: Set component storage directory (default: `$XDG_DATA_HOME/wassette/components`)
- `--local-component-dir <PATH>`: Local build drop directory, separate from the managed store
- `--local-components <off|startup|watch>`: Local discovery mode (default: `watch`)
- `--env <KEY=VALUE>`: Set environment variables (can be specified multiple times)
- `--env-file <PATH>`: Load environment variables from a file
- `--disable-builtin-tools`: Disable built-in tools (load-component, unload-component, etc.)

### `wassette serve`

Start the Wassette MCP server with Streamable HTTP for remote access. This is intended for remote deployment scenarios.

**Streamable HTTP transport:**
```bash
# Start server with Streamable HTTP transport
wassette serve

# The explicit compatibility flag is also supported
wassette serve --streamable-http

# Use a custom bind address
wassette serve --bind-address 0.0.0.0:8080

# Accept requests addressed by a name other than localhost
wassette serve --streamable-http \
  --bind-address 0.0.0.0:9001 \
  --allowed-host wassette.internal

# Serve only protocol revision 2026-07-28 and later, with plain JSON replies
wassette serve --legacy-sessions=false --json-response
```

**Options:**
- `--streamable-http`: Explicitly select Streamable HTTP transport
- `--bind-address <ADDRESS>`: Set the bind address (default: `127.0.0.1:9001`)
- `--allowed-host <HOST>`: Accept this `Host` header value on `/mcp`. Repeat for several. Defaults to loopback only
- `--component-dir <PATH>`: Set component storage directory (default: `$XDG_DATA_HOME/wassette/components`)
- `--local-component-dir <PATH>`: Local build drop directory
- `--local-components <off|startup|watch>`: Local discovery mode (default: `off`)
- `--env <KEY=VALUE>`: Set environment variables (can be specified multiple times)
- `--env-file <PATH>`: Load environment variables from a file
- `--disable-builtin-tools`: Disable built-in tools (load-component, unload-component, etc.)
- `--manifest <PATH>`: Provision components declaratively from a manifest at startup
- `--continue-on-provisioning-failure`: Start even when some manifest components fail to provision, serving only those that loaded (default: abort when any declared component fails)
- `--legacy-sessions <BOOL>`: Keep serving the pre-`2026-07-28` session lifecycle (default: `true`, env: `WASSETTE_LEGACY_SESSIONS`)
- `--json-response [<BOOL>]`: Reply to a simple stateless request with `application/json` instead of a request-scoped `text/event-stream` (default: `false`, env: `WASSETTE_JSON_RESPONSE`)

**Stateless clients:**

Clients that negotiate MCP protocol revision `2026-07-28` or later are always
served statelessly: they send a single POST carrying their client info in
`params._meta`, with no `initialize` handshake and no `Mcp-Session-Id`. That
happens regardless of `--legacy-sessions`, which only controls whether the
older session lifecycle is still offered alongside it.

Because a stateless client has no long-lived connection, it learns about tool
changes by holding open a `subscriptions/listen` response stream. Wassette
sends `notifications/tools/list_changed` on that stream whenever components are
loaded or unloaded, including changes made by a different client.

Setting `--legacy-sessions=false` removes the session lifecycle entirely:
`initialize` no longer mints a session id, and `GET /mcp` and `DELETE /mcp`
return `405 Method Not Allowed`. Only do this when every client speaks
`2026-07-28` or later.

**Note:** `--bind-address` and `--allowed-host` are independent. Binding to `0.0.0.0`
makes the server reachable on every interface, but requests are still rejected with
`403` before MCP dispatch unless their `Host` header is on the allowlist. That check
defends against DNS rebinding, so a server addressed as `http://wassette:9001/mcp` needs
`--allowed-host wassette` even though it is already listening. An entry without a port
matches any port; an entry with one must match exactly.

**A configured allowlist replaces the loopback default, it does not extend it.** After
`--allowed-host wassette.internal`, requests with `Host: localhost` or `Host: 127.0.0.1`
are rejected. Pass loopback explicitly if local clients must keep working:

```bash
wassette serve --streamable-http --bind-address 0.0.0.0:9001 \
  --allowed-host wassette.internal \
  --allowed-host localhost \
  --allowed-host 127.0.0.1
```

The `/health`, `/ready` and `/info` endpoints sit outside `/mcp` and are not subject to
this check, so health probes are unaffected either way.

## Component Management

### `wassette component load`

Load a WebAssembly component from various sources.

This command loads ordinary tool candidates, not ACP providers/layers or
non-runnable artifacts. Compatibility is checked by the ordinary runtime.
The logical ID comes from the acquisition source:

| Source | Logical component ID |
| --- | --- |
| OCI or wasm.directory package | Canonical registry/repository, without a tag or digest |
| Local file or local discovery | `local:<visible-filename-stem>` without `.wasm` |
| HTTPS download | Downloaded filename stem, without a `local:` prefix |
| Generated build | Exact spelling of the request's `build.component_name` |

Root `component-name` metadata is cosmetic; it may be absent or different and
does not block acquisition. The receipt maps the logical ID to a private opaque
storage key; use the ID reported by Wassette for component, policy, secret,
ACP, and tool selectors. Generated builds use the request's `build.component_name`
as their logical ID, never the private storage key.
The private storage key is validated separately from the logical ID. It must
be a portable ASCII filename; Windows device names, trailing dots, path
separators, and unsafe secret-filename projections are rejected rather than
renamed.

Replacement checks both source continuity and the current receipt revision.
Explicit policies and permission edits survive bundled upgrades. An existing
receipt keeps its identity, secret bindings, and ownership namespace only for
the same exact source and storage binding. Wassette does not automatically
migrate secrets or store entries. Uninstall retains a source/name reservation
and secret values, so another source cannot take over the slot. Files without
receipts remain protected and are not automatically loaded or adopted.

**Load from OCI registry:**
```bash
# Load a component from GitHub Container Registry
wassette component load oci://ghcr.io/microsoft/time-server-js:latest

# Load with custom component directory
wassette component load oci://ghcr.io/microsoft/gomodule:latest --component-dir /custom/components
```

**Load from local file:**
```bash
# Load a local component file
wassette component load file:///path/to/component.wasm

# Load with relative path
wassette component load file://./my-component.wasm
```

**Options:**
- `--component-dir <PATH>`: Component storage directory

### `wassette component unload`

Remove a loaded component by its ID.

```bash
# Unload a local component
wassette component unload local:my-component

# Unload with custom component directory
wassette component unload local:my-component --component-dir /custom/components
```

**Options:**
- `--component-dir <PATH>`: Component storage directory

### `wassette component list`

Display all currently loaded components.

**Basic JSON output:**
```bash
wassette component list
# Output: {"components":[...],"total":1}
```

**Formatted output options:**
```bash
# Pretty-printed JSON
wassette component list --output-format json

# YAML format
wassette component list --output-format yaml

# Table format (human-readable)
wassette component list --output-format table
```

**Example outputs:**

*JSON format:*
```json
{
  "components": [
    {
      "id": "ghcr.io/microsoft/time-server-js",
      "schema": {
        "tools": [
          {
            "name": "get-current-time",
            "description": "Get the current time",
            "inputSchema": {
              "type": "object",
              "properties": {}
            }
          }
        ]
      },
      "tools_count": 1
    }
  ],
  "total": 1
}
```

*Table format:*
```
ID                                | Tools | Description
----------------------------------|-------|----------------------------------
ghcr.io/microsoft/time-server-js  | 1     | Provides time-related functions
```

**Options:**
- `--output-format <FORMAT>`: Output format (json, yaml, table) [default: json]
- `--component-dir <PATH>`: Component storage directory

## Component Inspection

### `wassette inspect`

Inspect a loaded WebAssembly component and display its JSON schema. This command is useful for debugging and understanding the structure of a component's inputs and outputs.

**Note:** The component must be loaded first using `wassette component load` before it can be inspected.

**Basic usage:**
```bash
# First, load a component
wassette component load oci://ghcr.io/microsoft/time-server-js:latest

# The OCI logical ID is the canonical registry/repository, without the tag
wassette inspect ghcr.io/microsoft/time-server-js

# Or load from a local file
wassette component load file:///path/to/my-component.wasm

# The local logical ID uses the visible filename stem
wassette inspect local:my-component
```

**Example output:**
```
No package docs found, using auto-generated
get-weather, Some("Auto-generated schema for function 'get-weather'")
input schema: {
  "properties": {
    "city": {
      "type": "string"
    }
  },
  "required": [
    "city"
  ],
  "type": "object"
}
output schema: {
  "properties": {
    "result": {
      "oneOf": [
        {
          "properties": {
            "ok": {
              "type": "string"
            }
          },
          "required": [
            "ok"
          ],
          "type": "object"
        },
        {
          "properties": {
            "err": {
              "type": "string"
            }
          },
          "required": [
            "err"
          ],
          "type": "object"
        }
      ]
    }
  },
  "required": [
    "result"
  ],
  "type": "object"
}
```

The inspect command displays:
- **Function names**: The exported functions available in the component
- **Descriptions**: Either extracted from package documentation or auto-generated
- **Input schemas**: JSON schema describing the expected input parameters
- **Output schemas**: JSON schema describing the return values and result types

This is particularly useful for:
- **Development**: Verifying component interfaces during development
- **Debugging**: Understanding why a component might not be working as expected
- **Documentation**: Generating reference material for component APIs
- **Integration**: Understanding how to call component functions correctly

**Options:**
- `<PATH>`: Path to the WebAssembly component file (required)

## Registry Management

Registry commands discover and install packages from wasm.directory. Packages
are selected by their canonical `registry/repository` identity or by their exact
WIT identity, `namespace:package[@version]`.

### `wassette registry search`

Search wasm.directory for component packages. Search results are discovery
metadata only: the advertised package kind and WIT identity are not validation
of the artifact, and searching does not install or expose a component.

**Search all packages:**
```bash
# List packages indexed by wasm.directory
wassette registry search
```

**Search with a query:**
```bash
# Search for packages matching "weather"
wassette registry search weather
```

Use `--offset` and `--limit` to continue through upstream results. The default
page size is 20; the API limit is 100. `next_offset` advances by the raw
wasm.directory page size, including records excluded from component results.
The API base defaults to `https://api.wasm.directory`; set
`WASSETTE_WASM_DIRECTORY_URL` to use a compatible self-hosted endpoint.
Search requires the API to be reachable and does not fall back to a bundled
catalog. Direct local component operations remain available offline.

**Example output:**
```json
{
  "status": "success",
  "source": "wasm.directory",
  "discovery_only": true,
  "count": 1,
  "upstream_count": 1,
  "offset": 0,
  "limit": 20,
  "next_offset": null,
  "may_have_more": false,
  "components": [
    {
      "package_id": "ghcr.io/microsoft/get-weather-js",
      "description": "A weather component written in JavaScript",
      "advertised_kind": "component",
      "wit_identity": null,
      "tags": ["1.0.0"]
    }
  ]
}
```

**Options:**
- `--output-format <FORMAT>`: Output format (json, yaml, table) [default: json]
- `--offset <N>`: Upstream result offset [default: 0]
- `--limit <N>`: Upstream page size from 1 to 100 [default: 20]

### `wassette registry get`

Install a wasm.directory package by its canonical `registry/repository`
identity or its WIT identity. The selected version is resolved to an OCI
manifest digest before download; by default, Wassette selects the highest
indexed stable semver version, falling back to prereleases only when no stable
version exists.

```bash
# Install the package found by registry search
wassette registry get ghcr.io/microsoft/get-weather-js

# Install by WIT identity, optionally pinning a version
wassette registry get yosh:wordmark
wassette registry get yosh:wordmark@2.0.6

# Pin an exact indexed tag
wassette registry get ghcr.io/microsoft/get-weather-js --version 1.2.3

# Select a component storage directory (the old --plugin-dir name remains an alias)
wassette registry get ghcr.io/microsoft/get-weather-js --component-dir ./components
```

Installation validates the downloaded component and records its source-derived
logical ID, private physical storage key, package/version, manifest digest, and
provenance. An ordinary Tool becomes available in the shared tool catalog and
is enabled by default; ACP discovers installed providers from the shared store.
The existing `wassette component load PATH` command remains the direct
path/OCI/HTTPS load flow.

`--version` matches an exact indexed tag (including non-semver tags). A WIT
selector's `@version` is also an exact tag and must agree with `--version` when
both are given. A WIT selector must match the `wit_identity` of exactly one
wasm.directory component package, compared case-sensitively; zero matches is an
error, and multiple matches list the candidate `registry/repository` identities
to choose from instead of guessing.

The component ID is the canonical `registry/repository`, independent of the
selected tag or digest. Registry metadata and embedded root `component-name`
metadata do not determine it. Publishers may still include root metadata as
descriptive producer information, but Wassette accepts packages without it or
with a different value.

Search and package resolution require wasm.directory; direct local paths remain
available offline. There is no fallback to the removed bundled catalog.

## Policy Management

### `wassette policy get`

Retrieve policy information for a specific component.

```bash
# Get policy for a component loaded from my-component.wasm
wassette policy get local:my-component

# Get policy with pretty formatting
wassette policy get local:my-component --output-format json

# Get in YAML format
wassette policy get local:my-component --output-format yaml
```

**Example output:**
```json
{
  "component_id": "local:my-component",
  "permissions": {
    "storage": [
      {
        "uri": "fs://workspace/**",
        "access": ["read", "write"]
      }
    ],
    "network": [
      {
        "host": "api.openai.com"
      }
    ]
  }
}
```

**Options:**
- `--output-format <FORMAT>`: Output format (json, yaml, table) [default: json]
- `--component-dir <PATH>`: Component storage directory

## Permission Management

### `wassette permission grant`

Grant specific permissions to a component.

**Storage permissions:**
```bash
# Grant read access to a directory
wassette permission grant storage local:my-component fs://workspace/ --access read

# Grant read and write access
wassette permission grant storage local:my-component fs://workspace/ --access read,write

# Grant access to a specific file
wassette permission grant storage local:my-component fs://config/app.yaml --access read
```

**Network permissions:**
```bash
# Grant access to a specific host
wassette permission grant network local:my-component api.openai.com

# Grant access to a localhost service
wassette permission grant network local:my-component localhost:8080
```

**Environment variable permissions:**
```bash
# Grant access to an environment variable
wassette permission grant environment-variable local:my-component API_KEY

# Grant access to multiple variables
wassette permission grant environment-variable local:my-component HOME
wassette permission grant environment-variable local:my-component PATH
```

> **Note**: See the [Environment Variables reference](./environment-variables.md) for detailed instructions on how to set and pass environment variables to Wassette.

**Memory permissions:**
```bash
# Grant memory limit to a component (using Kubernetes format)
wassette permission grant memory local:my-component 512Mi

# Grant larger memory limit
wassette permission grant memory local:my-component 1Gi

# Grant memory limit with different units
wassette permission grant memory local:my-component 2048Ki
```

**Options:**
- `--access <ACCESS>`: For storage permissions, comma-separated list of access types (read, write)
- `--component-dir <PATH>`: Component storage directory

### `wassette permission revoke`

Remove specific permissions from a component.

**Storage permissions:**
```bash
# Revoke storage access
wassette permission revoke storage local:my-component fs://workspace/

# Revoke with custom component directory
wassette permission revoke storage local:my-component fs://config/ --component-dir /custom/components
```

**Network permissions:**
```bash
# Revoke network access
wassette permission revoke network local:my-component api.openai.com
```

**Environment variable permissions:**
```bash
# Revoke environment variable access
wassette permission revoke environment-variable local:my-component API_KEY
```

**Options:**
- `--component-dir <PATH>`: Component storage directory

### `wassette permission reset`

Remove all permissions for a component, resetting it to default state.

```bash
# Reset all permissions for a component
wassette permission reset local:my-component

# Reset with custom component directory
wassette permission reset local:my-component --component-dir /custom/components
```

**Options:**
- `--component-dir <PATH>`: Component storage directory

## Common Workflows

### Local Development

```bash
# 1. Inspect the component to understand its interface
wassette inspect ./target/wasm32-wasi/debug/my-tool.wasm

# 2. Build and load a local component
wassette component load file://./target/wasm32-wasi/debug/my-tool.wasm

# 3. Check it loaded correctly
wassette component list --output-format table

# 4. Grant necessary permissions
wassette permission grant storage local:my-tool fs://$(pwd)/workspace --access read,write
wassette permission grant network local:my-tool api.example.com
wassette permission grant memory local:my-tool 512Mi

# 5. Verify permissions
wassette policy get local:my-tool --output-format yaml

# 6. Test via the local stdio MCP server
wassette run
```

### Component Discovery and Installation

```bash
# 1. Search for available components in the registry
wassette registry search

# 2. Search for specific functionality
wassette registry search weather

# 3. Inspect metadata and canonical package identity
wassette registry search weather --output-format yaml

# 4. Install by stable package identity; this does not expose its tools
wassette registry get ghcr.io/microsoft/get-weather-js --version 1.2.3

# 5. Configure permissions for the component
wassette permission grant network ghcr.io/microsoft/get-weather-js api.openweathermap.org
wassette permission grant memory ghcr.io/microsoft/get-weather-js 256Mi

# 6. Verify the installed component and configure its permissions
wassette component list --output-format table
wassette policy get ghcr.io/microsoft/get-weather-js --output-format yaml

# 7. Start the local stdio MCP server; use MCP load-component with package/version
#    when the tool should be explicitly exposed
wassette run
```

### Component Distribution

```bash
# 1. Load component from OCI registry
wassette component load oci://ghcr.io/myorg/my-tool:1.0.0

# 2. Configure permissions based on component needs
wassette permission grant storage ghcr.io/myorg/my-tool fs://workspace/** --access read,write
wassette permission grant network ghcr.io/myorg/my-tool api.myservice.com
wassette permission grant memory ghcr.io/myorg/my-tool 1Gi

# 3. Start the Streamable HTTP server for remote clients
wassette serve --streamable-http
```

### Permission Auditing

```bash
# List all components and their tool counts
wassette component list --output-format table

# Check permissions for each component
for component in $(wassette component list | jq -r '.components[].id'); do
  echo "=== $component ==="
  wassette policy get $component --output-format yaml
done
```

### Cleanup Operations

```bash
# Reset permissions for a local component
wassette permission reset local:problematic-component

# Remove a component entirely
wassette component unload local:problematic-component

# List remaining components
wassette component list --output-format table
```

## Configuration

Wassette can be configured using configuration files, environment variables, and command-line options. The configuration sources are merged with the following order of precedence:

1. Command-line options (highest priority)
2. Environment variables prefixed with `WASSETTE_`
3. Configuration file (lowest priority)

### Configuration File

By default, Wassette looks for a configuration file at:
- **Linux/macOS**: `$XDG_CONFIG_HOME/wassette/config.toml` (typically `~/.config/wassette/config.toml`)
- **Windows**: `%APPDATA%\wassette\config.toml`

You can override the default configuration file location using the `WASSETTE_CONFIG_FILE` environment variable:

```bash
export WASSETTE_CONFIG_FILE=/custom/path/to/config.toml
wassette component list
```

Example configuration file (`config.toml`):

```toml
# Directory where components are stored
component_dir = "/opt/wassette/components"
```

### Environment Variables

- **`WASSETTE_CONFIG_FILE`**: Override the default configuration file location
- **`WASSETTE_COMPONENT_DIR`**: Override the default component storage location
- **`PORT`**: Set the port number for HTTP-based transports (default: 9001)
- **`BIND_HOST`**: Set the host address to bind to (default: 127.0.0.1)
- **`XDG_CONFIG_HOME`**: Base directory for configuration files (Linux/macOS)
- **`XDG_DATA_HOME`**: Base directory for data storage (Linux/macOS)

#### Bind Address Configuration

The bind address can be configured via multiple methods with the following precedence:

1. CLI option `--bind-address` (highest priority)
2. Configuration file `bind_address` field
3. PORT and BIND_HOST environment variables (used as defaults when above are not set)
4. Built-in defaults: 127.0.0.1:9001 (or 0.0.0.0:9001 in Docker)

### Component Storage

By default, Wassette stores components in `$XDG_DATA_HOME/wassette/components` (typically `~/.local/share/wassette/components` on Linux/macOS). You can override this with the `--component-dir` option:

```bash
# Use custom storage directory
export WASSETTE_COMPONENT_DIR=/opt/wassette/components
wassette component load oci://example.com/tool:latest --component-dir $WASSETTE_COMPONENT_DIR
```

## Integration with MCP Clients

The CLI commands complement the MCP server functionality. You can:

1. Use CLI commands to pre-configure components and permissions
2. Start the MCP server with `wassette serve`
3. Connect MCP clients to the running server
4. Use CLI commands for administrative tasks while the server runs

**Example VS Code configuration:**
```json
{
  "name": "wassette",
  "command": "wassette",
  "args": ["run"]
}
```

## Error Handling

The CLI provides clear error messages for common issues:

```bash
# Component not found
$ wassette component unload nonexistent
Error: Component 'nonexistent' not found

# Invalid path
$ wassette component load invalid://path
Error: Unsupported URI scheme 'invalid'. Use 'file://' or 'oci://'

# Permission denied
$ wassette permission grant storage local:my-component /restricted --access write
Error: Permission denied: cannot grant write access to /restricted
```

## Output Formats

All commands that return structured data support multiple output formats:

- **JSON** (default): Machine-readable, suitable for scripting
- **YAML**: Human-readable structured format
- **Table**: Formatted for terminal display

Use the `--output-format` or `-o` flag to specify the desired format:

```bash
wassette component list -o table
wassette policy get local:my-component -o yaml
```

## See Also

- [Main README](https://github.com/microsoft/wassette/blob/main/README.md) - Installation and basic usage
- [MCP Client Setup](../mcp-clients.md) - Configuring MCP clients
- [Architecture Overview](../overview.md) - Understanding Wassette's design
- [Examples](https://github.com/microsoft/wassette/tree/main/examples) - Sample WebAssembly components
