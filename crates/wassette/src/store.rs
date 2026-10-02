// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Synchronous, cooperating-reader transactional component storage.
//!
//! The permanent `.store.lock` guards a flat, immutable-replaced file layout.
//! Preparation and ordinary image copying happen outside that lock. A durable
//! undo/redo journal is activated before replacing live files; replacing
//! `.store-state.json` **last** is both the commit decision and the cursor.
//! Readers recover pending work before pinning files, then hash/read outside the
//! lock. Callers must not mutate these files directly or use live paths as
//! snapshots. Secrets are neither read, migrated, nor deleted here.
//! Unrecorded legacy slots are diagnostic-only, even when named: this core has
//! no migration/adoption API that infers their original source or ownership.
//!
//! Durability requires a local filesystem supporting advisory file locks, hard
//! links, atomic same-filesystem replacement, and directory synchronization.
//! Unsupported/read-only filesystems return errors; there is no unlocked or
//! delete-first fallback. Async callers should use their blocking executor.

mod journal;
mod types;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use journal::{Head, Transaction, SUFFIXES, TRANSACTIONS};
use serde::{Deserialize, Serialize};
use types::digest;
pub use types::*;

use crate::{inspect_artifact, ComponentId, StorageKey};

/// A filesystem store handle. Independent handles/processes share the same lock.
#[derive(Debug, Clone)]
pub struct ComponentStore {
    root: PathBuf,
    #[cfg(test)]
    failpoint: std::sync::Arc<std::sync::Mutex<Option<(&'static str, bool)>>>,
    #[cfg(test)]
    cache_publication_pauses:
        std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<CachePublicationPause>>>,
}

#[cfg(test)]
type CachePublicationPause = (
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);

/// A checked admission scope owning a shared store lock until dropped.
///
/// Within this scope, perform only bounded in-memory `try_lock`, swap, or clone
/// operations. If an in-memory lock is unavailable, drop this scope and retry;
/// never wait for that lock while holding the filesystem lock.
///
/// Do not await, invoke arbitrary callbacks, compile, fetch, copy files, execute
/// guests, construct runtime policy, or recursively access this store. Keep the
/// entire scope inside a synchronous/blocking operation. It is deliberately
/// neither `Send` nor `Sync` and must not escape to an async caller.
#[derive(Debug)]
#[must_use = "dropping the scope releases the checked admission lock"]
pub struct StoreReadScope {
    _lock: File,
    _thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl ComponentStore {
    /// Open/create a store and recover any pending transaction before returning.
    ///
    /// Opening does not infer ownership or synthesize receipts for legacy files.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(root.as_ref())?;
        let root = root.as_ref().canonicalize()?;
        journal::reject_symlink(&root.join(TRANSACTIONS))?;
        fs::create_dir_all(root.join(TRANSACTIONS))?;
        journal::initialize(&root)?;
        Ok(Self {
            root,
            #[cfg(test)]
            failpoint: Default::default(),
            #[cfg(test)]
            cache_publication_pauses: Default::default(),
        })
    }

    /// Capture all bindings unless this cooperating store's cursor is unchanged.
    ///
    /// Passing `None` always requests a complete inventory, including protected
    /// unrecorded files and inactive transaction diagnostics.
    pub fn snapshot_if_changed(
        &self,
        known: Option<&StoreCursor>,
    ) -> Result<Option<StoreSnapshot>> {
        {
            let _lock = journal::shared(&self.root)?;
            let head = journal::read_head(&self.root)?;
            if known == Some(&head.cursor) {
                return Ok(None);
            }
        }
        let capture = self.capture(None, None)?;
        Ok(Some(StoreSnapshot {
            cursor: capture.head.cursor,
            entries: capture.entries,
            protected: capture.protected,
            abandoned_transactions: capture.abandoned,
            last_change: capture.head.change,
        }))
    }

    /// Pin and capture the artifact/effective policy by semantic name.
    ///
    /// The returned bytes cannot be redirected by a concurrent replacement.
    /// Integrity errors, including operator edits not yet committed through the
    /// store, are explicit errors rather than policy absence or cache fallbacks.
    pub fn read(&self, id: &str) -> Result<ArtifactSnapshot> {
        let mut capture = self.capture(Some(id), None)?;
        let receipt = capture.installed(id)?.clone();
        let (wasm, policy) = capture.verify(&receipt)?;
        Ok(ArtifactSnapshot {
            receipt,
            wasm,
            policy,
            cursor: capture.head.cursor,
        })
    }

