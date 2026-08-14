# `wassette acp` — running agents as components

> **Status: experimental.** `wassette acp` is a prototype. Its CLI, its WIT
> world, and the layer chain model are all expected to change.

## What ACP is

The [Agent Client Protocol](https://agentclientprotocol.com) (ACP) is a
JSON-RPC protocol between a code editor and a coding agent. The editor is
the **client**; the agent is the **server**. The editor asks the agent to
start a session and to answer prompts; the agent streams back message
chunks, thoughts and tool calls, and asks the editor for permission,
file reads and file writes.

That is the mirror image of how Wassette normally works.

```text
                MCP (`wassette serve`)                ACP (`wassette acp`)

  editor ── tools ──▶ wassette ──▶ component    editor ── agent ──▶ wassette ──▶ component
  the agent lives in the editor;                the agent lives in the component;
  the component is a tool it calls              the editor drives it over ACP
```

In MCP mode Wassette is a tool server: the model doing the reasoning is
the editor's, and components are sandboxed capabilities it may invoke. In
ACP mode the **agent itself** is a WebAssembly component. The reasoning
loop, the prompt handling and the model calls all run inside Wasmtime
under a Wassette policy — so it is not only the tools that are sandboxed,
it is the agent.

Both modes share the same machinery: the same component store
(`wassette component load`), the same policy files, the same secrets
(`wassette secret set`), and the same stdio contract (JSON-RPC on stdout,
logs on stderr).

## The chain: providers and layers

An ACP session in Wassette is a **chain** of components, borrowing
upstream's terminology from
[`yoshuawuyts/playground-wasm-acp`](https://github.com/yoshuawuyts/playground-wasm-acp):

| Term | WIT world | Role |
| --- | --- | --- |
| **provider** | `yosh:acp/provider` | Terminal stage: exports `yosh:acp/agent`. The thing that actually talks to a model. |
| **layer** | `yosh:acp/layer` | Middleware: exports *and* imports both `yosh:acp/agent` and `yosh:acp/client`, so it sees traffic in both directions. |

Requests flow editor → outermost layer → … → provider; session updates
flow back the other way. A layer can rewrite a prompt on the way down,
rewrite or synthesise updates on the way up, or answer a request itself
without ever calling downstream — which is how the
`acp-uppercase-layer` example implements its `/shout` command.

```text
  editor ──▶ [ layer 1 ] ──▶ [ layer 2 ] ──▶ [ provider ]
         ◀──            ◀──             ◀──
```

One ACP session is one Wasmtime `Store` holding **every** stage of the
chain. That is what lets a `session` resource created by the provider be
handed to the layer that wraps it without tripping resource-type
identity, and it is why the stages share a single `WasiCtx`.

## Sandboxing

Each stage's capabilities come from its Wassette policy —
`<component-id>.policy.yaml`, looked up in the component directory
(where `wassette component load` and `wassette policy attach` put it) and
then next to the `.wasm` file — through
`wassette::create_wasi_state_template_from_policy`, the same function the
MCP server uses.

* No policy means **no network and no filesystem** beyond the
  per-session `/data` directory the host preopens (host-owned, scoped by
  project and component).
* `permissions.network.allow` grants sockets *and* registers the host in
  the outbound-HTTP allow-list; `wasi:http` requests to anything else are
  refused with `http-request-denied`.
* `permissions.storage.allow` becomes preopened directories.
* `permissions.environment.allow` forwards the named variables, and the
  component's secrets (`wassette secret set <id> KEY=value`) are injected
  the same way.
* `--allow-all` skips all of it: inherited network, inherited
  environment, no HTTP filtering. It is for demos and debugging.

Because a chain is one store and a store is one `WasiCtx`, the stages'
grants are **unioned** across the chain. A layer therefore runs with its
own policy plus the policies of the stages it wraps — worth knowing
before putting an untrusted layer in front of a network-granted provider.

Secrets are per component and structural rather than declared: a stage's
`wasmcloud:secrets/store.get` resolves against *its own* component id, so
no stage can read another's.

## CLI

```text
wassette acp [--provider <PATH|URI|COMPONENT_ID>]...
             [--layer    <PATH|URI|COMPONENT_ID>]...
             [--component-dir <DIR>] [--secrets-dir <DIR>]
             [--allow-all]
             [--log-file <PATH>] [--log-level <LEVEL>] [--log-filter <DIRECTIVE>]
```

* At least one `--provider` is required. Several may be given: every
  provider is instantiated per session and their models merge into one
  selector, so the user picks which model from which provider backs the
  session.
* `--layer` is repeatable and ordered editor-side → provider-side; the
  first `--layer` is the outermost stage. The same layer stack wraps
  every provider.
* Both accept whatever `wassette component load` accepts — a filesystem
  path, `oci://…`, `https://…` — or the id of a component already in the
  component directory.
* Logs go to **stderr**, never stdout: stdout is the protocol channel.
  `--log-file` mirrors them into a file for editors that hide stderr.

Point an ACP-speaking editor at it the same way you would point one at
`wassette serve --stdio`.

## Demo

Build the two example components and run the chain:

```sh
just build-acp-examples

cargo run -p wassette-mcp-server -- acp \
  --provider examples/acp-echo-provider/target/wasm32-wasip2/release/acp_echo_provider.wasm
```

`examples/acp-echo-provider` is a provider that answers a prompt by
streaming the user's own text back, one word at a time, and then ends the
turn. It uses `wit-bindgen` and nothing else — no network, no secrets —
so the demo is reproducible offline and needs no policy (and therefore no
`--allow-all`).

Add the layer to see chaining:

```sh
cargo run -p wassette-mcp-server -- acp \
  --provider examples/acp-echo-provider/target/wasm32-wasip2/release/acp_echo_provider.wasm \
  --layer    examples/acp-uppercase-layer/target/wasm32-wasip2/release/acp_uppercase_layer.wasm
```

Prompt `/shout` and the layer answers it itself, toggling on uppercase
rewriting; every later echo comes back `LIKE THIS`. The provider is
unaware any of this happened.

The tests drive exactly this flow over real stdio:

```sh
just test-acp
```

## Known blockers

* **The `ollama` and `copilot` providers cannot be built from source
  here.** Both depend on `wstd` with a `wasip3` feature that exists in
  neither the published crate nor `yoshuawuyts/wstd@main`; upstream's
  manifest patches `wstd` to an unpublished local working copy. The host
  will happily run a **prebuilt** `ollama_provider.wasm` — only building
  one from source is blocked. Real-model providers are otherwise a
  drop-in.
* Consequently the in-tree demo runs on the echo provider, and the
  end-to-end tests never touch a model.

## Implementation notes

* The crate is `crates/wassette-acp`; `src/` and `wit/acp/` are vendored
  from the upstream playground (Apache-2.0) and ported to Wasmtime 47.
  `install.rs`, `secrets.rs`, `sandbox.rs` and `http_policy.rs` are
  Wassette's own: they replace upstream's package manager, keyring and
  blanket-allow WASI context with the component store, `SecretsManager`
  and policy engine that already ship with Wassette.
* `wassette-acp` builds its **own** wasmtime `Engine` and `Linker`.
  Wassette's shared runtime is typed to `WassetteWasiState<WasiState>`
  and does not enable the async component model, which ACP requires
  (`CM_ASYNC`, `CM_MORE_ASYNC_BUILTINS`, `CM_ASYNC_STACKFUL`).
* Session updates emitted *during* `session/new` are held by a
  notification gate and flushed just after the response, because an
  editor cannot route an update for a session id it has not been told
  about yet.
