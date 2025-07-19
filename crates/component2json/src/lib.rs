#![doc = include_str!("../README.md")]

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

/// Given a component and a wasmtime engine, return a full JSON schema of the component's exports.
///
/// The `output` parameter determines whether to include the output schema for functions.
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

/// Converts a slice of component model [`Val`] objects into a JSON representation.
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

/// Converts a JSON object to a vector of `Val` objects based on the provided type mappings for each
/// field.
pub fn json_to_vals(value: &Value, types: &[(String, Type)]) -> Result<Vec<Val>, ValError> {
    match value {
        Value::Object(obj) => {
            let mut results = Vec::new();
            for (name, ty) in types {
                let value = obj.get(name).ok_or_else(|| {
                    ValError::ShapeError("object", format!("missing field {name}"))
                })?;
                results.push(json_to_val(value, ty)?);
            }
            Ok(results)
        }
        _ => Err(ValError::ShapeError(
            "object",
            format!("expected object, got {value:?}"),
        )),
    }
}

/// Prepares a placeholder `Vec<Val>` to receive the results of a component function call.
/// The vector will have the correct length and correctly-typed (but empty/zeroed) values.
pub fn create_placeholder_results(results: &[Type]) -> Vec<Val> {
    results.iter().map(default_val_for_type).collect()
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
            let items: Vec<Value> = tup.types().map(|ty| type_to_json_schema(&ty)).collect();
            json!({
                "type": "array",
                "prefixItems": items,
                "minItems": items.len(),
                "maxItems": items.len()
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
            let names: Vec<&str> = enum_handle.names().collect();
            json!({
                "type": "string",
                "enum": names
            })
        }

        Type::Option(opt_handle) => {
            let inner_schema = type_to_json_schema(&opt_handle.ty());
            json!({
                "anyOf": [
                    { "type": "null" },
                    inner_schema
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
            let mut props = serde_json::Map::new();
            for name in flags_handle.names() {
                props.insert(name.to_string(), json!({"type":"boolean"}));
            }
            json!({
                "type": "object",
                "properties": props
            })
        }

        Type::Own(r) => {
            json!({
                "type": "string",
                "description": format!("own'd resource: {:?}", r)
            })
        }
        Type::Borrow(r) => {
            json!({
                "type": "string",
                "description": format!("borrow'd resource: {:?}", r)
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
                format!("{prefix}.{export_name}")
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
        Val::Tuple(items) => Value::Array(items.iter().map(val_to_json).collect()),

        Val::Variant(tag, payload) => {
            let mut obj = Map::new();
            obj.insert("tag".to_string(), Value::String(tag.clone()));
            if let Some(val_box) = payload {
                obj.insert("val".to_string(), val_to_json(val_box));
            }
            Value::Object(obj)
        }
        Val::Enum(s) => Value::String(s.clone()),

        Val::Option(None) => Value::Null,
        Val::Option(Some(val_box)) => val_to_json(val_box),

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

        Val::Flags(flags) => Value::Array(flags.iter().map(|f| Value::String(f.clone())).collect()),
        Val::Resource(res) => Value::String(format!("resource: {res:?}")),
    }
}

fn json_to_val(value: &Value, ty: &Type) -> Result<Val, ValError> {
    match ty {
        Type::Bool => match value {
            Value::Bool(b) => Ok(Val::Bool(*b)),
            _ => Err(ValError::ShapeError("bool", format!("{value:?}"))),
        },
        Type::S8 => match value {
            Value::Number(n) => n
                .as_i64()
                .and_then(|i| i8::try_from(i).ok())
                .map(Val::S8)
                .ok_or_else(|| ValError::NumberError(format!("{n:?}"))),
            _ => Err(ValError::ShapeError("s8", format!("{value:?}"))),
        },
        Type::S16 => match value {
            Value::Number(n) => n
                .as_i64()
                .and_then(|i| i16::try_from(i).ok())
                .map(Val::S16)
                .ok_or_else(|| ValError::NumberError(format!("{n:?}"))),
            _ => Err(ValError::ShapeError("s16", format!("{value:?}"))),
        },
        Type::S32 => match value {
            Value::Number(n) => n
                .as_i64()
                .and_then(|i| i32::try_from(i).ok())
                .map(Val::S32)
                .ok_or_else(|| ValError::NumberError(format!("{n:?}"))),
            _ => Err(ValError::ShapeError("s32", format!("{value:?}"))),
        },
        Type::S64 => match value {
            Value::Number(n) => n
                .as_i64()
                .map(Val::S64)
                .ok_or_else(|| ValError::NumberError(format!("{n:?}"))),
            _ => Err(ValError::ShapeError("s64", format!("{value:?}"))),
        },
        Type::U8 => match value {
            Value::Number(n) => n
                .as_u64()
                .and_then(|i| u8::try_from(i).ok())
                .map(Val::U8)
                .ok_or_else(|| ValError::NumberError(format!("{n:?}"))),
            _ => Err(ValError::ShapeError("u8", format!("{value:?}"))),
        },
        Type::U16 => match value {
            Value::Number(n) => n
                .as_u64()
                .and_then(|i| u16::try_from(i).ok())
                .map(Val::U16)
                .ok_or_else(|| ValError::NumberError(format!("{n:?}"))),
            _ => Err(ValError::ShapeError("u16", format!("{value:?}"))),
        },
        Type::U32 => match value {
            Value::Number(n) => n
                .as_u64()
                .and_then(|i| u32::try_from(i).ok())
                .map(Val::U32)
                .ok_or_else(|| ValError::NumberError(format!("{n:?}"))),
            _ => Err(ValError::ShapeError("u32", format!("{value:?}"))),
        },
        Type::U64 => match value {
            Value::Number(n) => n
                .as_u64()
                .map(Val::U64)
                .ok_or_else(|| ValError::NumberError(format!("{n:?}"))),
            _ => Err(ValError::ShapeError("u64", format!("{value:?}"))),
        },
        Type::Float32 => match value {
            Value::Number(n) => n
                .as_f64()
                .map(|f| Val::Float32(f as f32))
                .ok_or_else(|| ValError::NumberError(format!("{n:?}"))),
            _ => Err(ValError::ShapeError("float32", format!("{value:?}"))),
        },
        Type::Float64 => match value {
            Value::Number(n) => n
                .as_f64()
                .map(Val::Float64)
                .ok_or_else(|| ValError::NumberError(format!("{n:?}"))),
            _ => Err(ValError::ShapeError("float64", format!("{value:?}"))),
        },
        Type::Char => match value {
            Value::String(s) => {
                if s.chars().count() == 1 {
                    Ok(Val::Char(s.chars().next().unwrap()))
                } else {
                    Err(ValError::InvalidChar(s.clone()))
                }
            }
            _ => Err(ValError::ShapeError("char", format!("{value:?}"))),
        },
        Type::String => match value {
            Value::String(s) => Ok(Val::String(s.clone())),
            _ => Err(ValError::ShapeError("string", format!("{value:?}"))),
        },
        Type::List(list_handle) => match value {
            Value::Array(arr) => {
                let mut vals = Vec::new();
                for item in arr {
                    vals.push(json_to_val(item, &list_handle.ty())?);
                }
                Ok(Val::List(vals))
            }
            _ => Err(ValError::ShapeError("list", format!("{value:?}"))),
        },
        Type::Record(r) => match value {
            Value::Object(obj) => {
                let mut fields = Vec::<(String, Val)>::new();
                for field in r.fields() {
                    let value = obj.get(field.name).ok_or_else(|| {
                        ValError::ShapeError("record", format!("missing field {}", field.name))
                    })?;
                    fields.push((field.name.to_string(), json_to_val(value, &field.ty)?));
                }
                Ok(Val::Record(fields))
            }
            _ => Err(ValError::ShapeError("record", format!("{value:?}"))),
        },
        Type::Tuple(tup) => match value {
            Value::Array(arr) => {
                let types: Vec<_> = tup.types().collect();
                if arr.len() != types.len() {
                    return Err(ValError::ShapeError(
                        "tuple",
                        format!("expected {} items, got {}", types.len(), arr.len()),
                    ));
                }
                let mut items = Vec::new();
                for (value, ty) in arr.iter().zip(types) {
                    items.push(json_to_val(value, &ty)?);
                }
                Ok(Val::Tuple(items))
            }
            _ => Err(ValError::ShapeError("tuple", format!("{value:?}"))),
        },
        Type::Variant(variant_handle) => match value {
            Value::Object(obj) => {
                let tag = obj
                    .get("tag")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ValError::ShapeError("variant", "missing tag".to_string()))?;

                let case = variant_handle
                    .cases()
                    .find(|c| c.name == tag)
                    .ok_or_else(|| ValError::UnknownShape(obj.clone()))?;

                let payload = if let Some(payload_ty) = &case.ty {
                    let val = obj.get("val").ok_or_else(|| {
                        ValError::ShapeError("variant", "missing val".to_string())
                    })?;
                    Some(Box::new(json_to_val(val, payload_ty)?))
                } else {
                    None
                };

                Ok(Val::Variant(tag.to_string(), payload))
            }
            _ => Err(ValError::ShapeError("variant", format!("{value:?}"))),
        },
        Type::Enum(enum_handle) => match value {
            Value::String(s) => {
                if enum_handle.names().any(|name| name == s) {
                    Ok(Val::Enum(s.clone()))
                } else {
                    Err(ValError::ShapeError(
                        "enum",
                        format!("invalid enum value: {s}"),
                    ))
                }
            }
            _ => Err(ValError::ShapeError("enum", format!("{value:?}"))),
        },
        Type::Option(opt_handle) => match value {
            Value::Null => Ok(Val::Option(None)),
            v => Ok(Val::Option(Some(Box::new(json_to_val(
                v,
                &opt_handle.ty(),
            )?)))),
        },
        Type::Result(res_handle) => match value {
            Value::Object(obj) => {
                if let Some(ok_val) = obj.get("ok") {
                    let ok_ty = res_handle.ok().unwrap_or(Type::Bool);
                    Ok(Val::Result(Ok(Some(Box::new(json_to_val(
                        ok_val, &ok_ty,
                    )?)))))
                } else if let Some(err_val) = obj.get("err") {
                    let err_ty = res_handle.err().unwrap_or(Type::Bool);
                    Ok(Val::Result(Err(Some(Box::new(json_to_val(
                        err_val, &err_ty,
                    )?)))))
                } else {
                    Err(ValError::ShapeError("result", format!("{value:?}")))
                }
            }
            _ => Err(ValError::ShapeError("result", format!("{value:?}"))),
        },
        Type::Flags(flags_handle) => match value {
            Value::Array(arr) => {
                let mut flags = Vec::new();
                for name in flags_handle.names() {
                    if arr.iter().any(|v| v.as_str() == Some(name)) {
                        flags.push(name.to_string());
                    }
                }
                Ok(Val::Flags(flags))
            }
            _ => Err(ValError::ShapeError("flags", format!("{value:?}"))),
        },
        Type::Own(_) | Type::Borrow(_) => Err(ValError::ResourceError),
    }
}

fn default_val_for_type(ty: &Type) -> Val {
    match ty {
        Type::Bool => Val::Bool(false),
        Type::S8 => Val::S8(0),
        Type::U8 => Val::U8(0),
        Type::S16 => Val::S16(0),
        Type::U16 => Val::U16(0),
        Type::S32 => Val::S32(0),
        Type::U32 => Val::U32(0),
        Type::S64 => Val::S64(0),
        Type::U64 => Val::U64(0),
        Type::Float32 => Val::Float32(0.0),
        Type::Float64 => Val::Float64(0.0),
        Type::Char => Val::Char('\0'),
        Type::String => Val::String("".to_string()),
        Type::List(_) => Val::List(Vec::new()),

        Type::Record(r) => {
            let fields = r
                .fields()
                .map(|field| (field.name.to_string(), default_val_for_type(&field.ty)))
                .collect();
            Val::Record(fields)
        }
        Type::Tuple(t) => {
            let vals = t.types().map(|ty| default_val_for_type(&ty)).collect();
            Val::Tuple(vals)
        }
        Type::Variant(v) => {
            // pick the first case as the default
            if let Some(first_case) = v.cases().next() {
                let payload = first_case
                    .ty
                    .map(|payload_ty| Box::new(default_val_for_type(&payload_ty)));
                Val::Variant(first_case.name.to_string(), payload)
            } else {
                panic!("Cannot create a default for a variant with no cases.");
            }
        }
        Type::Enum(e) => Val::Enum(e.names().next().unwrap_or("").to_string()),
        Type::Option(_) => Val::Option(None),
        Type::Result(_) => Val::Result(Ok(None)),
        Type::Flags(_) => Val::Flags(Vec::new()),

        // Resources cannot be created from scratch. This indicates a problem.
        Type::Own(_) | Type::Borrow(_) => {
            panic!("Cannot create a placeholder for a resource type.")
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wasmtime::component::{Type, Val};

    use super::*;

    #[test]
    fn test_vals_to_json_empty() {
        let json_val = vals_to_json(&[]);
        assert_eq!(json_val, json!(null));
    }

    #[test]
    fn test_vals_to_json_single() {
        let val = Val::Bool(true);
        let json_val = vals_to_json(std::slice::from_ref(&val));
        assert_eq!(json_val, val_to_json(&val));
    }

    #[test]
    fn test_vals_to_json_multiple_values() {
        let wit_vals = vec![Val::String("example".to_string()), Val::S64(42)];
        let json_result = vals_to_json(&wit_vals);

        let obj = json_result.as_object().unwrap();
        assert_eq!(obj.get("val0").unwrap(), &json!("example"));
        assert_eq!(obj.get("val1").unwrap(), &json!(42));
    }

    #[test]
    fn test_val_to_json_bool() {
        let val = Val::Bool(false);
        assert_eq!(val_to_json(&val), json!(false));
    }

    #[test]
    fn test_val_to_json_numbers() {
        let s8 = Val::S8(-5);
        assert_eq!(val_to_json(&s8), json!(-5));

        let u8 = Val::U8(200);
        assert_eq!(val_to_json(&u8), json!(200));

        let s16 = Val::S16(-123);
        assert_eq!(val_to_json(&s16), json!(-123));

        let u16 = Val::U16(123);
        assert_eq!(val_to_json(&u16), json!(123));

        let s32 = Val::S32(-1000);
        assert_eq!(val_to_json(&s32), json!(-1000));

        let u32 = Val::U32(1000);
        assert_eq!(val_to_json(&u32), json!(1000));

        let s64 = Val::S64(-9999);
        assert_eq!(val_to_json(&s64), json!(-9999));

        let u64 = Val::U64(9999);
        assert_eq!(val_to_json(&u64), json!(9999));
    }

    #[allow(clippy::approx_constant)]
    #[test]
    fn test_val_to_json_floats() {
        let float32 = Val::Float32(3.14);
        if let Value::Number(n) = val_to_json(&float32) {
            assert!((n.as_f64().unwrap() - 3.14).abs() < 1e-6);
        } else {
            panic!("Expected a JSON number for Float32");
        }

        let float64 = Val::Float64(2.718281828);
        if let Value::Number(n) = val_to_json(&float64) {
            assert!((n.as_f64().unwrap() - 2.718281828).abs() < 1e-9);
        } else {
            panic!("Expected a JSON number for Float64");
        }
    }

    #[test]
    fn test_val_to_json_char() {
        let val = Val::Char('A');
        assert_eq!(val_to_json(&val), json!("A"));
    }

    #[test]
    fn test_val_to_json_string() {
        let val = Val::String("hello".to_string());
        assert_eq!(val_to_json(&val), json!("hello"));
    }

    #[test]
    fn test_val_to_json_list() {
        let val = Val::List(vec![Val::S64(1), Val::S64(2)]);
        assert_eq!(val_to_json(&val), json!([1, 2]));
    }

    #[test]
    fn test_val_to_json_record() {
        let val = Val::Record(vec![
            ("key1".to_string(), Val::Bool(true)),
            ("key2".to_string(), Val::String("value".to_string())),
        ]);
        let json_val = val_to_json(&val);
        let obj = json_val.as_object().unwrap();
        assert_eq!(obj.get("key1").unwrap(), &json!(true));
        assert_eq!(obj.get("key2").unwrap(), &json!("value"));
    }

    #[test]
    fn test_val_to_json_tuple() {
        let val = Val::Tuple(vec![Val::S64(42), Val::String("tuple".to_string())]);
        assert_eq!(val_to_json(&val), json!([42, "tuple"]));
    }

    #[test]
    fn test_val_to_json_variant() {
        let variant_with = Val::Variant("tag1".to_string(), Some(Box::new(Val::S64(99))));
        let json_with = val_to_json(&variant_with);
        let obj_with = json_with.as_object().unwrap();
        assert_eq!(obj_with.get("tag").unwrap(), &json!("tag1"));
        assert_eq!(obj_with.get("val").unwrap(), &json!(99));

        let variant_without = Val::Variant("tag2".to_string(), None);
        let json_without = val_to_json(&variant_without);
        let obj_without = json_without.as_object().unwrap();
        assert_eq!(obj_without.get("tag").unwrap(), &json!("tag2"));
        assert!(obj_without.get("val").is_none());
    }

    #[test]
    fn test_val_to_json_enum() {
        let val = Val::Enum("green".to_string());
        assert_eq!(val_to_json(&val), json!("green"));
    }

    #[test]
    fn test_val_to_json_option() {
        let none_option = Val::Option(None);
        assert_eq!(val_to_json(&none_option), json!(null));

        let some_option = Val::Option(Some(Box::new(Val::String("some".to_string()))));
        assert_eq!(val_to_json(&some_option), json!("some"));
    }

    #[test]
    fn test_val_to_json_result() {
        let ok_result = Val::Result(Ok(Some(Box::new(Val::String("ok".to_string())))));
        let json_ok = val_to_json(&ok_result);
        let obj_ok = json_ok.as_object().unwrap();
        assert_eq!(obj_ok.get("ok").unwrap(), &json!("ok"));

        let err_result = Val::Result(Err(Some(Box::new(Val::String("err".to_string())))));
        let json_err = val_to_json(&err_result);
        let obj_err = json_err.as_object().unwrap();
        assert_eq!(obj_err.get("err").unwrap(), &json!("err"));

        let ok_none = Val::Result(Ok(None));
        let json_ok_none = val_to_json(&ok_none);
        let obj_ok_none = json_ok_none.as_object().unwrap();
        assert_eq!(obj_ok_none.get("ok").unwrap(), &json!(null));

        let err_none = Val::Result(Err(None));
        let json_err_none = val_to_json(&err_none);
        let obj_err_none = json_err_none.as_object().unwrap();
        assert_eq!(obj_err_none.get("err").unwrap(), &json!(null));
    }

    #[test]
    fn test_val_to_json_flags() {
        let val = Val::Flags(vec!["f1".to_string(), "f2".to_string()]);
        assert_eq!(val_to_json(&val), json!(["f1", "f2"]));
    }

    #[test]
    fn test_component_exports_empty() {
        let engine = Engine::default();
        // A minimal component with no exports
        let wat = r#"(component)"#;
        let component = Component::new(&engine, wat).unwrap();
        let schema = component_exports_to_json_schema(&component, &engine, false);
        let tools = schema.get("tools").unwrap().as_array().unwrap();
        assert_eq!(tools.len(), 0);
    }

    #[test]
    fn test_root_component_exports() {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        config.async_support(true);
        let engine = Engine::new(&config).unwrap();
        let component = Component::from_file(&engine, "testdata/filesystem-rs.wasm").unwrap();
        let schema = component_exports_to_json_schema(&component, &engine, true);

        let tools = schema.get("tools").unwrap().as_array().unwrap();
        assert_eq!(tools.len(), 4);

        let expected_exports = [
            "list-directory",
            "read-file",
            "search-file",
            "get-file-info",
        ];

        for (i, tool) in tools.iter().enumerate() {
            let fully_qualified_name =
                format!("{}.{}", "component:filesystem2/fs", expected_exports[i]);
            assert_eq!(json!(tool.get("name").unwrap()), fully_qualified_name);

            let input_schema = tool.get("inputSchema").unwrap();
            let properties = input_schema.get("properties").unwrap().as_object().unwrap();
            assert!(properties.contains_key("path"));

            if expected_exports[i] == "search-file" {
                assert!(properties.contains_key("pattern"));
            }

            let output_schema = tool.get("outputSchema").unwrap();
            if expected_exports[i] == "list-directory" {
                assert!(
                    output_schema.get("oneOf").unwrap().as_array().unwrap()[0]
                        .get("properties")
                        .unwrap()
                        .get("ok")
                        .unwrap()
                        .get("type")
                        .unwrap()
                        == "array"
                );
            } else {
                assert!(
                    output_schema.get("oneOf").unwrap().as_array().unwrap()[0]
                        .get("properties")
                        .unwrap()
                        .get("ok")
                        .unwrap()
                        .get("type")
                        .unwrap()
                        == "string"
                );
            }
        }
    }

    #[test]
    fn test_generate_function_schema() {
        let engine = Engine::default();
        let wat = r#"(component
            (type (component
                (type (component
                    (type (list u8))
                    (type (tuple string 0))
                    (type (list 1))
                    (type (result 2 (error string)))
                    (type (func (param "name" string) (param "wit" 0) (result 3)))
                    (export "generate" (func (type 4)))
                ))
                (export "foo:foo/foo" (component (type 0)))
            ))
            (export "foo" (type 0))
            (@custom "package-docs" "\00{}")
            (@producers (processed-by "wit-component" "0.223.0"))
        )"#;
        let component = Component::new(&engine, wat).unwrap();
        let schema = component_exports_to_json_schema(&component, &engine, true);

        let tools = schema.get("tools").unwrap().as_array().unwrap();
        assert_eq!(tools.len(), 1);

        let generate_tool = &tools[0];
        assert_eq!(generate_tool.get("name").unwrap(), "foo:foo/foo.generate");

        let input_schema = generate_tool.get("inputSchema").unwrap();
        let properties = input_schema.get("properties").unwrap().as_object().unwrap();
        assert!(properties.contains_key("name"));
        assert!(properties.contains_key("wit"));

        let output_schema = generate_tool.get("outputSchema").unwrap();
        assert!(output_schema.get("oneOf").is_some());
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
        assert_eq!(output_schema.get("type").unwrap(), "array");
        assert_eq!(output_schema.get("minItems").unwrap(), 4);
        assert_eq!(output_schema.get("maxItems").unwrap(), 4);
        let prefix_items = output_schema
            .get("prefixItems")
            .unwrap()
            .as_array()
            .unwrap();
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
                    .get("tag")
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
                    .get("tag")
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
                    .get("tag")
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

    #[test]
    fn test_wit_to_json_conversions() {
        let wit_bool = Val::Bool(false);
        assert_eq!(val_to_json(&wit_bool), json!(false));

        let wit_string = Val::String("test".to_string());
        assert_eq!(val_to_json(&wit_string), json!("test"));

        let wit_list = Val::List(vec![Val::S64(1), Val::S64(2)]);
        assert_eq!(val_to_json(&wit_list), json!([1, 2]));

        let wit_record = Val::Record(vec![
            ("key1".to_string(), Val::Bool(true)),
            ("key2".to_string(), Val::String("value".to_string())),
        ]);
        assert_eq!(
            val_to_json(&wit_record),
            json!({
                "key1": true,
                "key2": "value"
            })
        );

        let wit_option_none = Val::Option(None);
        assert_eq!(val_to_json(&wit_option_none), json!(null));
        let wit_option_some = Val::Option(Some(Box::new(Val::String("some".to_string()))));
        assert_eq!(val_to_json(&wit_option_some), json!("some"));

        let wit_result_ok = Val::Result(Ok(Some(Box::new(Val::String("success".to_string())))));
        assert_eq!(val_to_json(&wit_result_ok), json!({"ok": "success"}));
        let wit_result_err = Val::Result(Err(Some(Box::new(Val::String("error".to_string())))));
        assert_eq!(val_to_json(&wit_result_err), json!({"err": "error"}));
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
    fn test_json_to_eval() {
        let bool_ty = Type::Bool;
        let bool_val = json!(true);
        assert!(matches!(
            json_to_val(&bool_val, &bool_ty).unwrap(),
            Val::Bool(true)
        ));

        let s8_ty = Type::S8;
        let s8_val = json!(42);
        assert!(matches!(json_to_val(&s8_val, &s8_ty).unwrap(), Val::S8(42)));

        let string_ty = Type::String;
        let string_val = json!("hello");
        assert!(matches!(
            json_to_val(&string_val, &string_ty).unwrap(),
            Val::String(s) if s == "hello"
        ));
    }

    #[test]
    fn test_json_to_vals() {
        let types = vec![
            ("name".to_string(), Type::String),
            ("age".to_string(), Type::S32),
        ];
        let value = json!({
            "name": "John",
            "age": 30
        });
        let vals = json_to_vals(&value, &types).unwrap();
        assert_eq!(vals.len(), 2);
        assert!(matches!(&vals[0], Val::String(s) if s == "John"));
        assert!(matches!(&vals[1], Val::S32(30)));
    }

    #[test]
    fn test_json_to_val_errors() {
        let bool_ty = Type::Bool;
        let string_val = json!("true");
        assert!(json_to_val(&string_val, &bool_ty).is_err());

        let s8_ty = Type::S8;
        let overflow_val = json!(1000);
        assert!(json_to_val(&overflow_val, &s8_ty).is_err());
    }

    #[test]
    fn test_json_to_vals_errors() {
        let types = vec![
            ("name".to_string(), Type::String),
            ("age".to_string(), Type::S32),
        ];
        let missing_field = json!({"name": "John"});
        assert!(json_to_vals(&missing_field, &types).is_err());

        let invalid_type = json!({
            "name": "John",
            "age": "30"
        });
        assert!(json_to_vals(&invalid_type, &types).is_err());
    }

    #[test]
    fn test_roundtrip() {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        let engine = Engine::new(&config).unwrap();

        // A component that directly exports the types we want to test.
        // This is simpler and more direct than defining/importing a function.
        let wat = r#"
        (component
  (type (;0;)
    (component
      (type (;0;)
        (instance
          (type (;0;) (record (field "name" string) (field "value" u32)))
          (export (;1;) "r" (type (eq 0)))
          (type (;2;) (variant (case "u" u64) (case "s" string)))
          (export (;3;) "v" (type (eq 2)))
          (type (;4;) (tuple s32 bool))
          (export (;5;) "t" (type (eq 4)))
          (type (;6;) (enum "cat" "dog"))
          (export (;7;) "e" (type (eq 6)))
          (type (;8;) (option 1))
          (export (;9;) "o" (type (eq 8)))
          (type (;10;) (result 3 (error string)))
          (export (;11;) "res" (type (eq 10)))
          (type (;12;) (flags "read" "write"))
          (export (;13;) "f" (type (eq 12)))
          (type (;14;) (list 5))
          (export (;15;) "l" (type (eq 14)))
        )
      )
      (export (;0;) "wassette:test/types@0.1.0" (instance (type 0)))
    )
  )
  (export (;1;) "types" (type 0))
  (type (;2;)
    (component
      (type (;0;)
        (component
          (type (;0;)
            (instance
              (type (;0;) (record (field "name" string) (field "value" u32)))
              (export (;1;) "r" (type (eq 0)))
              (type (;2;) (variant (case "u" u64) (case "s" string)))
              (export (;3;) "v" (type (eq 2)))
              (type (;4;) (tuple s32 bool))
              (export (;5;) "t" (type (eq 4)))
              (type (;6;) (enum "cat" "dog"))
              (export (;7;) "e" (type (eq 6)))
              (type (;8;) (option 1))
              (export (;9;) "o" (type (eq 8)))
              (type (;10;) (result 3 (error string)))
              (export (;11;) "res" (type (eq 10)))
              (type (;12;) (flags "read" "write"))
              (export (;13;) "f" (type (eq 12)))
              (type (;14;) (list 5))
              (export (;15;) "l" (type (eq 14)))
            )
          )
          (import "wassette:test/types@0.1.0" (instance (;0;) (type 0)))
          (type (;1;)
            (instance
              (type (;0;) (record (field "name" string) (field "value" u32)))
              (export (;1;) "r" (type (eq 0)))
              (type (;2;) (variant (case "u" u64) (case "s" string)))
              (export (;3;) "v" (type (eq 2)))
              (type (;4;) (tuple s32 bool))
              (export (;5;) "t" (type (eq 4)))
              (type (;6;) (enum "cat" "dog"))
              (export (;7;) "e" (type (eq 6)))
              (type (;8;) (option 1))
              (export (;9;) "o" (type (eq 8)))
              (type (;10;) (result 3 (error string)))
              (export (;11;) "res" (type (eq 10)))
              (type (;12;) (flags "read" "write"))
              (export (;13;) "f" (type (eq 12)))
              (type (;14;) (list 5))
              (export (;15;) "l" (type (eq 14)))
            )
          )
          (export (;1;) "wassette:test/types@0.1.0" (instance (type 1)))
        )
      )
      (export (;0;) "wassette:test/tests@0.1.0" (component (type 0)))
    )
  )
  (export (;3;) "tests" (type 2))
)
        "#;
        let component = Component::new(&engine, wat).unwrap();
        let component_type = component.component_type();

        let types_component_export = component_type.get_export(&engine, "types").unwrap();
        let types_component = match types_component_export {
            wasmtime::component::types::ComponentItem::Component(c) => c,
            _ => panic!("Expected 'types' to be a component export"),
        };

        let types_instance_export = types_component
            .get_export(&engine, "wassette:test/types@0.1.0")
            .unwrap();
        let types_instance = match types_instance_export {
            wasmtime::component::types::ComponentItem::ComponentInstance(i) => i,
            _ => panic!("Expected an instance export"),
        };

        let get_exported_type = |name: &str| match types_instance.get_export(&engine, name).unwrap()
        {
            wasmtime::component::types::ComponentItem::Type(ty) => ty,
            _ => panic!("Expected a type export for '{name}'"),
        };

        let record_type = get_exported_type("r");
        let original_record = Val::Record(vec![
            ("name".to_string(), Val::String("alpha".to_string())),
            ("value".to_string(), Val::U32(101)),
        ]);
        let json_record = val_to_json(&original_record);
        let roundtrip_record = json_to_val(&json_record, &record_type).unwrap();
        assert_eq!(original_record, roundtrip_record);

        let variant_type = get_exported_type("v");
        let original_variant = Val::Variant(
            "s".to_string(),
            Some(Box::new(Val::String("beta".to_string()))),
        );
        let json_variant = val_to_json(&original_variant);
        let roundtrip_variant = json_to_val(&json_variant, &variant_type).unwrap();
        assert_eq!(original_variant, roundtrip_variant);

        let tuple_type = get_exported_type("t");
        let original_tuple = Val::Tuple(vec![Val::S32(-42), Val::Bool(true)]);
        let json_tuple = val_to_json(&original_tuple);
        let roundtrip_tuple = json_to_val(&json_tuple, &tuple_type).unwrap();
        assert_eq!(original_tuple, roundtrip_tuple);

        let enum_type = get_exported_type("e");
        let original_enum = Val::Enum("dog".to_string());
        let json_enum = val_to_json(&original_enum);
        let roundtrip_enum = json_to_val(&json_enum, &enum_type).unwrap();
        assert_eq!(original_enum, roundtrip_enum);

        let option_type = get_exported_type("o");
        let inner_val = Val::Record(vec![
            ("name".to_string(), Val::String("gamma".to_string())),
            ("value".to_string(), Val::U32(202)),
        ]);
        let original_some = Val::Option(Some(Box::new(inner_val.clone())));
        let json_inner = val_to_json(&inner_val);
        let roundtrip_some = json_to_val(&json_inner, &option_type).unwrap();
        assert_eq!(original_some, roundtrip_some);

        let result_type = get_exported_type("res");
        let ok_inner = Val::Variant("u".to_string(), Some(Box::new(Val::U64(303))));
        let original_ok = Val::Result(Ok(Some(Box::new(ok_inner))));
        let json_ok = val_to_json(&original_ok);
        let roundtrip_ok = json_to_val(&json_ok, &result_type).unwrap();
        assert_eq!(original_ok, roundtrip_ok);

        let flags_type = get_exported_type("f");
        let original_flags = Val::Flags(vec!["read".to_string(), "write".to_string()]);
        let json_flags = val_to_json(&original_flags);
        let roundtrip_flags = json_to_val(&json_flags, &flags_type).unwrap();
        assert_eq!(original_flags, roundtrip_flags);

        let list_type = get_exported_type("l");
        let original_list = Val::List(vec![
            Val::Tuple(vec![Val::S32(1), Val::Bool(true)]),
            Val::Tuple(vec![Val::S32(2), Val::Bool(false)]),
        ]);
        let json_list = val_to_json(&original_list);
        let roundtrip_list = json_to_val(&json_list, &list_type).unwrap();
        assert_eq!(original_list, roundtrip_list);
    }
}
