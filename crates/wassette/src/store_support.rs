// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::store::{ComponentStore, InstallReceipt, SourceIdentity};
use crate::SecretBinding;

impl InstallReceipt {
    /// Resolve the unchanged private secret namespace for this admitted binding.
    ///
    /// The stable source identity, not a version, artifact hash, or embedded name
    /// alone, determines whether later versions may reuse the namespace.
    pub fn secret_binding(&self) -> Result<SecretBinding> {
        SecretBinding::new(
            &self.component_id,
            &self.storage_key,
            source_binding_key(&self.source)?,
        )
    }
}

pub(crate) fn source_binding_key(source: &SourceIdentity) -> Result<String> {
    let bytes = serde_json::to_vec(source).context("encoding stable component source identity")?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

pub(crate) async fn store_operation<T, F>(store: &ComponentStore, operation: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(ComponentStore) -> Result<T> + Send + 'static,
{
    let store = store.clone();
    tokio::task::spawn_blocking(move || operation(store))
        .await
        .context("component store worker failed")?
}
