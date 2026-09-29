// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Protocol-neutral presentation of tool results using Wassette's existing conventions.

use anyhow::Result;
use serde_json::Value;

use crate::schema::{canonicalize_output_schema, ensure_structured_result};

/// Text and optional structured content for a tool's returned value.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolPresentation {
    /// Display text, without quotes around string values.
    pub text: String,
    /// A value aligned with the tool's canonical output schema, when provided.
    pub structured: Option<Value>,
}

/// Present a raw tool result using the existing text and structured-output conventions.
///
/// Non-JSON results are treated as plain text. An object containing only `result`
/// is unwrapped for display, while structured content retains the canonical
/// envelope. Missing or null output schemas omit structured content.
///
/// This does not validate JSON Schema or classify guest-returned WIT `err` values
/// as execution failures.
pub fn present_tool_output(
    raw_result: &str,
    output_schema: Option<&Value>,
) -> Result<ToolPresentation> {
    let parsed =
        serde_json::from_str(raw_result).unwrap_or_else(|_| Value::String(raw_result.to_string()));
    let display = match &parsed {
        Value::Object(object) if object.len() == 1 => object.get("result").unwrap_or(&parsed),
        _ => &parsed,
    };
    let text = match display {
        Value::String(text) => text.clone(),
        _ => serde_json::to_string(display)?,
    };
    let structured = output_schema
        .filter(|schema| !schema.is_null())
        .map(|schema| ensure_structured_result(&canonicalize_output_schema(schema), parsed));

    Ok(ToolPresentation { text, structured })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn preserves_display_text() -> Result<()> {
        let cases = [
            ("plain text", "plain text"),
            ("", ""),
            ("{not json", "{not json"),
            (r#""quoted\ntext""#, "quoted\ntext"),
            ("null", "null"),
            ("true", "true"),
            ("42", "42"),
            (r#"{"result":"hello"}"#, "hello"),
            (r#"{"result":null}"#, "null"),
            (r#"{"result":[1,2]}"#, "[1,2]"),
            (r#"{"result":{"answer":42}}"#, r#"{"answer":42}"#),
            (r#"{"value":"hello"}"#, r#"{"value":"hello"}"#),
            (
                r#"{"result":"hello","extra":true}"#,
                r#"{"extra":true,"result":"hello"}"#,
            ),
        ];

        for (raw, expected_text) in cases {
            let output = present_tool_output(raw, None)?;
            assert_eq!(output.text, expected_text, "{raw}");
            assert_eq!(output.structured, None, "{raw}");
        }
        Ok(())
    }

    #[test]
    fn omits_structured_content_for_null_schema() -> Result<()> {
        let output = present_tool_output(r#"{"result":"hello"}"#, Some(&Value::Null))?;
        assert_eq!(output.text, "hello");
        assert_eq!(output.structured, None);
        Ok(())
    }

    #[test]
    fn aligns_structured_content_with_canonical_schema() -> Result<()> {
        let cases = [
            (
                json!({"type": "string"}),
                "plain text",
                json!({"result": "plain text"}),
            ),
            (
                json!({"type": "string"}),
                r#"{"result":"hello"}"#,
                json!({"result": "hello"}),
            ),
            (json!({"type": "null"}), "null", json!({"result": null})),
            (
                json!({"type": "array", "items": {"type": "number"}}),
                "[1,2]",
                json!({"result": [1, 2]}),
            ),
            (
                json!({
                    "type": "object",
                    "properties": {"answer": {"type": "number"}}
                }),
                r#"{"answer":42}"#,
                json!({"result": {"answer": 42}}),
            ),
            (
                json!({
                    "type": "object",
                    "properties": {
                        "result": {
                            "type": "array",
                            "items": [{"type": "string"}, {"type": "number"}]
                        }
                    },
                    "required": ["result"]
                }),
                r#"{"result":["hello",7]}"#,
                json!({"result": {"val0": "hello", "val1": 7}}),
            ),
        ];

        for (schema, raw, expected) in cases {
            let output = present_tool_output(raw, Some(&schema))?;
            assert_eq!(output.structured, Some(expected), "{raw}");
        }
        Ok(())
    }

    #[test]
    fn aligns_legacy_tuple_values_without_changing_text() -> Result<()> {
        let schema = json!({
            "type": "object",
            "properties": {
                "result": {
                    "type": "object",
                    "properties": {
                        "val0": {"type": "string"},
                        "val1": {"type": "number"}
                    },
                    "required": ["val0", "val1"]
                }
            },
            "required": ["result"]
        });
        let output = present_tool_output("legacy", Some(&schema))?;
        assert_eq!(output.text, "legacy");
        assert_eq!(
            output.structured,
            Some(json!({"result": {"val0": "legacy"}}))
        );

        let output = present_tool_output(r#"["hello",7]"#, Some(&schema))?;
        assert_eq!(output.text, r#"["hello",7]"#);
        assert_eq!(
            output.structured,
            Some(json!({"result": {"val0": "hello", "val1": 7}}))
        );
        Ok(())
    }

    #[test]
    fn preserves_wit_result_values() -> Result<()> {
        let schema = json!({
            "oneOf": [
                {"type": "object", "properties": {"ok": {"type": "string"}}},
                {"type": "object", "properties": {"err": {"type": "string"}}}
            ]
        });
        for value in [json!({"ok": "answer"}), json!({"err": "guest error"})] {
            let raw = json!({"result": value}).to_string();
            let output = present_tool_output(&raw, Some(&schema))?;
            assert_eq!(output.text, value.to_string());
            assert_eq!(output.structured, Some(json!({"result": value})));
        }
        Ok(())
    }
}
