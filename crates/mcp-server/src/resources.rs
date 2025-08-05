// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use anyhow::Result;
use rmcp::model::{ListResourcesRequest, ListResourcesResult};

pub async fn handle_resources_list(req: serde_json::Value) -> Result<serde_json::Value> {
    let _parsed_req: ListResourcesRequest = serde_json::from_value(req)?;
    let response = ListResourcesResult {
        resources: vec![],
        next_cursor: None,
    };
    Ok(serde_json::to_value(response)?)
}
