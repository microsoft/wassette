# `wassette acp` — ACP support in Wassette

Status: **experimental prototype branch** (`acp`). Goal is a working demo, not
an upstreamable design.

## What this is

Bring the Agent Client Protocol host from
[`yoshuawuyts/playground-wasm-acp`](https://github.com/yoshuawuyts/playground-wasm-acp)
into Wassette as a `wassette acp` subcommand.

Today Wassette speaks **MCP**: it is a *tool server*, and the agent lives in the
editor. ACP inverts that: the editor is the client, and the **agent itself** is
the server. `wassette acp` makes Wassette an ACP agent whose brain is a
WebAssembly component — so the agent, not just its tools, runs inside the
Wasmtime sandbox under a Wassette policy.

```
                     MCP (today)                     ACP (this branch)
  editor ──tools──▶ wassette ──▶ component     editor ──agent──▶ wassette ──▶ component
  (agent lives in the editor)                  (agent lives in the component)
```

## Upstream terminology

* **provider** — a terminal ACP agent component. Exports `yosh:acp/agent`.
  This is the thing that actually talks to a model.
* **layer** — bidirectional middleware. Exports *and* imports both
  `yosh:acp/agent` and `yosh:acp/client`, so it can rewrite prompts on the way
  down and session updates on the way up. Layers compose into a chain.
* One ACP session = one `Store<HostState>` holding every stage of the chain.

## Baseline: what was verified before writing any code

| Question | Answer |
| --- | --- |
| Does the upstream host compile against Wasmtime **47** (Wassette's version, vs upstream's 44)? | **Yes**, after 7 mechanical fixes (below). |
| Do the guest components build with the stock 1.97.1 toolchain? | **Yes** — `uppercase_layer.wasm` builds in ~13 s, no `wit-bindgen` CLI, no linker override. |
| Can the `ollama` / `copilot` providers be built here? | **No** — see [Known blockers](#known-blockers). |

### The Wasmtime 44 → 47 delta (already applied to the staged sources)

1. `Config::wasm_component_model_async_builtins` → `wasm_component_model_more_async_builtins`
2. `WasmFeatures::CM_ASYNC_BUILTINS` → `WasmFeatures::CM_MORE_ASYNC_BUILTINS`
3. The five generated `*WithStore` traits moved their `T` from the method to the
   trait: `impl client::HostWithStore for HasSelf<HostState>` becomes
   `impl<T: Send> client::HostWithStore<T> for HasSelf<HostState>`, and the
   methods drop their own `<T: Send>`. Sites: `client_impl.rs:247`,
   `wasm.rs:1183`, `wasm.rs:1272`, `wasm.rs:1282`, `wasm.rs:1462`.

Everything else — `bridge/`, `translate.rs`, `group.rs`, the whole async
component-model dance — compiles unchanged.

## Layout

```
crates/wassette-acp/          # new crate — the ACP host
  wit/acp/*.wit               # yosh:acp@7.0.0, vendored
  wit/acp/deps/wasmcloud-secrets/secrets.wit
  src/                        # vendored from playground host/src, wasmtime-47 fixed
examples/acp-echo-provider/   # new — standalone guest, our demo artifact
  src/bindings.rs             # vendored yosh:acp provider-world bindings
```

`crates/*` is a workspace glob, so `wassette-acp` joins the workspace
automatically. `examples/` are standalone (not workspace members), matching
`fetch-rs` / `filesystem-rs`.

### Vendored WIT note

`wit/acp/deps` upstream is a symlink into a `wasm(1)`-populated `vendor/` dir
that is `.gitignore`d, so the WIT tree does not resolve from a fresh clone.
`wasmcloud:secrets@0.1.0-draft` is only on GHCR, which is not anonymously
reachable from this sandbox, so it is **hand-authored** from the shape the
host's `secrets_impl.rs` implements (synchronous `store.get` + `reveal`, i.e.
the pre-async revision of `wasmcloud:secrets@1.0.0`). This is the one vendored
artifact not byte-verified against upstream; it only matters for third-party
prebuilt guests.

## Where Wassette replaces upstream plumbing

This is the part that makes it *Wassette's* ACP rather than a copy of the
playground.

| Upstream | Replaced with | Why |
| --- | --- | --- |
| `install.rs` → `wasm-package-manager`, own XDG cache | `wassette::loader` (local path, `oci://`, `https://`) + the Wassette component dir | One component store for MCP and ACP; `wassette component load` and `wassette acp --provider` resolve the same names. |
| `secrets.rs` / `keyring-core` + 3 platform keyring crates | `wassette::SecretsManager` | `wassette secret set <id> KEY=…` already exists and feeds `wasmcloud:secrets/store`. Also drops the `libdbus-sys` build dep, which needs `libdbus-1-dev` and fails to build in this sandbox. |
| Blanket-allow `WasiCtxBuilder` | `wassette::create_wasi_state_template_from_policy` | **The point of the whole exercise**: an ACP agent component only reaches the hosts, paths, and env its Wassette policy grants. `--allow-all` keeps the permissive demo path. |

Engine: `wassette-acp` builds its **own** `Engine` + `Linker`. Wassette's shared
`RuntimeContext` is typed to `WassetteWasiState<WasiState>` and does not enable
the async component model, and ACP needs `CM_ASYNC` / `CM_ASYNC_BUILTINS` /
`CM_ASYNC_STACKFUL`. Not worth unifying on a prototype branch.

## CLI

```
wassette acp [--provider <PATH|oci://…|component-id>]...
             [--layer    <PATH|oci://…|component-id>]...
             [--component-dir <DIR>]
             [--allow-all]
             [--log-file <PATH>] [--log-level <LEVEL>] [--log-filter <DIRECTIVE>]
```

Speaks ACP JSON-RPC on stdio and logs to stderr — the same contract as
`wassette run`, so editor wiring is identical. At least one `--provider` is
required. Multiple providers merge into one cross-provider model selector;
layers wrap every provider, outermost first.

## Work items

1. **Crate skeleton.** `crates/wassette-acp/Cargo.toml`; `src/main.rs` → `src/lib.rs`
   exposing `pub struct AcpArgs` (clap `Args`) and `pub async fn run(args) -> Result<()>`.
   Add workspace deps: `agent-client-protocol = "1.2.0"`, `futures-concurrency`,
   `semver`, `tokio-util` (`compat`); enable `wasmtime/component-model-async`
   and `wasmtime-wasi-http/p3`.
2. **Resolution.** Replace `install.rs` with a `wassette::loader`-backed
   resolver. Component id = the Wassette component id (used to scope `/data`
   and secrets).
3. **Secrets.** Reimplement `secrets.rs` over `wassette::SecretsManager`,
   keeping `secrets_impl.rs`'s `Host`/`HostSecret` impls untouched. Delete
   `keyring-core` and the three platform keyring deps, and the `secret`
   subcommand (Wassette already has `wassette secret`).
4. **Policy sandbox.** Build each stage's `WasiCtx` from its policy via
   `create_wasi_state_template_from_policy`; `--allow-all` bypasses.
5. **Subcommand.** `Commands::Acp(wassette_acp::AcpArgs)` in
   `crates/wassette-mcp-server/src/{commands.rs,main.rs}`. stderr logging, as
   `Run` does — never stdout, it is the protocol channel.
6. **Demo guest.** `examples/acp-echo-provider` — a provider that answers a
   prompt by streaming back the user's text. `wit-bindgen` only, **no `wstd`**,
   no HTTP. This is what makes the demo self-contained.
7. **Tests.** `crates/wassette-acp/tests/` — spawn the binary, drive
   `initialize` → `session/new` → `session/prompt` → `session/cancel` over
   stdio, assert the responses. Port the upstream harness minus `wiremock`.
8. **Just + docs.** `just build-acp-examples`, `just test-acp`;
   `docs/design/acp.md` + a `docs/SUMMARY.md` entry.

## Demo

```sh
just build-acp-examples
cargo run -p wassette-mcp-server -- acp \
  --provider examples/acp-echo-provider/target/wasm32-wasip2/release/acp_echo_provider.wasm \
  --allow-all < fixture.jsonl
```

…and the same with `--layer uppercase_layer.wasm` to show chaining.

## Known blockers

* **`ollama-provider` and `copilot-provider` cannot be built here.** Both
  depend on `wstd` with a `wasip3` feature. That feature exists in neither
  crates.io (`0.6.1`–`0.6.8` expose only `json`) nor `yoshuawuyts/wstd@main`;
  upstream's `Cargo.toml` patches `wstd` to `/Users/yosh/Code/wstd`, an
  unpublished working copy. The host will still run a **prebuilt**
  `ollama_provider.wasm` if one is supplied — only building from source is
  blocked.
* **GHCR is not anonymously reachable** from this sandbox (`403` on the token
  endpoint) and `gh` is unauthenticated, so prebuilt provider components could
  not be pulled either.

Net effect: the demo runs on the in-tree echo provider. Real-model providers are
a drop-in once a `.wasm` is available.

## Conventions to respect

* Microsoft copyright header on every new `.rs` (`./scripts/copyright.sh`) —
  vendored upstream files keep their Apache-2.0 provenance; note it in the
  crate README.
* `cargo +nightly fmt`, `cargo clippy --workspace`.
* Prefer `just` recipes over ad-hoc `cargo`.
* Do not create or update `CHANGELOG.md`.
