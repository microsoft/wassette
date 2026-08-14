//! Host-side implementation of `wasmcloud:secrets@0.1.0-draft`.
//!
//! The `with:` clause on the layer's bindgen reuses the provider's
//! generated modules verbatim, so a single set of `Host` / `HostSecret`
//! impls on [`HostState`] satisfies both linkers.

use wasmtime::component::Resource;

use crate::secrets::{SecretValue, SecretsError};
use crate::state::HostState;
use crate::wasmcloud::secrets::reveal;
use crate::wasmcloud::secrets::store::{
    Host as StoreHost, HostSecret as StoreHostSecret, Secret, SecretValue as WitSecretValue,
    SecretsError as WitSecretsError,
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

impl StoreHost for HostState {
    async fn get(&mut self, key: String) -> Result<Resource<Secret>, WitSecretsError> {
        // Each component sees only its own secret store: scope the
        // lookup by the *currently executing* stage's component identity
        // (`namespace:component-name`; top of [`HostState::stage_stack`]).
        // The guest never supplies the namespace, so components can't
        // read each other's secrets.
        let component_id = self.current_stage().component_id.clone();
        let value = self
            .secrets
            .resolve(&component_id, &key)
            .await
            .map_err(map_error)?;
        let entry = self
            .table
            .push(SecretEntry { value })
            .map_err(|e| WitSecretsError::Io(format!("resource table push: {e}")))?;
        // Re-tag the resource handle under the WIT-side type. Same rep,
        // different phantom type.
        Ok(Resource::new_own(entry.rep()))
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

impl reveal::Host for HostState {
    async fn reveal(&mut self, secret: Resource<Secret>) -> WitSecretValue {
        let entry: Resource<SecretEntry> = Resource::new_borrow(secret.rep());
        match self.table.get(&entry) {
            Ok(e) => map_value(e.value.clone()),
            Err(_) => WitSecretValue::String(String::new()),
        }
    }
}
