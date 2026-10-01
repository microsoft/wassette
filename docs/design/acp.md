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

Each provider in an ACP session has its own Wasmtime `Store` holding
**every** stage of that provider's chain. That is what lets a `session` resource created by the provider be
handed to the layer that wraps it without tripping resource-type
identity, and it is why the stages share a single `WasiCtx`.

## Sandboxing

Each stage's capabilities come from its Wassette policy —
the receipt-bound `<storage-key>.policy.yaml`, captured together with the
artifact. Local acquisition can import an adjacent policy before installation;
selection does not reopen mutable policy paths. Policy preparation uses
`wassette::create_wasi_state_template_from_policy`, the same function the
MCP server uses.

Persistent `/data` keeps its project/private-key path, but a durable ownership
record outside the guest preopen binds it to the receipt's semantic name and
source. Separate stores cannot reuse another component's data by choosing the
same private key. Conflicting ownership or unowned nonempty legacy data fails
closed; installation does not migrate or adopt it.

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

Components read secrets through `wasmcloud:secrets@2.1.0`. `store.get(key)`
looks up a key at runtime. A component that knows its secrets up front can
instead import one labeled `secret` per key, and the label is the key:

```wit
import api-key: wasmcloud:secrets/secret@2.1.0;
```

Wassette checks every labeled secret before instantiating a chain, so a
missing one fails with a `wassette secret set <component> <label>=<value>`
hint instead of at first use. Both kinds of lookup normally resolve against
the executing stage's component id. This is **not an isolation guarantee** for layered
chains: policy-injected environment secrets are shared, and overlapping
callbacks can select the wrong stage's identity (see Known limitations).

## CLI

```text
wassette acp --provider <PATH|URI|COMPONENT_ID>...
             [--layer    <PATH|URI|COMPONENT_ID>]...
             [--component-dir <DIR>] [--secrets-dir <DIR>]
             [--allow-all] [--allow-shared-grants]
             [--log-file <PATH>] [--log-level <LEVEL>] [--log-filter <DIRECTIVE>]
```

* At least one `--provider` is required. Repeat it to load distinct providers;
  selecting the same semantic component id twice is an error.
* `--layer` is repeatable and ordered editor-side → provider-side; the
  first `--layer` is the outermost stage. The same layer stack wraps
  every provider.
* Both accept whatever `wassette component load` accepts — a filesystem
  path, `oci://…`, `https://…` — or the id of a component already in the
  component directory.
* Logs go to **stderr**, never stdout: stdout is the protocol channel.
  `--log-file` mirrors them into a timestamped file for editors that hide stderr.
* `/install` privately captures local, OCI and HTTPS inputs, validates them with
  the ACP engine, and commits them through the shared transactional store.
  Installation does not automatically select or activate a provider.
* `/version` is host-owned and reports the binary's version, full commit SHA
  (with a dirty marker), and UTC build time. It shadows a provider's
  `/version` and is not forwarded to the chain.
* `--tool <COMPONENT_ID>` explicitly exposes an installed ordinary component
  to the ACP provider. The guest imports
  `wassette:component-tools/tools@0.1.0` to list revision-bound descriptors,
  wait for catalog changes and invoke exact exports. Calls use the same
  captured artifact, policy, schema and secrets as MCP. They ask the editor for
  permission and emit `tool_call` / `tool_call_update` notifications. Tools are
  off by default; layers and the active provider/layer components are excluded.
* `--local-components startup|watch` enables the shared local-source reconciler
  for ACP, with `--local-component-dir` overriding its drop directory. ACP
  injects its engine-backed validator, which compiles ACP-shaped artifacts and
  checks their export world, version, role and effective policy before commit.
  This evidence does not claim imported-type or host-link validation; explicit
  provider/layer selection still performs linking and instantiation checks.
  Discovery is install-only and never activates a provider or exposes a tool
  without the corresponding selector.
* `RUST_LOG=debug` or `RUST_LOG=trace` logs full JSON-RPC payloads, including prompt text and any secrets a guest emits; enable it only when appropriate.

Point an ACP-speaking editor at it the same way you would point one at
`wassette run`.

For a source checkout, `just install` installs the executable through Cargo and
reconciles all finalized first-party components into the shared default store.
An editor can then keep an absolute executable path and semantic selectors
stable across rebuilds:

```text
command: /home/me/.cargo/bin/wassette
args: acp --provider acp-echo-provider --layer acp-uppercase-layer
```

