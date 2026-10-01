// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Conservative, receipt-backed discovery of locally dropped components.
//!
//! Files are merely untrusted inputs. Only a named, captured artifact validated
//! by the appropriate runtime may cross the store's compare-and-swap boundary.

mod capture;
mod config;
mod trust;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
pub use config::{LocalMode, LocalSourceConfig};

const SETTLE_INTERVAL: Duration = Duration::from_millis(100);
const CAPTURE_SIZE_CAP: u64 = 128 * 1024 * 1024;
const WATCH_POLL_INTERVAL: Duration = Duration::from_secs(2);
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;

use crate::loader::CapturedComponent;
use crate::store::{
    self, InstallIntent, InstallOptions, InstallOwner, ManagedLocalSource, OriginEvidence,
    PolicyProvenance, PreparedInstall, PreparedPolicy, RemovalAuthority, RemovalReason,
    SourceIdentity, StoredArtifactKind, StoredEntry, ValidationEvidence,
};
use crate::store_support::{source_binding_key, store_operation};
use crate::{inspect_artifact, ArtifactShape, LifecycleManager, SecretBinding, StorageKey};

/// An injected trusted ACP validator. It must compile and check exports against
/// its actual ACP runtime; structural inspection alone is never validation.
pub trait LocalValidator: Send + Sync {
    /// Validate the exact captured inputs, returning actual runtime evidence.
    fn validate(
        &self,
        wasm: &[u8],
        inspection: &crate::ArtifactInspection,
        policy: Option<&[u8]>,
    ) -> Result<ValidationEvidence>;
}

/// A successful local membership change at its authoritative store revision.
#[derive(Debug, Clone)]
pub struct LocalSourceEvent {
    /// Embedded semantic identity.
    pub component_id: String,
    /// Committed revision.
    pub revision: store::EntryRevision,
    /// Validated artifact route.
    pub kind: StoredArtifactKind,
    /// Change to managed membership.
    pub change: LocalSourceChange,
}

/// Managed membership transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalSourceChange {
    /// New managed component.
    Added,
    /// Replaced managed component.
    Updated,
    /// Managed source disappeared.
    Removed,
}

/// The resolution of exactly one source file or one managed store entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceStatus {
    /// Newly committed.
    Installed,
    /// Committed over an existing managed entry.
    Updated,
    /// Pruned because its source disappeared.
    Removed,
    /// The captured observation equals the installed receipt's.
    Unchanged,
    /// Not reinstalled after an explicit uninstall of the same capture.
    Suppressed,
    /// Retryable: unstable, unreadable, untrusted, or awaiting a validator.
    Pending,
    /// Unnamed, ambiguous, unsupported, or failing validation.
    Rejected,
    /// Identity, ownership, or revision conflict; nothing was written.
    Conflict,
}

impl SourceStatus {
    /// Whether this status leaves the source unresolved for this pass.
    pub fn unresolved(self) -> bool {
        matches!(self, Self::Pending | Self::Rejected | Self::Conflict)
    }
}

/// One resolved source; a filename is a discovery location, never an identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceOutcome {
    /// Explicit status for rendering and exit codes.
    pub status: SourceStatus,
    /// Relative source filename, absent for store-side prunes.
    pub source: Option<String>,
    /// Embedded semantic identity, when it could be established.
    pub component_id: Option<String>,
    /// Operator-facing reason, without artifact, policy or secret contents.
    pub detail: Option<String>,
}

/// Per-pass outcomes in discovery order, renderable as JSON, YAML or a table.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ReconcileReport {
    /// Every source and pruned entry resolved by this pass.
    pub outcomes: Vec<SourceOutcome>,
    /// Whether pruning was skipped because a source stayed unresolved.
    pub prune_skipped: bool,
}

impl ReconcileReport {
    /// Component IDs resolved with this status, in discovery order.
    pub fn ids(&self, status: SourceStatus) -> Vec<&str> {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.status == status)
            .filter_map(|outcome| outcome.component_id.as_deref())
            .collect()
    }

    /// Outcomes with this status, including those without a semantic identity.
    pub fn with_status(&self, status: SourceStatus) -> Vec<&SourceOutcome> {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.status == status)
            .collect()
    }

    /// Whether any source remained pending, rejected or conflicting.
    pub fn has_unresolved(&self) -> bool {
        self.outcomes
            .iter()
            .any(|outcome| outcome.status.unresolved())
    }

    fn push(
        &mut self,
        status: SourceStatus,
        source: Option<&str>,
        component_id: Option<&str>,
        detail: Option<String>,
    ) {
        self.outcomes.push(SourceOutcome {
            status,
            source: source.map(str::to_owned),
            component_id: component_id.map(str::to_owned),
            detail,
        });
    }
}

