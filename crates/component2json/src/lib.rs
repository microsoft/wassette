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
            // Tuples uses discriminator pattern: {"__tuple": [item1, item2, ...]}
            // This matches serialization format and enables unambiguous pattern recognition
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
            let mut cases_schema = Vec::new();
            for case in variant_handle.cases() {
                let case_name = case.name;
                if let Some(ref payload_ty) = case.ty {
                    cases_schema.push(json!({
                        "type": "object",
                        "properties": {
                            "tag": { "const": case_name },
                            "val": type_to_json_schema(payload_ty)
                        },
                        "required": ["tag", "val"]
                    }));
                } else {
                    cases_schema.push(json!({
                        "type": "object",
                        "properties": {
                            "tag": { "const": case_name },
                        },
                        "required": ["tag"]
                    }));
                }
            }
            json!({ "oneOf": cases_schema })
        }

        Type::Enum(enum_handle) => {
            // Enums now use discriminator pattern: {"__enum": "value"}
            // This matches serialization format and enables unambiguous pattern recognition
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
            // Options now use discriminator pattern: {"__option": "None"} or {"__option": "Some", "val": ...}
            // This matches serialization format and enables unambiguous pattern recognition
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
                        "ok": ok_schema
                      },
                      "required": ["ok"]
                    },
                    {
                      "type": "object",
                      "properties": {
                        "err": err_schema
                      },
                      "required": ["err"]
                    }
                ]
            })
        }

        Type::Flags(flags_handle) => {
            // Flags uses discriminator pattern: {"flags": {"read": true, "write": false}}
            // This matches serialization format and enables unambiguous pattern recognition
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
            // Resources now use discriminator pattern: {"__resource": "description"}
            // This matches serialization format and enables unambiguous pattern recognition
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
            // Resources now use discriminator pattern: {"__resource": "description"}
            // This matches serialization format and enables unambiguous pattern recognition
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
    // first we check for `Result` pattern
    // {"ok": value} or {"err": value}
    // there is exactly one key either "ok" or "err"
    if obj.len() == 1 {
        if let Some(ok_val) = obj.get("ok") {
            let inner_val = if ok_val.is_null() {
                None
            } else {
                Some(Box::new(json_to_val(ok_val)?))
            };
            return Ok(Val::Result(Ok(inner_val)));
        }
        if let Some(err_val) = obj.get("err") {
            let inner_val = if err_val.is_null() {
                None
            } else {
                Some(Box::new(json_to_val(err_val)?))
            };
            return Ok(Val::Result(Err(inner_val)));
        }
    }

    // secondly, we check for `Variant` pattern
    // Variant must have a "tag" key and optionally a "val" key
    if obj.contains_key("tag") {
        if let Some(Value::String(tag)) = obj.get("tag") {
            match obj.len() {
                1 => {
                    // {"tag": "empty-case"}
                    return Ok(Val::Variant(tag.clone(), None));
                }
                2 => {
                    if let Some(val) = obj.get("val") {
                        // {"tag": "with-payload", "val": 42}
                        return Ok(Val::Variant(tag.clone(), Some(Box::new(json_to_val(val)?))));
                    }
                }
                _ => {
                    // if it has "tag" and one other key that's not "val", fall through to Record
                }
            }
        }
    }

    // thirdly, we check for `Option` by it's discriminator
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
        // fourthly, we check for `Tuple` pattern by its discriminator
        if let Some(Value::Array(tuple_arr)) = obj.get("__tuple") {
            let mut items = Vec::new();
            for item in tuple_arr {
                items.push(json_to_val(item)?);
            }
            return Ok(Val::Tuple(items));
        }

        // fifthly, we check for `Enum` pattern by its discriminator
        if let Some(Value::String(enum_value)) = obj.get("__enum") {
            return Ok(Val::Enum(enum_value.clone()));
        }

        // Then we check for `Resource` pattern by its discriminator
        if let Some(Value::String(_resource_desc)) = obj.get("__resource") {
            return Err(ValError::ResourceError);
        }

        // lastly, we check for `Flags` pattern by its discriminator
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

    // if we reach here, we assume it's a Record
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
            let mut obj = Map::new();
            obj.insert("tag".to_string(), Value::String(tag.clone()));
            if let Some(val_box) = payload {
                obj.insert("val".to_string(), val_to_json(val_box));
            }
            Value::Object(obj)
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
            let mut obj = Map::new();
            obj.insert(
                "ok".to_string(),
                match opt_box {
                    Some(v) => val_to_json(v),
                    None => Value::Null,
                },
            );
            Value::Object(obj)
        }
        Val::Result(Err(opt_box)) => {
            let mut obj = Map::new();
            obj.insert(
                "err".to_string(),
                match opt_box {
                    Some(v) => val_to_json(v),
                    None => Value::Null,
                },
            );
            Value::Object(obj)
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
        // Test that Options and Variants no longer conflict

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

        // Variant patterns (should still work)
        let variant_empty = json!({"tag": "empty"});
        let variant_with_val = json!({"tag": "data", "val": 42});

        assert!(matches!(
            json_to_val(&variant_empty).unwrap(),
            Val::Variant(_, None)
        ));
        assert!(matches!(
            json_to_val(&variant_with_val).unwrap(),
            Val::Variant(_, Some(_))
        ));

        // Even variants that happen to use "None" or "Some" as tag names should work
        let variant_none_tag = json!({"tag": "None"});
        let variant_some_tag = json!({"tag": "Some", "val": "test"});

        assert!(
            matches!(json_to_val(&variant_none_tag).unwrap(), Val::Variant(tag, None) if tag == "None")
        );
        assert!(
            matches!(json_to_val(&variant_some_tag).unwrap(), Val::Variant(tag, Some(_)) if tag == "Some")
        );
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
}
