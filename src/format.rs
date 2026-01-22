// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Output formatting utilities for CLI commands

use anyhow::Result;
use clap::ValueEnum;
use rmcp::model::CallToolResult;
use serde_json::{Map, Value};

/// Output format options for CLI commands
#[derive(Debug, Clone, Copy, PartialEq, ValueEnum)]
pub enum OutputFormat {
    /// JSON format
    Json,
    /// YAML format
    Yaml,
    /// Table format
    Table,
}

impl Default for OutputFormat {
    fn default() -> Self {
        Self::Json
    }
}

/// Format a JSON value as YAML string
pub fn format_as_yaml(value: &Value) -> Result<String> {
    serde_yaml::to_string(value).map_err(|e| anyhow::anyhow!("Failed to format as YAML: {}", e))
}

/// Format a JSON value as a table string
pub fn format_as_table(value: &Value) -> Result<String> {
    // Check if this is a component list output
    if let Some(obj) = value.as_object() {
        if let Some(components) = obj.get("components").and_then(|v| v.as_array()) {
            let mut table = String::new();
            table.push_str("ID                    | Tools Count\n");
            table.push_str("----------------------|-------------\n");

            for component in components {
                if let Some(comp_obj) = component.as_object() {
                    let id = comp_obj
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let tools_count = comp_obj
                        .get("tools_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    table.push_str(&format!("{id:<21} | {tools_count}\n"));
                }
            }
            return Ok(table);
        }
    }

    // Default generic table format
    let mut table = String::new();
    table.push_str("Key                   | Value\n");
    table.push_str("----------------------|--------\n");

    fn add_to_table(obj: &Map<String, Value>, table: &mut String) {
        for (key, value) in obj {
            let value_str = match value {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                Value::Null => "null".to_string(),
                Value::Array(_) => "[array]".to_string(),
                Value::Object(_) => "[object]".to_string(),
            };
            table.push_str(&format!("{key:<21} | {value_str}\n"));
        }
    }

    match value {
        Value::Object(obj) => add_to_table(obj, &mut table),
        _ => table.push_str(&format!("Value: {value}\n")),
    }

    Ok(table)
}

/// Print the result of a tool call with the specified format
pub fn print_result(result: &CallToolResult, output_format: OutputFormat) -> Result<()> {
    for content in &result.content {
        // Check if we can get text content from the annotated content
        if let Some(text_content) = content.as_text() {
            // Try to parse as JSON first
            if let Ok(json_value) = serde_json::from_str::<Value>(&text_content.text) {
                match output_format {
                    OutputFormat::Json => {
                        // Always pretty-print JSON for better readability
                        println!("{}", serde_json::to_string_pretty(&json_value)?);
                    }
                    OutputFormat::Yaml => {
                        // Convert JSON to YAML
                        println!("{}", format_as_yaml(&json_value)?);
                    }
                    OutputFormat::Table => {
                        // Format as table
                        println!("{}", format_as_table(&json_value)?);
                    }
                }
            } else {
                // If it's not JSON, just print the text
                println!("{}", text_content.text);
            }
        } else {
            // Handle other content types by serializing to JSON
            println!("Content: {}", serde_json::to_string_pretty(content)?);
        }
    }

    Ok(())
}