/// A process-local reconciler; the shared store remains the authority.
pub struct LocalSourceService {
    manager: Arc<LifecycleManager>,
    config: LocalSourceConfig,
    root_key: String,
    validator: Option<Arc<dyn LocalValidator>>,
    gate: Mutex<()>,
    events: broadcast::Sender<LocalSourceEvent>,
}

impl LocalSourceService {
    /// Open the source root, creating it privately if discovery is enabled.
    ///
    /// An unsafe or inaccessible root is an error, not an empty inventory.
    pub fn new(manager: Arc<LifecycleManager>, mut config: LocalSourceConfig) -> Result<Self> {
        config.validate(manager.storage.root())?;
        if config.mode != LocalMode::Off {
            create_source_root(&config.root)?;
            trust::check_root(&config.root).context("checking local source root")?;
        }
        config.resolve(manager.storage.root())?;
        let root_key = root_key(&config.root);
        let (events, _) = broadcast::channel(128);
        Ok(Self {
            manager,
            config,
            root_key,
            validator: None,
            gate: Mutex::new(()),
            events,
        })
    }

    /// Supply the trusted ACP validator for ACP-shaped drops.
    pub fn with_validator(mut self, validator: Arc<dyn LocalValidator>) -> Self {
        self.validator = Some(validator);
        self
    }

    /// Subscribe to committed local membership changes (not pending inputs).
    pub fn subscribe(&self) -> broadcast::Receiver<LocalSourceEvent> {
        self.events.subscribe()
    }

    /// Obtain current managed receipts from the store, including external commits.
    pub async fn members(&self) -> Result<Vec<store::InstallReceipt>> {
        let snapshot = store_operation(self.manager.component_store(), |store| {
            Ok(store
                .snapshot_if_changed(None)?
                .expect("full inventory requested"))
        })
        .await?;
        Ok(snapshot
            .entries
            .into_iter()
            .filter_map(|entry| match entry {
                StoredEntry::Installed(receipt)
                    if matches!(&receipt.owner, InstallOwner::ManagedLocalSource(owner) if owner.root_key == self.root_key) =>
                {
                    Some(receipt)
                }
                _ => None,
            })
            .collect())
    }

    /// Scan, validate and compare-and-commit candidates; prune only after an
    /// entirely resolved pass. Refresh the runtime even for a no-op pass.
    pub async fn reconcile_once(&self, force: bool) -> Result<ReconcileReport> {
        let _gate = self.gate.lock().await;
        let outcome = self.reconcile(force).await;
        let refresh = self.manager.refresh_from_store().await;
        match (outcome, refresh) {
            (Ok(report), Ok(_)) => Ok(report),
            (Err(error), _) => Err(error),
            (_, Err(error)) => {
                Err(error.context("local reconciliation completed but catalog refresh failed"))
            }
        }
    }