    /// Read a revision-checked generated build request, if it was retained.
    #[cfg(feature = "component-generation")]
    pub fn read_source(
        &self,
        id: &str,
        expected: Option<&EntryRevision>,
    ) -> Result<wassette_builder::BuildRequest> {
        let mut capture = self.capture(Some(id), None)?;
        let receipt = capture.installed(id)?.clone();
        if let Some(expected) = expected {
            check_revision(&receipt.revision, expected)?;
        }
        capture.verify(&receipt)?;
        let bytes = capture.source_bytes(&receipt)?.ok_or_else(|| {
            StoreError::NotFound(format!("no retained source for component {id}"))
        })?;
        let request: wassette_builder::BuildRequest = serde_json::from_slice(&bytes)
            .map_err(|error| StoreError::Integrity(format!("invalid source bundle: {error}")))?;
        verify_source_request(&request, &receipt)?;
        Ok(request)
    }

    /// Observe the exact binding needed for a later installation CAS.
    ///
    /// Retired state is not absence. Source identity and all physical/secret
    /// projection aliases must match existing reservations.
    pub fn observe(
        &self,
        id: &str,
        storage_key: &StorageKey,
        source: &SourceIdentity,
    ) -> Result<ExpectedEntry> {
        source.validate()?;
        let component_id = ComponentId::from_declared_name(id).map_err(anyhow::Error::from)?;
        let capture = self.capture(Some(id), Some(storage_key))?;
        capture.admit(&component_id, storage_key, source)?;
        Ok(ExpectedEntry {
            component_id,
            storage_key: storage_key.clone(),
            source: source.clone(),
            entry: capture.entry(id).cloned(),
        })
    }

    /// Commit an exact validated capture, or return a receipt-bearing exact no-op.
    ///
    /// A bundled policy cannot silently replace an explicit/edited/legacy
    /// effective policy, including explicitly absent policy. Managed ownership
    /// may be explicitly adopted, but cannot take over an explicit installation.
    pub fn commit_install(
        &self,
        prepared: PreparedInstall,
        expected: ExpectedEntry,
    ) -> Result<CommitOutcome> {
        self.commit_install_inner(prepared, expected, false, None)
    }

    /// Commit captured generated source and Wasm in one journal transaction.
    #[cfg(feature = "component-generation")]
    pub fn commit_generated_install(
        &self,
        prepared: PreparedInstall,
        expected: ExpectedEntry,
        source: Option<wassette_builder::BuildRequest>,
    ) -> Result<CommitOutcome> {
        let source = source
            .map(|request| {
                verify_source_request_for_install(&request, &prepared)?;
                serde_json::to_vec(&request).map_err(anyhow::Error::from)
            })
            .transpose()?;
        self.commit_install_inner(prepared, expected, false, source)
    }

    /// Commit a managed local installation that explicitly adopts an existing
    /// explicit local-file binding without changing its source identity.
    pub fn commit_install_adopting_explicit_local(
        &self,
        prepared: PreparedInstall,
        expected: ExpectedEntry,
    ) -> Result<CommitOutcome> {
        self.commit_install_inner(prepared, expected, true, None)
    }

    fn commit_install_inner(
        &self,
        prepared: PreparedInstall,
        expected: ExpectedEntry,
        adopt_explicit_local: bool,
        source_bundle: Option<Vec<u8>>,
    ) -> Result<CommitOutcome> {
        if prepared.component_id != expected.component_id
            || prepared.options.storage_key != expected.storage_key
            || prepared.options.source != expected.source
        {
            return Err(conflict(
                "prepared input does not match observed identity/source/key",
            ));
        }
        let mut capture = self.capture(
            Some(prepared.component_id.as_str()),
            Some(&prepared.options.storage_key),
        )?;
        capture.admit(
            &prepared.component_id,
            &prepared.options.storage_key,
            &prepared.options.source,
        )?;
        let before = capture.entry(prepared.component_id.as_str()).cloned();
        if before != expected.entry {
            return Err(conflict("installation expected state changed"));
        }
        if let Some(previous) = &before {
            let old = previous.binding();
            if let StoredEntry::Installed(receipt) = previous {
                capture.verify(receipt)?;
            }
            if old.policy.provenance.protected()
                && old.policy != prepared.options.policy.evidence
                && !prepared.options.policy.evidence.provenance.protected()
            {
                return Err(conflict(
                    "incoming bundle would replace protected effective policy",
                ));
            }
            match (&old.owner, &prepared.options.owner) {
                (InstallOwner::Explicit, InstallOwner::ManagedLocalSource(_)) => {
                    let allowed = adopt_explicit_local
                        && matches!(old.source, SourceIdentity::File(_))
                        && old.source == prepared.options.source;
                    if !allowed {
                        return Err(conflict(
                            "managed installation cannot take over explicit ownership",
                        ));
                    }
                }
                (InstallOwner::ManagedLocalSource(old), InstallOwner::ManagedLocalSource(new))
                    if old != new =>
                {
                    return Err(conflict("managed source owner changed"))
                }
                _ => {}
            }
        }
        let revision = EntryRevision(capture.head.next_cursor()?);
        let receipt = InstallReceipt {
            schema: if prepared.options.origin.generation.is_some() {
                2
            } else {
                1
            },
            component_id: prepared.component_id,
            storage_key: prepared.options.storage_key,
            source: prepared.options.source,
            origin: prepared.options.origin,
            owner: prepared.options.owner,
            artifact_sha256: prepared.artifact_sha256,
            source_bundle_sha256: source_bundle.as_deref().map(digest),
            kind: prepared.kind,
            validation: prepared.validation,
            policy: prepared.options.policy.evidence,
            revision,
            observation: prepared.options.observation,
        };
        if let Some(StoredEntry::Installed(previous)) = &before {
            let mut comparable = receipt.clone();
            comparable.revision = previous.revision.clone();
            if comparable == *previous {
                return self.unchanged(&capture, StoredEntry::Installed(previous.clone()));
            }
        }
        self.mutate(
            capture,
            StoredEntry::Installed(receipt),
            Mutation::Install {
                wasm: prepared.wasm,
                policy: prepared.options.policy.bytes,
                source_bundle,
            },
        )
    }

