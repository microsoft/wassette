# config.toml

This page provides a comprehensive reference for the `config.toml` configuration file used by the Wassette MCP server. This file is optional and provides defaults for server behavior, including component storage locations, secrets directory, and environment variables.

### Location

- **Linux/macOS**: `$XDG_CONFIG_HOME/wassette/config.toml` (typically `~/.config/wassette/config.toml`)
- **Windows**: `%APPDATA%\wassette\config.toml`
- **Custom**: Set via `WASSETTE_CONFIG_FILE` environment variable

### Configuration Priority

Configuration values are merged with the following precedence (highest to lowest):

1. Command-line options (e.g., `--component-dir`)
2. Environment variables prefixed with `WASSETTE_`
3. Configuration file (`config.toml`)

### Schema

```toml
# Directory where WebAssembly components are stored
# Default: $XDG_DATA_HOME/wassette/components (~/.local/share/wassette/components)
component_dir = "/path/to/components"

# Optional local build drop directory (separate from the managed component store)
local_component_dir = "/path/to/local-components"

# Discovery for locally built components: "off", "startup", or "watch"
local_components = "watch"

# Optional trusted JSON profile; requires the component-generation feature
# Unset by default. This is an operator path, never a model-request parameter.
# generation_config = "/path/to/operator-generation.json"

# Directory where secrets are stored (API keys, credentials, etc.)
# Default: $XDG_CONFIG_HOME/wassette/secrets (~/.config/wassette/secrets)
secrets_dir = "/path/to/secrets"

# Bind address for Streamable HTTP
# Default: 127.0.0.1:9001
bind_address = "0.0.0.0:8080"

# Keep serving the pre-2026-07-28 MCP session lifecycle
# Default: true
legacy_sessions = true

# Reply to a simple stateless request with application/json
# Default: false
json_response = false

# Environment variables to be made available to components
# These are global defaults and can be overridden per-component in policy files
[environment_vars]
API_KEY = "your_api_key"
LOG_LEVEL = "info"
DATABASE_URL = "postgresql://localhost/mydb"
```

### Fields

#### `component_dir`

- **Type**: String (path)
- **Default**: Platform-specific data directory
- **Description**: Directory where loaded WebAssembly components are stored. Components loaded via `wassette component load` or the MCP interface are saved here.

#### `secrets_dir`

- **Type**: String (path)
- **Default**: Platform-specific config directory
- **Description**: Directory for storing sensitive data like API keys and credentials. This directory should have restricted permissions (e.g., `chmod 600`).

#### `local_component_dir` and `local_components`

- **Types**: Path and `"off" | "startup" | "watch"`
- **Default directory**: Platform data directory's `wassette/local-components`
- **Description**: Scans locally built components in a separate inbox before
  validating and installing them in the managed store. `run` defaults to
  `watch`; `serve` defaults to `off`. Set `local_components = "startup"` for
  a single startup scan, or `"off"` to disable it. See
  [Local component discovery](local-components.md).

#### `bind_address`

- **Type**: String
- **Default**: `127.0.0.1:9001`
- **Description**: Bind address for Streamable HTTP. The address should be in the format `host:port`. Use `0.0.0.0` to bind to all network interfaces, or a specific IP address to bind to a particular interface. This setting is ignored when using stdio transport.

#### `generation_config`

- **Type**: String (path to a trusted operator JSON profile)
- **Default**: Unset; generation is disabled
- **Availability**: `run`, `serve`, `acp`, and `component build` in binaries built with
  the default-off `component-generation` feature
- **Precedence**: `--generation-config` > `WASSETTE_GENERATION_CONFIG` >
  `generation_config` in `config.toml`

The profile is bounded to 1 MiB and rejects unknown fields. It contains
`builder` settings for the separate `wassette-builder` helper and its trusted
digest-pinned local initrd, optional finite `limits`, and four independently
controlled booleans, all false by default:

| Profile field | Authorizes |
| --- | --- |
| `allow_build` | Compilation in the isolated helper |
| `allow_install` | Committing validated output to the component store |
| `allow_expose` | Requesting ordinary-tool exposure, separately from installation |
| `allow_rebuild` | Replacing a generated lineage at an explicitly matched revision |

`retain_source` defaults to `true`. Set it to `false` to omit the private
generated-source bundle from new installs/rebuilds. The bundle contains the
author's exact Rust source and WIT, which may include anything the author
supplied (including secrets); protect the managed store accordingly. This
operator-only storage choice does not change the compiler inputs or
`profile_sha256`. Existing retained bundles are removed on a rebuild with
retention disabled or on uninstall.