    /// Safety-rescan watch loop. Polling also notices symlink target changes,
    /// missing roots, and events lost by filesystem notifications.
    pub async fn watch(&self, cancel: CancellationToken) -> Result<()> {
        if self.config.mode != LocalMode::Watch {
            return Ok(());
        }
        let mut interval = tokio::time::interval(WATCH_POLL_INTERVAL);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                _ = interval.tick() => {
                    if let Err(error) = self.reconcile_once(false).await {
                        tracing::error!(error = %error, "Local source scan failed; retrying");
                    }
                }
            }
        }
    }

    async fn reconcile(&self, force: bool) -> Result<ReconcileReport> {
        let mut report = ReconcileReport::default();
        if self.config.mode == LocalMode::Off {
            return Ok(report);
        }
        trust::check_root(&self.config.root)
            .context("local source root is unavailable or unsafe")?;
        let paths = discover(&self.config.root).context("reading local source root")?;
        let snapshot = store_operation(self.manager.component_store(), |store| {
            Ok(store
                .snapshot_if_changed(None)?
                .expect("full inventory requested"))
        })
        .await?;
        let entries: BTreeMap<_, _> = snapshot
            .entries
            .into_iter()
            .map(|entry| (entry.component_id().as_str().to_owned(), entry))
            .collect();
        let mut present = BTreeSet::new();
        let mut candidates = Vec::with_capacity(paths.len());
        let mut id_counts = BTreeMap::<String, usize>::new();
        let mut captured_bytes = 0u64;
        for path in paths {
            let source = path.clone();
            let settle = SETTLE_INTERVAL;
            let cap = CAPTURE_SIZE_CAP;
            let captured =
                tokio::task::spawn_blocking(move || capture::capture(&source, settle, cap))
                    .await
                    .context("source capture worker failed")?;
            let inspected = captured.and_then(|capture| {
                let size = capture.wasm.len() as u64
                    + capture
                        .sidecar
                        .as_ref()
                        .map_or(0, |policy| policy.len() as u64);
                captured_bytes = captured_bytes.saturating_add(size);
                if captured_bytes > CAPTURE_SIZE_CAP.saturating_mul(2) {
                    anyhow::bail!("aggregate local source capture size exceeded");
                }
                let inspection = inspect_artifact(&capture.wasm);
                Ok((capture, inspection))
            });
            if let Ok((_, Ok(inspection))) = &inspected {
                if let Ok(id) = &inspection.identity {
                    *id_counts.entry(id.as_str().to_owned()).or_default() += 1;
                }
            }
            candidates.push((path, inspected));
        }
        let mut complete = true;
        for (path, inspected) in candidates {
            let label = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let relative = PathBuf::from(path.file_name().context("source has no filename")?);
            let owner = ManagedLocalSource::new(&self.root_key, relative)?;
            present.insert(owner.relative_source.clone());
            let (captured, inspection) = match inspected {
                Ok(pair) => pair,
                Err(error) => {
                    complete = false;
                    report.push(
                        SourceStatus::Pending,
                        Some(&label),
                        None,
                        Some(format!("{error:#}")),
                    );
                    continue;
                }
            };
            let inspection = match inspection {
                Ok(inspection) => inspection,
                Err(error) => {
                    complete = false;
                    report.push(
                        SourceStatus::Rejected,
                        Some(&label),
                        None,
                        Some(format!("{error:#}")),
                    );
                    continue;
                }
            };
            let id = match inspection.identity {
                Ok(id) => id.as_str().to_owned(),
                Err(error) => {
                    complete = false;
                    report.push(
                        SourceStatus::Rejected,
                        Some(&label),
                        None,
                        Some(error.to_string()),
                    );
                    continue;
                }
            };
            if id_counts.get(&id).copied().unwrap_or(0) != 1 {
                complete = false;
                report.push(
                    SourceStatus::Conflict,
                    Some(&label),
                    Some(&id),
                    Some("another source in this root declares the same component name".into()),
                );
                continue;
            }
            if matches!(inspection.shape, ArtifactShape::Unsupported(_)) {
                complete = false;
                report.push(
                    SourceStatus::Rejected,
                    Some(&label),
                    Some(&id),
                    Some("unsupported artifact shape".into()),
                );
                continue;
            }
            let previous = entries.get(&id);
            if let Some(previous) = previous {
                if previous.binding().owner != InstallOwner::ManagedLocalSource(owner.clone()) {
                    complete = false;
                    report.push(
                        SourceStatus::Conflict,
                        Some(&label),
                        Some(&id),
                        Some("this component is owned by a different source".into()),
                    );
                    continue;
                }
                if let StoredEntry::Installed(receipt) = previous {
                    if !force && receipt.observation.as_ref() == Some(&captured.observation) {
                        report.push(SourceStatus::Unchanged, Some(&label), Some(&id), None);
                        continue;
                    }
                }
                if let StoredEntry::Retired(retired) = previous {
                    if !force
                        && retired.reason == RemovalReason::ExplicitUninstall
                        && retired.previous.observation.as_ref() == Some(&captured.observation)
                    {
                        report.push(
                            SourceStatus::Suppressed,
                            Some(&label),
                            Some(&id),
                            Some("explicitly unloaded; rebuild the source or sync --force".into()),
                        );
                        continue;
                    }
                }
            }
            if inspection.shape != ArtifactShape::ToolCandidate && self.validator.is_none() {
                complete = false;
                report.push(
                    SourceStatus::Pending,
                    Some(&label),
                    Some(&id),
                    Some("DeferredMissingValidator: no validator for this artifact kind".into()),
                );
                continue;
            }
            match self.install(&id, owner, &path, captured, previous).await {
                Ok((change, revision, kind)) => {
                    let changed = change.is_some();
                    let status = match change {
                        Some(LocalSourceChange::Added) => SourceStatus::Installed,
                        Some(LocalSourceChange::Updated) => SourceStatus::Updated,
                        _ => SourceStatus::Unchanged,
                    };
                    report.push(status, Some(&label), Some(&id), None);
                    if changed {
                        let _ = self.events.send(LocalSourceEvent {
                            component_id: id,
                            revision,
                            kind,
                            change: change.expect("changed"),
                        });
                    }
                }
                Err(error) => {
                    complete = false;
                    let status = if error
                        .downcast_ref::<store::StoreError>()
                        .is_some_and(|error| matches!(error, store::StoreError::Conflict(_)))
                    {
                        SourceStatus::Conflict
                    } else {
                        SourceStatus::Rejected
                    };
                    report.push(status, Some(&label), Some(&id), Some(format!("{error:#}")));
                }
            }
        }
        report.prune_skipped = !complete;
        if complete {
            for entry in entries.values() {
                let StoredEntry::Installed(receipt) = entry else {
                    continue;
                };
                let InstallOwner::ManagedLocalSource(owner) = &receipt.owner else {
                    continue;
                };
                if owner.root_key != self.root_key || present.contains(&owner.relative_source) {
                    continue;
                }
                let id = receipt.component_id.as_str().to_owned();
                let revision = receipt.revision.clone();
                let owned = owner.clone();
                match store_operation(self.manager.component_store(), move |store| {
                    Ok(store.remove(&id, &revision, RemovalAuthority::Owned(owned))?)
                })
                .await
                {
                    Ok(outcome) => {
                        if outcome.change.is_some() {
                            report.push(
                                SourceStatus::Removed,
                                Some(&owner.relative_source.to_string_lossy()),
                                Some(receipt.component_id.as_str()),
                                None,
                            );
                            let _ = self.events.send(LocalSourceEvent {
                                component_id: receipt.component_id.as_str().to_owned(),
                                revision: outcome.entry.revision().clone(),
                                kind: receipt.kind.clone(),
                                change: LocalSourceChange::Removed,
                            });
                        }
                    }
                    Err(error) => report.push(
                        SourceStatus::Conflict,
                        Some(&owner.relative_source.to_string_lossy()),
                        Some(receipt.component_id.as_str()),
                        Some(format!("{error:#}")),
                    ),
                }
            }
        }
        Ok(report)
    }

    async fn install(
        &self,
        id: &str,
        owner: ManagedLocalSource,
        path: &Path,
        captured: capture::Capture,
        previous: Option<&StoredEntry>,
    ) -> Result<(
        Option<LocalSourceChange>,
        store::EntryRevision,
        StoredArtifactKind,
    )> {
        let _guard = self.manager.load_guard(id).await.lock_owned().await;
        let key = match previous {
            Some(entry) => entry.storage_key().clone(),
            None => StorageKey::parse(&format!(
                "local-{}",
                hex::encode(Sha256::digest(id.as_bytes()))
            ))?,
        };
        let source = SourceIdentity::File(self.config.root.join(&owner.relative_source));
        let observed_id = id.to_owned();
        let observed_key = key.clone();
        let observed_source = source.clone();
        let expected = store_operation(self.manager.component_store(), move |store| {
            Ok(store.observe(&observed_id, &observed_key, &observed_source)?)
        })
        .await?;
        if expected.entry() != previous {
            return Err(
                store::StoreError::Conflict("source changed since inventory".into()).into(),
            );
        }
        let policy = match previous {
            Some(StoredEntry::Installed(receipt))
                if receipt.policy.provenance != PolicyProvenance::Default
                    && receipt.policy.provenance != PolicyProvenance::Bundled =>
            {
                let selected_id = id.to_owned();
                let snapshot = store_operation(self.manager.component_store(), move |store| {
                    Ok(store.read(&selected_id)?)
                })
                .await?;
                if snapshot.receipt.revision != receipt.revision {
                    return Err(anyhow!("effective policy changed during selection"));
                }
                let selected = match snapshot.policy {
                    Some(bytes) => PreparedPolicy::parse(bytes, receipt.policy.provenance.clone())?,
                    None => PreparedPolicy::absent(receipt.policy.provenance.clone()),
                };
                selected.with_metadata(receipt.policy.metadata.clone())?
            }
            Some(StoredEntry::Retired(retired))
                if !matches!(
                    retired.previous.policy.provenance,
                    PolicyProvenance::Default | PolicyProvenance::Bundled
                ) =>
            {
                return Err(anyhow!("retired explicit policy cannot be reconstructed from source; reinstall explicitly"));
            }
            _ => match captured.sidecar {
                Some(bytes) => PreparedPolicy::parse(bytes, PolicyProvenance::Bundled)?,
                None => PreparedPolicy::absent(PolicyProvenance::Default),
            },
        };
        let binding = SecretBinding::new(
            &inspect_artifact(&captured.wasm)?.identity?,
            &key,
            source_binding_key(&source)?,
        )?;
        self.manager.secrets_manager.check_binding(&binding).await?;
        let inspection = inspect_artifact(&captured.wasm)?;
        let kind = inspection.shape.clone();
        let evidence = match kind {
            ArtifactShape::ToolCandidate => {
                self.manager
                    .prepare_component_load(
                        CapturedComponent {
                            storage_key: key.clone(),
                            wasm: captured.wasm.clone(),
                            bundled_policy: None,
                        },
                        &binding,
                        policy.bytes().map(<[u8]>::to_vec),
                    )
                    .await?;
                ValidationEvidence::OrdinaryPrepared {
                    runtime: self.manager.cache_engine(),
                }
            }
            ArtifactShape::AcpProvider | ArtifactShape::AcpLayer => self
                .validator
                .as_ref()
                .context("DeferredMissingValidator")?
                .validate(&captured.wasm, &inspection, policy.bytes())?,
            ArtifactShape::Unsupported(_) => return Err(anyhow!("unsupported artifact shape")),
        };
        let intent = if kind == ArtifactShape::ToolCandidate {
            InstallIntent::ExposeTools
        } else {
            InstallIntent::AcpSelection
        };
        let prepared = PreparedInstall::prepare(
            captured.wasm,
            InstallOptions {
                storage_key: key,
                source,
                origin: OriginEvidence {
                    location: path.display().to_string(),
                    requested_version: None,
                    selected_version: None,
                    manifest_digest: None,
                    immutable_uri: None,
                    generation: None,
                },
                owner: InstallOwner::ManagedLocalSource(owner),
                intent,
                policy,
                observation: Some(captured.observation),
            },
            move |_, _, _| Ok(evidence),
        )?;
        let updated = matches!(previous, Some(StoredEntry::Installed(_)));
        let outcome = store_operation(self.manager.component_store(), move |store| {
            Ok(store.commit_install(prepared, expected)?)
        })
        .await?;
        let kind = outcome.entry.binding().kind.clone();
        let change = outcome.change.map(|_| {
            if updated {
                LocalSourceChange::Updated
            } else {
                LocalSourceChange::Added
            }
        });
        Ok((change, outcome.entry.revision().clone(), kind))
    }
}