    /// Compare-and-commit a parsed effective policy, including explicit clearing.
    pub fn update_policy(
        &self,
        id: &str,
        expected: &EntryRevision,
        policy: PreparedPolicy,
    ) -> Result<CommitOutcome> {
        let mut capture = self.capture(Some(id), None)?;
        let mut receipt = capture.installed(id)?.clone();
        check_revision(&receipt.revision, expected)?;
        capture.verify(&receipt)?;
        if receipt.policy == policy.evidence {
            return self.unchanged(&capture, StoredEntry::Installed(receipt));
        }
        if receipt.policy.provenance.protected() && !policy.evidence.provenance.protected() {
            return Err(conflict("bundled policy cannot replace protected policy"));
        }
        receipt.policy = policy.evidence;
        receipt.revision = EntryRevision(capture.head.next_cursor()?);
        self.mutate(
            capture,
            StoredEntry::Installed(receipt),
            Mutation::Policy(policy.bytes),
        )
    }

    /// Remove authoritative artifact/policy/cache files but retain the reservation.
    ///
    /// Managed cleanup requires both the exact owner and the exact revision.
    /// Secret files are not among the transaction's paths.
    pub fn remove(
        &self,
        id: &str,
        expected: &EntryRevision,
        authority: RemovalAuthority,
    ) -> Result<CommitOutcome> {
        let mut capture = self.capture(Some(id), None)?;
        let previous = capture.installed(id)?.clone();
        check_revision(&previous.revision, expected)?;
        let reason = match authority {
            RemovalAuthority::Explicit => RemovalReason::ExplicitUninstall,
            RemovalAuthority::Owned(owner) => {
                if previous.owner != InstallOwner::ManagedLocalSource(owner) {
                    return Err(conflict("managed cleanup no longer owns this entry"));
                }
                RemovalReason::SourceMissing
            }
        };
        capture.verify(&previous)?;
        let entry = StoredEntry::Retired(RetiredEntry {
            previous,
            revision: EntryRevision(capture.head.next_cursor()?),
            reason,
        });
        self.mutate(capture, entry, Mutation::Remove)
    }

    /// Conditionally publish derived cache bytes, bound to revision/hash/engine/schema.
    ///
    /// Cache publication journals its files but does not advance the authoritative
    /// cursor. It never modifies or deletes `.wasm` or policy.
    pub fn publish_cache(
        &self,
        id: &str,
        expected: &EntryRevision,
        cache: PreparedCache,
    ) -> Result<()> {
        let mut capture = self.capture(Some(id), None)?;
        let receipt = capture.installed(id)?.clone();
        check_revision(&receipt.revision, expected)?;
        if cache.artifact_sha256 != receipt.artifact_sha256
            || cache.engine.is_empty()
            || cache.schema.is_empty()
        {
            return Err(conflict(
                "cache bindings do not match the installed artifact",
            ));
        }
        capture.verify(&receipt)?;
        let envelope = CacheEnvelope {
            revision: receipt.revision,
            artifact_sha256: receipt.artifact_sha256,
            engine: cache.engine,
            schema: cache.schema,
            native_sha256: digest(&cache.native),
            metadata: cache.metadata,
        };
        let metadata = serde_json::to_vec(&envelope).map_err(anyhow::Error::from)?;
        let mut transaction = Transaction::new(&self.root)?;
        for (suffix, bytes) in [
            (".metadata.json", metadata.as_slice()),
            (".cwasm", cache.native.as_slice()),
        ] {
            let path = format!("{}{suffix}", receipt.storage_key.as_str());
            transaction.replace(path.clone(), capture.pinned.remove(&path), Some(bytes))?;
        }
        let mut head = capture.head.clone();
        head.operation = Some(transaction.operation.clone());
        let transaction = transaction.seal(capture.head.clone(), head)?;
        #[cfg(test)]
        {
            let pause = self
                .cache_publication_pauses
                .lock()
                .expect("cache publication pause mutex")
                .pop_front();
            if let Some((ready, resume)) = pause {
                ready
                    .send(())
                    .map_err(|_| anyhow::anyhow!("Cache publication test listener closed"))?;
                resume.blocking_recv().map_err(anyhow::Error::from)?;
            }
        }
        let _lock = journal::exclusive(&self.root)?;
        self.recheck(&capture)?;
        transaction.commit(&self.root, |point| self.inject(point))
    }

