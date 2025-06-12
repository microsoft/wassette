# component2json

A Rust library for converting WebAssembly Components to JSON Schema and handling WebAssembly Interface Type (WIT) value conversions.

## Usage

```rust
use component2json::{component_exports_to_json_schema, json_to_vals, vals_to_json};
use wasmtime::component::Component;
use wasmtime::Engine;

// Create a WebAssembly engine with component model enabled
let mut config = wasmtime::Config::new();
config.wasm_component_model(true);
let engine = Engine::new(&config)?;

// Load your component
let component = Component::from_file(&engine, "path/to/component.wasm")?;

// Get JSON schema for all exported functions
let schema = component_exports_to_json_schema(&component, &engine, true);

// Convert JSON arguments to WIT values
let json_args = serde_json::json!({
    "name": "example",
    "value": 42
});
let wit_vals = json_to_vals(&json_args)?;

// Convert WIT values back to JSON
let json_result = vals_to_json(&wit_vals);
```

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

`list<T>` is variable-length list where all elements are of type `T`.

```json
{
    "type": "array",
    "items": <schema-of-element-type>
}
```

##### Records

`Record` is a structure with named fields.

```json
{
    "type": "object",
    "properties": {
        "<field-name>": <schema-of-field-type>,
        ...
    },
    "required": ["<field-names>"]
}
```

##### Tuples

`Tuple<T1, T2, ...>` is a fixed-length structure where each element is of potentially a different type.

```json
{
    "type": "object",
    "properties": {
        "__tuple": {
            "type": "array",
            "prefixItems": [<schema-of-each-type>, ...],
            "minItems": <length>,
            "maxItems": <length>
        }
    },
    "required": ["__tuple"],
    "additionalProperties": false
}
```

**Serialization Format:**

```json
{
    "__tuple": [value1, value2, value3]
}
```

The discriminator pattern `{"__tuple": [...]}` ensures tuples are never confused with regular lists.

##### Variants

`Variant<T1, T2, ...>` is a tagged union that can hold one of several different types

```json
{
    "oneOf": [
        {
            "type": "object",
            "properties": {
                "tag": { "const": "<case-with-payload>" },
                "val": <schema-of-payload-type>
            },
            "required": ["tag", "val"]
        },
        {
            "type": "object",
            "properties": {
                "tag": { "const": "<case-without-payload>" }
            },
            "required": ["tag"]
        }
    ]
}
```

##### Enums

`Enum<T1, T2, ...>`: a type that can be one of several named string values.

```json
{
    "oneOf": [
        {
            "type": "object",
            "properties": {
                "__enum": { "const": "<enum-value-1>" }
            },
            "required": ["__enum"],
            "additionalProperties": false
        },
        {
            "type": "object", 
            "properties": {
                "__enum": { "const": "<enum-value-2>" }
            },
            "required": ["__enum"],
            "additionalProperties": false
        }
    ]
}
```

**Serialization Format:**
```json
{
    "__enum": "enum-value"
}
```

The discriminator pattern `{"__enum": "..."}` ensures enums are never confused with regular strings.

##### Options

`Option<T>`: a type that can be either `None` or `Some(T)`.

```json
{
    "oneOf": [
        {
            "type": "object",
            "properties": {
                "__option": { "const": "None" }
            },
            "required": ["__option"],
            "additionalProperties": false
        },
        {
            "type": "object", 
            "properties": {
                "__option": { "const": "Some" },
                "val": <schema-of-inner-type>
            },
            "required": ["__option", "val"],
            "additionalProperties": false
        }
    ]
}
```

**Serialization Format:**
```json
// For None
{
    "__option": "None"
}

// For Some(value)
{
    "__option": "Some",
    "val": <inner-value>
}
```

The discriminator pattern `{"__option": "..."}` ensures options are never confused with other types and resolves ambiguities that existed with the previous null-based approach.

##### Results

`Result<T, E>`: a type that can be either `ok(T)` or `err(E)`.

```json
{
    "oneOf": [
        {
            "type": "object",
            "properties": {
                "ok": <schema-of-ok-type>
            },
            "required": ["ok"]
        },
        {
            "type": "object",
            "properties": {
                "err": <schema-of-err-type>
            },
            "required": ["err"]
        }
    ]
}
```

##### Flags

`Flags<T1, T2, ...>` represents a set of enabled flags.

**Schema Format:**
```json
{
    "type": "object",
    "properties": {
        "__flags": {
            "type": "object",
            "properties": {
                "<flag-name-1>": { "type": "boolean" },
                "<flag-name-2>": { "type": "boolean" },
                ...
            },
            "additionalProperties": false
        }
    },
    "required": ["__flags"],
    "additionalProperties": false
}
```

**Serialization Format:**
```json
{
    "__flags": {
        "flag1": true,
        "flag3": true
    }
}
```

The discriminator pattern `{"__flags": {...}}` ensures flags are never confused with regular records containing boolean fields.

##### Resources

> Note: Resources cannot be created from JSON in practice.