**Operator profile example:** Replace both SHA-256 placeholders with the
64-character lowercase hexadecimal digests of the provisioned files. Create the
private staging directory before starting Wassette; it must not be the live
component store. The example permits native CLI/MCP build-and-install only, not
exposure, rebuild, or ordinary-Wasm caller access.

`scripts/generation-profile.py --helper <helper> --initrd <initrd> --output
<profile>` writes such a profile for local files, computing both digests. It
never downloads or publishes an image and refuses to overwrite an existing file.

```json
{
  "builder": {
    "helper_path": "helper/wassette-builder",
    "helper_sha256": "<sha256-of-provisioned-helper>",
    "initrd_path": "images/rust-builder.initrd",
    "initrd_sha256": "<sha256-of-provisioned-initrd>",
    "staging_root": "staging",
    "wit_dependencies": []
  },
  "allow_build": true,
  "allow_install": true,
  "allow_expose": false,
  "allow_rebuild": false,
  "callers": []
}
```

The six `builder` fields shown are required. Both helper and initrd files must already
exist; their digests are verified for each job. `wit_dependencies` contains
**inline complete WIT package strings**, in dependency-first order, not paths,
URLs, or registry coordinates. Use an empty list for a self-contained WIT
world. For ACP layers, the operator must supply the pinned canonical ACP package
and its dependency graph. At most 32 packages are accepted, and their combined
UTF-8 bytes plus the request WIT must fit `limits.wit_bytes`. The 1 MiB
profile-file cap also includes JSON escaping and the rest of the configuration.

`builder.rust_crates` is optional and defaults to an empty list. Each entry pins
a gzip-compressed registry `.crate` archive that the guest extracts and compiles
with the trusted `rustc` before the request source, which can then `use` the
crate by `name`. List entries in dependency-first order; requests cannot add,
remove or reorder them.

```json
"rust_crates": [
  {
    "name": "memchr",
    "archive_path": "crates/memchr-2.8.3.crate",
    "archive_sha256": "<Cargo.lock checksum>",
    "root": "src/lib.rs",
    "edition": "2021",
    "features": ["alloc", "default", "std"]
  },
  {
    "name": "aho_corasick",
    "archive_path": "crates/aho-corasick-1.1.5.crate",
    "archive_sha256": "<Cargo.lock checksum>",
    "root": "src/lib.rs",
    "edition": "2021",
    "features": ["perf-literal", "std"],
    "dependencies": [{ "crate": "memchr" }]
  }
]
```

| Crate field | Meaning |
| --- | --- |
| `name` | Rust crate name (`grep_searcher`, not `grep-searcher`) and request extern |
| `archive_path` | `.crate` archive containing one top-level directory; relative paths resolve against the profile directory |
| `archive_sha256` | Archive digest; for crates.io this is the `Cargo.lock` checksum |
| `root` | Library root inside the archive's top-level directory |
| `edition` | `2015`, `2018`, `2021` or `2024` |
| `features` | Enabled Cargo features, passed as `--cfg feature="…"` |
| `dependencies` | Earlier entries this crate links to; `rename` sets the extern name the crate's source uses |

Copy the features, dependency renames, editions and roots that `cargo build -v`
passes to `rustc` for `wasm32-wasip2`. The image has no Cargo and no host
standard library, so crates that need build scripts or procedural macros
(including the `multiversion_no_op` macro used by `encoding_rs` 0.8.36 and later)
are not supported. At most 64 crates are accepted, each archive must fit 16 MiB,
and all archives together 64 MiB. Archives are re-hashed when staged, and the
crate list is bound into the generated component's `profile_sha256`; crate-free
profiles keep their previous digest. Crate compiler output is not returned to
requesters, so a failing crate reports the builder as unavailable.

Omit `limits` to use the builder defaults below. If `limits` is present, supply
all eight fields; individual fields do not have JSON defaults. Values must be
positive and no greater than the listed ceiling. Only `guest_scratch_mib` has
a ceiling above its default.

| `limits` field | Default | Ceiling |
| --- | ---: | ---: |
| `source_bytes` | 1048576 | 1048576 |
| `wit_bytes` | 262144 | 262144 |
| `wasm_bytes` | 33554432 | 33554432 |
| `diagnostics_bytes` | 262144 | 262144 |
| `wall_time_ms` | 120000 | 120000 |
| `guest_scratch_mib` | 2048 | 8192 |
| `generated_bindings_bytes` | 8388608 | 8388608 |
| `max_parallel_jobs` | 1 | 4 |

