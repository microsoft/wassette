//! WIT-named component plugin installer.
//!
//! Wraps [`wasm_package_manager::manager::Manager`] with an
//! app-scoped XDG cache and a couple of convenience helpers used by the
//! CLI (`--provider`/`--layer`) and by the host-side `/install` slash
//! command.
//!
//! The on-disk layout is:
//!
//! ```text
//! $XDG_DATA_HOME/acp-wasm/components/
//!   store/     # cacache-managed OCI blobs + metadata.db3
//!   vendor/    # reflinked .wasm components, one subdir per WIT name
//! ```
//!
//! Component lookups always live in `vendor/<slug>/` so subsequent
//! launches can find the file without going through the package
//! manager.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use wasm_package_manager::manager::{
    Manager, SyncPolicy, SyncResult,
    install::{looks_like_wit_name, resolve_wit_name},
};

/// Returns the app-scoped XDG data directory for installed components.
///
/// Honours `$XDG_DATA_HOME` when set, else falls back to
/// `dirs::data_dir()` (e.g. `~/.local/share` on Linux, `~/Library/
/// Application Support` on macOS).
pub fn cache_root() -> Result<PathBuf> {
    if let Some(val) = std::env::var_os("XDG_DATA_HOME") {
        let p = PathBuf::from(val);
        if p.is_absolute() {
            return Ok(p.join("acp-wasm").join("components"));
        }
    }
    let base = dirs::data_dir()
        .ok_or_else(|| anyhow!("cannot determine user data dir for component cache"))?;
    Ok(base.join("acp-wasm").join("components"))
}

fn store_dir() -> Result<PathBuf> {
    Ok(cache_root()?.join("store"))
}

fn vendor_root() -> Result<PathBuf> {
    Ok(cache_root()?.join("vendor"))
}

/// Per-WIT-name vendor subdirectory. Slug is `namespace__package[@version]`
/// with `:` replaced by `__` so the path is filesystem-safe.
fn vendor_dir_for(wit_name: &str) -> Result<PathBuf> {
    let slug = wit_name.replace(':', "__");
    Ok(vendor_root()?.join(slug))
}

/// Open the package manager rooted at our app-scoped cache.
pub async fn manager() -> Result<Manager> {
    let dir = store_dir()?;
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("failed to create component cache dir at {}", dir.display()))?;
    Manager::open_at(dir).await
}

/// Sync the local known-package index from the meta-registry. Best
/// effort: any failure is logged but not propagated, so installs can
/// still proceed against a previously-cached index.
async fn sync_registry(mgr: &Manager) {
    match mgr
        .sync_from_meta_registry(
            Manager::DEFAULT_REGISTRY_URL,
            Manager::DEFAULT_SYNC_INTERVAL,
            SyncPolicy::IfStale,
        )
        .await
    {
        Ok(SyncResult::Degraded { error }) => {
            tracing::warn!(error = %error, "registry sync degraded");
        }
        Err(e) => tracing::warn!(error = %e, "registry sync failed"),
        Ok(_) => {}
    }
}

/// Result of installing a single WIT-named component.
#[derive(Debug, Clone)]
pub struct InstalledComponent {
    /// Fully qualified `namespace:package@version` (version always
    /// filled in from the resolved OCI tag, even when the input WIT
    /// name omitted it).
    pub wit_name: String,
    pub path: PathBuf,
}

/// Install (or refresh) a WIT-named component and return the path to
/// its `.wasm` file. Always goes through the package manager: cache
/// hits are cheap (cacache + reflink) and a no-op on disk.
pub async fn install_wit(wit_name: &str) -> Result<InstalledComponent> {
    install_wit_with_progress(wit_name, None).await
}