fn create_source_root(root: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(root)
            .with_context(|| format!("creating private local source root {}", root.display()))?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(root)
        .with_context(|| format!("creating local source root {}", root.display()))?;
    Ok(())
}

fn root_key(root: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        hex::encode(Sha256::digest(root.as_os_str().as_bytes()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let mut hash = Sha256::new();
        for code_unit in root.as_os_str().encode_wide() {
            hash.update(code_unit.to_le_bytes());
        }
        hex::encode(hash.finalize())
    }
}

fn discover(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.')
            || !name.ends_with(".wasm")
            || name.ends_with(".tmp")
            || name.ends_with(".part")
            || name.ends_with(".crdownload")
            || name.ends_with('~')
        {
            continue;
        }
        paths.push(entry.path());
    }
    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        wat::parse_str(format!(
            r#"(component $"{name}"
                (core module $m (func (export "run")))
                (core instance $i (instantiate $m))
                (func (export "run") (canon lift (core func $i "run")))
            )"#
        ))
        .unwrap()
    }

    async fn service() -> Result<(tempfile::TempDir, LocalSourceService)> {
        let temp = tempfile::Builder::new()
            .prefix(".local-source-")
            .tempdir_in(std::env::current_dir()?)?;
        let manager = LifecycleManager::builder(temp.path().join("store"))
            .with_secrets_dir(temp.path().join("secrets"))
            .with_eager_loading(false)
            .build()
            .await?;
        let root = temp.path().join("drops");
        fs::create_dir(&root)?;
        let service = LocalSourceService::new(
            Arc::new(manager),
            LocalSourceConfig {
                root,
                mode: LocalMode::Startup,
            },
        )?;
        Ok((temp, service))
    }

    #[tokio::test]
    async fn watch_retries_after_source_root_disappears() -> Result<()> {
        let (_temp, mut service) = service().await?;
        service.config.mode = LocalMode::Watch;
        let cancel = CancellationToken::new();
        let watcher = {
            let cancel = cancel.clone();
            let service = Arc::new(service);
            let watcher = Arc::clone(&service);
            let task = tokio::spawn(async move { watcher.watch(cancel).await });
            (service, task)
        };
        let (service, task) = watcher;
        tokio::fs::remove_dir(&service.config.root).await?;
        tokio::time::sleep(WATCH_POLL_INTERVAL + Duration::from_millis(100)).await;
        assert!(
            !task.is_finished(),
            "transient source failure must not stop discovery"
        );
        tokio::fs::create_dir(&service.config.root).await?;
        tokio::fs::write(
            service.config.root.join("recovered.wasm"),
            fixture("recovered"),
        )
        .await?;
        tokio::time::timeout(WATCH_POLL_INTERVAL * 2, async {
            loop {
                if service.members().await?.len() == 1 {
                    return Result::<()>::Ok(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await??;
        cancel.cancel();
        task.await??;
        Ok(())
    }

    #[tokio::test]
    async fn embedded_name_not_filename_and_unload_suppresses_until_force() -> Result<()> {
        let (_temp, service) = service().await?;
        let path = service.config.root.join("CON.wasm");
        fs::write(&path, fixture("semantic:Weather"))?;
        let mut events = service.subscribe();
        let first = service.reconcile_once(false).await?;
        assert_eq!(first.ids(SourceStatus::Installed), ["semantic:Weather"]);
        assert_eq!(events.try_recv()?.change, LocalSourceChange::Added);
        assert_eq!(service.members().await?.len(), 1);
        assert_eq!(
            service
                .reconcile_once(false)
                .await?
                .ids(SourceStatus::Unchanged),
            ["semantic:Weather"]
        );
        assert!(events.try_recv().is_err());
        service.manager.unload_component("semantic:Weather").await?;
        assert_eq!(
            service
                .reconcile_once(false)
                .await?
                .ids(SourceStatus::Suppressed),
            ["semantic:Weather"]
        );
        assert!(service.members().await?.is_empty());
        assert_eq!(
            service
                .reconcile_once(true)
                .await?
                .ids(SourceStatus::Installed),
            ["semantic:Weather"]
        );
        Ok(())
    }

    #[tokio::test]
    async fn rejected_or_unreadable_source_never_prunes_last_good() -> Result<()> {
        let (_temp, service) = service().await?;
        let path = service.config.root.join("tool.wasm");
        fs::write(&path, fixture("semantic:tool"))?;
        assert_eq!(
            service
                .reconcile_once(false)
                .await?
                .ids(SourceStatus::Installed)
                .len(),
            1
        );
        fs::write(&path, b"not wasm")?;
        assert_eq!(
            service
                .reconcile_once(false)
                .await?
                .with_status(SourceStatus::Rejected)
                .len(),
            1
        );
        assert_eq!(service.members().await?.len(), 1);
        fs::remove_file(&path)?;
        let report = service.reconcile_once(false).await?;
        assert_eq!(report.ids(SourceStatus::Removed), ["semantic:tool"]);
        assert!(service.members().await?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn acp_without_validator_deferred_and_never_committed() -> Result<()> {
        let (_temp, service) = service().await?;
        fs::write(
            service.config.root.join("acp.wasm"),
            wat::parse_str(
                r#"(component $"named-acp" (instance $agent)
                    (export "wassette:acp/agent@7.0.0" (instance $agent)))"#,
            )?,
        )?;
        let report = service.reconcile_once(false).await?;
        assert!(report.with_status(SourceStatus::Pending)[0]
            .detail
            .as_ref()
            .unwrap()
            .contains("DeferredMissingValidator"));
        assert!(service.members().await?.is_empty());
        Ok(())
    }

    struct TestAcpValidator;

    impl LocalValidator for TestAcpValidator {
        fn validate(
            &self,
            _wasm: &[u8],
            _inspection: &crate::ArtifactInspection,
            _policy: Option<&[u8]>,
        ) -> Result<ValidationEvidence> {
            Ok(ValidationEvidence::AcpCompiledAndExportChecked {
                runtime: "test-acp".into(),
            })
        }
    }

    #[tokio::test]
    async fn injected_acp_validator_commits_without_exposing_tools() -> Result<()> {
        let (_temp, service) = service().await?;
        fs::write(
            service.config.root.join("acp.wasm"),
            wat::parse_str(
                r#"(component $"named-acp" (instance $agent)
                    (export "wassette:acp/agent@7.0.0" (instance $agent)))"#,
            )?,
        )?;
        let service = service.with_validator(Arc::new(TestAcpValidator));
        assert_eq!(
            service
                .reconcile_once(false)
                .await?
                .ids(SourceStatus::Installed),
            ["named-acp"]
        );
        let receipt = service.members().await?.remove(0);
        assert_eq!(receipt.kind, StoredArtifactKind::AcpProvider);
        assert!(service.manager.list_tool_descriptors().await?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn deferred_rebuild_keeps_last_good_acp_receipt() -> Result<()> {
        let (_temp, service) = service().await?;
        let path = service.config.root.join("acp.wasm");
        fs::write(
            &path,
            wat::parse_str(
                r#"(component $"named-acp" (instance $agent)
                    (export "wassette:acp/agent@7.0.0" (instance $agent)))"#,
            )?,
        )?;
        let service = service.with_validator(Arc::new(TestAcpValidator));
        service.reconcile_once(false).await?;
        let receipt = service.members().await?.remove(0);
        let service = LocalSourceService::new(service.manager.clone(), service.config.clone())?;
        fs::write(
            &path,
            wat::parse_str(
                r#"(component $"named-acp" (instance $agent)
                    (export "wassette:acp/agent@8.0.0" (instance $agent)))"#,
            )?,
        )?;
        let report = service.reconcile_once(false).await?;
        assert!(report.with_status(SourceStatus::Pending)[0]
            .detail
            .as_ref()
            .unwrap()
            .contains("DeferredMissingValidator"));
        assert!(report.with_status(SourceStatus::Removed).is_empty());
        assert_eq!(service.members().await?[0].revision, receipt.revision);
        Ok(())
    }

    #[tokio::test]
    async fn polling_watch_discovers_new_drop_and_stops_on_cancel() -> Result<()> {
        let (_temp, mut service) = service().await?;
        service.config.mode = LocalMode::Watch;
        let service = Arc::new(service);
        let cancel = CancellationToken::new();
        let watcher = tokio::spawn({
            let service = service.clone();
            let cancel = cancel.clone();
            async move { service.watch(cancel).await }
        });
        fs::write(service.config.root.join("new.wasm"), fixture("discovered"))?;
        let mut events = service.subscribe();
        tokio::time::timeout(std::time::Duration::from_secs(3), events.recv()).await??;
        assert_eq!(
            service.members().await?[0].component_id.as_str(),
            "discovered"
        );
        cancel.cancel();
        watcher.await??;
        Ok(())
    }

    #[tokio::test]
    async fn duplicate_names_conflict_before_either_commit() -> Result<()> {
        let (_temp, service) = service().await?;
        for filename in ["a.wasm", "b.wasm"] {
            fs::write(service.config.root.join(filename), fixture("same-name"))?;
        }
        let report = service.reconcile_once(false).await?;
        assert_eq!(report.with_status(SourceStatus::Conflict).len(), 2);
        assert!(service.members().await?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn sidecar_update_is_observed_and_missing_root_does_not_prune() -> Result<()> {
        let (_temp, service) = service().await?;
        fs::write(service.config.root.join("one.wasm"), fixture("named-one"))?;
        let first = service.reconcile_once(false).await?;
        assert_eq!(first.ids(SourceStatus::Installed), ["named-one"]);
        let before = service.members().await?.remove(0);
        fs::write(service.config.root.join("one.policy.yaml"), b"invalid: [")?;
        assert_eq!(
            service
                .reconcile_once(false)
                .await?
                .with_status(SourceStatus::Rejected)
                .len(),
            1
        );
        assert_eq!(service.members().await?[0].revision, before.revision);
        fs::remove_file(service.config.root.join("one.policy.yaml"))?;
        fs::remove_dir_all(&service.config.root)?;
        assert!(service.reconcile_once(false).await.is_err());
        assert_eq!(service.members().await?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn missing_root_created_privately_and_unsafe_root_fails() -> Result<()> {
        let temp = tempfile::Builder::new()
            .prefix(".local-create-")
            .tempdir_in(std::env::current_dir()?)?;
        let manager = Arc::new(
            LifecycleManager::builder(temp.path().join("store"))
                .with_secrets_dir(temp.path().join("secrets"))
                .with_eager_loading(false)
                .build()
                .await?,
        );
        let root = temp.path().join("new").join("drops");
        let config = LocalSourceConfig::new(root.clone(), LocalMode::Watch);
        assert!(!root.exists());
        let service = LocalSourceService::new(manager.clone(), config)?;
        assert!(root.is_dir());
        assert!(service.reconcile_once(false).await?.outcomes.is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&root)?.permissions().mode() & 0o777, 0o700);
            fs::set_permissions(&root, fs::Permissions::from_mode(0o777))?;
            assert!(service.reconcile_once(false).await.is_err());
            assert!(LocalSourceService::new(
                manager,
                LocalSourceConfig::new(root, LocalMode::Startup),
            )
            .is_err());
        }
        Ok(())
    }

    #[tokio::test]
    async fn sidecar_changes_update_observation_and_break_suppression() -> Result<()> {
        let (_temp, service) = service().await?;
        fs::write(service.config.root.join("one.wasm"), fixture("named-one"))?;
        let policy = service.config.root.join("one.policy.yaml");
        fs::write(
            &policy,
            b"version: '1.0'\ndescription: first\npermissions: {}\n",
        )?;
        assert_eq!(
            service
                .reconcile_once(false)
                .await?
                .ids(SourceStatus::Installed),
            ["named-one"]
        );
        let first = service.members().await?.remove(0);
        fs::write(
            &policy,
            b"version: '1.0'\ndescription: second\npermissions: {}\n",
        )?;
        assert_eq!(
            service
                .reconcile_once(false)
                .await?
                .ids(SourceStatus::Updated),
            ["named-one"]
        );
        let second = service.members().await?.remove(0);
        assert_ne!(first.observation, second.observation);
        service.manager.unload_component("named-one").await?;
        fs::remove_file(&policy)?;
        assert_eq!(
            service
                .reconcile_once(false)
                .await?
                .ids(SourceStatus::Installed),
            ["named-one"]
        );
        assert!(service.members().await?[0]
            .observation
            .as_ref()
            .unwrap()
            .sidecar_sha256
            .is_none());
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn writable_source_and_symlink_target_are_not_admitted() -> Result<()> {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let (temp, service) = service().await?;
        let path = service.config.root.join("tool.wasm");
        fs::write(&path, fixture("trusted"))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666))?;
        assert_eq!(
            service
                .reconcile_once(false)
                .await?
                .with_status(SourceStatus::Pending)
                .len(),
            1
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let target = temp.path().join("target.wasm");
        fs::write(&target, fixture("trusted"))?;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o666))?;
        fs::remove_file(&path)?;
        symlink(&target, &path)?;
        assert_eq!(
            service
                .reconcile_once(false)
                .await?
                .with_status(SourceStatus::Pending)
                .len(),
            1
        );
        assert!(service.members().await?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn report_serializes_explicit_statuses_and_details() -> Result<()> {
        let (_temp, service) = service().await?;
        fs::write(service.config.root.join("good.wasm"), fixture("named-good"))?;
        fs::write(service.config.root.join("bad.wasm"), b"not wasm")?;
        let report = service.reconcile_once(false).await?;
        assert!(report.has_unresolved() && report.prune_skipped);
        let value = serde_json::to_value(&report)?;
        let outcomes = value["outcomes"].as_array().unwrap();
        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0]["status"], "rejected");
        assert_eq!(outcomes[0]["source"], "bad.wasm");
        assert!(outcomes[0]["component_id"].is_null());
        assert!(outcomes[0]["detail"].is_string());
        assert_eq!(outcomes[1]["status"], "installed");
        assert_eq!(outcomes[1]["component_id"], "named-good");
        assert_eq!(value["prune_skipped"], true);
        serde_yaml::to_string(&report)?;
        Ok(())
    }

    #[test]
    fn filters_are_nonrecursive_and_source_overlap_is_rejected() -> Result<()> {
        let temp = tempfile::Builder::new()
            .prefix(".local-filter-")
            .tempdir_in(std::env::current_dir()?)?;
        fs::create_dir(temp.path().join("nested"))?;
        for name in [
            "z.wasm",
            "a.wasm",
            ".hidden.wasm",
            "a.wasm.part",
            "a.policy.yaml",
        ] {
            fs::write(temp.path().join(name), b"")?;
        }
        fs::write(temp.path().join("nested/deep.wasm"), b"")?;
        let names: Vec<_> = discover(temp.path())?
            .iter()
            .map(|path| path.file_name().unwrap().to_owned())
            .collect();
        assert_eq!(names, ["a.wasm", "z.wasm"]);
        let store = temp.path().join("store");
        fs::create_dir(&store)?;
        let config = LocalSourceConfig::new(store.join("drops"), LocalMode::Startup);
        assert!(config.validate(&store).is_err());
        Ok(())
    }
}
