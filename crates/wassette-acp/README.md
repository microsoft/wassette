# `wassette-acp`

The Agent Client Protocol (ACP) host for Wassette. `wassette acp` speaks ACP
JSON-RPC on stdio and routes it into a chain of WebAssembly components: one or
more terminal **providers** (`--provider`) wrapped by zero or more bidirectional
**layers** (`--layer`). See [`ACP-PLAN.md`](../../ACP-PLAN.md) for the design.

```sh
wassette acp --provider ./my_agent.wasm --layer ./uppercase_layer.wasm
```

Components resolve exactly like `wassette component load` does — a filesystem
path, an `oci://` reference, an `https://` URL, or the id of a component already
in the Wassette component directory — and each stage's secrets come from
`wassette secret set <component-id> KEY=value`.

Logs go to **stderr only**; stdout is the protocol channel.

## Sandboxing

Each stage is sandboxed from its Wassette policy
(`<component-id>.policy.yaml`, looked up in the component directory and then
beside the `.wasm`) via `wassette::create_wasi_state_template_from_policy` —
the same function the MCP server uses. A stage with no policy gets no network
and no filesystem beyond the per-session `/data` directory the host preopens
for it. `--allow-all` restores the permissive upstream behaviour for demos.

Because one ACP session is one `Store`, and a store has one `WasiCtx`, the
grants of a chain's stages are unioned: a layer runs with its own policy plus
those of the stages it wraps.

## Provenance

`src/` (except `install.rs` and `secrets.rs`) and `wit/acp/` are vendored from
[`yoshuawuyts/playground-wasm-acp`](https://github.com/yoshuawuyts/playground-wasm-acp)
(Apache-2.0), ported to Wasmtime 47. The repository's copyright check
(`./scripts/copyright.sh`, enforced in CI) stamps a Microsoft header onto every
`.rs` file including those; it does not displace their upstream Apache-2.0
provenance, which this section records.
`wit/acp/deps/wasmcloud-secrets/secrets.wit` is hand-authored — upstream's copy
lives behind a registry this tree cannot reach.
