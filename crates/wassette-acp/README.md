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

## Provenance

`src/` (except `install.rs` and `secrets.rs`) and `wit/acp/` are vendored from
[`yoshuawuyts/playground-wasm-acp`](https://github.com/yoshuawuyts/playground-wasm-acp)
(Apache-2.0), ported to Wasmtime 47. Those files keep their upstream headers;
files written for Wassette carry the usual Microsoft header.
`wit/acp/deps/wasmcloud-secrets/secrets.wit` is hand-authored — upstream's copy
lives behind a registry this tree cannot reach.
