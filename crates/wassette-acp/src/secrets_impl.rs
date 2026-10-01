// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Host-side implementation of `wasmcloud:secrets@2.1.0`.
//!
//! The `with:` clause on the layer's bindgen reuses the provider's
//! generated modules verbatim, so a single set of `Host` / `HostSecret`
//! impls on [`HostState`] satisfies both linkers.

use std::future::Future;
use std::sync::Arc;

use wasmtime::Engine;
use wasmtime::component::{Accessor, Component, HasSelf, Linker, Resource};

use crate::secrets::{SecretValue, SecretsError, SecretsRegistry};
use crate::state::HostState;
use crate::wasmcloud::secrets::reveal;
use crate::wasmcloud::secrets::store::{
    Host as StoreHost, HostSecret as StoreHostSecret, HostWithStore as StoreHostWithStore, Secret,
    SecretValue as WitSecretValue, SecretsError as WitSecretsError,
};

/// Host-owned payload for a `wasmcloud:secrets/store.secret` resource.
/// Stored in the per-instance `ResourceTable`; the guest only ever sees
/// the opaque handle.
pub struct SecretEntry {
    pub value: SecretValue,
}

fn map_value(v: SecretValue) -> WitSecretValue {
    match v {
        SecretValue::String(s) => WitSecretValue::String(s),
        SecretValue::Bytes(b) => WitSecretValue::Bytes(b),
    }
}

fn map_error(e: SecretsError) -> WitSecretsError {
    match e {
        SecretsError::Upstream(s) => WitSecretsError::Upstream(s),
        SecretsError::Io(s) => WitSecretsError::Io(s),
        SecretsError::NotFound => WitSecretsError::NotFound,
    }
}

impl StoreHost for HostState {}

impl<T: Send> StoreHostWithStore<T> for HasSelf<HostState> {
    fn get(
        accessor: &Accessor<T, Self>,
        key: String,
    ) -> impl Future<Output = Result<Resource<Secret>, WitSecretsError>> + Send {
        // Each component sees only its own secret store: scope the
        // lookup by the *currently executing* stage's component identity
        // (`namespace:component-name`; top of [`HostState::stage_stack`]).
        // The guest never supplies the namespace, so components can't
        // read each other's secrets.
        let (component_id, secrets) = lookup_context(accessor);
        async move {
            let value = secrets
                .resolve(&component_id, &key)
                .await
                .map_err(map_error)?;
            push_secret(accessor, value).map_err(WitSecretsError::Io)
        }
    }
}

impl StoreHostSecret for HostState {
    async fn drop(&mut self, rep: Resource<Secret>) -> wasmtime::Result<()> {
        // Re-tag back to our host type for the table delete.
        let entry: Resource<SecretEntry> = Resource::new_own(rep.rep());
        self.table.delete(entry)?;
        Ok(())
    }
}

impl reveal::Host for HostState {}

impl<T: Send> reveal::HostWithStore<T> for HasSelf<HostState> {
    fn reveal(
        accessor: &Accessor<T, Self>,
        secret: Resource<Secret>,
    ) -> impl Future<Output = WitSecretValue> + Send {
        let value = accessor.with(|mut a| {
            let entry: Resource<SecretEntry> = Resource::new_borrow(secret.rep());
            match a.get().table.get(&entry) {
                Ok(e) => map_value(e.value.clone()),
                Err(_) => WitSecretValue::String(String::new()),
            }
        });
        std::future::ready(value)
    }
}

/// Snapshot the executing stage's component id and the shared registry.
fn lookup_context<T: Send>(
    accessor: &Accessor<T, HasSelf<HostState>>,
) -> (String, Arc<SecretsRegistry>) {
    accessor.with(|mut a| {
        let state = a.get();
        (
            state.current_stage().component_id.clone(),
            state.secrets.clone(),
        )
    })
}

/// Park `value` in the resource table and hand back a guest-typed handle.
fn push_secret<T: Send>(
    accessor: &Accessor<T, HasSelf<HostState>>,
    value: SecretValue,
) -> Result<Resource<Secret>, String> {
    accessor.with(|mut a| {
        let entry = a
            .get()
            .table
            .push(SecretEntry { value })
            .map_err(|e| format!("resource table push: {e}"))?;
        // Re-tag the resource handle under the WIT-side type. Same rep,
        // different phantom type.
        Ok(Resource::new_own(entry.rep()))
    })
}

/// The interface a component imports once per statically-known secret,
/// under a label naming that secret: `import api-key: wasmcloud:secrets/secret;`.
pub const LABELED_SECRET_INTERFACE: &str = "wasmcloud:secrets/secret@2.1.0";

