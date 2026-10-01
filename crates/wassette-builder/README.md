# wassette-builder

An isolated, Rust-first component compiler. The library links Hyperlight and
invokes a fresh VM in-process for each build. The operator supplies the
immutable compiler initrd; this crate does not acquire or publish images.
Linux requires KVM/MSHV access;
other platforms fail closed. There is no host rustc, Cargo, linker, or interpreter
fallback.

```rust,ignore
let builder = Builder::new(config, BuildLimits::default())?;
let artifact = builder.build(BuildRequest {
    component_name: "example:add".into(),
    source: r#"
        struct Component;
        impl bindings::Guest for Component {
            fn add(a: i32, b: i32) -> i32 { a.wrapping_add(b) }
        }
        bindings::export!(Component with_types_in bindings);
    "#.into(),
    wit: "package example:add; world tool {
        export add: func(a: s32, b: s32) -> s32;
    }".into(),
    world: "tool".into(),
    kind: ComponentKind::Tool,
}, cancel).await?;
```

The operator chooses absolute initrd/private-staging paths. Each build captures
and hashes the initrd at execution time, so configuration setup does not scan
the potentially large image. The resulting digest identifies the captured
bytes in build evidence; it does not establish publisher identity or
trustworthy provenance. Each job boots from that exact backing, never by
reopening the original mutable path after hashing it. On macOS this is an unlinked APFS copy-on-write
snapshot accessed through a read-only descriptor (no bulk-copy fallback;
staging must support same-volume cloning). On Linux it is a sealed anonymous
memory file. The SDK opens the held descriptor, not a replaceable snapshot
pathname. Snapshots remain alive through VM teardown; private staging remains
owned until the VM is torn down.
Source replacement or in-place edits cannot change captured boot bytes.
The supported image profile is the existing
Debian/glibc `python-shell` image with Rust **1.98.1**, WASI p2 std, and
`wasm-component-ld` baked into `/opt/rust`, including `/opt/rust/bin/wasm-ld`.
The compiler architecture must match the host. The Python driver is fixed crate
content, executes only inside the VM, and spawns absolute executable paths from
its main thread.

## Boundaries

Requests are strict serde structs, with no flags, commands, paths, profiles,
Cargo manifests or dependencies. Defaults cap source at 1 MiB, combined WIT
at 256 KiB, final named Wasm at 32 MiB, diagnostics at 256 KiB, the job at
120 seconds, and guest scratch at 2048 MiB. Settings are host-only, finite,
validated, and cannot raise these ceilings, except that guest scratch may be
raised to 8192 MiB for profiles with pinned crates. A builder admits one job by
default (host-configurable up to four); applications should share the builder
to share its admission limit. Admission uses `try_acquire_owned`: saturation
returns a typed `Busy` error immediately, without a queue, timer, VM or
staging allocation. A pre-cancelled call remains `Cancelled`. The builder does
not retain requests waiting for a slot; any bounded retry/admission policy
belongs explicitly to the caller.

Each job owns a fresh VM. Host supervision interrupts and tears down the VM on
cancellation or timeout. Dropping the async future cancels its dedicated
supervisor; staging and admission permits remain owned until VM teardown, even
if the async runtime shuts down. No VM is detached.

Only the fresh staging directory's `input/` subdirectory is mounted,
**read-only**. The boot snapshot is outside that mount. Source, generated
bindings, fixed driver parameters and the inline binding runtime are the only
host input files visible to the guest, together with any operator-pinned crate
archives copied from `BuilderConfig::rust_crates` and re-hashed while staging. There are no writable host mounts,
live stores, project mounts, secrets, network policies or application tool
imports. Compilation, linking,
intermediate files and diagnostics reside in bounded guest scratch. A single
write-only, cumulative-size-checked extraction callback returns bytes; it
cannot supply evidence or grant validation. The compiler driver's stdout and
stderr are drained under the configured diagnostics budget. Hyperlight's
separate guest-console capture is discarded and size-checked after execution.

The verified image uses `vfork`: sufficiently large compiler diagnostics can
fill a finite guest pipe before the Python reader resumes. Such jobs fail
explicitly with the SDK's `guest is deadlocked` error instead of returning
partial output. The driver additionally caps bytes while draining both
diagnostic pipes; there is no unbounded `communicate()` capture.

## Pinned crates

`BuilderConfig::rust_crates` lists registry `.crate` archives in
dependency-first order. The driver extracts each archive with Python's `data`
tar filter, compiles it to an rlib with the fixed target and optimisation flags,
`--cap-lints=allow`, its pinned features and its dependency externs, then passes
every crate to the request source with `--extern`. Crate output and failures
are redacted from requesters and reported as `Unavailable`. The image has no
Cargo and no host standard library, so build scripts and procedural macros
cannot run. The guest also does not reclaim memory from exited compiler
processes: the ripgrep `grep-searcher`/`grep-regex` graph of thirteen crates
needs 8192 MiB of guest scratch. A guest crash, typically from scratch
exhaustion, is reported as `CompilationFailed` with a diagnostic naming
`limits.guest_scratch_mib`, rather than as an unrelated compiler diagnostic or
an unavailable builder. The crate list is part of `profile_sha256`.