    #[cfg(test)]
    pub(crate) fn pause_next_cache_publication(
        &self,
        ready: tokio::sync::oneshot::Sender<()>,
        resume: tokio::sync::oneshot::Receiver<()>,
    ) {
        self.cache_publication_pauses
            .lock()
            .expect("cache publication pause mutex")
            .push_back((ready, resume));
    }

    /// Capture an eligible native cache; stale/unbound caches are simply ineligible.
    pub fn read_cache(
        &self,
        id: &str,
        expected: &EntryRevision,
        engine: &str,
        schema: &str,
    ) -> Result<Option<CacheSnapshot>> {
        let mut capture = self.capture(Some(id), None)?;
        let receipt = capture.installed(id)?.clone();
        check_revision(&receipt.revision, expected)?;
        capture.verify(&receipt)?;
        let Some(metadata) = capture
            .pinned
            .remove(&format!("{}.metadata.json", receipt.storage_key.as_str()))
        else {
            return Ok(None);
        };
        let envelope: CacheEnvelope = journal::read_json_file(metadata)?;
        if envelope.revision != receipt.revision
            || envelope.artifact_sha256 != receipt.artifact_sha256
            || envelope.engine != engine
            || envelope.schema != schema
        {
            return Ok(None);
        }
        let Some(mut native) = capture
            .pinned
            .remove(&format!("{}.cwasm", receipt.storage_key.as_str()))
        else {
            return Ok(None);
        };
        let bytes = read_bytes(&mut native)?;
        if digest(&bytes) != envelope.native_sha256 {
            return Ok(None);
        }
        Ok(Some(CacheSnapshot {
            revision: receipt.revision,
            metadata: envelope.metadata,
            native: bytes,
        }))
    }

    /// Acquire a scope for already-prepared, revision-checked runtime admission.
    ///
    /// The returned [`StoreReadScope`] owns the shared filesystem lock. Use only
    /// bounded in-memory `try_lock`, swap, or clone operations before dropping it.
    /// Drop and retry if an in-memory lock is unavailable; do not await or block.
    pub fn checked_read(
        &self,
        cursor: &StoreCursor,
        entry: Option<(&str, &EntryRevision)>,
    ) -> Result<StoreReadScope> {
        let lock = journal::shared(&self.root)?;
        let index = self.index()?;
        if &index.head.cursor != cursor {
            return Err(conflict("store cursor changed before admission"));
        }
        if let Some((id, revision)) = entry {
            let receipt = index
                .entries
                .iter()
                .find_map(|entry| match entry {
                    StoredEntry::Installed(receipt) if receipt.component_id.as_str() == id => {
                        Some(receipt)
                    }
                    _ => None,
                })
                .ok_or_else(|| StoreError::NotFound(id.into()))?;
            check_revision(&receipt.revision, revision)?;
        }
        Ok(StoreReadScope {
            _lock: lock,
            _thread_bound: std::marker::PhantomData,
        })
    }

    fn unchanged(&self, capture: &Capture, entry: StoredEntry) -> Result<CommitOutcome> {
        let _lock = journal::shared(&self.root)?;
        self.recheck(capture)?;
        Ok(CommitOutcome {
            entry,
            cursor: capture.head.cursor.clone(),
            change: None,
        })
    }

