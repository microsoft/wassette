# `wassette-acp`

> **Experimental:** `wassette acp` may change or be removed.

The Agent Client Protocol (ACP) host for Wassette. `wassette acp` speaks ACP
JSON-RPC on stdio and routes it into a chain of WebAssembly components: exactly
one terminal **provider** (`--provider`) wrapped by zero or more bidirectional
**layers** (`--layer`). See [the ACP design](../../docs/design/acp.md).

```sh
wassette acp --provider ./my_agent.wasm --layer ./uppercase_layer.wasm
```

Components resolve exactly like `wassette component load` does — a filesystem
path, an `oci://` reference, an `https://` URL, or the id of a component already
in the Wassette component directory — and each stage's secrets come from
`wassette secret set <component-id> KEY=value`.

Logs go to **stderr only**; stdout is the protocol channel.
Multiple `--provider` flags are rejected until multi-provider session IDs
and outbound callbacks can be mapped safely.

## Sandboxing

Each stage is sandboxed from its Wassette policy
(`<component-id>.policy.yaml`, looked up in the component directory and then
beside the `.wasm`) via `wassette::create_wasi_state_template_from_policy` —
the same function the MCP server uses. A stage with no policy gets no network
and no filesystem beyond the per-session `/data` directory the host preopens
for a provider running alone or in an opted-in chain. `--allow-all` restores
the permissive upstream behaviour for demos.

Because one ACP session is one `Store`, and a store has one `WasiCtx`, the
grants of a chain's stages are unioned. Layered chains with policy grants,
stored secrets or `--allow-all` require `--allow-shared-grants`, which also
mounts the provider's persistent `/data` directory into the shared context.
Without the flag, policy-free, secret-free layers run without `/data`. Concurrent
callbacks can be attributed to the wrong stage, including secret lookups.
This flag acknowledges the risks; it does not fix routing or isolate stages.

## Provenance

`src/` (except `install.rs`, `secrets.rs`, `sandbox.rs` and `http_policy.rs`)
and `wit/acp/` are vendored from
[`yoshuawuyts/playground-wasm-acp`](https://github.com/yoshuawuyts/playground-wasm-acp)
(Apache-2.0), ported to Wasmtime 47. The repository's copyright check
(`./scripts/copyright.sh`, enforced in CI) stamps a Microsoft header onto every
`.rs` file including those; it does not displace their upstream Apache-2.0
provenance, which this section records. The same applies to
`components/acp-ollama-provider` and `components/acp-copilot-provider`, which
are ported from the playground's `ollama-provider` and `copilot-provider`
crates.
`wit/acp/deps/wasmcloud-secrets/secrets.wit` is `wasmcloud:secrets@2.1.0`,
copied from [`wit/secrets/wit/world.wit`](https://github.com/wasmCloud/wasmCloud/blob/e598479/wit/secrets/wit/world.wit)
in the wasmCloud repository.

## Ordinary Wassette tools

`wassette acp --tool <COMPONENT_ID>` explicitly exposes an installed ordinary
tool component to the provider. Providers import
`wassette:component-tools/tools@0.1.0` to list revision-bound tools and invoke
them with JSON arguments. The host asks the editor for permission, emits ACP
tool-call updates, and executes the pinned artifact and policy through
`LifecycleManager`. Tools are off by default; layers and active chain stages
cannot call them.
Layered chains with exposed ordinary tools require `--allow-shared-grants`,
because layers can intercept permission requests and share the provider's store.
Cancelling a turn stops waiting, not necessarily the tool: the host reports that
execution may still be finishing. A supervised worker retains its concurrency
permit until execution actually ends.

`--local-components startup|watch` opts into local-source discovery, defaulting
to `off`. Use `--local-component-dir` to override the drop directory. Discovery
can install ACP-shaped artifacts with ACP-engine compilation, export/version
checks and policy validation; it does not prove they can link or instantiate.
Provider/layer activation remains an explicit selection at startup, and ordinary
tools still require `--tool <COMPONENT_ID>`.

**Not implemented:** `/install` accepts ACP artifacts only, not ordinary tools
or registry package selectors. There are no `--tool-path`, `--tool-package` or
`--expose-tools local` adapters. Those interfaces remain proposals rather than
implicit exposure defaults. Committed store changes are refreshed on catalog
listing/call resolution; idle external changes alone do not wake the guest's
`wait-for-change`. Local watch publishes changes through the shared catalog.

## Explicit component generation

Build with the `component-generation` Cargo feature and pass
`--generation-config <PATH>` to read a trusted operator profile. The profile
selects a local, digest-pinned builder initrd and isolated helper, and independently
enables build, install, exposure, and rebuild authority. No profile means disabled;
guest JSON cannot choose configuration paths or grant itself authority.

Providers and layers may request `wassette:component-generation/builder@0.1.0`.
The host asks for one-call build approval before starting the VM, then install
approval bound to the actual output hash and expected revision. Exposure has a
third approval: `ExposeTools` changes shared-store ordinary-tool eligibility,
while adding broker handles affects only the requesting ACP session. Existing
per-tool invocation permissions still apply. Layered chains require the explicit
`--allow-shared-grants` acknowledgement for generation too.
An ordinary artifact with no callable exports can still be installed and made
eligible in the shared store, but returns no session tool handles.

Generated ACP layers are install-only, return no tool handles, and require later
explicit selection. Neither providers nor running chain replacements are generated.
Cancellation before commit prevents installation; cancellation after commit
acceptance cannot roll it back. Supervised build/install work retains its permit
until it actually finishes, including VM termination and reaping.
Disconnect closes the shared supervisor to new jobs, cancels precommit generation,
and awaits all admitted jobs before returning from the transport. Accepted commits
still finish. Ordinary Wasmtime tool cancellation remains cooperative: a
non-yielding tool can delay shutdown indefinitely; it is not hard-killed like the
isolated builder VM.

The neutral synchronous-ABI WIT is canonical at
[`wit/component-generation`](../../wit/component-generation/builder.wit);
the ACP dependency directory is a symbolic link, not a separately maintained copy.
ACP uses an asynchronous host implementation of the synchronous import and
drives its store event loop for session binding. Generation's host-managed
permission prompts and status updates go **directly to the bound editor, not
through upstream layers**, using the existing client transport helpers:
Wasmtime 47 cannot re-enter an active upstream layer during a synchronous guest
import. Layers cannot mediate or rewrite these approvals;
ordinary tool permission routing is unchanged. Ordinary WASIp2 hosts can
implement the same ABI without ACP engine features.
`generation_validator` reuses ACP compilation, export and policy checks
without starting a guest or claiming full host-link compatibility.
Typed v3 builder failures return only sanitized guest/compiler/WIT diagnostics
to the authorized requester, capped at 16 KiB of JSON-serialized text. Host
error chains are not rendered, and automatic status notifications omit diagnostics.

Real-VM stdio coverage is opt-in. Build the feature-enabled CLI and ACP fixtures,
then set `WASSETTE_ACP_GENERATION_CONFIG` to a trusted local profile and run:

```sh
cargo test -p wassette-acp --features component-generation \
  --test acp_stdio generation::real_ -- --ignored --test-threads=1
```

These tests isolate stores, secrets and builder staging; they cover phase
denials, install-only behavior, session-local exposure, layered callers,
disconnect cleanup and later explicit layer selection. Use an immutable signed
helper path: Cargo can relink a helper in `target/`, invalidating its signature
and configured digest.
