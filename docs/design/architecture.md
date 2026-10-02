```mermaid
sequenceDiagram
    participant Client as MCP Client
    participant Server as Wassette MCP Server
    participant LM as LifecycleManager
    participant Engine as Wasmtime Engine
    participant Registry as Component Registry
    participant Policy as Policy Engine
    
    Client->>Server: load-component(path)
    Server->>LM: load_component(uri)
    
    alt OCI Registry
        LM->>LM: Download from OCI
    else Local File
        LM->>LM: Load from filesystem
    else HTTP URL
        LM->>LM: Download from URL
    end
    
    LM->>LM: Capture source bytes and provenance
    LM->>LM: Inspect root name and artifact kind
    Note over LM: Only ordinary tool candidates enter this runtime
    LM->>Policy: Prepare captured effective policy
    Policy->>Policy: Create private WASI State Template
    LM->>Engine: Create Component
    Engine->>Engine: Compile WebAssembly
    Engine->>Engine: Link imports and prepare tool schemas
    Note over LM,Policy: Validation failures leave installed files and runtime unchanged
    LM->>LM: Compare owner/revision and commit store journal
    LM->>Policy: Pin committed effective policy to the instance
    
    LM->>Registry: Register Component
    Registry->>Registry: Publish prepared JSON schemas
    Registry->>Registry: Map Tools to Component
    
    LM-->>Server: Semantic component ID + LoadResult + commit revision
    Server-->>Client: Success with ID
    
    Note over Client,Policy: Component is now loaded and ready
    
    Client->>Server: call_tool(tool_name, args)
    Server->>LM: invoke_unique_tool(tool_name, args)
    LM->>LM: Restore selected component from its store receipt
    LM->>Registry: Resolve unique name and clone instance, export and schema
    Note over LM,Registry: Collisions are errors; release registry lock before execution
    
    LM->>Policy: Get WASI State for Component
    Policy->>Policy: Apply Security Policy
    Policy->>Engine: Create Store with WASI Context
    
    LM->>Engine: Instantiate Component
    Engine->>Engine: Call Function with Args
    Engine->>Engine: Execute in Sandbox
    
    Engine-->>LM: Results
    LM-->>Server: Raw result + selected descriptor
    Server-->>Client: Tool Result
```

## Transactional installation and replacement

`load_component` captures the incoming Wasm and any policy supplied by the
existing acquisition path before changing installed files. It inspects and
compiles those bytes directly, links imports, prepares schemas, and validates the
effective policy without publishing runtime state or caches. An old `.cwasm`
cannot validate a replacement. Invalid Wasm, unsupported imports, invalid or
unreadable policy, and private staging failures leave the installed component,
policy, caches, and loaded tools unchanged.

An unbundled replacement retains the effective policy and attachment metadata.
Explicit attachments and permission edits also take precedence over incoming
bundled policies, including an explicit policy clear. This conservative
precedence is an intentional behavior change: use an explicit policy operation
or a manifest override to change operator-selected grants. Ordinary local loads
do not discover adjacent policies; ACP acquisition retains that behavior.
All paths preserve the supplied HTTP/OCI clients, environment and secrets
directory.

`acquisition::acquire_component` captures inputs without installing or choosing a
runtime. It replaces the former `loader::fetch_component*` and
`promote_component_artifact` publication APIs. `ComponentStore::commit_install`
requires a `PreparedInstall` validated against the exact Wasm and effective
policy. Ordinary loads compile, link and prepare schemas; ACP compiles and checks
the exported stage and protocol version with its own engine. ACP evidence does
not promise complete host linking: remaining link failures are selection errors.
Installation alone neither exposes tools nor starts an ACP agent.
Ordinary startup, cache hydration and restoration require `ExposeTools` intent.
An `InstallOnly` receipt stays unexposed even with a valid tool cache or after a
policy edit; an explicit ordinary load can commit the exposure intent.

The store retains the flat artifact layout:

```text
<storage-key>.wasm
<storage-key>.policy.yaml
<storage-key>.policy.meta.json
<storage-key>.install.json
.store.lock
.store-state.json
.transactions/<operation>/
.active-transaction
```

