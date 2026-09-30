// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Support utilities for sharing Wasmtime engine and linker state across lifecycle
//! manager instances.

use std::sync::Arc;

use anyhow::Result;
use wasmtime::component::{Component, InstancePre, Linker};
use wasmtime::Engine;
use wasmtime_wasi_config::WasiConfig;

use crate::{WasiState, WassetteWasiState};

/// Encapsulates Wasmtime engine and linker setup for reuse across the lifecycle manager.
#[derive(Clone)]
pub struct RuntimeContext {
    engine: Arc<Engine>,
    linker: Arc<Linker<WassetteWasiState<WasiState>>>,
}

impl RuntimeContext {
    /// Build a runtime context with the standard configuration used by Wassette.
    pub fn initialize() -> Result<Self> {
        if rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .is_err()
        {
            tracing::debug!("Using the previously installed rustls crypto provider");
        }

        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        config.wasm_component_model_map(true);
        config.wasm_component_model_fixed_length_lists(true);

        let engine = Arc::new(Engine::new(&config)?);

        let mut linker = Linker::new(engine.as_ref());
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
        wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)?;
        wasmtime_wasi_config::add_to_linker(
            &mut linker,
            |h: &mut WassetteWasiState<WasiState>| WasiConfig::from(&h.inner.wasi_config_vars),
        )?;

        Ok(Self {
            engine,
            linker: Arc::new(linker),
        })
    }

    /// Produce a cached `InstancePre` handle for the provided component using
    /// the shared linker configuration.
    pub fn instantiate_pre(
        &self,
        component: &Component,
    ) -> wasmtime::Result<InstancePre<WassetteWasiState<WasiState>>> {
        self.linker.instantiate_pre(component)
    }
}

impl AsRef<Engine> for RuntimeContext {
    fn as_ref(&self) -> &Engine {
        self.engine.as_ref()
    }
}

impl AsRef<Linker<WassetteWasiState<WasiState>>> for RuntimeContext {
    fn as_ref(&self) -> &Linker<WassetteWasiState<WasiState>> {
        self.linker.as_ref()
    }
}

impl std::ops::Deref for RuntimeContext {
    type Target = Engine;

    fn deref(&self) -> &Self::Target {
        self.engine.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;
    use crate::LifecycleManager;

    #[tokio::test]
    async fn fixed_length_lists_load_and_execute_with_production_runtime() -> Result<()> {
        let dir = tempfile::tempdir_in(std::env::current_dir()?)?;
        let component_path = dir.path().join("fixed-list.wasm");
        std::fs::write(
            &component_path,
            r#"(component
                (core module $m
                    (memory (export "memory") 1)
                    (func (export "reverse") (param i32 i32 i32) (result i32)
                        (i32.store8 (i32.const 0) (local.get 2))
                        (i32.store8 (i32.const 1) (local.get 1))
                        (i32.store8 (i32.const 2) (local.get 0))
                        (i32.const 0)))
                (core instance $i (instantiate $m))
                (type $bytes (list u8 3))
                (func (export "reverse") (param "values" $bytes) (result $bytes)
                    (canon lift (core func $i "reverse")
                        (memory (core memory $i "memory")))))"#,
        )?;
        let manager = LifecycleManager::builder(dir.path().join("components"))
            .with_secrets_dir(dir.path().join("secrets"))
            .build()
            .await?;
        let loaded = manager
            .load_component(&format!("file://{}", component_path.display()))
            .await?;
        assert_eq!(loaded.tool_names, ["reverse"]);

        let schema = manager
            .get_tool_schema_for_component(&loaded.component_id, "reverse")
            .await
            .unwrap();
        let list_schema = json!({
            "type": "array",
            "items": { "type": "number" },
            "minItems": 3,
            "maxItems": 3,
        });
        assert_eq!(schema["inputSchema"]["properties"]["values"], list_schema);
        assert_eq!(schema["outputSchema"]["properties"]["result"], list_schema);

        for (input, expected) in [
            (json!([1, 2, 3]), json!([3, 2, 1])),
            (json!([0, 128, 255]), json!([255, 128, 0])),
        ] {
            let output = manager
                .execute_component_call(
                    &loaded.component_id,
                    "reverse",
                    &json!({ "values": input }).to_string(),
                )
                .await?;
            assert_eq!(
                serde_json::from_str::<Value>(&output)?,
                json!({ "result": expected })
            );
        }

        for input in [json!([]), json!([1, 2]), json!([1, 2, 3, 4])] {
            let error = manager
                .execute_component_call(
                    &loaded.component_id,
                    "reverse",
                    &json!({ "values": input }).to_string(),
                )
                .await
                .unwrap_err();
            assert!(
                error.to_string().contains(&format!(
                    "expected 3 items, got {}",
                    input.as_array().unwrap().len()
                )),
                "{error:#}"
            );
        }
        Ok(())
    }
}
