// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use anyhow::Result;
use rmcp::model::{ListPromptsRequest, ListPromptsResult};

pub async fn handle_prompts_list(req: serde_json::Value) -> Result<serde_json::Value> {
    let _parsed_req: ListPromptsRequest = serde_json::from_value(req)?;
    let response = ListPromptsResult {
        prompts: vec![],
        next_cursor: None,
    };
    Ok(serde_json::to_value(response)?)
}
