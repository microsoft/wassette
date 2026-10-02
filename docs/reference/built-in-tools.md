# Built-in Tools

Wassette comes with several built-in tools for managing components and their permissions. These tools are available immediately when you start the MCP server, unless `--disable-builtin-tools` is set. Component generation additionally requires a binary built with the `component-generation` feature and the private local builder image described below.

| Tool | Description |
|------|-------------|
| `load-component` | Loads a component from a direct path/URI or an explicit wasm.directory package request |
| `unload-component` | Unloads a tool or component |
| `list-components` | Lists all currently loaded components or tools |
| `search-components` | Searches wasm.directory for component packages; results are discovery-only |
| `get-policy` | Gets the policy information for a specific component |
| `grant-storage-permission` | Grants storage access permission to a component, allowing it to read from and/or write to specific storage locations |
| `grant-network-permission` | Grants network access permission to a component, allowing it to make network requests to specific hosts |
| `grant-environment-variable-permission` | Grants environment variable access permission to a component, allowing it to access specific environment variables |
| `revoke-storage-permission` | Revokes all storage access permissions from a component for the specified URI path, removing both read and write access to that location |
| `revoke-network-permission` | Revokes network access permission from a component, removing its ability to make network requests to specific hosts |
| `revoke-environment-variable-permission` | Revokes environment variable access permission from a component, removing its ability to access specific environment variables |
| `reset-permission` | Resets all permissions for a component, removing all granted permissions and returning it to the default state |

<details>
<summary><strong>Component Management Tools</strong></summary>

## build-component (opt-in)

This combined native management operation builds, validates, and installs a
component. It is listed when the binary includes `component-generation` and the
private builder image is available at
`~/.local/share/wassette/builder/rust-initrd.cpio`.
`--disable-builtin-tools` hides it and rejects invocation.

**Parameters:**
- `build` (object, required):
  - `component_name` (string): Logical ID for the generated component, up to 512 UTF-8 bytes; exact spelling is preserved
  - `source` (string): Inline Rust source, up to 256 KiB
  - `wit` (string): Inline WIT, up to 256 KiB
  - `world` (string): Selected WIT world, up to 256 UTF-8 bytes; exact spelling is preserved
  - `kind`: `Tool` or `AcpLayer` (case-sensitive builder enum); providers are not supported
- `target` (object, optional): `{"mode":"new"}` by default, or
  `{"mode":"rebuild","expected_revision":"<opaque revision from prior report>"}`
- `reinstall_policy` (string, optional): Exact previous policy YAML, up to
  128 KiB, only when reinstalling a retired generated lineage; never new grants

Unknown fields are rejected. The 2 MiB encoded-request cap and 256 KiB decoded
source/WIT caps are independent **adapter transport restrictions**, not builder
defaults. JSON escaping can exceed the total cap even when every decoded field
fits its byte limit. Revision tokens cannot exceed 256 bytes. The builder
defaults to a 1 MiB source budget and a 256 KiB budget shared by request WIT and
the host's shipped WIT dependencies.
No profile path, image path, host filesystem path, compiler flags, environment,
source identity, or storage key can be selected in this request.

Client approval to invoke the combined management tool is outside the server:
there is **no second per-request approval dialog** between building and
installing. Keep `allow_rebuild` false unless that operation is intended.
ACP also authorizes new builds and installations without host editor prompts,
independently of the provider. Generated tool components are
enabled in every ACP session. Tool-kind eligibility follows the
admitted artifact kind, and ACP layers require a new session. Rebuild remains
unavailable by default; explicitly enabled ACP rebuilds still require editor
approval before both phases. Tool execution needs its separate revision
approval and policy grants. Denied host permissions are checked before
compilation.

Generated ordinary tools join the shared tool catalog after installation and
are enabled by default in every ACP session, including the one that generated
them. Sessions can opt out with `/tools disable`. Installing an ACP layer does
not change a running provider chain; providers and layers are not hot-swapped.
Agent/client interfaces are never exposed as MCP tools.