/// Like [`install_wit`] but emits coarse phase messages
/// ("Syncing registry…", "Pulling…", byte counts, …) on `progress`
/// when set. Used by the host-side `/install` command to drive an
/// ACP tool-call progress card. Failures on the channel are ignored
/// so progress reporting never blocks the install itself.
pub async fn install_wit_with_progress(
    wit_name: &str,
    progress: Option<tokio::sync::mpsc::Sender<String>>,
) -> Result<InstalledComponent> {
    if !looks_like_wit_name(wit_name) {
        return Err(anyhow!(
            "`{wit_name}` is not a WIT-style name (expected `namespace:package[@version]`)"
        ));
    }
    let report = |msg: String| {
        if let Some(tx) = progress.as_ref() {
            let _ = tx.try_send(msg);
        }
    };

    report("Opening component cache…".to_string());
    let mgr = manager().await?;

    report("Syncing package index…".to_string());
    // Best-effort: refresh the local known-package index from the
    // meta-registry so freshly published WIT names resolve. Failures
    // are logged but don't block resolution against any cached index.
    sync_registry(&mgr).await;

    report(format!("Resolving `{wit_name}`…"));
    let reference = resolve_wit_name(wit_name, &mgr)
        .await
        .with_context(|| format!("resolving WIT name `{wit_name}`"))?;
    // Build a fully-qualified `namespace:package@version` string. The
    // input may have omitted `@version`; the resolved OCI reference
    // always carries a concrete tag.
    let base = wit_name.split_once('@').map_or(wit_name, |(b, _)| b);
    let qualified = match reference.tag() {
        Some(tag) => format!("{base}@{tag}"),
        None => base.to_string(),
    };
    let vendor_dir = vendor_dir_for(&qualified)?;

    report(format!(
        "Pulling `{qualified}` from {}…",
        reference.registry()
    ));
    let install = if let Some(tx) = progress.clone() {
        // Forward the package manager's own ProgressEvents as
        // human-readable strings on the same channel.
        let (pe_tx, mut pe_rx) =
            tokio::sync::mpsc::channel::<wasm_package_manager::ProgressEvent>(32);
        let forward = tokio::spawn(async move {
            let mut total: Option<u64> = None;
            while let Some(ev) = pe_rx.recv().await {
                if let Some(msg) = format_progress(&ev, &mut total) {
                    let _ = tx.try_send(msg);
                }
            }
        });
        let result = mgr
            .install_with_progress(reference, &vendor_dir, &pe_tx)
            .await;
        drop(pe_tx);
        let _ = forward.await;
        result
    } else {
        mgr.install(reference, &vendor_dir).await
    }
    .with_context(|| format!("installing `{wit_name}`"))?;

    let path = install
        .vendored_files
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("package `{wit_name}` contained no wasm layers"))?;
    Ok(InstalledComponent {
        wit_name: qualified,
        path,
    })
}

/// Map a [`wasm_package_manager::ProgressEvent`] to a one-line
/// user-facing string. Returns `None` for events that don't merit a
/// UI update (e.g. fine-grained byte deltas — we throttle those to a
/// single "downloaded N bytes" line per layer at a time).
fn format_progress(
    ev: &wasm_package_manager::ProgressEvent,
    total: &mut Option<u64>,
) -> Option<String> {
    use wasm_package_manager::ProgressEvent as P;
    match ev {
        P::ManifestFetched { layer_count, .. } => {
            *total = None;
            Some(format!("Manifest fetched ({layer_count} layer(s))"))
        }
        P::LayerStarted {
            index,
            total_bytes,
            title,
            ..
        } => {
            *total = *total_bytes;
            let label = title.clone().unwrap_or_else(|| format!("layer {index}"));
            match total_bytes {
                Some(n) => Some(format!("Downloading {label} ({})…", human_bytes(*n))),
                None => Some(format!("Downloading {label}…")),
            }
        }
        P::LayerProgress {
            bytes_downloaded, ..
        } => match *total {
            Some(t) if t > 0 => Some(format!(
                "Downloaded {} / {}",
                human_bytes(*bytes_downloaded),
                human_bytes(t)
            )),
            _ => Some(format!("Downloaded {}", human_bytes(*bytes_downloaded))),
        },
        P::LayerStored { .. } => Some("Stored layer".to_string()),
        P::LayerDownloaded { .. } | P::InstallComplete => None,
    }
}

fn human_bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if n >= GB {
        format!("{:.2} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.2} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{:.2} KB", n as f64 / KB as f64)
    } else {
        format!("{n} B")
    }
}

/// Resolve a CLI argument to a wasm component path.
///
/// - If `arg` looks like a WIT name (`namespace:package[@version]`),
///   reuse an already-vendored copy when present, otherwise install it.
/// - Otherwise treat `arg` as a filesystem path and return it as-is.
pub async fn resolve(arg: &str) -> Result<PathBuf> {
    if looks_like_wit_name(arg) {
        if let Some(existing) = first_wasm_in(&vendor_dir_for(arg)?).await? {
            return Ok(existing);
        }
        Ok(install_wit(arg).await?.path)
    } else {
        Ok(PathBuf::from(arg))
    }
}

/// The component identity (`namespace:component-name`) for a provider or
/// layer argument, using the same WIT-name-vs-path classification as
/// [`resolve`]. See [`crate::utils::component_id_from_arg`].
pub fn component_id_for_arg(arg: &str) -> Result<String> {
    crate::utils::component_id_from_arg(arg, looks_like_wit_name(arg))
}

/// Return the first `*.wasm` file in `dir` if the directory exists.
async fn first_wasm_in(dir: &Path) -> Result<Option<PathBuf>> {
    let mut rd = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    while let Some(entry) = rd.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("wasm") {
            return Ok(Some(path));
        }
    }
    Ok(None)
}
