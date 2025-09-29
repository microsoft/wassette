# Component Schemas and Structured Output

New contributors quickly encounter two pieces of infrastructure when working on Wassette's
component runtime:

1. The `component2json` crate, which introspects WebAssembly Components and converts their
   WebAssembly Interface Types (WIT) into JSON-friendly schemas and values.
2. The structured output utilities in `wassette::schema`, which normalize those schemas and align
   runtime values so Model Context Protocol (MCP) clients always see a predictable `{ "result": ... }`
   envelope.

This document explains how the two layers interact, why the result wrapper exists, and what to keep
in mind when changing either side. It is intended for engineers extending the schema pipeline or
adding new tooling around component execution.

## High-Level Flow

```
WIT component  ──► component2json::component_exports_to_tools
                    │
                    ▼
             tool metadata with `outputSchema`
                    │
                    ▼
       wassette::schema::canonicalize_output_schema
                    │
                    ▼
        registry + MCP-facing structured responses
```

The flow repeats in three places:

- **During component load** we call `component_exports_to_tools` to populate the in-memory registry
  and cache metadata on disk.
- **When serving metadata to clients** we run `canonicalize_output_schema` to guarantee the result
  wrapper and tuple-normalization appear even if the cached schema predates the newer format.
- **When returning call responses** we run `ensure_structured_result` so the JSON payload we return
  in `structured_content` matches the schema that was advertised.

## component2json Responsibilities

`component2json` handles three jobs:

1. **Schema generation** – `component_exports_to_json_schema` and
   `component_exports_to_tools` walk the component exports and convert each parameter and result
   type to JSON Schema.
2. **Value conversion** – `json_to_vals` converts incoming JSON arguments into WIT `Val`s, while
   `vals_to_json` converts WIT results back to JSON.
3. **Result envelope** – all non-empty result sets are wrapped in `{ "result": ... }`. For
   multi-value returns, each position is named `val0`, `val1`, etc so the metadata remains stable
   even when the component author reorders tuple fields.

### Quick Reference

| Function                                           | Purpose                                                               |
|----------------------------------------------------|-----------------------------------------------------------------------|
| `component_exports_to_json_schema`                 | Returns `{ "tools": [ ... ] }` for every exported function.          |
| `component_exports_to_tools`                       | Lower-level API returning `ToolMetadata` structs.                     |
| `type_to_json_schema`                              | Core translator from WIT `Type` to JSON Schema.                       |
| `vals_to_json`                                     | Converts `[Val]` results into the canonical JSON wrapper.             |
| `json_to_vals`                                     | Converts JSON arguments into `[Val]` based on `(name, Type)` pairs.   |
| `create_placeholder_results`                       | Allocates correctly-typed buffers before invoking a component.        |

### Example: Single Return Value

Consider a component function defined in WIT as:

```wit
package example:math

interface calculator {
    use wasi:clocks/monotonic-clock

    /// Adds one to the given integer.
    add-one: func(x: s32) -> s32
}
```

Running `component_exports_to_tools` produces (simplified) metadata:

```json
{
  "name": "example_math_calculator_add_one",
  "inputSchema": {
    "type": "object",
    "properties": { "x": { "type": "number" } },
    "required": ["x"]
  },
  "outputSchema": {
    "type": "object",
    "properties": {
      "result": { "type": "number" }
    },
    "required": ["result"]
  }
}
```

When the function executes and returns `42`, `vals_to_json` emits:

```json
{ "result": 42 }
```

Even though only a single scalar is returned, the wrapper ensures clients always access the payload
through the `result` property.

### Example: Multiple Return Values

Suppose the component exposes:

```wit
interface time-range {
    /// Returns the (start, end) timestamps in nanoseconds.
    span: func() -> tuple<u64, u64>
}
```

The generated schema becomes:

```json
{
  "type": "object",
  "properties": {
    "result": {
      "type": "object",
      "properties": {
        "val0": { "type": "number" },
        "val1": { "type": "number" }
      },
      "required": ["val0", "val1"]
    }
  },
  "required": ["result"]
}
```

And the runtime result for `(123, 456)` is

```json
{ "result": { "val0": 123, "val1": 456 } }
```

The positional names (`val0`, `val1`, ...) keep the schema stable and make it easy for language-
agnostic MCP clients to reason about tuple-like output.

### Example: Result Types

`component2json` also understands `result<T, E>` shapes. Given a WIT signature:

```wit
interface fetcher {
    fetch: func(url: string) -> result<string, string>
}
```

The `outputSchema` includes the `result` wrapper and the familiar `oneOf` structure for `ok` and
`err` variants:

```json
{
  "type": "object",
  "properties": {
    "result": {
      "type": "object",
      "oneOf": [
        {
          "type": "object",
          "properties": { "ok": { "type": "string" } },
          "required": ["ok"]
        },
        {
          "type": "object",
          "properties": { "err": { "type": "string" } },
          "required": ["err"]
        }
      ]
    }
  },
  "required": ["result"]
}
```

### Value Conversion Helpers

`create_placeholder_results` and `json_to_vals` are mostly used inside Wassette, but you may need
them when writing integration tests or CLIs:

```rust
let params = vec![
    ("url".to_string(), Type::String),
];
let json_args = serde_json::json!({ "url": "https://example.com" });
let wit_vals = json_to_vals(&json_args, &params)?;

// Later, when the component returns:
let raw_results: Vec<Val> = component_call()?;
let json_result = vals_to_json(&raw_results);
assert!(json_result.get("result").is_some());
```

## Canonicalization in Wassette