/// Labels of every `wasmcloud:secrets/secret` import in `component`.
pub fn labeled_secrets(engine: &Engine, component: &Component) -> Vec<String> {
    component
        .component_type()
        .imports(engine)
        .filter(|(_, import)| import.is_implements(LABELED_SECRET_INTERFACE))
        .map(|(name, _)| name.to_string())
        .collect()
}

/// Fail fast when `component_id` imports a labeled secret that its secret
/// store doesn't hold. `secret.get` can't return an error, so this is the
/// only place a missing secret can be reported cleanly.
pub async fn check_labeled_secrets(
    secrets: &SecretsRegistry,
    component_id: &str,
    labels: &[String],
) -> anyhow::Result<()> {
    for label in labels {
        match secrets.resolve(component_id, label).await {
            Ok(_) => {}
            Err(SecretsError::NotFound) => anyhow::bail!(
                "component `{component_id}` imports the secret `{label}`, but none is set; \
                 run `wassette secret set {component_id} {label}=<value>`"
            ),
            Err(e) => {
                anyhow::bail!("resolving secret `{label}` for `{component_id}`: {e:?}")
            }
        }
    }
    Ok(())
}

/// Define `get` for each labeled `wasmcloud:secrets/secret` import. The
/// label is the secret's key, scoped to the calling stage's component.
pub fn add_labeled_secrets_to_linker<'a>(
    linker: &mut Linker<HostState>,
    labels: impl IntoIterator<Item = &'a String>,
) -> anyhow::Result<()> {
    let mut seen = std::collections::HashSet::new();
    for label in labels {
        if !seen.insert(label.as_str()) {
            continue;
        }
        let key = label.clone();
        linker.instance(label)?.func_wrap_concurrent(
            "get",
            move |accessor: &Accessor<HostState>, (): ()| {
                let key = key.clone();
                Box::pin(async move {
                    let (component_id, secrets) = accessor.with(|mut a| {
                        let state = a.get();
                        (
                            state.current_stage().component_id.clone(),
                            state.secrets.clone(),
                        )
                    });
                    let value = secrets.resolve(&component_id, &key).await.map_err(|e| {
                        wasmtime::format_err!(
                            "resolving secret `{key}` for `{component_id}`: {e:?}"
                        )
                    })?;
                    let entry = accessor.with(|mut a| a.get().table.push(SecretEntry { value }))?;
                    Ok((Resource::<Secret>::new_own(entry.rep()),))
                })
            },
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{SecretsRegistry, test_binding};

    #[tokio::test]
    async fn failed_lookup_does_not_expose_other_secret_in_error() {
        const VALUE: &str = "never-include-this-secret-in-errors";
        let dir = tempfile::tempdir().unwrap();
        let registry = SecretsRegistry::new(dir.path());
        registry.register(test_binding("comp-error")).unwrap();
        let manager = wassette::SecretsManager::new(dir.path().to_path_buf());
        manager
            .set_bound_component_secrets(
                &test_binding("comp-error"),
                &[("api_key".into(), VALUE.into())],
            )
            .await
            .unwrap();

        let error = registry.resolve("comp-error", "missing").await.unwrap_err();
        assert!(matches!(error, SecretsError::NotFound));
        assert!(!format!("{error:?}").contains(VALUE));
        let guest_error = map_error(error);
        assert!(!format!("{guest_error:?}").contains(VALUE));
        assert!(!guest_error.to_string().contains(VALUE));
    }

    #[tokio::test]
    async fn invalid_secrets_file_does_not_expose_values_in_error() {
        const VALUE: &str = "never-include-this-secret-in-parse-errors";
        let dir = tempfile::tempdir().unwrap();
        let manager = wassette::SecretsManager::new(dir.path().to_path_buf());
        manager
            .set_bound_component_secrets(&test_binding("comp-invalid"), &[])
            .await
            .unwrap();
        tokio::fs::write(
            manager.get_component_secrets_path("comp-invalid"),
            format!("api_key: {VALUE}\ninvalid: [\n"),
        )
        .await
        .unwrap();

        let registry = SecretsRegistry::new(dir.path());
        registry.register(test_binding("comp-invalid")).unwrap();
        let error = registry
            .resolve("comp-invalid", "api_key")
            .await
            .unwrap_err();
        assert!(matches!(error, SecretsError::Io(_)));
        assert!(!format!("{error:?}").contains(VALUE));
        let guest_error = map_error(error);
        assert!(!format!("{guest_error:?}").contains(VALUE));
        assert!(!guest_error.to_string().contains(VALUE));
    }

    /// A component importing one labeled secret and exporting `probe`,
    /// which returns whatever `api-key.get` hands back.
    const LABELED_SECRET_COMPONENT: &str = r#"
        (component
          (import "wasmcloud:secrets/store@2.1.0" (instance $store
            (export "secret" (type (sub resource)))
          ))
          (alias export $store "secret" (type $secret))
          (import "api-key" (implements "wasmcloud:secrets/secret@2.1.0") (instance $label
            (alias outer 1 $secret (type $s))
            (export "secret" (type $t (eq $s)))
            (export "get" (func async (result (own $t))))
          ))
          (core func $get (canon lower (func $label "get")))
          (core func $return (canon task.return (result (own $secret))))
          (core module $m
            (import "" "get" (func $get (result i32)))
            (import "" "return" (func $return (param i32)))
            (func (export "probe") (call $return (call $get))))
          (core instance $i (instantiate $m (with "" (instance
            (export "get" (func $get))
            (export "return" (func $return))
          ))))
          (func (export "probe") async (result (own $secret))
            (canon lift (core func $i "probe") async))
        )
    "#;

    fn engine() -> Engine {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model_async(true);
        config.wasm_component_model_async_stackful(true);
        config.wasm_component_model_implements(true);
        Engine::new(&config).unwrap()
    }

    fn host_state(secrets: SecretsRegistry, component_id: &str) -> HostState {
        let (outbound, _) = tokio::sync::mpsc::channel(1);
        HostState {
            wasi: wasmtime_wasi::WasiCtx::builder().build(),
            http: wasmtime_wasi_http::WasiHttpCtx::new(),
            http_hooks: crate::http_policy::HttpPolicyHooks::new(None),
            table: wasmtime::component::ResourceTable::new(),
            stages: vec![crate::state::StageData {
                kind: crate::state::StageKind::Provider,
                component_id: component_id.to_string(),
                bindings: None,
                sink: crate::state::ClientSink::Outbound(outbound),
                downstream_idx: None,
            }],
            stage_stack: vec![0],
            secrets: Arc::new(secrets),
            downstream_sessions: Default::default(),
            next_downstream_rep: 1,
            editor_session_id: None,
            provider_routing: None,
            terminal_enabled: false,
            tool_broker: None,
            tool_decisions: Vec::new(),
            active_tool_calls: Default::default(),
        }
    }

    #[tokio::test]
    async fn labeled_secret_import_resolves_the_label_for_the_calling_component() {
        let dir = tempfile::tempdir().unwrap();
        wassette::SecretsManager::new(dir.path().to_path_buf())
            .set_bound_component_secrets(
                &test_binding("comp-labeled"),
                &[("api-key".into(), "s3cr3t".into())],
            )
            .await
            .unwrap();
        let registry = SecretsRegistry::new(dir.path());
        registry.register(test_binding("comp-labeled")).unwrap();

        let engine = engine();
        let component = Component::new(&engine, LABELED_SECRET_COMPONENT).unwrap();
        let labels = labeled_secrets(&engine, &component);
        assert_eq!(labels, ["api-key"]);
        check_labeled_secrets(&registry, "comp-labeled", &labels)
            .await
            .unwrap();

        let mut linker = Linker::new(&engine);
        crate::Layer::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |s| s).unwrap();
        add_labeled_secrets_to_linker(&mut linker, &labels).unwrap();

        let mut store = wasmtime::Store::new(&engine, host_state(registry, "comp-labeled"));
        let instance = linker
            .instantiate_async(&mut store, &component)
            .await
            .unwrap();
        let probe = instance
            .get_typed_func::<(), (Resource<Secret>,)>(&mut store, "probe")
            .unwrap();
        let (secret,) = probe.call_async(&mut store, ()).await.unwrap();

        let entry: Resource<SecretEntry> = Resource::new_borrow(secret.rep());
        let value = &store.data().table.get(&entry).unwrap().value;
        assert!(matches!(value, SecretValue::String(s) if s == "s3cr3t"));
    }

    #[tokio::test]
    async fn missing_labeled_secret_is_reported_before_instantiation() {
        let dir = tempfile::tempdir().unwrap();
        let registry = SecretsRegistry::new(dir.path());
        registry.register(test_binding("comp-missing")).unwrap();
        let error = check_labeled_secrets(&registry, "comp-missing", &["api-key".to_string()])
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("`api-key`"), "{error}");
        assert!(
            error.contains("wassette secret set comp-missing api-key=<value>"),
            "{error}"
        );
    }
}
