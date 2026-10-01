# Offline ACP routing provider

This test-only component reuses the echo provider's checked-in
`wassette:acp@7.0.0` bindings through `#[path]`. It never contacts a model or
opens a network connection. Explicit `/tool` and `/generate` commands delegate
to host imports; tests must supply offline implementations.

Build only this fixture from the repository root:

```sh
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 \
  just build-acp-routing-fixture
```

The output is
`crates/wassette-acp/tests/fixtures/routing-provider/target/wasm32-wasip2/release/acp_routing_provider.wasm`.
The recipe pins `CARGO_TARGET_DIR=target`, independently of the host build.
`test-acp` builds it automatically. Unit tests and lint checks are standalone:

```sh
cd crates/wassette-acp/tests/fixtures/routing-provider
CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 cargo test --locked
cargo clippy --locked --target wasm32-wasip2 -- -D warnings
```

## Per-provider configuration

Only environment variables permitted by the provider's policy are visible.
Flags are enabled by any nonempty value except `0` and `false`.
For distinct per-provider values, seed these keys in each component's stored
secrets with the integration harness's `common::seed_secrets`, and include them
in that provider policy's `environment.allow` entries. Production secret
injection projects the allowed values into the guest environment; the fixture
needs no separate injection mechanism. Process-wide environment values cannot
distinguish providers.

| Variable | Default | Effect |
| --- | --- | --- |
| `ROUTING_LABEL` | `fixture` | Prefix for output and agent title. |
| `ROUTING_NO_MODELS` | disabled | Omit both legacy models and the model config option; reject model selection. |
| `ROUTING_EMPTY_MODELS` | disabled | Advertise empty legacy model and `fixture-model` option lists; reject model selection. |
| `ROUTING_REJECT_ALTERNATE` | disabled | Still advertise `alternate`, but selecting it returns `invalid_params` without changing state. |
| `ROUTING_LOAD_SUPPORTED` | disabled | Advertise and enable `session/load`. |
| `ROUTING_FAIL_NEW` | disabled | Fail `session/new` before emitting startup events. |
| `ROUTING_MCP_HTTP` | disabled | Advertise MCP HTTP capability; never connect to supplied servers. |
| `ROUTING_CALLBACK_NEW` | disabled | Await editor `fs/read_text_file` during new/load startup. An absolute value is the requested path; other enabled values use `/routing/startup.txt`. |

Initialize is required before new/load. Every new session returns the literal
provider-local ID **`collision`**, including across instances. A new session
starts with model `shared` and style `plain`. Models are `shared`/`alternate`;
config options are `fixture-model` (category `Model`) and `style` (uncategorized,
values `plain`/`loud`). Legacy modes also select the style. Config updates return
the full current option set; invalid selections leave state unchanged.

`ROUTING_REJECT_ALTERNATE` rejects both legacy model selection and
`fixture-model` config updates with the exact error message
`alternate model selection rejected by fixture`.
`ROUTING_EMPTY_MODELS` keeps the `models` record and `fixture-model` option
present, but empties their choice lists; the internal/current model remains
`shared`. This intentionally exercises a provider with no selectable models.
The `style` option remains available for both empty and absent model lists.
If both flags are set, `ROUTING_NO_MODELS` takes precedence and omits the model
metadata entirely.

New/load emit `<label>:startup:new` or `<label>:startup:load` as an
`agent_message_chunk` **before returning**. With the startup callback enabled,
they then await the editor read and emit `<label>:startup:<new|load>:<content>`.
All notifications and callbacks use the local session ID `collision`.
Load accepts only that ID, returns the last session's current options, and uses
defaults when a fresh instance has no previous session. Empty synthetic replay
prompts emit nothing.

## Prompt commands

Arguments are split on literal spaces; `/write` content and `/tool` JSON keep
everything after the first argument. All successful turns end with `end_turn`
except cancelled host tool calls, which end with `cancelled`. Failures return
concise ACP errors rather than panicking.

| Prompt | Agent message |
| --- | --- |
| ordinary text | `<label>:<model>:<style>:<text>` |
| `/state` | `<label>:<model>:<style>` |
| `/history` | `<label>:history:<count>` |
| `/read <path>` | `<label>:read:<editor-content>` |
| `/write <path> <content>` | `<label>:write:ok` |
| `/permission` | `<label>:permission:<allow|reject|cancelled>` |
| `/secret <key>` | `<label>:secret:<value>` |
| `/data-write <value>` | `<label>:data-write:ok` |
| `/data-read` | `<label>:data-read:<value>` |
| `/env <key>` | `<label>:env:<value>` or `<label>:env:<unset>` |
| `/tool <name> <arguments-json>` | `<label>:tool:<result-text>` |
| `/generate <request-json>` | `<label>:generation:<disposition>:<report-json>:<tool-handles>` |

`/read` and `/write` call the editor's ACP filesystem methods, not WASI.
`/permission` uses tool-call ID **`collision-tool`** in every instance, title
`<label>:permission`, and option IDs `allow`/`reject` (`allow_once`/`reject_once`).
`/secret` uses `wasmcloud:secrets/store` followed by `reveal` (strings or UTF-8
bytes). Use only dummy secrets: this test fixture intentionally echoes them.
`/data-*` uses WASI at `/data/value`; the host must preopen the provider's data
directory. `/env` reads only the guest's environment. `/tool` uses
`component-tools.call-tool-by-name`; `/generate` passes request JSON unchanged
to the component-generation import.

`/history` counts ordinary nonempty text prompts in this session. Slash commands,
including `/history` itself, startup notifications, and empty replay prompts do
not increment it. A new session starts at zero; model/style changes preserve the
count, and loading the same retained session returns its existing count.

## Integration-test helpers

The build leaves the fixture's root unnamed. Build once, then have the test
copy this first-party artifact and add a root `ComponentNameSection` declaring
`test:alpha` or `test:beta` on each test-owned copy. This does not rename a
third-party artifact. Give each provider its own policy, stored label,
data directory, and dummy secrets. No separate source copies are needed.
Fixture-authored `provided-by` metadata remains `test:routing-provider`;
root names belong to the test copies.

Use an async-component-aware `wasm-tools` matching the current bindings;
`wasm-tools 1.240.0` cannot decode this artifact. Validation with
`wasmparser 0.259.0` and all features enabled succeeds. A test-local Rust
metadata helper using the workspace's parser/encoder avoids depending on a
system-installed CLI version.

The stdio harness should collect notifications and answer editor requests
while awaiting **any** response, including new/load, rather than waiting for
session creation before starting the callback pump. Startup reads make that
ordering observable; colliding session/tool IDs expose missing reverse mapping.