Receipts separate semantic identity, private storage key, stable source, owner,
acquisition evidence and deployment intent. They bind the Wasm hash and effective
policy, including policy provenance and attachment metadata. An OCI manifest
digest is not the Wasm byte hash; unresolved versions/digests remain unknown.
Every authoritative change, including a policy-only edit, advances a persistent
store cursor and entry revision.

All cooperating readers and writers use the same filesystem lock and journal.
Private staging precedes the exclusive commit lock; owner/source/revision checks
are repeated under it. The durable store head is replaced last. Recovery restores
the complete old image before that decision or finishes the new image afterward.
Several renames are **not** independently an atomic bundle. Snapshot readers pin
the coherent file set before releasing the shared lock; compilation and execution
never reopen mutable live artifact or policy paths.

The lock order is per-component guard, store lock, then a bounded in-memory
try-lock. Network access, compilation, permission prompts and guest execution
hold no store lock. `checked_read` checks the global cursor and optional entry
revision while admitting a prepared in-memory swap; it cannot cross an await.
Transaction workers retain recovery material across request cancellation.
Disk commitment and runtime publication remain separate outcomes: a subsequent
conflict is reported as committed-to-disk but requiring a runtime retry, not a
rollback. Existing calls retain their captured inputs.

Caches are derived, revision/hash/engine/schema-bound and conditionally published.
Stale compilation cannot overwrite a new installation or resurrect a removal.
Invalidating a cache never deletes installed Wasm. Cache failures are logged and
can require rebuilding. `snapshot_if_changed(None)` always returns full inventory;
a known cursor detects changes from other processes without relying on an
in-process notification. The runtime catalog consumes this cursor; it is not a
second durable generation file or a filesystem watcher.

Explicit uninstall records a retired reservation and preserves secrets.
Managed-source cleanup additionally compares exact ownership and revision, so
it cannot prune a later explicit adoption. Retirement retains the previous
owner and source observation; local discovery decides whether reappearance
should be suppressed. Reinstall cannot transfer a name, storage slot or secrets
to a different source.
If a retired entry previously had a nonempty explicit policy, reinstall must
supply that policy or deliberately select a new explicit policy; a new bundle
does not silently inherit or replace its grants.

The protocol requires cooperating binaries on a local filesystem with advisory
locks, same-filesystem replacement, hard links and suitable durability barriers.
It provides process-crash recovery; power-loss durability is platform-dependent.
Windows sharing/directory-sync behavior and network filesystems are not claimed
as validated. Read-only stores or unavailable coordination/recovery fail
explicitly. Old binaries and direct edits to authoritative live files are outside
the protocol. Abandoned staging is diagnosed, not deleted based on age.

## Component names, storage keys, and runtime kinds

`wassette::inspect_artifact` reads binary metadata without compiling. It returns
the explicit root component name, if present, separately from the artifact's
shape: an ordinary tool candidate, ACP provider, ACP layer, or unsupported
artifact. Root `component-name` metadata is cosmetic: it may be missing or
differ from the logical `ComponentId`, and it does not establish source identity
or provenance.

New logical IDs come from the acquisition source:

| Source | Logical `ComponentId` |
| --- | --- |
| OCI or wasm.directory | Canonical registry/repository, excluding tag and digest |
| Local file or discovery | `local:` plus the visible filename stem, without `.wasm` |
| HTTPS download | Downloaded filename stem, without a `local:` prefix |
| Generated build | Exact `build.component_name` spelling from the request |

The private `StorageKey` is an opaque physical key, not a logical name or
sanitized form of one. An existing receipt retains its identity, policy and
secret bindings, and ownership namespace only for the same exact source and
storage binding. Wassette does not automatically migrate store entries or
secrets to a new identity. Unrecorded artifacts remain protected inventory and
are not automatically adopted.

