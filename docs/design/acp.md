# `wassette acp` — running agents as components

> **Status: experimental.** `wassette acp` is a prototype. Its CLI, its WIT
> world, and the layer chain model may change or be removed.

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
| **provider** | `wassette:acp/provider` | Terminal stage: exports `wassette:acp/agent`. The thing that actually talks to a model. |
| **layer** | `wassette:acp/layer` | Middleware: exports *and* imports both `wassette:acp/agent` and `wassette:acp/client`, so it sees traffic in both directions. |

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
  per-session `/data` directory the host preopens for a provider running
  alone or in a chain with `--allow-shared-grants` (host-owned, scoped by
  project and component).
* `permissions.network.allow` registers hosts in the outbound-HTTP allow-list;
  `wasi:http` requests to anything else are refused with `http-request-denied`.
  Raw `wasi:sockets` TCP, UDP and DNS remain disabled for host-scoped grants
  because their address checks cannot enforce the HTTP host allow-list.
* `permissions.storage.allow` becomes preopened directories.
* `permissions.environment.allow` forwards the named variables, and the
  component's secrets (`wassette secret set <id> KEY=value`) are injected
  the same way.
* `--allow-all` skips all of it: inherited network, inherited
  environment, no HTTP filtering. It is for demos and debugging.

Because a chain is one store and a store is one `WasiCtx`, the stages'
grants are **unioned** across the chain. A layer can access a provider's
storage, network and policy-injected secret environment variables, so
layered chains with policy grants, stored secrets or `--allow-all` require
`--allow-shared-grants`. Without the flag, a policy-free, secret-free
layered chain does not mount the provider's persistent `/data` directory;
the echo + uppercase demo therefore works without opt-in. With the flag,
`/data` is shared with every layer. Concurrent callbacks may be attributed
to the wrong stage, including `wasmcloud:secrets/store.get` lookups. The
flag acknowledges both risks; it does not fix stage routing or isolate
stages. Do not run untrusted layers.

`wasmcloud:secrets/store.get` normally resolves against the executing
stage's component id. This is **not an isolation guarantee** for layered
chains: policy-injected environment secrets are shared, and overlapping
callbacks can select the wrong stage's identity (see Known limitations).

## CLI

```text
wassette acp --provider <PATH|URI|COMPONENT_ID>
             [--layer    <PATH|URI|COMPONENT_ID>]...
             [--component-dir <DIR>] [--secrets-dir <DIR>]
             [--allow-all] [--allow-shared-grants]
             [--log-file <PATH>] [--log-level <LEVEL>] [--log-filter <DIRECTIVE>]
```

* Exactly one `--provider` is required. Multiple providers are rejected
  until session IDs and outbound callbacks can be mapped safely.
* `--layer` is repeatable and ordered editor-side → provider-side; the
  first `--layer` is the outermost stage. The same layer stack wraps
  every provider.
* Both accept whatever `wassette component load` accepts — a filesystem
  path, `oci://…`, `https://…` — or the id of a component already in the
  component directory.
* Logs go to **stderr**, never stdout: stdout is the protocol channel.
  `--log-file` mirrors them into a file for editors that hide stderr.

Point an ACP-speaking editor at it the same way you would point one at
`wassette run`.

## Demo

Build the two example components and run the chain:

```sh
just build-acp-examples

cargo run -p wassette-mcp-server -- acp \
  --provider components/acp-echo-provider/target/wasm32-wasip2/release/acp_echo_provider.wasm
```

`components/acp-echo-provider` is a provider that answers a prompt by
streaming the user's own text back, one word at a time, and then ends the
turn. It uses `wit-bindgen` and nothing else — no network, no secrets —
so the demo is reproducible offline and needs no policy (and therefore no
`--allow-all`).

Add the layer to see chaining:

```sh
cargo run -p wassette-mcp-server -- acp \
  --provider components/acp-echo-provider/target/wasm32-wasip2/release/acp_echo_provider.wasm \
  --layer    components/acp-uppercase-layer/target/wasm32-wasip2/release/acp_uppercase_layer.wasm
```

Prompt `/shout` and the layer answers it itself, toggling on uppercase
rewriting; every later echo comes back `LIKE THIS`. The provider is
unaware any of this happened.

The tests drive exactly this flow over real stdio:

```sh
just test-acp
```

## Known limitations

* Provider terminal requests go directly to the host; layers cannot intercept
  or deny them. The example layer's terminal exports are unfinished.
* `authenticate` uses a throwaway component instance. Authentication stored
  only in guest memory does not persist into a session.
