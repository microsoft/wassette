use serde_json::{json, Map, Value};
use thiserror::Error;
use wasmtime::component::types::{ComponentFunc, ComponentItem};
use wasmtime::component::{Component, Type, Val};
use wasmtime::Engine;

#[derive(Error, Debug)]
pub enum ValError {
    /// The JSON number could not be interpreted as either an integer or a float.
    #[error("cannot interpret number as i64 or f64: {0}")]
    NumberError(String),

    /// A character field was invalid, for example an empty or multi-character string
    /// when you expected a single char.
    #[error("invalid char: {0}")]
    InvalidChar(String),

    /// An object had an unexpected shape for a particular conceptual type.
    #[error("expected object shape for {0}, found: {1}")]
    ShapeError(&'static str, String),

    /// A JSON object was recognized, but does not match any known variant shape.
    #[error("unknown object shape: {0:?}")]
    UnknownShape(serde_json::Map<String, Value>),

    /// Could not interpret a resource from the JSON field(s).
    #[error("cannot interpret resource from JSON")]
    ResourceError,
}

fn type_to_json_schema(t: &Type) -> Value {
    match t {
        Type::Bool => json!({ "type": "boolean" }),
        Type::S8
        | Type::S16
        | Type::S32
        | Type::S64
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::Float32
        | Type::Float64 => json!({ "type": "number" }),
        Type::Char => json!({
            "type": "string",
            "description": "1 unicode codepoint"
        }),
        Type::String => json!({ "type": "string" }),

        // represent a `list<T>` as an array with items = schema-of-T
        Type::List(list_handle) => {
            let elem_schema = type_to_json_schema(&list_handle.ty());
            json!({
                "type": "array",
                "items": elem_schema
            })
        }

        Type::Record(r) => {
            let mut props = serde_json::Map::new();
            let mut required_fields = Vec::new();
            for field in r.fields() {
                required_fields.push(field.name.to_string());
                props.insert(field.name.to_string(), type_to_json_schema(&field.ty));
            }
            json!({
                "type": "object",
                "properties": props,
                "required": required_fields
            })
        }

        Type::Tuple(tup) => {
            // Tuples discriminator pattern: {"__tuple": [item1, item2, ...]}
            let items: Vec<Value> = tup.types().map(|ty| type_to_json_schema(&ty)).collect();
            json!({
                "type": "object",
                "properties": {
                    "__tuple": {
                "type": "array",
                "prefixItems": items,
                "minItems": items.len(),
                "maxItems": items.len()
                    }
                },
                "required": ["__tuple"],
                "additionalProperties": false
            })
        }

        Type::Variant(variant_handle) => {
            // Variants discriminator pattern: {"__variant": "tag"} or {"__variant": "tag", "val": ...}
            let mut cases_schema = Vec::new();
            for case in variant_handle.cases() {
                let case_name = case.name;
                if let Some(ref payload_ty) = case.ty {
                    cases_schema.push(json!({
                        "type": "object",
                        "properties": {
                            "__variant": { "const": case_name },
                            "val": type_to_json_schema(payload_ty)
                        },
                        "required": ["__variant", "val"],
                        "additionalProperties": false
                    }));
                } else {
                    cases_schema.push(json!({
                        "type": "object",
                        "properties": {
                            "__variant": { "const": case_name }
                        },
                        "required": ["__variant"],
                        "additionalProperties": false
                    }));
                }
            }
            json!({ "oneOf": cases_schema })
        }

        Type::Enum(enum_handle) => {
            // Enums discriminator pattern: {"__enum": "value"}
            let names: Vec<&str> = enum_handle.names().collect();
            let enum_schemas: Vec<Value> = names
                .iter()
                .map(|name| {
                    json!({
                        "type": "object",
                        "properties": {
                            "__enum": { "const": name }
                        },
                        "required": ["__enum"],
                        "additionalProperties": false
                    })
                })
                .collect();
            json!({ "oneOf": enum_schemas })
        }

        Type::Option(opt_handle) => {
            // Options discriminator pattern: {"__option": "None"} or {"__option": "Some", "val": ...}
            let inner_schema = type_to_json_schema(&opt_handle.ty());
            json!({
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
                            "val": inner_schema
                        },
                        "required": ["__option", "val"],
                        "additionalProperties": false
                    }
                ]
            })
        }

        Type::Result(res_handle) => {
            // Results discriminator pattern: {"__result": "Ok", "val": ...} or {"__result": "Err", "val": ...}
            let ok_schema = res_handle
                .ok()
                .map(|ok_ty| type_to_json_schema(&ok_ty))
                .unwrap_or(json!({ "type": "null" }));

            let err_schema = res_handle
                .err()
                .map(|err_ty| type_to_json_schema(&err_ty))
                .unwrap_or(json!({ "type": "null" }));

            json!({
                "oneOf": [
                    {
                        "type": "object",
                        "properties": {
                            "__result": { "const": "Ok" },
                            "val": ok_schema
                        },
                        "required": ["__result", "val"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "__result": { "const": "Err" },
                            "val": err_schema
                        },
                        "required": ["__result", "val"],
                        "additionalProperties": false
                    }
                ]
            })
        }

        Type::Flags(flags_handle) => {
            // Flags discriminator pattern: {"flags": {"read": true, "write": false}}
            let mut flag_props = serde_json::Map::new();
            for name in flags_handle.names() {
                flag_props.insert(name.to_string(), json!({"type": "boolean"}));
            }
            json!({
                "type": "object",
                "properties": {
                    "__flags": {
                        "type": "object",
                        "properties": flag_props,
                        "additionalProperties": false
                    }
                },
                "required": ["__flags"],
                "additionalProperties": false
            })
        }

        Type::Own(r) => {
            // Resources discriminator pattern: {"__resource": "description"}
            json!({
                "type": "object",
                "properties": {
                    "__resource": {
                "type": "string",
                "description": format!("own'd resource: {:?}", r)
                    }
                },
                "required": ["__resource"],
                "additionalProperties": false
            })
        }
        Type::Borrow(r) => {
            // Resources discriminator pattern: {"__resource": "description"}
            json!({
                "type": "object",
                "properties": {
                    "__resource": {
                "type": "string",
                "description": format!("borrow'd resource: {:?}", r)
                    }
                },
                "required": ["__resource"],
                "additionalProperties": false
            })
        }
    }
}

