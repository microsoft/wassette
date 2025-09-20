use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use sha2::Digest;
use tokio::sync::Semaphore;

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
    pub fn new(root: PathBuf, max_concurrent_downloads: usize) -> Self {
        let downloads_dir = root.join(crate::DOWNLOADS_DIR);
        Self {
            root,
            downloads_dir,
            downloads_semaphore: Arc::new(Semaphore::new(max_concurrent_downloads.max(1))),
        }
    }

    /// Ensure the directory structure exists on disk.
    pub async fn ensure_layout(&self) -> Result<()> {
        tokio::fs::create_dir_all(&self.root)
            .await
            .with_context(|| {
                format!(
                    "Failed to create plugin directory at {}",
                    self.root.display()
                )
            })?;
        tokio::fs::create_dir_all(&self.downloads_dir)
            .await
            .with_context(|| {
                format!(
                    "Failed to create downloads directory at {}",
                    self.downloads_dir.display()
                )
            })?;
        Ok(())
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

    /// Acquire a permit for filesystem-bound downloads.
    pub async fn acquire_download_permit(&self) -> tokio::sync::OwnedSemaphorePermit {
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

        let contents = tokio::fs::read_to_string(&path).await.with_context(|| {
            format!("Failed to read component metadata from {}", path.display())
        })?;

        let metadata = serde_json::from_str(&contents).with_context(|| {
            format!("Failed to parse component metadata from {}", path.display())
        })?;
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

    /// Create a validation stamp for a file.
    pub async fn create_validation_stamp(
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
            let bytes = tokio::fs::read(path)
                .await
                .with_context(|| format!("Failed to read {} for hashing", path.display()))?;
            let mut hasher = sha2::Sha256::new();
            hasher.update(&bytes);
            Some(format!("{:x}", hasher.finalize()))
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
        let Ok(metadata) = tokio::fs::metadata(path).await else {
            return false;
        };

        if metadata.len() != stamp.file_size {
            return false;
        }

        let Ok(mtime) = metadata
            .modified()
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))
            .and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))
            })
            .map(|d| d.as_secs())
        else {
            return false;
        };

        if mtime != stamp.mtime {
            return false;
        }

        if let Some(expected_hash) = &stamp.content_hash {
            let Ok(content) = tokio::fs::read(path).await else {
                return false;
            };
            let mut hasher = sha2::Sha256::new();
            hasher.update(&content);
            let actual_hash = format!("{:x}", hasher.finalize());
            if &actual_hash != expected_hash {
                return false;
            }
        }

        true
    }
}