    fn mutate(
        &self,
        mut capture: Capture,
        entry: StoredEntry,
        mutation: Mutation,
    ) -> Result<CommitOutcome> {
        let mut transaction = Transaction::new(&self.root)?;
        let before = capture.entry(entry.component_id().as_str());
        let cause = match (&mutation, &entry) {
            (Mutation::Install { .. }, _) => StoreChangeCause::Install,
            (Mutation::Policy(_), _) => StoreChangeCause::PolicyUpdate,
            (Mutation::Remove, StoredEntry::Retired(retired)) => {
                StoreChangeCause::Removal(retired.reason.clone())
            }
            (Mutation::Remove, StoredEntry::Installed(_)) => {
                return Err(StoreError::Integrity(
                    "removal requires retired record".into(),
                ));
            }
        };
        let change = make_change(&transaction.operation, before, &entry, cause);
        let cursor = entry.revision().0.clone();
        let head = Head {
            schema: 1,
            cursor: cursor.clone(),
            operation: Some(transaction.operation.clone()),
            change: Some(change.clone()),
        };
        let key = entry.storage_key().as_str();
        let record = serde_json::to_vec(&entry).map_err(anyhow::Error::from)?;
        let policy_meta = match &entry {
            StoredEntry::Installed(receipt) => {
                Some(serde_json::to_vec(&receipt.policy.metadata).map_err(anyhow::Error::from)?)
            }
            StoredEntry::Retired(_) => None,
        };
        for suffix in SUFFIXES {
            if *suffix == ".wasm" && matches!(mutation, Mutation::Policy(_)) {
                continue;
            }
            let new = match (*suffix, &mutation) {
                (".install.json", _) => Some(record.as_slice()),
                (".wasm", Mutation::Install { wasm, .. }) => Some(wasm.as_slice()),
                (".source.json", Mutation::Install { source_bundle, .. }) => {
                    source_bundle.as_deref()
                }
                (".source.json", Mutation::Policy(_)) => continue,
                (".policy.yaml", Mutation::Install { policy, .. } | Mutation::Policy(policy)) => {
                    policy.as_deref()
                }
                (".policy.meta.json", _) => policy_meta.as_deref(),
                _ => None,
            };
            let path = format!("{key}{suffix}");
            transaction.replace(path.clone(), capture.pinned.remove(&path), new)?;
        }
        let transaction = transaction.seal(capture.head.clone(), head)?;
        let _lock = journal::exclusive(&self.root)?;
        self.recheck(&capture)?;
        transaction.commit(&self.root, |point| self.inject(point))?;
        Ok(CommitOutcome {
            entry,
            cursor,
            change: Some(change),
        })
    }

    fn recheck(&self, capture: &Capture) -> Result<()> {
        let current = self.index()?;
        if current.head != capture.head
            || current.stamps != capture.stamps
            || current.entries != capture.entries
        {
            return Err(conflict(
                "store changed while transaction images were prepared",
            ));
        }
        Ok(())
    }

    fn capture(&self, id: Option<&str>, key: Option<&StorageKey>) -> Result<Capture> {
        let lock = journal::shared(&self.root)?;
        let index = self.index()?;
        let key = key.map(StorageKey::as_str).or_else(|| {
            index
                .entries
                .iter()
                .find(|entry| Some(entry.component_id().as_str()) == id)
                .map(|entry| entry.storage_key().as_str())
        });
        let mut pinned = BTreeMap::new();
        if let Some(key) = key {
            for suffix in SUFFIXES {
                let path = format!("{key}{suffix}");
                if let Some(file) = journal::open_optional(&self.root.join(&path))? {
                    pinned.insert(path, file);
                }
            }
        }
        let recorded: BTreeSet<_> = index
            .entries
            .iter()
            .map(|entry| entry.storage_key().as_str())
            .collect();
        let mut legacy = BTreeMap::new();
        for name in index.stamps.keys() {
            let Some(key) = physical_key(name) else {
                continue;
            };
            if !recorded.contains(key) {
                legacy.entry(key.to_owned()).or_insert_with(Vec::new);
                if name.ends_with(".wasm") || name.ends_with(".policy.yaml") {
                    if let Some(file) = journal::open_optional(&self.root.join(name))? {
                        legacy
                            .get_mut(key)
                            .expect("legacy slot inserted")
                            .push((name.clone(), file));
                    }
                }
            }
        }
        let abandoned = fs::read_dir(self.root.join(TRANSACTIONS))?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect::<std::io::Result<Vec<_>>>()?;
        drop(lock);
        let protected = legacy
            .into_iter()
            .map(|(key, files)| legacy_entry(key, files))
            .collect::<Result<_>>()?;
        Ok(Capture {
            head: index.head,
            entries: index.entries,
            stamps: index.stamps,
            protected,
            pinned,
            abandoned,
        })
    }