**Returns:** Text and structured JSON carrying the canonical `commit` receipt
(component ID, private storage key, provenance and revision), `refresh`,
`preview` (actual kind/name, Wasm hash, evidence and bounded diagnostics), and
an opaque `revision` string suitable for a later rebuild. The adapter does not
write a second receipt or emit a second catalog notification.

Each error, builder-diagnostic, or preview-diagnostic string is capped at 16 KiB of encoded JSON,
including control-character escaping, and marked when truncated. Diagnostics
are returned only to the caller, not written to transport logs.
Typed builder failures return `phase: "build"`, `code: "builder-error"`, and
`build_error_kind`. Request, WIT, compilation, and output-validation failures
may also include the builder's sanitized `diagnostic` for correcting the input.
Unavailable/internal failures do not disclose host or configuration details.

Failures before commit have `status: "failed"`. A refresh failure after commit
has `status: "committed-refresh-failed"`, retains the actual `commit`, and has
`refresh: null`. This is not a rollback: refresh the catalog rather
than retrying as a new generation.

An interrupted store commit can instead return `status: "commit-unknown"` or,
when core observes the exact operation's committed receipt,
`status: "committed-recovery-required"`. Both remain MCP errors with
`phase: "commit-recovery"`, an `operation` ID, the canonical optional `commit`,
and `refresh: null`. Neither means the operation rolled back. Inspect and
recover that existing store operation before continuing; do not retry it as a
new generation or infer the outcome from an artifact hash.

Request cancellation or transport closure cancels precommit work; jobs remain
owned until the in-process VM stops and any accepted transaction finishes.