Pure host WIT parsing, bindgen and captured-component validation run in-process
with finite input/graph/generated-source budgets and the job deadline.
Validation is limited to 128 levels and 100,000 type comparisons per
requested/runtime version graph. These transforms are not inside the VM; they
remain part of the host parser attack surface.
Parser 0.252 is used for binary validation and root metadata;
wit-bindgen 0.62 carries its own required parser version. No workspace parser
or Wasmtime dependency is upgraded.

## ACP and validation

Use `AcpLayer` with WIT exporting both `wassette:acp/agent` and
`wassette:acp/client`. Supply complete canonical WIT packages in
`BuilderConfig::wit_dependencies`, dependency-first, then include the layer
world from request WIT. The original canonical source bodies are not placed in
evidence. Native async traits, resources, streams and futures use wit-bindgen
0.62's inline runtime. Application crates are available only when pinned
in the profile; `async-spawn`, procedural macros and build scripts are not.

Every generated canonical export, callback, destructor and post-return
function is rooted at link time; exports are not hardcoded to the example.
The library adds an explicit standard **root `component-name`** equal to the
requested name. It preserves exactly one matching declaration and rejects
duplicates, conflicts, core modules and provider shape.
Nested names are ignored, not promoted to root identity.

Before emitting any success, the host validates those named captured bytes
and decodes their **actual binary type graph**, not embedded bindgen metadata.
Root exports and exported-interface members must exactly match the requested
world; function signatures, async kinds, nested value types and nominal
resource identities are checked. Unused imports may be omitted, but remaining
imports must be correctly typed subsets of the resolved requested world or
the fixed WASI P2 `wasi:cli/imports` runtime schema. There is no `wasi:*`
wildcard. Runtime versions and `@since` gates are bounded by the packaged
schema in `runtime/wasi-p2/`.

**The result is not installed or prepared for execution.** Parent code must
independently repeat L1 validation, exact name/kind checks, matching Wasmtime
runtime link/schema/policy preparation, and store validation on the final
named bytes. The parent computes
the final Wasm digest and owns issuance, ownership, installation and CAS.
Evidence contains host-observed input/profile/toolchain/image identity,
not source bodies, diagnostics, secrets, guest assertions, or a self-hash.

`BuildEvidence` uses the store's provenance names: `source_sha256`,
`wit_sha256`, `builder_initrd_sha256`, `builder_manifest_digest`, `profile`,
`compiler`, `bindgen`, `world`, and `target`. Its richer fields also use matching
names: `wit_dependencies_sha256`, `profile_sha256`,
`binding_runtime`, `vm_runtime`, and `host_platform`. `kind` and `component_name`
remain available for the parent's request/result binding check.
The local-initrd profile always emits `builder_manifest_digest: None`: the
profile digest is not an OCI manifest digest. It does not
select a registry, authentication mechanism, or image-distribution policy.
Prototype evidence field names remain deserialization aliases; serialization
uses only the current names, and duplicate aliases and unknown fields fail.
Generated lineage IDs, receipt schema selection, and the final Wasm hash remain
entirely parent-owned.

## Failure diagnostics

`Builder::new` and `Builder::build` keep `anyhow::Result` return types. Their
errors contain a `BuildError`; use `BuildError::from_error(&error)` to find it
through context/source wrappers. `kind()` returns `BuildErrorKind`, including
separate `Cancelled`, `Busy`, `DeadlineExceeded`, `InvalidRequest`, `InvalidWit`,
`CompilationFailed`, `InvalidOutput`, `Unavailable`, and `Internal` cases.
These are pre-install failures, not the parent's postcommit outcomes.

After the parent has authorized the requester, `diagnostic()` provides bounded
compiler/WIT/output-validation details and `diagnostic_truncated()` indicates
clipping. Rust uses short diagnostics with codes and source line/column, not
source excerpts. WIT locations are preserved, source gutters and paths are
removed, and host console output is never forwarded. Configuration and
internal failures expose no diagnostic body. Limits apply in UTF-8 bytes and
cannot exceed the host's diagnostic budget.

Log the typed kind or `BuildError` itself, **not the diagnostic accessor or a
generic error/backtrace**. `BuildError` has redacted `Display`/`Debug`, no raw
host cause chain, and no serialization implementation that could accidentally
dump its body. `BuildArtifact` debug output also omits diagnostic/Wasm bodies.
Diagnostics are untrusted compiler feedback for the authorized requester, not
instructions, provenance, or a successful fallback.

Run focused tests with `cargo test -p wassette-builder`; add
`--features hyperlight` for bindgen/extraction tests. The ignored `real_vm`
tests require `WASSETTE_BUILDER_INITRD`; run them serially with
`-- --ignored --test-threads=1`. They reuse the existing image without modifying
it. Hyperlight execution on macOS requires the consuming executable to carry
the `com.apple.security.hypervisor` entitlement.
`WASSETTE_BUILDER_ACP_WIT` optionally selects a specific canonical WIT snapshot
for the layer fixture. Set `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0`, and `CARGO_INCREMENTAL=0` for these builds.
