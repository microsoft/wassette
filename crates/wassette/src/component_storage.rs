// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Private physical layout, not a component-store publication or read protocol.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::StorageKey;

/// Resolves validated physical keys without inferring semantic component names.
#[derive(Clone)]
pub(crate) struct ComponentStorage {
    root: PathBuf,
}

impl ComponentStorage {
    /// Create the component directory; transactions are owned by `ComponentStore`.
    pub(crate) async fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        tokio::fs::create_dir_all(&root).await.with_context(|| {
            format!("Failed to create component directory at {}", root.display())
        })?;
        Ok(Self { root })
    }

    /// Root directory for the shared store and private runtime data.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Policy location for reporting an already-resolved receipt binding.
    pub(crate) fn policy_path(&self, key: &StorageKey) -> PathBuf {
        self.root.join(format!("{}.policy.yaml", key.as_str()))
    }

    #[cfg(test)]
    pub(crate) fn component_path(&self, key: &StorageKey) -> PathBuf {
        self.root.join(format!("{}.wasm", key.as_str()))
    }

    #[cfg(test)]
    pub(crate) fn metadata_path(&self, key: &StorageKey) -> PathBuf {
        self.root.join(format!("{}.metadata.json", key.as_str()))
    }

    #[cfg(test)]
    pub(crate) fn precompiled_path(&self, key: &StorageKey) -> PathBuf {
        self.root.join(format!("{}.cwasm", key.as_str()))
    }

    #[cfg(test)]
    pub(crate) fn policy_metadata_path(&self, key: &StorageKey) -> PathBuf {
        self.root.join(format!("{}.policy.meta.json", key.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn layout_preserves_validated_private_keys() -> Result<()> {
        let directory = tempfile::Builder::new()
            .prefix(".component-layout-")
            .tempdir_in(std::env::current_dir()?)?;
        let root = directory.path().join("components");
        let storage = ComponentStorage::new(&root).await?;
        let key = StorageKey::parse("Private_Key")?;
        assert_eq!(storage.root(), root);
        assert_eq!(storage.component_path(&key), root.join("Private_Key.wasm"));
        assert_eq!(
            storage.policy_path(&key),
            root.join("Private_Key.policy.yaml")
        );
        assert_eq!(
            storage.metadata_path(&key),
            root.join("Private_Key.metadata.json")
        );
        assert_eq!(
            storage.precompiled_path(&key),
            root.join("Private_Key.cwasm")
        );
        assert_eq!(
            storage.policy_metadata_path(&key),
            root.join("Private_Key.policy.meta.json")
        );
        assert_eq!(std::fs::read_dir(root)?.count(), 0);
        Ok(())
    }
}