Generation uses the private local builder image; Wassette does not download or
distribute it. There is no host compiler/Cargo fallback. For setup details, see
[component generation](configuration-files.md#component-generation).

## load-component
**Parameters:**
- Exactly one of:
  - `path` (string): Direct component source such as `file:///path/to/component.wasm` or `oci://ghcr.io/microsoft/time-server-js:1.2.3`
  - `package` (string): Canonical wasm.directory identity such as `ghcr.io/microsoft/time-server-js`, or an exact WIT identity such as `yosh:wordmark` or `yosh:wordmark@2.0.6` that must match exactly one package
- `version` (string, optional): Exact indexed tag; valid only with `package`, and must agree with a WIT selector's `@version`

The `package` form resolves the selected tag to an immutable manifest digest
and explicitly loads an ordinary tool component into this runtime. Use
`wassette registry get` to store a package without loading it into this runtime;
its Tool kind still makes it eligible for the shared catalog.
ACP providers/layers cannot be loaded as ordinary MCP tools.

**Returns:**
```json
{
  "status": "component loaded successfully",
  "id": "component-unique-id",
  "tools": ["tool-one", "tool-two"]
}
```
When an existing component is replaced, the `status` value becomes
`component reloaded successfully`.
Package loads also return `package`, `wit_identity`, `requested_version`, `selected_version`,
`manifest_digest`, `storage_key`, `revision`, and the persisted `receipt`.

ACP providers/layers and unsupported artifact shapes cannot be loaded as
ordinary tools, including through cached schemas. An ordinary candidate still
needs runtime validation. The returned `id` is derived from the acquisition
source: canonical registry/repository for OCI and wasm.directory, `local:` plus
the visible filename stem for local files and discovery, the downloaded
filename stem for HTTPS, or the exact `build.component_name` for generated
builds. Embedded root names are cosmetic and may be missing or different. Use
the returned ID for policy and secret operations; the receipt separately
preserves its private opaque storage key. New storage keys must still be
portable; Wassette does not derive them from logical IDs.
Artifact, effective policy and receipt are committed together for cooperating
readers. Failed validation preserves the installed revision; conflicting
ownership or revisions require an explicit retry, not an overwrite.

## unload-component
**Parameters:**
- `id` (string, required): Unique identifier of the component to unload

**Returns:**
```json
{
  "status": "component unloaded successfully",
  "id": "component-unique-id"
}
```

## list-components
**Parameters:** None

**Returns:**
```json
{
  "components": [
    {
      "id": "component-id",
      "tools_count": 2,
      "schema": {
        "tools": [...]
      }
    }
  ],
  "total": 1
}
```

## search-components
**Parameters:**
- `query` (string, optional): Search query sent to wasm.directory
- `offset` (integer, optional): Upstream result offset (default: `0`)
- `limit` (integer, optional): Upstream page size from 1 to 100 (default: `20`)

Results are discovery-only. `advertised_kind` is registry metadata, not
validation of the downloaded artifact. Interface packages are excluded from
component results, and no package is installed or exposed by searching.
`next_offset` is based on raw upstream records; `may_have_more` means a later
upstream page may exist.

**Returns:**
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

</details>

<details>
<summary><strong>Policy Management Tools</strong></summary>

## get-policy
**Parameters:**
- `component_id` (string, required): ID of the component to get policy information for

**Returns:**
```json
{
  "status": "policy found",
  "component_id": "component-id",
  "policy_info": {
    "policy_id": "policy-uuid",
    "source_uri": "oci://registry.example.com/component:tag",
    "local_path": "/path/to/cached/component",
    "created_at": 1640995200
  }
}
```

</details>

<details>
<summary><strong>Permission Grant Tools</strong></summary>

## grant-storage-permission
**Parameters:**
- `component_id` (string, required): ID of the component to grant storage permission to
- `details` (object, required):
  - `uri` (string, required): URI of the storage resource (e.g., `fs:///tmp/test`)
  - `access` (array, required): Array of access types, must be `["read"]`, `["write"]`, or `["read", "write"]`

**Returns:**
```json
{
  "status": "permission granted successfully",
  "component_id": "component-id",
  "permission_type": "storage",
  "details": {
    "uri": "fs:///tmp/test",
    "access": ["read", "write"]
  }
}
```

## grant-network-permission
**Parameters:**
- `component_id` (string, required): ID of the component to grant network permission to
- `details` (object, required):
  - `host` (string, required): Host to grant network access to (e.g., `api.example.com`)

**Returns:**
```json
{
  "status": "permission granted successfully",
  "component_id": "component-id",
  "permission_type": "network",
  "details": {
    "host": "api.example.com"
  }
}
```

## grant-environment-variable-permission
**Parameters:**
- `component_id` (string, required): ID of the component to grant environment variable permission to
- `details` (object, required):
  - `key` (string, required): Environment variable key to grant access to (e.g., `API_KEY`)

**Returns:**
```json
{
  "status": "permission granted successfully",
  "component_id": "component-id",
  "permission_type": "environment",
  "details": {
    "key": "API_KEY"
  }
}
```

</details>

<details>
<summary><strong>Permission Revoke Tools</strong></summary>

## revoke-storage-permission
**Parameters:**
- `component_id` (string, required): ID of the component to revoke storage permission from
- `details` (object, required):
  - `uri` (string, required): URI of the storage resource to revoke access from (e.g., `fs:///tmp/test`)

**Returns:**
```json
{
  "status": "permission revoked successfully",
  "component_id": "component-id",
  "uri": "fs:///tmp/test",
  "message": "All access (read and write) to the specified URI has been revoked"
}
```

## revoke-network-permission
**Parameters:**
- `component_id` (string, required): ID of the component to revoke network permission from
- `details` (object, required):
  - `host` (string, required): Host to revoke network access from (e.g., `api.example.com`)

**Returns:**
```json
{
  "status": "permission revoked",
  "component_id": "component-id",
  "permission_type": "network",
  "details": {
    "host": "api.example.com"
  }
}
```

## revoke-environment-variable-permission
**Parameters:**
- `component_id` (string, required): ID of the component to revoke environment variable permission from
- `details` (object, required):
  - `key` (string, required): Environment variable key to revoke access from (e.g., `API_KEY`)

**Returns:**
```json
{
  "status": "permission revoked",
  "component_id": "component-id",
  "permission_type": "environment",
  "details": {
    "key": "API_KEY"
  }
}
```

## reset-permission
**Parameters:**
- `component_id` (string, required): ID of the component to reset permissions for

**Returns:**
```json
{
  "status": "permissions reset successfully",
  "component_id": "component-id"
}
```

</details>

These tools enable you to dynamically manage components and their security permissions without needing to restart the server or modify configuration files directly.