    /// Small records and metadata only; caller holds a global lock.
    fn index(&self) -> Result<Index> {
        let head = journal::read_head(&self.root)?;
        let mut stamps = BTreeMap::new();
        let mut entries = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err(StoreError::Integrity("non-UTF-8 store filename".into()));
            };
            if physical_key(name).is_none() {
                continue;
            }
            journal::reject_symlink(&entry.path())?;
            let metadata = entry.metadata()?;
            if !metadata.is_file() {
                return Err(StoreError::Integrity(format!(
                    "non-file store entry: {name}"
                )));
            }
            stamps.insert(name.to_owned(), FileStamp::from_metadata(&metadata)?);
            if name.ends_with(".install.json") {
                let record: StoredEntry = journal::read_json(&entry.path())?;
                validate_record(&record, &head)?;
                if format!("{}.install.json", record.storage_key().as_str()) != name {
                    return Err(StoreError::Integrity(
                        "receipt/key filename mismatch".into(),
                    ));
                }
                entries.push(record);
            }
        }
        entries.sort_by(|left, right| {
            left.storage_key()
                .as_str()
                .cmp(right.storage_key().as_str())
        });
        for (index, entry) in entries.iter().enumerate() {
            for other in &entries[..index] {
                if entry.component_id() == other.component_id()
                    || aliases(entry.storage_key().as_str(), other.storage_key().as_str())
                {
                    return Err(StoreError::Integrity(
                        "conflicting persisted reservations".into(),
                    ));
                }
            }
            let wasm = stamps.contains_key(&format!("{}.wasm", entry.storage_key().as_str()));
            if wasm != matches!(entry, StoredEntry::Installed(_)) {
                return Err(StoreError::Integrity(
                    "receipt/artifact presence mismatch".into(),
                ));
            }
            let source =
                stamps.contains_key(&format!("{}.source.json", entry.storage_key().as_str()));
            if source
                != matches!(entry, StoredEntry::Installed(receipt) if receipt.source_bundle_sha256.is_some())
            {
                return Err(StoreError::Integrity(
                    "receipt/source presence mismatch".into(),
                ));
            }
        }
        Ok(Index {
            head,
            entries,
            stamps,
        })
    }

    fn inject(&self, point: &str) -> Result<()> {
        #[cfg(test)]
        {
            let mut selected = self.failpoint.lock().expect("test failpoint mutex");
            if selected.as_ref().is_some_and(|(name, _)| *name == point) {
                let (_, crash) = selected.take().expect("selected failpoint");
                drop(selected);
                if crash {
                    panic!("simulated process interruption: {point}");
                }
                return Err(std::io::Error::other(format!("injected failure: {point}")).into());
            }
            if std::env::var("WASSETTE_STORE_CRASH_POINT").as_deref() == Ok(point) {
                std::process::exit(91);
            }
        }
        let _ = point;
        Ok(())
    }

    #[cfg(all(test, feature = "component-generation"))]
    pub(crate) fn fail_next_commit_for_test(&self, point: &'static str) {
        *self.failpoint.lock().expect("test failpoint mutex") = Some((point, false));
    }
}

enum Mutation {
    Install {
        wasm: Vec<u8>,
        policy: Option<Vec<u8>>,
        source_bundle: Option<Vec<u8>>,
    },
    Policy(Option<Vec<u8>>),
    Remove,
}

#[cfg(feature = "component-generation")]
fn verify_source_request_for_install(
    request: &wassette_builder::BuildRequest,
    prepared: &PreparedInstall,
) -> Result<()> {
    let evidence = prepared.options.origin.generation.as_ref().ok_or_else(|| {
        StoreError::Invalid(anyhow::anyhow!("source requires generation evidence"))
    })?;
    if request.component_name != prepared.component_id.as_str()
        || digest(request.source.as_bytes()) != evidence.source_sha256
        || digest(request.wit.as_bytes()) != evidence.wit_sha256
        || request.world != evidence.world
        || !matches!(
            (&request.kind, &prepared.kind),
            (
                wassette_builder::ComponentKind::Tool,
                StoredArtifactKind::Tool
            ) | (
                wassette_builder::ComponentKind::AcpLayer,
                StoredArtifactKind::AcpLayer
            )
        )
    {
        return Err(StoreError::Invalid(anyhow::anyhow!(
            "source bundle does not match generated artifact evidence"
        )));
    }
    Ok(())
}

