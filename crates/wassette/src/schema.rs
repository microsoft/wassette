//! Helpers for canonicalizing tool output schemas and aligning structured results.

use serde_json::{Map, Value};

/// Canonicalize a tool output schema so that it always represents structured
/// data as an object with a required `result` property.
pub fn canonicalize_output_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            if map.get("type").and_then(|v| v.as_str()) == Some("object") {
                if let Some(result_schema) = extract_result_schema(schema) {
                    let mut outer = map.clone();
                    let mut props = map
                        .get("properties")
                        .and_then(|v| v.as_object())
                        .cloned()
                        .unwrap_or_default();
                    props.insert(
                        "result".to_string(),
                        canonicalize_result_schema(result_schema),
                    );
                    outer.insert("properties".to_string(), Value::Object(props));
                    ensure_result_required(outer)
                } else {
                    wrap_schema_in_result(canonicalize_result_schema(schema))
                }
            } else {
                wrap_schema_in_result(canonicalize_result_schema(schema))
            }
        }
        _ => wrap_schema_in_result(canonicalize_result_schema(schema)),
    }
}

/// Ensure a structured result value matches the canonical schema form.
pub fn ensure_structured_result(schema: &Value, structured_value: Value) -> Value {
    let Some(result_schema) = extract_result_schema(schema) else {
        return structured_value;
    };

    match structured_value {
        Value::Object(mut obj) => {
            if let Some(result_value) = obj.remove("result") {
                let normalized = normalize_result_value(result_schema, result_value);
                obj.insert("result".to_string(), normalized);
                Value::Object(obj)
            } else {
                let normalized = normalize_result_value(result_schema, Value::Object(obj));
                let mut wrapper = Map::new();
                wrapper.insert("result".to_string(), normalized);
                Value::Object(wrapper)
            }
        }
        other => {
            let normalized = normalize_result_value(result_schema, other);
            let mut wrapper = Map::new();
            wrapper.insert("result".to_string(), normalized);
            Value::Object(wrapper)
        }
    }
}

/// Wrap an inner schema inside the canonical `{ "result": ... }` envelope.
pub fn wrap_schema_in_result(schema: Value) -> Value {
    build_result_wrapper(schema)
}

fn canonicalize_result_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            if map.get("type").and_then(|v| v.as_str()) == Some("array") {
                if let Some(items) = map.get("items") {
                    if let Some(items_arr) = items.as_array() {
                        return tuple_items_to_object_schema(items_arr);
                    }
                }
            }

            if map.get("type").and_then(|v| v.as_str()) == Some("object") {
                let mut normalized = map.clone();
                if let Some(props) = map.get("properties").and_then(|v| v.as_object()) {
                    let mut new_props = Map::new();
                    for (k, v) in props {
                        new_props.insert(k.clone(), canonicalize_result_schema(v));
                    }
                    normalized.insert("properties".to_string(), Value::Object(new_props));
                }
                Value::Object(normalized)
            } else {
                Value::Object(map.clone())
            }
        }
        Value::Array(items) => tuple_items_to_object_schema(items),
        _ => schema.clone(),
    }
}

fn tuple_items_to_object_schema(items: &[Value]) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    for (idx, item) in items.iter().enumerate() {
        let key = format!("val{idx}");
        props.insert(key.clone(), canonicalize_result_schema(item));
        required.push(Value::String(key));
    }

    let mut map = Map::new();
    map.insert("type".to_string(), Value::String("object".to_string()));
    map.insert("properties".to_string(), Value::Object(props));
    map.insert("required".to_string(), Value::Array(required));
    Value::Object(map)
}

fn extract_result_schema(schema: &Value) -> Option<&Value> {
    schema
        .get("properties")
        .and_then(|v| v.as_object())
        .and_then(|props| props.get("result"))
}

fn ensure_result_required(mut outer: Map<String, Value>) -> Value {
    let mut required = outer
        .get("required")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let has_result = required.iter().any(|v| v.as_str() == Some("result"));
    if !has_result {
        required.push(Value::String("result".to_string()));
    }

    outer.insert("required".to_string(), Value::Array(required));
    Value::Object(outer)
}

fn build_result_wrapper(result_schema: Value) -> Value {
    let mut props = Map::new();
    props.insert("result".to_string(), result_schema);

    let mut wrapped = Map::new();
    wrapped.insert("type".to_string(), Value::String("object".to_string()));
    wrapped.insert("properties".to_string(), Value::Object(props));
    wrapped.insert(
        "required".to_string(),
        Value::Array(vec![Value::String("result".to_string())]),
    );
    Value::Object(wrapped)
}

fn normalize_result_value(schema: &Value, value: Value) -> Value {
    match schema.get("type").and_then(|v| v.as_str()) {
        Some("object") => {
            if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
                if props.keys().all(|k| k.starts_with("val")) {
                    match value {
                        Value::Array(items) => {
                            let mut map = Map::new();
                            for (idx, item) in items.into_iter().enumerate() {
                                map.insert(format!("val{idx}"), item);
                            }
                            Value::Object(map)
                        }
                        Value::Object(map) => Value::Object(map),
                        other => {
                            let mut map = Map::new();
                            map.insert("val0".to_string(), other);
                            Value::Object(map)
                        }
                    }
                } else {
                    match value {
                        Value::Object(mut obj) => {
                            let mut normalized = Map::new();
                            for (key, prop_schema) in props {
                                if let Some(val) = obj.remove(key) {
                                    normalized.insert(
                                        key.clone(),
                                        normalize_result_value(prop_schema, val),
                                    );
                                } else {
                                    normalized.insert(key.clone(), Value::Null);
                                }
                            }
                            for (key, val) in obj {
                                normalized.insert(key, val);
                            }
                            Value::Object(normalized)
                        }
                        other => other,
                    }
                }
            } else {
                value
            }
        }
        Some("array") => match value {
            Value::Object(map) if looks_like_tuple_keys(&map) => {
                let mut arr = Vec::new();
                let mut idx = 0;
                while let Some(item) = map.get(&format!("val{idx}")) {
                    arr.push(item.clone());
                    idx += 1;
                }
                Value::Array(arr)
            }
            other => other,
        },
        _ => value,
    }
}

fn looks_like_tuple_keys(map: &Map<String, Value>) -> bool {
    if map.is_empty() {
        return false;
    }
    let mut idx = 0;
    while map.contains_key(&format!("val{idx}")) {
        idx += 1;
    }
    idx > 0 && map.len() == idx
}
