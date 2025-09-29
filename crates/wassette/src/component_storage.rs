// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Filesystem helpers that manage component artifacts, metadata, and cache
//! layout for the lifecycle manager.

use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::spawn_blocking;

use crate::loader::DownloadedResource;
use crate::{ComponentMetadata, ValidationStamp};

/// Handles filesystem layout and metadata persistence for components.
#[derive(Clone)]
pub struct ComponentStorage {
    root: PathBuf,
    downloads_dir: PathBuf,
    downloads_semaphore: Arc<Semaphore>,
}

impl ComponentStorage {
    /// Create a new storage manager rooted at the plugin directory.
    pub async fn new(root: impl Into<PathBuf>, max_concurrent_downloads: usize) -> Result<Self> {
        let root = root.into();
        let downloads_dir = root.join(crate::DOWNLOADS_DIR);

        tokio::fs::create_dir_all(&root)
            .await
            .with_context(|| format!("Failed to create plugin directory at {}", root.display()))?;

        tokio::fs::create_dir_all(&downloads_dir)
            .await
            .with_context(|| {
                format!(
                    "Failed to create downloads directory at {}",
                    downloads_dir.display()
                )
            })?;

        Ok(Self {
            root,
            downloads_dir,
            downloads_semaphore: Arc::new(Semaphore::new(max_concurrent_downloads.max(1))),
        })
    }

    /// Root plugin directory containing components.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory used for staging downloaded artifacts.
    #[allow(dead_code)]
    pub fn downloads_dir(&self) -> &Path {
        &self.downloads_dir
    }

    async fn acquire_download_permit(&self) -> OwnedSemaphorePermit {
        self.downloads_semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("Semaphore closed")
    }

    /// Absolute path to the component `.wasm` file.
    pub fn component_path(&self, component_id: &str) -> PathBuf {
        self.root.join(format!("{component_id}.wasm"))
    }

    /// Absolute path to the policy file associated with a component.
    pub fn policy_path(&self, component_id: &str) -> PathBuf {
        self.root.join(format!("{component_id}.policy.yaml"))
    }

    /// Absolute path to the metadata JSON for a component.
    pub fn metadata_path(&self, component_id: &str) -> PathBuf {
        self.root
            .join(format!("{component_id}.{}", crate::METADATA_EXT))
    }

    /// Absolute path to the precompiled component cache file.
    pub fn precompiled_path(&self, component_id: &str) -> PathBuf {
        self.root
            .join(format!("{component_id}.{}", crate::PRECOMPILED_EXT))
    }

    /// Absolute path to the policy metadata JSON for a component.
    pub fn policy_metadata_path(&self, component_id: &str) -> PathBuf {
        self.root.join(format!("{component_id}.policy.meta.json"))
    }

    /// Stage a downloaded component artifact into storage, replacing any existing files.
    pub async fn install_component_artifact(
        &self,
        component_id: &str,
        resource: DownloadedResource,
    ) -> Result<PathBuf> {
        let _permit = self.acquire_download_permit().await;

        self.remove_component_artifacts(component_id).await?;

        resource.copy_to(self.root()).await.with_context(|| {
            format!(
                "Failed to copy component to destination: {}",
                self.root.display()
            )
        })?;

        Ok(self.component_path(component_id))
    }

    /// Remove persisted component artifacts (wasm, metadata, cache) if they exist.
    pub async fn remove_component_artifacts(&self, component_id: &str) -> Result<()> {
        self.remove_if_exists(
            &self.component_path(component_id),
            "component file",
            component_id,
        )
        .await?;
        self.remove_if_exists(
            &self.metadata_path(component_id),
            "component metadata file",
            component_id,
        )
        .await?;
        self.remove_if_exists(
            &self.precompiled_path(component_id),
            "precompiled component file",
            component_id,
        )
        .await?;
        Ok(())
    }

    /// Persist component metadata to disk.
    pub async fn write_metadata(&self, metadata: &ComponentMetadata) -> Result<()> {
        let path = self.metadata_path(&metadata.component_id);
        let json = serde_json::to_string_pretty(metadata)
            .context("Failed to serialize component metadata")?;
        tokio::fs::write(&path, json)
            .await
            .with_context(|| format!("Failed to write component metadata to {}", path.display()))
    }

