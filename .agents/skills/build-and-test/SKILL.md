---
name: build-and-test
description: Build the Wassette workspace and run its test suite with the just recipes — debug and release builds, example and test components, and the combined unit plus documentation tests. Use when compiling Wassette, building example components, or running and debugging tests.
allowed-tools: Bash, Read, Write, Edit, Glob, Grep
---

# build-and-test skill

Build Wassette and run its tests through the `just` recipes defined in the
repository `Justfile`. Prefer these recipes over raw `cargo` invocations so the
WebAssembly component prerequisites are built automatically.

## Prerequisites

- **Rust**: the version pinned in `rust-toolchain.toml` (nightly is required
  only for formatting).
- **Cargo**: Rust's package manager.
- **Just**: the command runner used for every development task.
- **Node.js**: only needed for the MCP Inspector when debugging.
- **mdBook**: only needed to build documentation.

## Building

```bash
just build            # Debug build (default)
just build release    # Release build
just build-examples   # Build the example components under examples/
just clean            # Remove build artifacts
```

## Testing

```bash
just test                     # Build test components, then run all tests
just build-test-components    # Pre-build only the test components
just clean-test-components    # Remove test-component artifacts
```

`just test` runs both unit tests and documentation tests, and automatically
builds the WebAssembly components the tests depend on. When a test fails because
a component is stale, run `just clean-test-components` and retry.

## Notes

- Run the smallest recipe that covers your change; escalate to a full `just
  test` only when targeted checks pass but you need broader coverage.
- After changing dependencies in a `Cargo.toml`, rebuild before testing.
- Follow the `rust-code-style` skill for formatting and linting, and validate
  server behavior with the `mcp-inspector-testing` skill before committing.