`StorageKey` validates portable filesystem stems and reports collision keys for
case-insensitive artifact paths and the existing secrets filename projection.
It neither generates semantic names nor grants permission to replace a component.
New keys reject nonportable spelling, Windows device names, trailing dots, and
unsafe projected secret filenames instead of silently renaming them.

First-party build recipes embed the explicit names declared in
`scripts/component-names.json` before hashing or publishing their outputs.
These names remain useful descriptive producer metadata, but do not determine
runtime selectors. Nested metadata is preserved; opaque third-party downloads
are not renamed. See
[producer naming](../development/getting-started.md#declaring-first-party-component-names).

Load results and component, policy, secret, and ACP selectors use the
source-derived logical ID. Receipts map that ID to the private storage key;
logical IDs are not sanitized into filenames. Existing physical policy and
secret bindings remain associated with their receipt, and unknown secret files
cannot be adopted by a new component.

Local sources bind to the canonical source path; HTTPS sources bind to the
normalized request (query redacted in receipts, but significant through its
hash); OCI sources bind to the canonical repository. These are conservative
continuity rules, not source authentication. Moving a file or changing a signed
URL can therefore conflict even when its source-derived logical ID is
unchanged.

The ordinary runtime inspects current Wasm bytes before eager or lazy compilation,
native-cache loading, and cached tool/schema publication. ACP providers/layers
and non-runnable shapes cannot become MCP tools through stale metadata.
ACP applies its own stage/version checks before compiling with its async engine.
Classification does not validate imports, promise compatibility, or authorize
execution; those remain the selected runtime's responsibilities.

Lazy restoration reads the current receipt and replaces a stale runtime revision
from one coherent snapshot. Catalog reads validate current captured bytes even
when the store cursor has not advanced, so caches cannot hide a missing semantic
name, changed runtime kind, or damaged artifact.

## Component-scoped tools

`ToolKey` pairs the semantic `ComponentId` with the exact `FunctionIdentifier`
(package, interface and function spelling). `list_tool_descriptors` and
`list_tools_for_component` return these keys with the existing tool schemas;
`describe_scoped_tool` selects an exact export, and `invoke_scoped_tool` calls it.
Listing uses receipt-bound metadata when available and validated cold compilation otherwise.
Install-only and ACP receipts cannot enter these APIs, including through caches.
These compatibility APIs drop revision references from the atomic catalog;
use `catalog()` when a consumer must retain permission-relevant identity.

Exact keys distinguish exports with the same normalized tool name, both across
components and within a component. The legacy `execute_component_call` resolves
a unique normalized name **only within the requested component**. The MCP
consumer uses `invoke_unique_tool`, which retains global collision refusal and
rechecks it at final admission. Neither path silently picks the first match or
renames colliding tools.

Invocation clones the component instance, exact export and descriptor together.
`ScopedToolOutput` carries that descriptor alongside the raw result, so an
adapter never formats a completed call using a replacement component's schema.
Every call still creates a fresh Wasmtime store with the selected instance's
policy and bound component secrets; no registry or store lock spans execution.

These keys and descriptors are **unversioned**, not permission tokens. Reusing
a key after replacement can invoke new code. Use the revision-bound interface
below across a permission prompt or an admission queue.

## Revision-bound catalog and admission

```rust
use serde_json::Value;
use wassette::{LifecycleManager, ToolInvocationError, ToolOutput, ToolRef};

async fn invoke_approved(
    manager: &LifecycleManager,
    reference: &ToolRef,
    arguments: &Value,
) -> Result<ToolOutput, ToolInvocationError> {
    // The caller retains the reference originally shown for approval.
    manager.prepare_invocation(reference, arguments).await?.run().await
}
```

`catalog()` returns one `CatalogSnapshot` of `ToolDescriptor` values. Each contains
the complete canonical schema and an opaque `ToolRef`: semantic component ID,
exact export, opaque `EntryRevision`, and a descriptor-contract fingerprint.
Remove/reinstall and store recreation invalidate old references even when bytes
and schemas are identical. A reference is not itself authorization. Equal names
do not bypass source continuity or the receipt's private storage/secret binding.

`prepare_invocation` captures immutable code, schema, effective policy, private
configuration/secrets and fresh call resources without executing guest code.
The owned `Send + 'static` `PreparedInvocation` can move into the caller's job.
`run` starts with a short cursor/revision admission check; name-based convenience
calls also verify uniqueness in the complete eligible catalog. Stale references
fail before guest side effects rather than selecting replacement code.

No load, store or registry lock spans execution. Once admitted, a call completes
using its original code, policy, secrets and output schema. Later replacement,
removal or policy narrowing rejects new stale admissions but **does not revoke
an already-admitted call**. Secret values are not included in reference hashes;
secret edits do not independently invalidate approval references. Each preparation
captures the current bound secrets, and read/parse failure is not treated as absence.

Host failures are typed as `ToolInvocationError`: `NotFound`, `Stale`,
`InvalidArguments`, `PolicyDenied`, `ExecutionFailed` and `Unavailable`, with
contextual sources. Initial alias lookup retains `ToolLookupError::NotFound` and
`Ambiguous`; an issued reference whose entry disappears is stale. A known host
policy denial is distinct from a guest trap or a returned WIT `err`.

### Refresh and notification ownership

`refresh_from_store()` checks the durable cursor; first use always hydrates.
`resnapshot_from_store()` forces a full observation after lost hints or an epoch
change. Concurrent callers share the manager's refresh gate. Compilation,
metadata rebuilding and policy preparation happen outside filesystem locks.
The complete candidate is published only after a final checked cursor comparison.
Old preparation cannot resurrect removed entries or overwrite current caches.
Cold name lookup rebuilds hashless/missing metadata synchronously; it does not
depend on a background loader or return an empty-success fallback.

Refresh failure does not acknowledge the attempted cursor. It reports errors
and unavailable entries; partial hydration cannot make an ambiguous name appear
unique. `CatalogRefreshError::report` exposes availability diagnostics without
captured policy/secret contents. Exact unaffected references can still be
prepared and admitted against their own current receipt.

`CatalogGeneration` identifies a runtime instance and its in-memory publication
sequence, not a store revision. Clones share it; independent managers do not.
Only changes to references, descriptors, membership or availability advance it.
Cache warming alone does not. Retired and InstallOnly receipts never contribute
callable tools; `ExposeTools` eligibility still does not grant permission.

`wait_changed(&generation)` observes **in-memory publications only**. A foreign
token returns immediately so the caller can resnapshot; lagging consumers read
the latest snapshot rather than replaying a durable event log. Reads and own
commits (including no-ops) reconcile the catalog. For idle external commits, a
host can run `run_refresh_driver(interval, shutdown)` in a task it owns and reaps.
The interval must be nonzero; there is no default polling task or new CLI flag.
Driver errors are returned to the owner, not swallowed. Catalog publication and
admission retry contention at most eight times before returning an error.

The driver only polls the installed store. It cannot discover a new source file
that has not been installed; local source scanning/watching and reconciliation
remain separate. A source watcher and store polling may coexist. Neither may
disable post-reconcile freshness. MCP deduplicates list-change notifications by
published generation, after publication and before a direct mutation response.

### Execution supervision

Core does not spawn invocation jobs. An ACP broker or other supervisor owns the
actual job, its handle and its concurrency permit; a guest waits on a separate
response receiver. Dropping the receiver must not release the permit while the
job continues. The permit is released on actual completion.

Tokio cancellation/timeouts do not interrupt CPU-bound Wasm that does not yield.
This interface adds no fuel, epoch interruption, hard deadline or rollback of
completed side effects, and does not establish a global MCP concurrency limit.

## Tool result presentation

`wassette::tool_result::present_tool_output` converts a raw tool result into
display text and optional structured content without depending on MCP types.
It preserves plain text, unwraps a sole `result` property for display, and aligns
structured content with the canonical output schema. A missing or null schema
omits structured content.

The MCP adapter constructs its protocol response from this presentation. A
guest-returned WIT `err` remains a returned value, not a host execution failure;
traps and host errors continue through the adapter's error path.
