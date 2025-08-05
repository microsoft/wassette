// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::sync::Arc;

use anyhow::Result;
use wasmtime::component::Component;
use wasmtime::{Config, Engine};

#[tokio::main]
async fn main() -> Result<()> {
    // load the component from command line argument `path`
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("Usage: component2json <path>");
        std::process::exit(1);
    });

    let mut config = Config::new();
    config.wasm_component_model(true);
    config.async_support(true);
    let engine = Arc::new(Engine::new(&config)?);

    let component = Arc::new(Component::from_file(&engine, path)?);

    let schema = component2json::component_exports_to_json_schema(&component, &engine, true);
    if let Some(arr) = schema["tools"].as_array() {
        for t in arr {
            let name = t["name"].as_str().unwrap_or("<unnamed>").to_string();
            let description: Option<String> = t["description"].as_str().map(|s| s.to_string());
            let input_schema = t["inputSchema"].clone(); // already a serde_json::Value
            let output_schema = t["outputSchema"].clone(); // already a serde_json::Value

            println!("{name}, {description:?}");
            println!(
                "input schema: {}",
                serde_json::to_string_pretty(&input_schema)?
            );
            println!(
                "output schema: {}",
                serde_json::to_string_pretty(&output_schema)?
            );
        }
    }
    Ok(())
}