#[cfg(feature = "component-generation")]
fn verify_source_request(
    request: &wassette_builder::BuildRequest,
    receipt: &InstallReceipt,
) -> Result<()> {
    let evidence =
        receipt.origin.generation.as_ref().ok_or_else(|| {
            StoreError::Integrity("source bundle has no generation evidence".into())
        })?;
    if request.component_name != receipt.component_id.as_str()
        || digest(request.source.as_bytes()) != evidence.source_sha256
        || digest(request.wit.as_bytes()) != evidence.wit_sha256
        || request.world != evidence.world
        || !matches!(
            (&request.kind, &receipt.kind),
            (
                wassette_builder::ComponentKind::Tool,
                StoredArtifactKind::Tool
            ) | (
                wassette_builder::ComponentKind::AcpLayer,
                StoredArtifactKind::AcpLayer
            )
        )
    {
        return Err(StoreError::Integrity(
            "source bundle differs from generation evidence".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl FileStamp {
    fn from_metadata(metadata: &fs::Metadata) -> Result<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified()?,
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        })
    }
}

struct Index {
    head: Head,
    entries: Vec<StoredEntry>,
    stamps: BTreeMap<String, FileStamp>,
}

struct Capture {
    head: Head,
    entries: Vec<StoredEntry>,
    stamps: BTreeMap<String, FileStamp>,
    protected: Vec<ProtectedLegacyEntry>,
    pinned: BTreeMap<String, File>,
    abandoned: Vec<String>,
}

impl Capture {
    fn entry(&self, id: &str) -> Option<&StoredEntry> {
        self.entries
            .iter()
            .find(|entry| entry.component_id().as_str() == id)
    }

    fn installed(&self, id: &str) -> Result<&InstallReceipt> {
        match self.entry(id) {
            Some(StoredEntry::Installed(receipt)) => Ok(receipt),
            _ => Err(StoreError::NotFound(id.into())),
        }
    }

    fn admit(&self, id: &ComponentId, key: &StorageKey, source: &SourceIdentity) -> Result<()> {
        for entry in &self.entries {
            if entry.component_id() == id || aliases(entry.storage_key().as_str(), key.as_str()) {
                let binding = entry.binding();
                if binding.component_id != *id
                    || binding.storage_key != *key
                    || binding.source != *source
                {
                    return Err(conflict(
                        "semantic name, source, or storage aliases are reserved",
                    ));
                }
            }
        }
        for legacy in &self.protected {
            if legacy.component_id.as_ref() == Some(id)
                || aliases(&legacy.physical_key, key.as_str())
            {
                return Err(conflict(
                    "protected unrecorded files reserve this name or storage alias",
                ));
            }
        }
        Ok(())
    }

    fn verify(&mut self, receipt: &InstallReceipt) -> Result<(Vec<u8>, Option<Vec<u8>>)> {
        self.source_bytes(receipt)?;
        let key = receipt.storage_key.as_str();
        let mut get = |suffix: &str| -> Result<Option<Vec<u8>>> {
            self.pinned
                .get_mut(&format!("{key}{suffix}"))
                .map(read_bytes)
                .transpose()
        };
        let wasm = get(".wasm")?
            .ok_or_else(|| StoreError::Integrity("receipt has no pinned Wasm".into()))?;
        if digest(&wasm) != receipt.artifact_sha256 {
            return Err(StoreError::Integrity(
                "artifact hash differs from receipt".into(),
            ));
        }
        let policy = get(".policy.yaml")?;
        if policy.as_deref().map(digest) != receipt.policy.sha256 {
            return Err(StoreError::Integrity(
                "effective policy was modified outside its receipt".into(),
            ));
        }
        let meta = get(".policy.meta.json")?
            .ok_or_else(|| StoreError::Integrity("missing policy provenance record".into()))?;
        if digest(&meta) != receipt.policy.metadata_sha256 {
            return Err(StoreError::Integrity(
                "policy attachment metadata was modified outside its receipt".into(),
            ));
        }
        let evidence: Option<PolicyMetadata> = serde_json::from_slice(&meta).map_err(|error| {
            StoreError::Integrity(format!("invalid policy provenance: {error}"))
        })?;
        if evidence != receipt.policy.metadata {
            return Err(StoreError::Integrity(
                "policy provenance differs from receipt".into(),
            ));
        }
        Ok((wasm, policy))
    }

    fn source_bytes(&mut self, receipt: &InstallReceipt) -> Result<Option<Vec<u8>>> {
        let path = format!("{}.source.json", receipt.storage_key.as_str());
        if let Some(file) = self.pinned.get(&path) {
            if file.metadata()?.len() > 8 * 1024 * 1024 {
                return Err(StoreError::Integrity(
                    "source bundle exceeds size limit".into(),
                ));
            }
        }
        let bytes = self.pinned.get_mut(&path).map(read_bytes).transpose()?;
        if bytes.as_deref().map(digest) != receipt.source_bundle_sha256 {
            return Err(StoreError::Integrity(
                "source bundle differs from receipt".into(),
            ));
        }
        Ok(bytes)
    }
}

#[derive(Serialize, Deserialize)]
struct CacheEnvelope {
    revision: EntryRevision,
    artifact_sha256: String,
    engine: String,
    schema: String,
    native_sha256: String,
    metadata: serde_json::Value,
}

fn read_bytes(file: &mut File) -> Result<Vec<u8>> {
    file.rewind()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    file.rewind()?;
    Ok(bytes)
}

fn physical_key(name: &str) -> Option<&str> {
    SUFFIXES.iter().find_map(|suffix| name.strip_suffix(suffix))
}

fn aliases(left: &str, right: &str) -> bool {
    let keys = |raw: &str| match StorageKey::parse(raw) {
        Ok(key) => key.collision_keys(),
        Err(_) => crate::StorageCollisionKeys {
            artifact: raw.to_ascii_lowercase(),
            legacy_secrets: crate::identity::legacy_secret_stem(raw).to_ascii_lowercase(),
        },
    };
    let left = keys(left);
    let right = keys(right);
    left.artifact == right.artifact || left.legacy_secrets == right.legacy_secrets
}

fn legacy_entry(key: String, files: Vec<(String, File)>) -> Result<ProtectedLegacyEntry> {
    let mut entry = ProtectedLegacyEntry {
        storage_key: StorageKey::parse(&key).ok(),
        physical_key: key,
        component_id: None,
        diagnostic: Some("unrecorded slot has no artifact".into()),
        artifact_sha256: None,
        policy_sha256: None,
    };
    for (name, mut file) in files {
        let bytes = read_bytes(&mut file)?;
        if name.ends_with(".wasm") {
            entry.artifact_sha256 = Some(digest(&bytes));
            match inspect_artifact(&bytes) {
                Ok(inspection) => match inspection.identity {
                    Ok(id) => {
                        entry.component_id = Some(id);
                        entry.diagnostic =
                            Some("protected legacy artifact: source and validation unknown".into());
                    }
                    Err(error) => entry.diagnostic = Some(error.to_string()),
                },
                Err(error) => {
                    entry.diagnostic = Some(format!("malformed legacy artifact: {error}"))
                }
            }
        } else {
            entry.policy_sha256 = Some(digest(&bytes));
        }
    }
    Ok(entry)
}

fn validate_record(entry: &StoredEntry, head: &Head) -> Result<()> {
    let receipt = entry.binding();
    let valid_revision = |revision: &EntryRevision| {
        revision.0.epoch == head.cursor.epoch
            && revision.0.sequence > 0
            && revision.0.sequence <= head.cursor.sequence
    };
    let hash =
        |value: &str| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit());
    let (ordinary, runtime) = match &receipt.validation {
        ValidationEvidence::OrdinaryPrepared { runtime } => (true, runtime),
        ValidationEvidence::AcpCompiledAndExportChecked { runtime } => (false, runtime),
    };
    let expected_schema = if receipt.origin.generation.is_some() {
        2
    } else {
        1
    };
    if receipt.schema != expected_schema
        || !valid_revision(entry.revision())
        || !valid_revision(&receipt.revision)
        || !hash(&receipt.artifact_sha256)
        || receipt
            .source_bundle_sha256
            .as_ref()
            .is_some_and(|value| !hash(value))
        || (receipt.source_bundle_sha256.is_some() && receipt.origin.generation.is_none())
        || !hash(&receipt.policy.metadata_sha256)
        || (receipt.policy.provenance == PolicyProvenance::Default
            && receipt.policy.sha256.is_some())
        || receipt
            .policy
            .sha256
            .as_ref()
            .is_some_and(|digest| !hash(digest))
        || runtime.is_empty()
        || ordinary != (receipt.kind == StoredArtifactKind::Tool)
    {
        return Err(StoreError::Integrity(
            "unsupported or invalid installation receipt".into(),
        ));
    }
    receipt.source.validate()?;
    receipt.origin.validate()?;
    types::validate_generation_binding(
        &receipt.source,
        &receipt.origin,
        &receipt.kind,
        &receipt.owner,
    )?;
    if let Some(metadata) = &receipt.policy.metadata {
        metadata.validate()?;
    }
    if let InstallOwner::ManagedLocalSource(owner) = &receipt.owner {
        owner.validate()?;
        if !matches!(receipt.source, SourceIdentity::File(_)) {
            return Err(StoreError::Integrity(
                "invalid managed-local ownership in receipt".into(),
            ));
        }
    }
    if receipt
        .observation
        .as_ref()
        .is_some_and(|observation| observation.artifact_sha256 != receipt.artifact_sha256)
    {
        return Err(StoreError::Integrity(
            "source observation does not bind receipt artifact".into(),
        ));
    }
    if let StoredEntry::Retired(retired) = entry {
        if retired.revision.0.sequence <= receipt.revision.0.sequence {
            return Err(StoreError::Integrity("invalid retirement revision".into()));
        }
    }
    Ok(())
}