fn component_func_to_schema(name: &str, func: &ComponentFunc, output: bool) -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();

    for (param_name, param_type) in func.params() {
        required.push(param_name.to_string());
        properties.insert(param_name.to_string(), type_to_json_schema(&param_type));
    }

    let input_schema = json!({
        "type": "object",
        "properties": properties,
        "required": required
    });

    let mut tool_obj = serde_json::Map::new();
    tool_obj.insert("name".to_string(), json!(name));
    tool_obj.insert(
        "description".to_string(),
        json!(format!("Auto-generated schema for function '{name}'")),
    );
    tool_obj.insert("inputSchema".to_string(), input_schema);

    if output {
        let mut results_iter = func.results();
        let output_schema = match results_iter.len() {
            0 => None,
            1 => Some(type_to_json_schema(&results_iter.next().unwrap())),
            _ => {
                let schemas: Vec<_> = results_iter.map(|ty| type_to_json_schema(&ty)).collect();
                Some(json!({
                    "type": "array",
                    "items": schemas
                }))
            }
        };
        if let Some(o) = output_schema {
            tool_obj.insert("outputSchema".to_string(), o);
        }
    }
    json!(tool_obj)
}

fn gather_exported_functions(
    export_name: &str,
    previous_name: Option<String>,
    item: &ComponentItem,
    engine: &Engine,
    results: &mut Vec<Value>,
    output: bool,
) {
    match item {
        ComponentItem::ComponentFunc(func) => {
            let name = if let Some(prefix) = previous_name {
                format!("{}.{}", prefix, export_name)
            } else {
                export_name.to_string()
            };
            results.push(component_func_to_schema(&name, func, output));
        }
        ComponentItem::Component(sub_component) => {
            let previous_name = Some(export_name.to_string());
            for (export_name, export_item) in sub_component.exports(engine) {
                gather_exported_functions(
                    export_name,
                    previous_name.clone(),
                    &export_item,
                    engine,
                    results,
                    output,
                );
            }
        }
        ComponentItem::ComponentInstance(instance) => {
            let previous_name = Some(export_name.to_string());
            for (export_name, export_item) in instance.exports(engine) {
                gather_exported_functions(
                    export_name,
                    previous_name.clone(),
                    &export_item,
                    engine,
                    results,
                    output,
                );
            }
        }
        ComponentItem::CoreFunc(_)
        | ComponentItem::Module(_)
        | ComponentItem::Type(_)
        | ComponentItem::Resource(_) => {}
    }
}

