// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use etcetera::BaseStrategy;
use serde::{Deserialize, Serialize};

/// Whether local component discovery is disabled, one-shot, or continuously polled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LocalMode {
    /// Do not inspect the drop directory.
    Off,
    /// Reconcile once when requested.
    Startup,
    /// Periodically rescan the drop directory.
    Watch,
}

/// Process-local discovery settings; no files are written to the source root.
#[derive(Debug, Clone)]
pub struct LocalSourceConfig {
    /// Non-recursive drop directory.
    pub root: PathBuf,
    /// Discovery scheduling mode.
    pub mode: LocalMode,
}

impl LocalSourceConfig {
    /// Construct process-local discovery settings.
    pub fn new(root: PathBuf, mode: LocalMode) -> Self {
        Self { root, mode }
    }

    /// Return the OS data directory's default local-component root.
    ///
    /// An unavailable platform data directory is an error, never a fallback to
    /// the current working directory.
    pub fn default_root() -> Result<PathBuf> {
        Ok(etcetera::choose_base_strategy()
            .context("determining platform data directory for local components")?
            .data_dir()
            .join("wassette")
            .join("local-components"))
    }

    /// Reject local source and component-store overlap, including symlink aliases.
    ///
    /// Does not create either directory. The component store need not yet exist.
    pub fn validate(&self, component_dir: &Path) -> Result<()> {
        self.resolved_root(component_dir)?;
        Ok(())
    }

    pub(crate) fn resolve(&mut self, component_dir: &Path) -> Result<()> {
        self.root = self.resolved_root(component_dir)?;
        Ok(())
    }

    fn resolved_root(&self, component_dir: &Path) -> Result<PathBuf> {
        let cwd = std::env::current_dir()?;
        let absolute = if self.root.is_absolute() {
            self.root.clone()
        } else {
            cwd.join(&self.root)
        };
        let component_dir = if component_dir.is_absolute() {
            component_dir.to_path_buf()
        } else {
            cwd.join(component_dir)
        };
        let store = canonicalize_missing(&component_dir).context("resolving component store")?;
        let source = canonicalize_missing(&absolute)?;
        if source.starts_with(&store) || store.starts_with(&source) {
            bail!("local source directory must not overlap the component store");
        }
        Ok(source)
    }
}

fn canonicalize_missing(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return path.canonicalize().context("resolving local source root");
    }
    let parent = path.parent().context("local source root has no parent")?;
    Ok(canonicalize_missing(parent)?
        .join(path.file_name().context("local source root has no name")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_has_lowercase_config_representation() -> Result<()> {
        for (mode, spelling) in [
            (LocalMode::Off, "off"),
            (LocalMode::Startup, "startup"),
            (LocalMode::Watch, "watch"),
        ] {
            assert_eq!(serde_json::to_string(&mode)?, format!("\"{spelling}\""));
            assert_eq!(
                serde_json::from_str::<LocalMode>(&format!("\"{spelling}\""))?,
                mode
            );
        }
        assert!(serde_json::from_str::<LocalMode>("\"WATCH\"").is_err());
        Ok(())
    }

    #[test]
    fn default_uses_platform_data_dir() -> Result<()> {
        let expected = etcetera::choose_base_strategy()?
            .data_dir()
            .join("wassette")
            .join("local-components");
        assert_eq!(LocalSourceConfig::default_root()?, expected);
        Ok(())
    }

    #[test]
    fn overlap_validation_does_not_create_directories() -> Result<()> {
        let directory = tempfile::Builder::new()
            .prefix(".local-config-")
            .tempdir_in(std::env::current_dir()?)?;
        let store = directory.path().join("uncreated-store");
        LocalSourceConfig::new(directory.path().join("source"), LocalMode::Startup)
            .validate(&store)?;
        assert!(!store.exists());
        LocalSourceConfig::new(store.join("source"), LocalMode::Startup)
            .validate(&store)
            .unwrap_err();
        LocalSourceConfig::new(directory.path().to_path_buf(), LocalMode::Startup)
            .validate(&store)
            .unwrap_err();
        Ok(())
    }
}