Use repeated `--provider` arguments for multiple providers and
`--tool microsoft:filesystem-rs` for an ordinary component. The exact Cargo
install root may differ when `CARGO_INSTALL_ROOT`, Cargo `install.root`, or
`CARGO_HOME` is configured. Installation does not select providers, expose
ordinary tools to ACP without `--tool`, grant permissions, set secrets, or add
`--allow-all` / `--allow-shared-grants`.

### Selecting between providers

```sh
wassette acp --provider acp-ollama-provider --provider acp-copilot-provider
```

Each new editor session creates a separate chain for every selected provider.
Providers without model choices are omitted in multi-provider mode. The first
remaining provider starts active, as shown by the Model selector's current value;
if none remain, session creation fails with an explicit error. A single provider
without models, such as the echo provider, still works normally.
The selector groups model choices by provider's semantic component id. Selecting
a model switches that session to its owning provider and returns that provider's
other configuration options. Send the advertised value unchanged:

```json
{"jsonrpc":"2.0","id":3,"method":"session/set_config_option","params":{"sessionId":"<returned session id>","configId":"model","value":"<advertised model value>"}}
```

Prompts, legacy mode changes and other configuration changes go only to the
active provider. Switching back retains its independent in-memory conversation;
history is neither copied between providers nor broadcast. A busy session rejects
overlapping prompts and configuration changes: finish or cancel the current turn
before switching. Other editor sessions remain independent.

Multi-provider session ids are host-owned, even if providers return identical
local ids. Notifications, filesystem callbacks and permission requests use the
editor id from the start of session creation. Provider tool-call ids are
namespaced, and replies return to their original caller, not the currently
selected provider. Failed creation discards the partial group and held updates.
Inactive providers' command advertisements are retained and replayed when selected.

Each provider chain retains its own effective policy, secrets binding, persistent
`/data` ownership, tool catalog view and remembered tool approvals. A permission
granted to one provider does not authorize another provider or editor session.
The same layer stack is instantiated independently for each provider; the existing
`--allow-shared-grants` requirement still applies *within* each chain. Generation
approvals still go directly to that chain's bound editor session; loading several
providers does not change this limitation or grant generation authority.

Initialization advertises Wassette as the multi-provider host and intersects
connection-wide MCP transport capabilities. Composite session restoration and
authentication are not implemented: multi-provider mode does not advertise
`loadSession` or authentication methods, and rejects direct requests for them.
Single-provider metadata, native model values and supported `session/load` remain
unchanged. Providers stay pinned to their startup receipts; installing or
discovering a replacement does not switch an active chain.

## Generating components from ACP

Providers and layers may import `wassette:component-generation/builder@0.1.0`
to build, validate and install a component from Rust source and WIT. The
Copilot provider exposes this to its model as a `build_component` tool. It is
off unless the operator enables it:

1. Install a feature-enabled CLI and the signed builder helper:

   ```sh
   just install-generation          # or: just install-generation release
   ```

   This builds `wassette` with `component-generation` and copies the
   `wassette-builder` helper (signed on macOS) next to the installed executable.
   It does not acquire a builder image; plain `just install` stays featureless.