fn object_to_val(obj: &Map<String, Value>) -> Result<Val, ValError> {
    // Check for Result
    if obj.contains_key("__result") {
        if let Some(Value::String(result_type)) = obj.get("__result") {
            if obj.len() == 2 {
                if let Some(val) = obj.get("val") {
                    let inner_val = if val.is_null() {
                        None
                    } else {
                        Some(Box::new(json_to_val(val)?))
                    };

                    match result_type.as_str() {
                        "Ok" => return Ok(Val::Result(Ok(inner_val))),
                        "Err" => return Ok(Val::Result(Err(inner_val))),
                        _ => {}
                    }
                }
            }
        }
    }

    // Check for Variant
    if obj.contains_key("__variant") {
        if let Some(Value::String(variant_tag)) = obj.get("__variant") {
            match obj.len() {
                1 => {
                    // {"__variant": "empty-case"}
                    return Ok(Val::Variant(variant_tag.clone(), None));
                }
                2 => {
                    if let Some(val) = obj.get("val") {
                        // {"__variant": "with-payload", "val": 42}
                        return Ok(Val::Variant(
                            variant_tag.clone(),
                            Some(Box::new(json_to_val(val)?)),
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    // Check for Option
    if obj.contains_key("__option") {
        if let Some(Value::String(option_type)) = obj.get("__option") {
            match option_type.as_str() {
                "None" => {
                    if obj.len() == 1 {
                        return Ok(Val::Option(None));
                    }
                }
                "Some" => {
                    if obj.len() == 2 {
                        if let Some(val) = obj.get("val") {
                            return Ok(Val::Option(Some(Box::new(json_to_val(val)?))));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    if obj.len() == 1 {
        // Check for Tuple
        if let Some(Value::Array(tuple_arr)) = obj.get("__tuple") {
            let mut items = Vec::new();
            for item in tuple_arr {
                items.push(json_to_val(item)?);
            }
            return Ok(Val::Tuple(items));
        }

        // Check for Enum
        if let Some(Value::String(enum_value)) = obj.get("__enum") {
            return Ok(Val::Enum(enum_value.clone()));
        }

        // Check for Resource
        if let Some(Value::String(_resource_desc)) = obj.get("__resource") {
            return Err(ValError::ResourceError);
        }

        // Check for Flags
        if let Some(Value::Object(flags_obj)) = obj.get("__flags") {
            let mut flags = Vec::new();
            for (k, v) in flags_obj {
                if let Value::Bool(true) = v {
                    flags.push(k.to_string());
                }
                // false values are omitted (not enabled flags)
            }
            return Ok(Val::Flags(flags));
        }
    }

    // If we reach here, we assume it's a Record
    let mut fields = Vec::new();
    for (k, v) in obj {
        fields.push((k.clone(), json_to_val(v)?));
    }
    Ok(Val::Record(fields))
}

pub fn component_exports_to_json_schema(
    component: &Component,
    engine: &Engine,
    output: bool,
) -> Value {
    let mut tools_array = Vec::new();

    for (export_name, export_item) in component.component_type().exports(engine) {
        gather_exported_functions(
            export_name,
            None,
            &export_item,
            engine,
            &mut tools_array,
            output,
        );
    }

    json!({ "tools": tools_array })
}

/// Parses a single `serde_json::Value` into one `Val`.
pub fn json_to_val(value: &Value) -> Result<Val, ValError> {
    match value {
        Value::Null => Err(ValError::ShapeError(
            "null",
            "stand-alone null not allowed".into(),
        )),
        Value::Bool(b) => Ok(Val::Bool(*b)),
        Value::Number(num) => {
            if let Some(i) = num.as_i64() {
                Ok(Val::S64(i))
            } else if let Some(f) = num.as_f64() {
                Ok(Val::Float64(f))
            } else {
                Err(ValError::NumberError(format!("{num:?}")))
            }
        }
        Value::String(s) => Ok(Val::String(s.clone())),
        Value::Array(arr) => {
            let mut vals = Vec::new();
            for item in arr {
                vals.push(json_to_val(item)?);
            }
            Ok(Val::List(vals))
        }
        Value::Object(obj) => object_to_val(obj),
    }
}

pub fn json_to_vals(value: &Value) -> Result<Vec<Val>, ValError> {
    match value {
        Value::Object(obj) => {
            let mut results = Vec::new();
            for (_, v) in obj {
                let subval = json_to_val(v)?;
                results.push(subval);
            }
            Ok(results)
        }
        _ => {
            let single = json_to_val(value)?;
            Ok(vec![single])
        }
    }
}

pub fn vals_to_json(vals: &[Val]) -> Value {
    match vals.len() {
        0 => Value::Null,
        1 => val_to_json(&vals[0]),
        _ => {
            let mut map = Map::new();
            for (i, v) in vals.iter().enumerate() {
                map.insert(format!("val{i}"), val_to_json(v));
            }
            Value::Object(map)
        }
    }
}

fn val_to_json(val: &Val) -> Value {
    match val {
        Val::Bool(b) => Value::Bool(*b),
        Val::S8(n) => Value::Number((*n as i64).into()),
        Val::U8(n) => Value::Number((*n as u64).into()),
        Val::S16(n) => Value::Number((*n as i64).into()),
        Val::U16(n) => Value::Number((*n as u64).into()),
        Val::S32(n) => Value::Number((*n as i64).into()),
        Val::U32(n) => Value::Number((*n as u64).into()),
        Val::S64(n) => Value::Number((*n).into()),
        Val::U64(n) => Value::Number((*n).into()),
        Val::Float32(f) => serde_json::Number::from_f64(*f as f64)
            .map(Value::Number)
            .unwrap_or_else(|| Value::String(f.to_string())),
        Val::Float64(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or_else(|| Value::String(f.to_string())),
        Val::Char(c) => Value::String(c.to_string()),
        Val::String(s) => Value::String(s.clone()),

        Val::List(list) => Value::Array(list.iter().map(val_to_json).collect()),
        Val::Record(fields) => {
            let mut map = Map::new();
            for (k, v) in fields {
                map.insert(k.clone(), val_to_json(v));
            }
            Value::Object(map)
        }
        Val::Tuple(items) => {
            let tuple_array = Value::Array(items.iter().map(val_to_json).collect());
            json!({
                "__tuple": tuple_array
            })
        }

        Val::Variant(tag, payload) => {
            // Use discriminator pattern for variants
            if let Some(val_box) = payload {
                json!({
                    "__variant": tag.clone(),
                    "val": val_to_json(val_box)
                })
            } else {
                json!({
                    "__variant": tag.clone()
                })
            }
        }
        Val::Enum(s) => {
            json!({
                "__enum": s.clone()
            })
        }

        Val::Option(None) => {
            json!({
                "__option": "None"
            })
        }
        Val::Option(Some(val_box)) => {
            json!({
                "__option": "Some",
                "val": val_to_json(val_box)
            })
        }

        Val::Result(Ok(opt_box)) => {
            json!({
                "__result": "Ok",
                "val": match opt_box {
                    Some(v) => val_to_json(v),
                    None => Value::Null,
                }
            })
        }
        Val::Result(Err(opt_box)) => {
            json!({
                "__result": "Err",
                "val": match opt_box {
                    Some(v) => val_to_json(v),
                    None => Value::Null,
                }
            })
        }

        Val::Flags(flags) => {
            let mut flags_obj = Map::new();
            for flag in flags {
                flags_obj.insert(flag.clone(), Value::Bool(true));
            }
            json!({
                "__flags": Value::Object(flags_obj)
            })
        }
        Val::Resource(res) => {
            json!({
                "__resource": format!("{:?}", res)
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;
    use wasmtime::component::Val;

    use super::*;

    #[test]
    #[should_panic]
    fn test_json_to_val_null() {
        let json_val = Value::Null;
        let _ = json_to_val(&json_val).unwrap();
    }

    #[test]
    fn test_json_to_val_bool() {
        let json_val = json!(true);
        let val = json_to_val(&json_val).unwrap();
        assert!(matches!(val, Val::Bool(true)));
        assert_eq!(val_to_json(&val), json!(true));
    }

    #[test]
    fn test_json_to_val_integer() {
        let json_val = json!(123);
        let val = json_to_val(&json_val).unwrap();
        assert!(matches!(val, Val::S64(123)));
        assert_eq!(val_to_json(&val), json!(123));
    }

    #[test]
    fn test_json_to_val_float() {
        let json_val = json!(123.45);
        let val = json_to_val(&json_val).unwrap();
        if let Val::Float64(f) = val {
            assert!((f - 123.45).abs() < 1e-6);
        } else {
            panic!("Expected Float64 variant");
        }
        if let Value::Number(n) = val_to_json(&val) {
            let f_value = n.as_f64().unwrap();
            assert!((f_value - 123.45).abs() < 1e-6);
        } else {
            panic!("Expected JSON number");
        }
    }

    #[test]
    fn test_json_to_val_string() {
        let json_val = json!("test");
        let val = json_to_val(&json_val).unwrap();
        assert!(matches!(val, Val::String(ref s) if s == "test"));
        assert_eq!(val_to_json(&val), json!("test"));
    }

    #[test]
    fn test_json_to_val_array() {
        let json_val = json!([1, 2, 3]);
        let val = json_to_val(&json_val).unwrap();
        if let Val::List(list) = val.clone() {
            assert_eq!(list.len(), 3);
            for (i, item) in list.iter().enumerate() {
                assert!(matches!(item, Val::S64(n) if *n == i as i64 + 1));
            }
        } else {
            panic!("Expected List variant");
        }
        let json_val = val_to_json(&val);
        assert_eq!(json_val, json!([1, 2, 3]));
    }

    #[test]
    fn test_json_to_val_object() {
        let json_val = json!({"a": 10, "b": false});
        let val = json_to_val(&json_val).unwrap();
        if let Val::Record(fields) = val.clone() {
            let mut field_map: HashMap<&str, &Val> = HashMap::new();
            for (k, v) in fields.iter() {
                field_map.insert(k, v);
            }
            match field_map.get("a") {
                Some(Val::S64(n)) => assert_eq!(*n, 10),
                _ => panic!("Expected field 'a' to be S64(10)"),
            }
            match field_map.get("b") {
                Some(Val::Bool(b)) => assert!(!(*b)),
                _ => panic!("Expected field 'b' to be Bool(false)"),
            }
        } else {
            panic!("Expected Record variant");
        }
        assert_eq!(val_to_json(&val), json_val);
    }

    #[test]
    fn test_json_to_vals_with_object() {
        let json_val = json!({"x": 5, "y": 6});
        let vals = json_to_vals(&json_val).unwrap();
        assert_eq!(vals.len(), 2);
        let mut found_x = false;
        let mut found_y = false;
        for v in vals {
            match v {
                Val::S64(5) => found_x = true,
                Val::S64(6) => found_y = true,
                _ => {}
            }
        }
        assert!(found_x && found_y);
    }

    #[test]
    fn test_json_to_vals_with_non_object() {
        let json_val = json!("single");
        let vals = json_to_vals(&json_val).unwrap();
        assert_eq!(vals.len(), 1);
        assert!(matches!(vals[0], Val::String(ref s) if s == "single"));
    }

    #[test]
    fn test_vals_to_json_empty() {
        let json_val = vals_to_json(&[]);
        assert_eq!(json_val, json!(null));
    }

    #[test]
    fn test_vals_to_json_single() {
        let val = Val::Bool(true);
        let json_val = vals_to_json(&[val.clone()]);
        assert_eq!(json_val, val_to_json(&val));
    }

    #[test]
    fn test_vals_to_json_multiple() {
        let wit_vals = vec![Val::String("example".to_string()), Val::S64(42)];
        let json_result = vals_to_json(&wit_vals);

        let obj = json_result.as_object().unwrap();
        assert_eq!(obj.get("val0").unwrap(), &json!("example"));
        assert_eq!(obj.get("val1").unwrap(), &json!(42));
    }

    #[test]
    fn test_option_discriminator_pattern_recognition() {
        // Test discriminator-based option pattern recognition
        let none_json = json!({"__option": "None"});
        let none_val = json_to_val(&none_json).unwrap();
        if let Val::Option(None) = none_val {
        } else {
            panic!("Expected Option(None), got: {:?}", none_val);
        }

        // Test Some with value
        let some_json = json!({"__option": "Some", "val": 42});
        let some_val = json_to_val(&some_json).unwrap();
        if let Val::Option(Some(inner)) = some_val {
            assert!(matches!(inner.as_ref(), Val::S64(42)));
        } else {
            panic!("Expected Option(Some(42)), got: {:?}", some_val);
        }

        // Test Some with complex value
        let complex_json = json!({"__option": "Some", "val": {"name": "test"}});
        let complex_val = json_to_val(&complex_json).unwrap();
        if let Val::Option(Some(inner)) = complex_val {
            assert!(matches!(inner.as_ref(), Val::Record(_)));
        } else {
            panic!("Expected Option(Some(Record)), got: {:?}", complex_val);
        }
    }

    #[test]
    fn test_option_round_trip() {
        // Test Option(None) round trip
        let none_val = Val::Option(None);
        let none_json = val_to_json(&none_val);
        assert_eq!(none_json, json!({"__option": "None"}));

        let parsed_none = json_to_val(&none_json).unwrap();
        assert_eq!(parsed_none, none_val);

        // Test Option(Some) round trip
        let some_val = Val::Option(Some(Box::new(Val::String("test".to_string()))));
        let some_json = val_to_json(&some_val);
        assert_eq!(some_json, json!({"__option": "Some", "val": "test"}));

        let parsed_some = json_to_val(&some_json).unwrap();
        assert_eq!(parsed_some, some_val);
    }

    #[test]
    fn test_discriminator_pattern_conflicts_resolved() {
        // Test that all complex types now use unambiguous discriminator patterns

        // Option patterns
        let option_none = json!({"__option": "None"});
        let option_some = json!({"__option": "Some", "val": 42});

        assert!(matches!(
            json_to_val(&option_none).unwrap(),
            Val::Option(None)
        ));
        assert!(matches!(
            json_to_val(&option_some).unwrap(),
            Val::Option(Some(_))
        ));

        // New Variant discriminator patterns
        let variant_empty = json!({"__variant": "empty"});
        let variant_with_val = json!({"__variant": "data", "val": 42});

        assert!(matches!(
            json_to_val(&variant_empty).unwrap(),
            Val::Variant(_, None)
        ));
        assert!(matches!(
            json_to_val(&variant_with_val).unwrap(),
            Val::Variant(_, Some(_))
        ));

        // New Result discriminator patterns
        let result_ok = json!({"__result": "Ok", "val": 42});
        let result_err = json!({"__result": "Err", "val": "error message"});

        assert!(matches!(
            json_to_val(&result_ok).unwrap(),
            Val::Result(Ok(Some(_)))
        ));
        assert!(matches!(
            json_to_val(&result_err).unwrap(),
            Val::Result(Err(Some(_)))
        ));

        // Test that Records can now safely use previously conflicting field names
        let record_with_tag = json!({"tag": "record-field", "other": "value"});
        let record_with_ok = json!({"ok": "not-a-result", "status": "good"});
        let record_with_err = json!({"err": "not-a-result", "details": "info"});

        assert!(matches!(
            json_to_val(&record_with_tag).unwrap(),
            Val::Record(_)
        ));
        assert!(matches!(
            json_to_val(&record_with_ok).unwrap(),
            Val::Record(_)
        ));
        assert!(matches!(
            json_to_val(&record_with_err).unwrap(),
            Val::Record(_)
        ));
    }

    #[test]
    fn test_discriminator_round_trip() {
        // Test that all discriminator patterns round-trip correctly

        // Variant round trip
        let variant_empty = Val::Variant("empty".to_string(), None);
        let variant_json = val_to_json(&variant_empty);
        assert_eq!(variant_json, json!({"__variant": "empty"}));
        let parsed_variant = json_to_val(&variant_json).unwrap();
        assert_eq!(parsed_variant, variant_empty);

        let variant_with_payload = Val::Variant("data".to_string(), Some(Box::new(Val::S64(42))));
        let variant_json = val_to_json(&variant_with_payload);
        assert_eq!(variant_json, json!({"__variant": "data", "val": 42}));
        let parsed_variant = json_to_val(&variant_json).unwrap();
        assert_eq!(parsed_variant, variant_with_payload);

        // Result round trip
        let result_ok = Val::Result(Ok(Some(Box::new(Val::String("success".to_string())))));
        let result_json = val_to_json(&result_ok);
        assert_eq!(result_json, json!({"__result": "Ok", "val": "success"}));
        let parsed_result = json_to_val(&result_json).unwrap();
        assert_eq!(parsed_result, result_ok);

        let result_err = Val::Result(Err(Some(Box::new(Val::String("error".to_string())))));
        let result_json = val_to_json(&result_err);
        assert_eq!(result_json, json!({"__result": "Err", "val": "error"}));
        let parsed_result = json_to_val(&result_json).unwrap();
        assert_eq!(parsed_result, result_err);

        // Result with null values
        let result_ok_null = Val::Result(Ok(None));
        let result_json = val_to_json(&result_ok_null);
        assert_eq!(result_json, json!({"__result": "Ok", "val": null}));
        let parsed_result = json_to_val(&result_json).unwrap();
        assert_eq!(parsed_result, result_ok_null);
    }

    #[test]
    fn test_remaining_ambiguities() {
        // Test 1: Character vs String ambiguity
        let char_val = Val::Char('A');
        let string_val = Val::String("A".to_string());

        let char_json = val_to_json(&char_val);
        let string_json = val_to_json(&string_val);

        // Both serialize to the same JSON
        assert_eq!(char_json, string_json);
        assert_eq!(char_json, json!("A"));

        // But deserialization always chooses String
        let parsed = json_to_val(&char_json).unwrap();
        assert!(matches!(parsed, Val::String(_)));
        // Character info is lost - no roundtrip!

        // Test 2: Number type ambiguity
        let s32_val = Val::S32(42);
        let s64_val = Val::S64(42);
        let u32_val = Val::U32(42);

        let s32_json = val_to_json(&s32_val);
        let s64_json = val_to_json(&s64_val);
        let u32_json = val_to_json(&u32_val);

        // All serialize to the same JSON
        assert_eq!(s32_json, s64_json);
        assert_eq!(s64_json, u32_json);
        assert_eq!(s32_json, json!(42));

        // But deserialization always chooses S64
        let parsed = json_to_val(&s32_json).unwrap();
        assert!(matches!(parsed, Val::S64(42)));
        // Specific numeric type info is lost - no roundtrip for S32, U32, etc.

        println!("Note: Character and numeric type information is lost during JSON roundtrip");
        println!("This may be acceptable for most use cases where semantic meaning is preserved");
    }

    #[test]
    fn test_component_exports_schema() {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        let engine = Engine::new(&config).unwrap();

        // A complex component with nested components, various types and functions
        let wat = r#"(component
            (core module (;0;)
                (type (;0;) (func (param i32 i32 i32 i32) (result i32)))
                (type (;1;) (func))
                (type (;2;) (func (param i32 i32 i32 i64) (result i32)))
                (type (;3;) (func (param i32)))
                (type (;4;) (func (result i32)))
                (type (;5;) (func (param i32 i32) (result i32)))
                (type (;6;) (func (param i32 i64 i32) (result i32)))
                (memory (;0;) 1)
                (export "memory" (memory 0))
                (export "cabi_realloc" (func 0))
                (export "a" (func 1))
                (export "b" (func 2))
                (export "cabi_post_b" (func 3))
                (export "c" (func 4))
                (export "foo#a" (func 5))
                (export "foo#b" (func 6))
                (export "cabi_post_foo#b" (func 7))
                (export "foo#c" (func 8))
                (export "cabi_post_foo#c" (func 9))
                (export "bar#a" (func 10))
                (func (;0;) (type 0) (param i32 i32 i32 i32) (result i32)
                unreachable
                )
                (func (;1;) (type 1)
                unreachable
                )
                (func (;2;) (type 2) (param i32 i32 i32 i64) (result i32)
                unreachable
                )
                (func (;3;) (type 3) (param i32)
                unreachable
                )
                (func (;4;) (type 4) (result i32)
                unreachable
                )
                (func (;5;) (type 1)
                unreachable
                )
                (func (;6;) (type 5) (param i32 i32) (result i32)
                unreachable
                )
                (func (;7;) (type 3) (param i32)
                unreachable
                )
                (func (;8;) (type 6) (param i32 i64 i32) (result i32)
                unreachable
                )
                (func (;9;) (type 3) (param i32)
                unreachable
                )
                (func (;10;) (type 3) (param i32)
                unreachable
                )
                (@producers
                (processed-by "wit-component" "$CARGO_PKG_VERSION")
                (processed-by "my-fake-bindgen" "123.45")
                )
            )
            (core instance (;0;) (instantiate 0))
            (alias core export 0 "memory" (core memory (;0;)))
            (type (;0;) (func))
            (alias core export 0 "a" (core func (;0;)))
            (alias core export 0 "cabi_realloc" (core func (;1;)))
            (func (;0;) (type 0) (canon lift (core func 0)))
            (export (;1;) "a" (func 0))
            (type (;1;) (func (param "a" s8) (param "b" s16) (param "c" s32) (param "d" s64) (result string)))
            (alias core export 0 "b" (core func (;2;)))
            (alias core export 0 "cabi_post_b" (core func (;3;)))
            (func (;2;) (type 1) (canon lift (core func 2) (memory 0) string-encoding=utf8 (post-return 3)))
            (export (;3;) "b" (func 2))
            (type (;2;) (tuple s8 s16 s32 s64))
            (type (;3;) (func (result 2)))
            (alias core export 0 "c" (core func (;4;)))
            (func (;4;) (type 3) (canon lift (core func 4) (memory 0)))
            (export (;5;) "c" (func 4))
            (type (;4;) (flags "a" "b" "c"))
            (type (;5;) (func (param "x" 4)))
            (alias core export 0 "bar#a" (core func (;5;)))
            (func (;6;) (type 5) (canon lift (core func 5)))
            (component (;0;)
                (type (;0;) (flags "a" "b" "c"))
                (import "import-type-x" (type (;1;) (eq 0)))
                (type (;2;) (func (param "x" 1)))
                (import "import-func-a" (func (;0;) (type 2)))
                (type (;3;) (flags "a" "b" "c"))
                (export (;4;) "x" (type 3))
                (type (;5;) (func (param "x" 4)))
                (export (;1;) "a" (func 0) (func (type 5)))
            )
            (instance (;0;) (instantiate 0
                (with "import-func-a" (func 6))
                (with "import-type-x" (type 4))
                )
            )
            (export (;1;) "bar" (instance 0))
            (type (;6;) (func))
            (alias core export 0 "foo#a" (core func (;6;)))
            (func (;7;) (type 6) (canon lift (core func 6)))
            (type (;7;) (variant (case "a") (case "b" string) (case "c" s64)))
            (type (;8;) (func (param "x" string) (result 7)))
            (alias core export 0 "foo#b" (core func (;7;)))
            (alias core export 0 "cabi_post_foo#b" (core func (;8;)))
            (func (;8;) (type 8) (canon lift (core func 7) (memory 0) (realloc 1) string-encoding=utf8 (post-return 8)))
            (type (;9;) (func (param "x" 7) (result string)))
            (alias core export 0 "foo#c" (core func (;9;)))
            (alias core export 0 "cabi_post_foo#c" (core func (;10;)))
            (func (;9;) (type 9) (canon lift (core func 9) (memory 0) (realloc 1) string-encoding=utf8 (post-return 10)))
            (component (;1;)
                (type (;0;) (func))
                (import "import-func-a" (func (;0;) (type 0)))
                (type (;1;) (variant (case "a") (case "b" string) (case "c" s64)))
                (import "import-type-x" (type (;2;) (eq 1)))
                (type (;3;) (func (param "x" string) (result 2)))
                (import "import-func-b" (func (;1;) (type 3)))
                (type (;4;) (func (param "x" 2) (result string)))
                (import "import-func-c" (func (;2;) (type 4)))
                (type (;5;) (variant (case "a") (case "b" string) (case "c" s64)))
                (export (;6;) "x" (type 5))
                (type (;7;) (func))
                (export (;3;) "a" (func 0) (func (type 7)))
                (type (;8;) (func (param "x" string) (result 6)))
                (export (;4;) "b" (func 1) (func (type 8)))
                (type (;9;) (func (param "x" 6) (result string)))
                (export (;5;) "c" (func 2) (func (type 9)))
            )
            (instance (;2;) (instantiate 1
                (with "import-func-a" (func 7))
                (with "import-func-b" (func 8))
                (with "import-func-c" (func 9))
                (with "import-type-x" (type 7))
                )
            )
            (export (;3;) "foo" (instance 2))
            (@producers
                (processed-by "wit-component" "$CARGO_PKG_VERSION")
            )
            )"#;
        let component = Component::new(&engine, wat).unwrap();
        let schema = component_exports_to_json_schema(&component, &engine, true);

        let tools = schema.get("tools").unwrap().as_array().unwrap();
        assert_eq!(tools.len(), 7);

        fn find_tool<'a>(tools: &'a [Value], name: &str) -> Option<&'a Value> {
            tools
                .iter()
                .find(|t| t.get("name").and_then(|n| n.as_str()) == Some(name))
        }
        // Test root-level functions
        let root_a = find_tool(tools, "a").unwrap();
        assert!(root_a
            .get("inputSchema")
            .unwrap()
            .get("properties")
            .unwrap()
            .is_object());
        assert!(root_a.get("outputSchema").is_none());

        let root_b = find_tool(tools, "b").unwrap();
        let input_schema = root_b.get("inputSchema").unwrap();
        let properties = input_schema.get("properties").unwrap().as_object().unwrap();
        assert_eq!(properties.len(), 4);
        assert!(properties.contains_key("a"));
        assert!(properties.contains_key("b"));
        assert!(properties.contains_key("c"));
        assert!(properties.contains_key("d"));
        let output_schema = root_b.get("outputSchema").unwrap();
        assert_eq!(output_schema.get("type").unwrap(), "string");

        let root_c = find_tool(tools, "c").unwrap();
        let output_schema = root_c.get("outputSchema").unwrap();
        // Updated to check object-based tuple schema with __tuple discriminator
        assert_eq!(output_schema.get("type").unwrap(), "object");
        let props = output_schema
            .get("properties")
            .unwrap()
            .as_object()
            .unwrap();
        let tuple_schema = props.get("__tuple").unwrap();
        assert_eq!(tuple_schema.get("type").unwrap(), "array");
        assert_eq!(tuple_schema.get("minItems").unwrap(), 4);
        assert_eq!(tuple_schema.get("maxItems").unwrap(), 4);
        let prefix_items = tuple_schema.get("prefixItems").unwrap().as_array().unwrap();
        assert_eq!(prefix_items.len(), 4);
        for item in prefix_items {
            assert_eq!(item.get("type").unwrap(), "number");
        }

        // Test foo namespace functions
        let foo_a = find_tool(tools, "foo.a").unwrap();
        assert!(foo_a
            .get("inputSchema")
            .unwrap()
            .get("properties")
            .unwrap()
            .is_object());
        assert!(foo_a.get("outputSchema").is_none());

        let foo_b = find_tool(tools, "foo.b").unwrap();
        {
            let input_props = foo_b
                .get("inputSchema")
                .unwrap()
                .get("properties")
                .unwrap()
                .as_object()
                .unwrap();
            assert_eq!(input_props.len(), 1);
            assert!(input_props.contains_key("x")); // string

            let output_schema = foo_b.get("outputSchema").unwrap();
            let cases = output_schema.get("oneOf").unwrap().as_array().unwrap();
            assert_eq!(cases.len(), 3);

            let case_a = &cases[0];
            assert_eq!(
                case_a
                    .get("properties")
                    .unwrap()
                    .get("__variant")
                    .unwrap()
                    .get("const")
                    .unwrap(),
                "a"
            );

            let case_b = &cases[1];
            assert_eq!(
                case_b
                    .get("properties")
                    .unwrap()
                    .get("__variant")
                    .unwrap()
                    .get("const")
                    .unwrap(),
                "b"
            );
            assert_eq!(
                case_b
                    .get("properties")
                    .unwrap()
                    .get("val")
                    .unwrap()
                    .get("type")
                    .unwrap(),
                "string"
            );

            let case_c = &cases[2];
            assert_eq!(
                case_c
                    .get("properties")
                    .unwrap()
                    .get("__variant")
                    .unwrap()
                    .get("const")
                    .unwrap(),
                "c"
            );
            assert_eq!(
                case_c
                    .get("properties")
                    .unwrap()
                    .get("val")
                    .unwrap()
                    .get("type")
                    .unwrap(),
                "number"
            );
        }

        let foo_c = find_tool(tools, "foo.c").unwrap();
        {
            let input_props = foo_c
                .get("inputSchema")
                .unwrap()
                .get("properties")
                .unwrap()
                .as_object()
                .unwrap();
            assert_eq!(input_props.len(), 1);
            assert!(input_props.contains_key("x")); // variant type

            let output_schema = foo_c.get("outputSchema").unwrap();
            assert_eq!(output_schema.get("type").unwrap(), "string");
        }
    }
}