fn check_revision(actual: &EntryRevision, expected: &EntryRevision) -> Result<()> {
    if actual != expected {
        Err(conflict("entry revision changed"))
    } else {
        Ok(())
    }
}

fn conflict(message: &str) -> StoreError {
    StoreError::Conflict(message.into())
}

fn make_change(
    operation: &str,
    before: Option<&StoredEntry>,
    after: &StoredEntry,
    cause: StoreChangeCause,
) -> StoreChange {
    let old = before.map(StoredEntry::binding);
    let new = after.binding();
    let removal = matches!(after, StoredEntry::Retired(_));
    StoreChange {
        operation: operation.into(),
        component_id: after.component_id().clone(),
        cause,
        before: before.map(|entry| entry.revision().clone()),
        after: after.revision().clone(),
        artifact_changed: removal
            || matches!(before, Some(StoredEntry::Retired(_)))
            || old.is_none_or(|old| old.artifact_sha256 != new.artifact_sha256),
        policy_changed: removal || old.is_none_or(|old| old.policy != new.policy),
        provenance_changed: old.is_none_or(|old| {
            old.origin != new.origin
                || old.source != new.source
                || old.source_bundle_sha256 != new.source_bundle_sha256
                || old.validation != new.validation
                || old.observation != new.observation
        }),
        owner_changed: old.is_none_or(|old| old.owner != new.owner),
        cursor: after.revision().0.clone(),
    }
}

#[cfg(test)]
mod tests;