* Guest-created tool-call resources are not implemented and can trap.
  The host's `/install` notifications do not provide guest tool-call
  lifecycle support or let ACP agents call Wassette Wasm tools.
* `initialize` omits the provider's session list/resume/close capabilities
  and authentication methods. The bridge does not support those lifecycle
  methods or stateful authentication; advertising them would mislead editors.
* Sessions stay registered until the host exits; there is no eviction or
  session-close path.
* Remote components and their policies are staged and validated before
  replacement. Publishing the two files requires separate filesystem
  operations; a concurrent reader outside the ACP host can briefly see a
  mixed pair. Do not modify the shared component store from another process
  while ACP is loading a component.
* Terminal output limits currently forward the first bytes (a prefix),
  not the latest bytes as described in WIT, and cannot report truncation
  through the current streaming API. The host also caps each command at
  1 MiB of forwarded output, regardless of the guest's requested limit;
  commands producing more output are drained but their excess is discarded.
  A command can pause on output backpressure until its stream is consumed.

Concurrent callbacks in a layered chain still share one store-wide stage
stack. Overlapping Wasmtime subtasks can misroute stage-specific imports,
including secret lookups, and cancellation can leave stale entries. Layered
chains with policy grants or stored secrets require `--allow-shared-grants`;
the provider's persistent `/data` is only mounted in an opted-in chain.
This is an explicit risk acknowledgement, not a routing fix. A
drop-safe, subtask-scoped stage identity is required before layered chains
can safely handle concurrent callbacks; avoid untrusted layers. Per-stage
WASI isolation is also a follow-up. Multi-provider sessions require unique
editor IDs and consistent outbound request remapping before the
single-provider restriction can be removed.

## Building the real providers

The `ollama` and `copilot` providers build against the `p3` branch of
`bytecodealliance/wstd` (PR #129) — the `wasip3` feature is not on crates.io.
Those external providers in
`yoshuawuyts/playground-wasm-acp` still export `yosh:acp@7.0.0`; they must
rename their WIT package and regenerate their bindings as
`wassette:acp@7.0.0` before they can load in this host. The build script does
not perform that rename. Two further adjustments are needed to target this
workspace's Wasmtime 47 rather than upstream's 44:

* Bump wstd's `wasip3` pin from `0.5` to `0.7.1`. Wasmtime 44 ships
  `wasi:http@0.3.0-rc-2026-03-15`; wasmtime 47 ships final `wasi:http@0.3.0`.
  A guest built against the older pin fails to link, and the error names the
  mismatched import directly.
* Add an optional, renamed `wit-bindgen` 0.57 dependency to wstd's `wasip3`
  feature, enabling `async`, `async-spawn` and `inter-task-wakeup`. The
  `wasip3` 0.7.1 dependency uses the same 0.57 version, so Cargo unifies
  these features. Wstd's existing 0.54 dependency cannot enable them on
  0.57, leaving `async_support::spawn` private without this hunk.

Both steps are captured as a temporary patch in
`crates/wassette-acp/real-providers/wstd-p3-wasmtime47.patch` and applied by
`just build-acp-real-provider <path-to-playground-wasm-acp>`, which fetches
the tested `p3` commit `c3bac234b01774b95ff3510351ec6fc674fd81e2`,
checks it out, patches it, and builds the component. Keep it until wstd
publishes a `wasip3` release compatible with Wasmtime 47.

As of September 2026, the
[wstd WASIp3 port](https://github.com/bytecodealliance/wstd/issues/141)
on `main` has superseded the older `p3` branch used here. The bridge can be
removed once wstd releases compatible WASIp3 HTTP support
([HTTP tracking issue](https://github.com/bytecodealliance/wstd/issues/164)).

Before the package rename, `ollama_provider.wasm` was verified end to end
streaming a chat completion over real `wasi:http`, and refused without a
network grant. The external providers need the WIT rename described above
to load again. Because these are local patches over an unmerged branch, the
in-tree demo and end-to-end tests deliberately use the echo provider instead,
so they never depend on a model or on a moving upstream.

GHCR is not anonymously reachable from this sandbox, so prebuilt components
cannot be pulled either.

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
* The notification gate buffers updates for registered pending session IDs.
  During `session/new`, it also holds bounded early updates until the guest
  returns its ID, then discards updates for other IDs and flushes the
  matching ones after the response. Per-session and global limits still
  apply. The host also advertises `/install` after `session/new`, and
  `session/load` can buffer updates because its ID is known in advance.
  The flush runs on a 200ms timer, but an inbound request naming the
  session opens the gate immediately.
