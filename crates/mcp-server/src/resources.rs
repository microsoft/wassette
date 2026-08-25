// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use anyhow::Result;
use rmcp::model::ListResourcesResult;

pub async fn handle_resources_list() -> Result<serde_json::Value> {
    let response = ListResourcesResult {
        resources: vec![],
        ..Default::default()
    };
    Ok(serde_json::to_value(response)?)
}
