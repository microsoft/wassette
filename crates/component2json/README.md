# component2json

A Rust library for converting WebAssembly Components to JSON Schema and handling WebAssembly Interface Type (WIT) value conversions.

## Usage

```rust
# fn main() -> Result<(), Box<dyn std::error::Error>> {
use component2json::{component_exports_to_json_schema, json_to_vals, vals_to_json, create_placeholder_results};
use wasmtime::component::{Component, Type, Val};
use wasmtime::Engine;

// Create a WebAssembly engine with component model enabled
let mut config = wasmtime::Config::new();
config.wasm_component_model(true);
let engine = Engine::new(&config)?;

// Load your component
# let component_wat = r#"(component)"#;
let component = Component::new(&engine, component_wat)?;

// Get JSON schema for all exported functions
let schema = component_exports_to_json_schema(&component, &engine, true);

// To convert JSON to WIT Val arguments, you must provide the expected types.
// These would typically be derived from inspecting a function's parameters.
let func_param_types = vec![
    ("name".to_string(), Type::String),
    ("value".to_string(), Type::U32),
];

// Convert a JSON object to WIT values according to the function's parameter types
let json_args = serde_json::json!({
    "name": "example",
    "value": 42
});
let wit_vals = json_to_vals(&json_args, &func_param_types)?;

// Convert WIT values back to JSON
let json_result = vals_to_json(&wit_vals);
assert_eq!(json_result, serde_json::json!({"result": {"val0": "example", "val1": 42}}));

// Create placeholder results for function call results
// This is useful when you need to prepare storage for function return values
let result_types = vec![Type::String, Type::U32];
let placeholder_results = create_placeholder_results(&result_types);
# Ok(())
# }
```

## Structured Result Wrapper

All function return values produced by `component_exports_to_json_schema` and `vals_to_json` are wrapped in an object with a required `result` property.

- Single return values appear as `{ "result": VALUE }`.
- Multiple return values appear as `{ "result": { "val0": VALUE0, "val1": VALUE1, ... } }`.

The generated `outputSchema` for each tool mirrors this shape, ensuring downstream consumers can always access the payload through the `result` key.

## Type Conversion Specification

### WIT to JSON Schema

#### Primitive Types

| WIT Type | JSON Schema |
|----------|-------------|
| `bool` | `{"type": "boolean"}` |
| `s8`, `s16`, `s32`, `s64` | `{"type": "number"}` |
| `u8`, `u16`, `u32`, `u64` | `{"type": "number"}` |
| `float32`, `float64` | `{"type": "number"}` |
| `char` | `{"type": "string", "description": "1 unicode codepoint"}` |
| `string` | `{"type": "string"}` |

#### Composite Types

##### Lists

```json
{
    "type": "array",
    "items": "SCHEMA_OF_ELEMENT_TYPE"
}
```

##### Records

```json
{
    "type": "object",
    "properties": {
        "FIELD_NAME": "SCHEMA_OF_FIELD_TYPE"
    },
    "required": ["FIELD_NAMES"]
}
```

##### Tuples

```json
{
    "type": "array",
    "prefixItems": ["SCHEMA_OF_EACH_TYPE"],
    "minItems": "LENGTH",
    "maxItems": "LENGTH"
}
```

##### Variants

```json
{
    "oneOf": [
        {
            "type": "object",
            "properties": {
                "tag": { "const": "CASE_NAME" },
                "val": "SCHEMA_OF_PAYLOAD_TYPE"
            },
            "required": ["tag", "val"]
        },
        {
            "type": "object",
            "properties": {
                "tag": { "const": "CASE_NAME" }
            },
            "required": ["tag"]
        }
    ]
}
```

##### Enums

```json
{
    "type": "string",
    "enum": ["ENUM_VALUES"]
}
```

##### Options

```json
{
    "anyOf": [
        { "type": "null" },
        "SCHEMA_OF_INNER_TYPE"
    ]
}
```

##### Results

```json
{
    "oneOf": [
        {
            "type": "object",
            "properties": {
                "ok": "SCHEMA_OF_OK_TYPE"
            },
            "required": ["ok"]
        },
        {
            "type": "object",
            "properties": {
                "err": "SCHEMA_OF_ERR_TYPE"
            },
            "required": ["err"]
        }
    ]
}
```

##### Flags

```json
{
    "type": "array",
    "items": { "type": "string" }
}
```

##### Resources

```json
{
    "type": "string",
    "description": "RESOURCE_TYPE resource: RESOURCE_NAME"
}
```
