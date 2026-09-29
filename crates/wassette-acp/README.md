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
provenance, which this section records.
`wit/acp/deps/wasmcloud-secrets/secrets.wit` is hand-authored — upstream's copy
lives behind a registry this tree cannot reach.
