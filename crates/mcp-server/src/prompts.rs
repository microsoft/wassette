// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use anyhow::Result;
use rmcp::model::{
    GetPromptRequestParam, GetPromptResult, ListPromptsResult, Prompt, PromptArgument,
    PromptMessage, PromptMessageRole,
};

// Include the prompt templates generated at build time
include!(concat!(env!("OUT_DIR"), "/prompt_templates.rs"));

/// Get the list of available prompts
pub async fn handle_prompts_list(_req: serde_json::Value) -> Result<serde_json::Value> {
    let response = ListPromptsResult {
        prompts: get_available_prompts(),
        next_cursor: None,
    };
    Ok(serde_json::to_value(response)?)
}

/// Get a specific prompt by name
pub async fn handle_prompts_get(req: serde_json::Value) -> Result<serde_json::Value> {
    let parsed_req: GetPromptRequestParam = serde_json::from_value(req)?;

    let prompt_name = parsed_req.name.as_str();
    let arguments = parsed_req.arguments.unwrap_or_default();

    let result = match prompt_name {
        "build-rust-component" => build_rust_component_prompt(arguments)?,
        "build-javascript-component" => build_javascript_component_prompt(arguments)?,
        _ => {
            return Err(anyhow::anyhow!("Unknown prompt: {}", prompt_name));
        }
    };

    Ok(serde_json::to_value(result)?)
}

/// Returns the list of available prompts
fn get_available_prompts() -> Vec<Prompt> {
    vec![
        Prompt::new(
            "build-rust-component",
            Some("Guide to building a WebAssembly component for Wassette using Rust"),
            Some(vec![PromptArgument {
                name: "component_name".to_string(),
                description: Some("The name of the component to build".to_string()),
                required: Some(false),
            }]),
        ),
        Prompt::new(
            "build-javascript-component",
            Some("Guide to building a WebAssembly component for Wassette using JavaScript"),
            Some(vec![PromptArgument {
                name: "component_name".to_string(),
                description: Some("The name of the component to build".to_string()),
                required: Some(false),
            }]),
        ),
    ]
}

/// Generate the Rust component building prompt
fn build_rust_component_prompt(
    arguments: serde_json::Map<String, serde_json::Value>,
) -> Result<GetPromptResult> {
    let component_name = arguments
        .get("component_name")
        .and_then(|v| v.as_str())
        .unwrap_or("my-component");

    let content = RUST_COMPONENT_TEMPLATE.replace("{component_name}", component_name);

    Ok(GetPromptResult {
        description: Some(format!(
            "A step-by-step guide to building a Rust WebAssembly component named '{}'",
            component_name
        )),
        messages: vec![PromptMessage::new_text(PromptMessageRole::User, content)],
    })
}

/// Generate the JavaScript component building prompt
fn build_javascript_component_prompt(
    arguments: serde_json::Map<String, serde_json::Value>,
) -> Result<GetPromptResult> {
    let component_name = arguments
        .get("component_name")
        .and_then(|v| v.as_str())
        .unwrap_or("my-component");

    let content = JAVASCRIPT_COMPONENT_TEMPLATE.replace("{component_name}", component_name);

    Ok(GetPromptResult {
        description: Some(format!(
            "A step-by-step guide to building a JavaScript WebAssembly component named '{}'",
            component_name
        )),
        messages: vec![PromptMessage::new_text(PromptMessageRole::User, content)],
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn test_handle_prompts_list() {
        let result = handle_prompts_list(json!(null)).await.unwrap();
        let list_result: ListPromptsResult = serde_json::from_value(result).unwrap();

        assert_eq!(list_result.prompts.len(), 2);
        assert_eq!(list_result.prompts[0].name, "build-rust-component");
        assert_eq!(list_result.prompts[1].name, "build-javascript-component");

        // Check that all prompts have descriptions
        for prompt in &list_result.prompts {
            assert!(prompt.description.is_some());
            assert!(prompt.arguments.is_some());
        }
    }

    #[tokio::test]
    async fn test_handle_prompts_get_rust() {
        let req = json!({
            "name": "build-rust-component",
            "arguments": {
                "component_name": "test-component"
            }
        });

        let result = handle_prompts_get(req).await.unwrap();
        let get_result: GetPromptResult = serde_json::from_value(result).unwrap();

        assert!(get_result.description.is_some());
        assert!(get_result.description.unwrap().contains("test-component"));
        assert_eq!(get_result.messages.len(), 1);
        assert_eq!(get_result.messages[0].role, PromptMessageRole::User);

        // Check content includes expected sections
        if let PromptMessageRole::User = get_result.messages[0].role {
            let content_text = match &get_result.messages[0].content {
                rmcp::model::PromptMessageContent::Text { text } => text,
                _ => panic!("Expected text content"),
            };
            assert!(content_text.contains("Building a Rust WebAssembly Component"));
            assert!(content_text.contains("test-component"));
            assert!(content_text.contains("cargo build"));
            assert!(content_text.contains("wasm32-wasip2"));
        }
    }

    #[tokio::test]
    async fn test_handle_prompts_get_javascript() {
        let req = json!({
            "name": "build-javascript-component",
            "arguments": {
                "component_name": "js-tool"
            }
        });

        let result = handle_prompts_get(req).await.unwrap();
        let get_result: GetPromptResult = serde_json::from_value(result).unwrap();

        assert!(get_result.description.is_some());
        assert!(get_result.description.unwrap().contains("js-tool"));
        assert_eq!(get_result.messages.len(), 1);

        let content_text = match &get_result.messages[0].content {
            rmcp::model::PromptMessageContent::Text { text } => text,
            _ => panic!("Expected text content"),
        };
        assert!(content_text.contains("Building a JavaScript WebAssembly Component"));
        assert!(content_text.contains("js-tool"));
        assert!(content_text.contains("jco componentize"));
    }

    #[tokio::test]
    async fn test_handle_prompts_get_default_component_name() {
        let req = json!({
            "name": "build-rust-component"
        });

        let result = handle_prompts_get(req).await.unwrap();
        let get_result: GetPromptResult = serde_json::from_value(result).unwrap();

        let content_text = match &get_result.messages[0].content {
            rmcp::model::PromptMessageContent::Text { text } => text,
            _ => panic!("Expected text content"),
        };
        // Should use default "my-component" when no argument provided
        assert!(content_text.contains("my-component"));
    }

    #[tokio::test]
    async fn test_handle_prompts_get_unknown_prompt() {
        let req = json!({
            "name": "unknown-prompt"
        });

        let result = handle_prompts_get(req).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown prompt: unknown-prompt"));
    }
}