The guest does not reclaim memory from compiler processes that have exited,
so each pinned crate adds to peak scratch use. A guest that exhausts its scratch
memory reports `compilation_failed` with a diagnostic naming
`limits.guest_scratch_mib`. The thirteen crates behind `grep-searcher` and
`grep-regex` need `guest_scratch_mib` of 8192.

The CLI/MCP adapter independently caps encoded requests at 2 MiB, decoded
source and WIT at 256 KiB each, and each returned diagnostic string at 16 KiB of
JSON-encoded bytes. These transport restrictions are not builder defaults.
Component-name and world input bounds match the builder's 512-byte and 256-byte
allowances, respectively, and preserve exact spelling.
`callers` defaults to an empty list of explicit revision-bound ordinary-Wasm
grants; native CLI/MCP permission does not implicitly grant guest callers access.

Relative `helper_path`, `initrd_path`, and `staging_root` values in the JSON
profile resolve against the profile's containing directory, not the process
working directory. The profile path itself follows normal CLI/config path
semantics (a relative profile path is relative to the working directory).
The request cannot override the profile, helper, image, limits, permissions or
compiler configuration. No host compiler/Cargo fallback or OCI builder-image
download/distribution is provided; only local initrds are supported.

The combined CLI/MCP adapter requires both build and install authorization.
Profiles permitting only builds do not advertise `build-component`.
`--disable-builtin-tools` still disables that management tool. A requested
`ExposeTools` intent is not authority to expose a tool; ACP layers must remain
install-only. Generation uses the same configured lifecycle manager, clients,
component store, secrets directory and component environment as other operations;
those component environment settings are not compiler-environment overrides.

Only trusted operators should be able to modify this file or its helper/initrd.
MCP clients never receive a configuration-path parameter.

#### `allowed_hosts`

- **Type**: Array of strings
- **Default**: Unset, which accepts loopback (`localhost`, `127.0.0.1`, `::1`) only
- **Description**: `Host` header values accepted on the `/mcp` endpoint for Streamable HTTP. Requests whose `Host` is not listed are rejected with `403` before MCP dispatch, which is what prevents DNS rebinding against a locally running server. Set this when the server is addressed by a service name, container name or DNS name rather than by `localhost`. Entries may be a bare hostname, which matches any port, or `host:port`, which must match exactly. A configured list **replaces** the loopback default rather than extending it, so include loopback explicitly if local clients must keep working. An empty list is treated as unset, leaving the loopback default in place. This setting is independent of `bind_address` and is ignored when using stdio transport.

```toml
allowed_hosts = ["wassette.internal", "localhost", "127.0.0.1"]
```

#### `legacy_sessions`

- **Type**: Boolean
- **Default**: `true`
- **Description**: Whether to keep serving the MCP session lifecycle used by protocol revisions before `2026-07-28`. Clients that negotiate `2026-07-28` or later are served statelessly either way, so setting this to `false` only removes support for older clients: `initialize` stops minting a session id and `GET`/`DELETE` on `/mcp` return `405`. This setting is ignored when using stdio transport.

#### `json_response`

- **Type**: Boolean
- **Default**: `false`
- **Description**: Whether a simple stateless request that produces a single reply is answered with `application/json` instead of a request-scoped `text/event-stream`. Requests that produce more than one message still fall back to an event stream. This setting is ignored when using stdio transport.

#### `environment_vars`

- **Type**: Table/Map
- **Default**: Empty
- **Description**: Key-value pairs of environment variables to make available to components. Note that components must explicitly request access to environment variables via their policy files. See the [Environment Variables reference](./environment-variables.md) for detailed usage patterns and examples.

### Example Configurations

**Minimal Configuration:**
```toml
# Use all defaults
```

**Development Configuration:**
```toml
component_dir = "./dev-components"
secrets_dir = "./dev-secrets"
bind_address = "127.0.0.1:9001"

[environment_vars]
LOG_LEVEL = "debug"
RUST_LOG = "trace"
```

**Production Configuration:**
```toml
component_dir = "/opt/wassette/components"
secrets_dir = "/opt/wassette/secrets"
bind_address = "0.0.0.0:8080"

[environment_vars]
LOG_LEVEL = "info"
NODE_ENV = "production"
```

### Environment Variables

You can override any configuration value using environment variables with the `WASSETTE_` prefix:

```bash
# Override component directory
export WASSETTE_COMPONENT_DIR=/custom/components

# Override bind address using PORT and BIND_HOST
export PORT=8080
export BIND_HOST=0.0.0.0

# Override config file location
export WASSETTE_CONFIG_FILE=/etc/wassette/config.toml

# Start server
wassette serve --streamable-http
```

## See Also

- [CLI Reference](cli.md) - Command-line usage and options
- [Permissions Guide](permissions.md) - Working with permissions