`component2json` always emits the result wrapper, but Wassette defensively normalizes schemas from
multiple sources:

- **Fresh introspection** when a component is loaded
- **Cached metadata** stored on disk
- **Third-party tooling** that may not yet wrap results

The `wassette::schema` module provides three key helpers.

### `canonicalize_output_schema`

Ensures the schema we pass to clients has:

1. A top-level object with a required `result` property.
2. Tuple-like arrays converted into `{ "val0": ..., "val1": ... }` objects.
3. Nested schemas recursively normalized.

This runs whenever we:

- Load schemas from disk (`LifecycleManager::populate_registry_from_metadata`).
- Return schema data to clients (`get_component_schema`, `handle_list_components`).

### `ensure_structured_result`

Aligns actual runtime output with the canonical schema. It:

- Inserts the `{ "result": ... }` wrapper if the component returned a bare value.
- Rewrites arrays to the `valN` object shape when necessary.
- Fills in missing object properties with `null` to match the schema.

This function is applied in `handle_component_call` before we set `structured_content` in the MCP
response.

### `wrap_schema_in_result`

A small helper used by the other functions; exposed for completeness. It is helpful when building
synthetic schemas (e.g., tests that fake tool metadata) because it mirrors the runtime behavior.

## End-to-End Example: Fetch Tool

The integration test in `tests/structured_output_integration_test.rs` exercises the full pipeline.
Below is a trimmed version showing the key checkpoints:

```rust
let fetch_tool = tools.iter()
    .find(|tool| tool["name"] == "fetch")
    .expect("fetch tool present");

let output_schema = fetch_tool["outputSchema"].clone();
let canonical = canonicalize_output_schema(&output_schema);
assert!(canonical["properties"]["result"].is_object());

let response_json = lifecycle_manager
    .execute_component_call(&component_id, "fetch", request_json)
    .await?;

let structured_value = ensure_structured_result(&canonical, serde_json::from_str(&response_json)?);
assert!(structured_value["result"].is_object());
```

If the component returns an error, the wrapper still appears:

```json
{
  "result": {
    "err": "network unavailable"
  }
}
```

That predictable shape is what makes it possible for MCP clients to handle successes and failures
without special-casing each tool.

## How Metadata Caching Uses These Pieces

When we save component metadata for fast start-up, we serialize the tool schemas exactly as produced
by `component2json`. Upon restart, `populate_registry_from_metadata` canonicalizes each schema before
re-registering it. This guards against older metadata that may lack the wrapper and ensures all
run-time code operates on the same normalized representation.

Key call sites:

- `LifecycleManager::ensure_component_loaded` – introspects a live component, stores the raw schema,
  and updates the registry with canonicalized copies.
- `LifecycleManager::populate_registry_from_metadata` – reads cached schemas, canonicalizes, then
  registers them.
- `LifecycleManager::get_component_schema` – canonicalizes again just before returning JSON to CLI
  or MCP clients.

## Tips for Contributors

- **Always return structured values through `vals_to_json`.** If you add a new code path that
  constructs responses manually, wrap them with `ensure_structured_result` or reuse
  `vals_to_json` to avoid schema drift.
- **Update tests when tweaking schemas.** The integration test mentioned above is the best safety
  net. Add new assertions that validate the `result` wrapper for your scenario.
- **Prefer `component_exports_to_tools` over manual schema creation.** It already handles nested
  components, package names, and the result wrapper.
- **Use canonicalization helpers in custom tooling.** If you build CLIs or services on top of
  Wassette metadata, calling `canonicalize_output_schema` will keep your consumers aligned with the
  server's behavior.

## Additional Scenarios

### Custom Structured Payloads

If a component returns an object directly (for example, a record of file metadata), the schema looks
like this:

```json
{
  "result": {
    "type": "object",
    "properties": {
      "path": { "type": "string" },
      "size": { "type": "number" }
    },
    "required": ["path", "size"]
  }
}
```

At runtime the payload is wrapped without modifying the inner object:

```json
{
  "result": {
    "path": "./Cargo.toml",
    "size": 4096
  }
}
```

### Mixing Tuples and Records

WIT supports complex types such as `result<(string, u32), error-record>`. The generated schema nests
both tuple normalization and structured objects:

```json
{
  "result": {
    "oneOf": [
      {
        "type": "object",
        "properties": {
          "ok": {
            "type": "object",
            "properties": {
              "val0": { "type": "string" },
              "val1": { "type": "number" }
            },
            "required": ["val0", "val1"]
          }
        },
        "required": ["ok"]
      },
      {
        "type": "object",
        "properties": {
          "err": {
            "type": "object",
            "properties": {
              "code": { "type": "number" },
              "message": { "type": "string" }
            },
            "required": ["code", "message"]
          }
        },
        "required": ["err"]
      }
    ]
  }
}
```

No extra work is required in Wassette—the canonicalizer recognizes both patterns automatically.

### Handling Empty Results

A function that returns no values produces `outputSchema: null`. `canonicalize_output_schema`
converts that into `None`, and `ensure_structured_result` skips wrapping the response. Consumers can
still rely on the `result` key for all functions that actually return data.

## Where to Look in the Code

- `crates/component2json/src/lib.rs` – schema translation, value conversion, result wrapper.
- `crates/wassette/src/schema.rs` – canonicalization helpers.
- `crates/mcp-server/src/components.rs` – wiring between lifecycle manager and MCP responses.
- `tests/structured_output_integration_test.rs` – end-to-end assertions covering the entire stack.

Understanding this pipeline makes it easier to add new type translations, improve client ergonomics,
or debug mismatched schemas.