2. Create an operator profile from the helper and a trusted local initrd. The
   script pins both SHA-256 digests, permits build and install only, creates a
   private staging directory, and refuses to overwrite an existing profile:

   ```sh
   python3 scripts/generation-profile.py \
       --helper ~/.cargo/bin/wassette-builder \
       --initrd /path/to/rust-initrd.cpio \
       --output ~/.config/wassette/generation.json
   ```

   Add `--allow-expose` or `--allow-rebuild` only when those operations are
   intended. See [`generation_config`](../reference/configuration-files.md#generation_config).

3. Add the profile to the agent's arguments, for example in Zed:

   ```json
   "args": ["acp", "--allow-all", "--provider", "acp-copilot-provider",
            "--generation-config", "/home/me/.config/wassette/generation.json"]
   ```

The host tells Copilot sessions that generation is available only when the
profile permits both build and install; otherwise the tool is not advertised.
The host stays authoritative: it checks the profile on every request, asks the
editor to approve the build, install and any exposure phase, and returns
`disabled` when no profile is configured. A generated tool component is
installed but is not added to the running conversation; start a **new** ACP
session with `--tool <component-id>` (or load it in the MCP server). A generated
layer requires a new session with `--layer <component-id>`. The Copilot provider
does not yet call ordinary `--tool` components itself.

In layered chains, generation approval requests go directly to the bound editor
session and bypass upstream layers, and generation requires
`--allow-shared-grants`.

## Demo

Build the example components and run the echo provider:

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

With an ordinary component selected by `--tool`, prompt
`/tool <name> <arguments-json>` to invoke it through the host permission flow.
For example, `/tool file-exists {"path":"/some/permitted/path"}` invokes the
selected filesystem tool under that tool's own policy. The echo fixture also
supports `/remember-tool <name>`, `/call-saved <arguments-json>` and
`/wait-tools` for exercising revision-bound handles and catalog updates.

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

Core and ACP share binary kind inspection. Root `wassette:acp/agent` instance
exports identify providers; `agent` plus `client` identifies layers. Client
imports alone do not make a layer. These shapes are excluded from ordinary MCP
loading and cached tool metadata before choosing an engine. ACP still checks
the protocol version and expected stage, and linking checks runtime compatibility.
`just build-acp-examples` embeds each producer's explicitly declared Cargo package
name (`acp-echo-provider`, `acp-uppercase-layer`, `acp-ollama-provider`, or
`acp-copilot-provider`) at the root. The shared `wassette:acp` interface package
does not identify a particular producer.
Selectors use the exact embedded root semantic name; receipts separately retain
private artifact, policy, secret and persistent-data bindings. Unnamed producers
must add a root component name before admission. Unreceipted artifacts are
protected legacy inventory, not filename aliases.

* Provider terminal requests go directly to the host; layers cannot intercept
  or deny them. The example layer's terminal exports are unfinished.
* `authenticate` uses a throwaway component instance. Authentication stored
  only in guest memory does not persist into a session.
* Guest-created `wassette:acp/tools.tool-call` resources are not implemented
  and can trap. Ordinary Wassette tool calls use the separate
  `wassette:component-tools/tools` import and host-owned lifecycle updates.
  Cancellation is soft: a non-yielding tool can retain its bounded execution
  permit until it actually exits.
* `initialize` omits the provider's session list/resume/close capabilities
  and authentication methods. The bridge does not support those lifecycle
  methods or stateful authentication; advertising them would mislead editors.
* Sessions stay registered until the host exits; there is no eviction or
  session-close path.
* Cooperating CLI, MCP and ACP readers use receipt-bound snapshots and the shared
  journal. Old binaries and direct filesystem edits do not participate. See the
  [transaction and platform limits](architecture.md#transactional-installation-and-replacement).
  ACP compilation and export checks do not prove full host-link compatibility;
  selection can still fail to link without rolling back a committed installation.
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

## Model-backed providers

Two providers talk to real models:

* `components/acp-ollama-provider` forwards prompts to a local
  [Ollama](https://ollama.com) server (`OLLAMA_URL`, default
  `http://localhost:11434/api/chat`; `OLLAMA_MODEL`, default `llama3.2`).
* `components/acp-copilot-provider` forwards prompts to the GitHub Copilot
  chat API. It reads a GitHub token from its `github_token` secret, falling
  back to `COPILOT_GITHUB_TOKEN`, `GH_TOKEN` or `GITHUB_TOKEN`. See
  `components/acp-copilot-provider/README.md` for token types, tools and
  config options.

`just build-acp-examples` builds both alongside the echo provider:

```sh
just build-acp-examples
GH_TOKEN="$(gh auth token)" cargo run -p wassette-mcp-server -- acp --allow-all \
  --provider components/acp-copilot-provider/target/wasm32-wasip2/release/acp_copilot_provider.wasm
```

To keep the token out of the environment, load the component into the
component directory and store it as a secret instead:
`wassette secret set acp_copilot_provider "github_token=$(gh auth token)"`,
then run `wassette acp --allow-all --provider acp_copilot_provider`.

`--allow-all` grants network and environment access; a policy granting the
model's host is the least-privilege alternative. The end-to-end tests in
`crates/wassette-acp/tests/acp_model_providers.rs` drive both providers
against a local mock of each API, so they need no network or credentials.

Both use wstd's WASIp3 HTTP client, which no released wstd ships yet
([wstd#164](https://github.com/bytecodealliance/wstd/issues/164)). The
`p3` branch ([wstd#129](https://github.com/bytecodealliance/wstd/pull/129))
is vendored under `vendor/wstd`, patched to `wasip3` 0.9 (final
`wasi:http@0.3.0`, as Wasmtime 47 requires) and `wit-bindgen` 0.62; see
`vendor/wstd/README.vendor.md`. Replace it with a crates.io release once one
is available.

All ACP components check in `wit-bindgen` output as `src/bindings.rs`;
`just acp-bindgen` regenerates them from `crates/wassette-acp/wit/acp`.

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
  apply. The host also advertises `/install` and `/version` after `session/new`, and
  `session/load` can buffer updates because its ID is known in advance.
  The flush runs on a 200ms timer, but an inbound request naming the
  session opens the gate immediately.
