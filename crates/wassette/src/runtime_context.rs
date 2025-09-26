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
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        config.async_support(true);

        let engine = Arc::new(Engine::new(&config)?);

        let mut linker = Linker::new(engine.as_ref());
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
        wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;
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
