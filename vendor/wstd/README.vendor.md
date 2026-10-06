# Vendored `wstd` (WASIp3 branch)

`vendor/wstd` and `vendor/wstd-macro` are a vendored copy of
[`bytecodealliance/wstd`](https://github.com/bytecodealliance/wstd), branch
`p3` ([PR #129](https://github.com/bytecodealliance/wstd/pull/129)), commit
`c3bac234b01774b95ff3510351ec6fc674fd81e2`, licensed Apache-2.0 WITH
LLVM-exception (see `LICENSE-Apache-2.0_WITH_LLVM-exception`).

The ACP providers in `components/acp-ollama-provider` and
`components/acp-copilot-provider` need wstd's WASIp3 HTTP client, which no
released wstd has yet
([wstd#164](https://github.com/bytecodealliance/wstd/issues/164)).

## Local patches

- Standalone manifests replace the upstream workspace inheritance; the
  `axum`, test, and example crates are not vendored, and the crate-level doc
  examples that `include_str!` them were replaced with a link.
- `wasip3` is bumped from `0.5` (`wasi:http@0.3.0-rc-2026-03-15`) to `0.9`
  (final `wasi:http@0.3.0`, which Wasmtime 47 requires).
- `wit-bindgen` is bumped from `0.54` to `0.62`, matching `wasip3 0.9` so
  Cargo unifies one async runtime, and the calls to the renamed
  `async_support::spawn` use `async_support::spawn_local`.

Delete this directory and depend on a crates.io `wstd` once it ships a
compatible WASIp3 HTTP client.