    /// Load component metadata from disk if present.
    pub async fn read_metadata(&self, component_id: &str) -> Result<Option<ComponentMetadata>> {
        let path = self.metadata_path(component_id);
        if !path.exists() {
            return Ok(None);
        }

        let file = tokio::fs::File::open(&path)
            .await
            .with_context(|| format!("Failed to open component metadata at {}", path.display()))?;

        let file = file.into_std().await;

        let metadata = spawn_blocking(move || {
            let reader = BufReader::new(file);
            serde_json::from_reader(reader).context("Failed to deserialize component metadata")
        })
        .await??;
        Ok(Some(metadata))
    }

    /// Write precompiled component bytes to disk.
    pub async fn write_precompiled(&self, component_id: &str, bytes: &[u8]) -> Result<()> {
        let path = self.precompiled_path(component_id);
        tokio::fs::write(&path, bytes).await.with_context(|| {
            format!(
                "Failed to write precompiled component to {}",
                path.display()
            )
        })
    }

    /// Remove a file if it exists, translating IO errors into `anyhow`.
    pub async fn remove_if_exists(
        &self,
        path: &Path,
        description: &str,
        component_id: &str,
    ) -> Result<()> {
        match tokio::fs::remove_file(path).await {
            Ok(()) => {
                tracing::debug!(component_id = %component_id, path = %path.display(), "Removed {}", description);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(component_id = %component_id, path = %path.display(), "{} already absent", description);
            }
            Err(e) => {
                return Err(anyhow!(
                    "Failed to remove {} at {}: {}",
                    description,
                    path.display(),
                    e
                ));
            }
        }
        Ok(())
    }

    /// Create a validation stamp for a component artifact to track stale data on disk.
    ///
    /// When `include_hash` is `true` the SHA-256 hash of the file is
    /// recorded in addition to size and modification time so changes can be
    /// detected even when timestamps are unreliable.
    pub async fn create_validation_stamp(
        &self,
        path: &Path,
        include_hash: bool,
    ) -> Result<ValidationStamp> {
        let metadata = tokio::fs::metadata(path)
            .await
            .with_context(|| format!("Failed to read metadata for {}", path.display()))?;

        let file_size = metadata.len();
        let mtime = metadata
            .modified()
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))
            .and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))
            })?
            .as_secs();

        let content_hash = if include_hash {
            Some(compute_file_hash(path).await?)
        } else {
            None
        };

        Ok(ValidationStamp {
            file_size,
            mtime,
            content_hash,
        })
    }

    /// Check if the validation stamp matches the current file on disk.
    pub async fn validate_stamp(path: &Path, stamp: &ValidationStamp) -> bool {
        let metadata = match tokio::fs::metadata(path).await {
            Ok(metadata) => metadata,
            Err(_) => return false,
        };

        if metadata.len() != stamp.file_size {
            return false;
        }

        if let Some(expected_hash) = &stamp.content_hash {
            match compute_file_hash(path).await {
                Ok(actual_hash) => return actual_hash == *expected_hash,
                Err(_) => return false,
            }
        }

        let mtime = match metadata
            .modified()
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))
            .and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))
            })
            .map(|d| d.as_secs())
        {
            Ok(mtime) => mtime,
            Err(_) => return false,
        };

        if mtime != stamp.mtime {
            return false;
        }

        true
    }
}

async fn compute_file_hash(path: &Path) -> Result<String> {
    let file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("Failed to open {} for hashing", path.display()))?;

    let file = file.into_std().await;

    let path = path.to_path_buf();
    spawn_blocking(move || -> Result<String> {
        let mut reader = BufReader::new(file);
        let mut hasher = Sha256::new();

        std::io::copy(&mut reader, &mut hasher)?;

        Ok(format!("{:x}", hasher.finalize()))
    })
    .await?
    .with_context(|| format!("Failed to hash file {}", path.display()))
}
