// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{inspect_artifact, ArtifactInspection, ArtifactShape, ComponentId, StorageKey};

/// A store operation's result.
pub type Result<T> = std::result::Result<T, StoreError>;

/// A failed operation never implies that a transaction was committed.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// An operating-system operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Input or a persisted record failed validation.
    #[error(transparent)]
    Invalid(#[from] anyhow::Error),
    /// The expected revision, owner, source, or namespace no longer matches.
    #[error("component store conflict: {0}")]
    Conflict(String),
    /// No installed receipt has this semantic name.
    #[error("component is not installed: {0}")]
    NotFound(String),
    /// Files disagree with their receipt; no repair or adoption is inferred.
    #[error("component store integrity error: {0}")]
    Integrity(String),
    /// The operation may have committed; reopen/read the store before retrying.
    #[error("recovery required for transaction {operation}: {detail}")]
    RecoveryRequired {
        /// The durable operation identifier.
        operation: String,
        /// The error and, where known, recovery outcome.
        detail: String,
    },
}

/// An opaque, persistent store epoch and monotonic sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreCursor {
    pub(super) epoch: String,
    pub(super) sequence: u64,
}

impl std::fmt::Display for StoreCursor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}:{}", self.epoch, self.sequence)
    }
}

/// An ABA-safe revision, changed by every authoritative entry mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryRevision(pub(super) StoreCursor);

impl std::fmt::Display for EntryRevision {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Source continuity is distinct from a semantic name and from acquisition.
///
/// Adapters must canonicalize their source before constructing this value.
/// Equal names, owners, downloaded bytes, or release hashes do not establish it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceIdentity {
    /// Canonical OCI registry/repository, excluding tags and digest selectors.
    OciRepository(String),
    /// An absolute, canonical local source path captured by the adapter.
    File(PathBuf),
    /// A stable HTTPS source without persisting potentially credential-bearing queries.
    Https {
        /// Canonical HTTPS URL with user information, query, and fragment removed.
        location: String,
        /// SHA-256 of the complete canonical request URL; queries remain significant.
        request_sha256: String,
    },
}

impl SourceIdentity {
    pub(super) fn validate(&self) -> Result<()> {
        let valid = match self {
            Self::File(path) => path.is_absolute(),
            Self::Https {
                location,
                request_sha256,
            } => {
                request_sha256.len() == 64
                    && request_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && url::Url::parse(location).is_ok_and(|url| {
                        url.scheme() == "https"
                            && url.host_str().is_some()
                            && url.username().is_empty()
                            && url.password().is_none()
                            && url.query().is_none()
                            && url.fragment().is_none()
                    })
            }
            Self::OciRepository(repository) => {
                let (registry, path) = repository.split_once('/').unwrap_or_default();
                !registry.is_empty()
                    && !path.is_empty()
                    && !repository.contains(['@', '?', '#', '\\'])
                    && !path.contains(':')
                    && !path.split('/').any(|part| matches!(part, "" | "." | ".."))
                    && !repository.chars().any(char::is_whitespace)
                    && !repository.chars().any(char::is_control)
            }
        };
        if !valid {
            return Err(StoreError::Invalid(anyhow::anyhow!(
                "source identity is not an absolute file, canonical OCI repository, or HTTPS URL"
            )));
        }
        Ok(())
    }
}

/// Non-secret acquisition evidence supplied by the source adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginEvidence {
    /// The acquisition URI or path; credentials must be removed by the adapter.
    pub location: String,
    /// Requested package version/tag, if any.
    pub requested_version: Option<String>,
    /// Exact selected package version/tag, if any.
    pub selected_version: Option<String>,
    /// OCI manifest digest, not the Wasm artifact hash.
    pub manifest_digest: Option<String>,
    /// Immutable pull URI, when supplied by a resolver.
    pub immutable_uri: Option<String>,
}

impl OriginEvidence {
    pub(super) fn validate(&self) -> Result<()> {
        for location in std::iter::once(self.location.as_str()).chain(self.immutable_uri.as_deref())
        {
            validate_evidence_location(location)?;
        }
        Ok(())
    }
}

fn validate_evidence_location(location: &str) -> Result<()> {
    if location.contains("://") {
        let url = url::Url::parse(location)
            .map_err(|_| StoreError::Invalid(anyhow::anyhow!("invalid source evidence URI")))?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(StoreError::Invalid(anyhow::anyhow!(
                "source evidence must not contain user information, queries, or fragments"
            )));
        }
    }
    Ok(())
}

/// Exact local discovery ownership, without delimiter-based token ambiguity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedLocalSource {
    /// Stable discovery-root configuration key, not the artifact's source path.
    pub root_key: String,
    /// Normalized relative path below that root, with no parent/dot components.
    pub relative_source: PathBuf,
}

impl ManagedLocalSource {
    /// Construct a structured owner and validate its root and relative path.
    pub fn new(root_key: impl Into<String>, relative_source: impl Into<PathBuf>) -> Result<Self> {
        let owner = Self {
            root_key: root_key.into(),
            relative_source: relative_source.into(),
        };
        owner.validate()?;
        Ok(owner)
    }

    pub(super) fn validate(&self) -> Result<()> {
        let normalized: PathBuf = self.relative_source.components().collect();
        if self.root_key.trim().is_empty()
            || self.root_key.chars().any(char::is_control)
            || self.relative_source.as_os_str().is_empty()
            || !self
                .relative_source
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
            || normalized.as_os_str() != self.relative_source.as_os_str()
        {
            return Err(StoreError::Invalid(anyhow::anyhow!(
                "managed owner requires a nonblank root key and a normalized relative source"
            )));
        }
        Ok(())
    }
}

/// Installation ownership, independent of provenance and intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstallOwner {
    /// An explicit operator installation, never eligible for managed pruning.
    Explicit,
    /// A particular local discovery source; cleanup must match both exact fields.
    ManagedLocalSource(ManagedLocalSource),
}

/// Installation does not by itself register tools or activate an ACP provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstallIntent {
    /// Persist only.
    InstallOnly,
    /// The caller intends to expose ordinary tools after admission.
    ExposeTools,
    /// The caller intends to select this artifact through its ACP adapter.
    AcpSelection,
}

/// The checks actually completed by the trusted runtime validator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValidationEvidence {
    /// Ordinary-engine compilation, link/preparation, schema, and policy checks.
    OrdinaryPrepared {
        /// Runtime/engine compatibility identifier chosen by the adapter.
        runtime: String,
    },
    /// ACP compilation and export-world/version checks, not full host linking.
    AcpCompiledAndExportChecked {
        /// ACP runtime/engine compatibility identifier chosen by the adapter.
        runtime: String,
    },
}

/// The admitted L1 route, not a runtime execution or exposure guarantee.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoredArtifactKind {
    /// An ordinary tool candidate.
    Tool,
    /// An ACP provider.
    AcpProvider,
    /// An ACP layer.
    AcpLayer,
}

/// Where the effective policy came from, including explicit policy absence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyProvenance {
    /// Deny-by-default absence when no policy was selected.
    Default,
    /// Explicitly attached, or explicitly cleared, by an operator.
    ExplicitAttachment,
    /// Edited through a permission-management API.
    PermissionEdit,
    /// Selected from the incoming bundle.
    Bundled,
    /// Protected pre-receipt state, without inferred source ownership.
    Legacy,
}

impl PolicyProvenance {
    pub(super) fn protected(&self) -> bool {
        !matches!(self, Self::Default | Self::Bundled)
    }
}

/// Non-secret policy attachment metadata, committed together with effective YAML.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyMetadata {
    /// Policy acquisition URI/path with user information, query, and fragment removed.
    pub source_uri: String,
    /// Original attachment time in seconds since the Unix epoch, when known.
    pub attached_at: Option<u64>,
}

impl PolicyMetadata {
    pub(super) fn validate(&self) -> Result<()> {
        validate_evidence_location(&self.source_uri)
    }
}

/// Effective policy evidence; absent and malformed are never conflated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectivePolicy {
    /// SHA-256 of the exact effective YAML, or explicit absence.
    pub sha256: Option<String>,
    /// Authority for selecting these bytes or their absence.
    pub provenance: PolicyProvenance,
    /// Captured attachment metadata, independent of source-sidecar observations.
    pub metadata: Option<PolicyMetadata>,
    /// SHA-256 of the exact `.policy.meta.json` bytes, including explicit `null`.
    pub metadata_sha256: String,
}

/// Exact source observations, separate from the effective attached policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceObservation {
    /// Adapter-defined capture/fingerprint token.
    pub token: String,
    /// Captured source artifact digest.
    pub artifact_sha256: String,
    /// Source sidecar digest; may differ from effective policy evidence.
    pub sidecar_sha256: Option<String>,
}

/// A validated policy input whose captured bytes cannot subsequently change.
#[derive(Debug, Clone)]
pub struct PreparedPolicy {
    pub(super) bytes: Option<Vec<u8>>,
    pub(super) evidence: EffectivePolicy,
}

impl PreparedPolicy {
    /// Parse and validate exact YAML bytes without changing any live files.
    pub fn parse(bytes: Vec<u8>, provenance: PolicyProvenance) -> Result<Self> {
        if matches!(provenance, PolicyProvenance::Default) {
            return Err(StoreError::Invalid(anyhow::anyhow!(
                "default policy provenance requires absent policy"
            )));
        }
        policy::PolicyParser::parse_bytes(&bytes)?;
        Ok(Self {
            evidence: EffectivePolicy {
                sha256: Some(digest(&bytes)),
                provenance,
                metadata: None,
                metadata_sha256: digest(b"null"),
            },
            bytes: Some(bytes),
        })
    }

    /// Record deliberately absent policy, retaining its selection authority.
    pub fn absent(provenance: PolicyProvenance) -> Self {
        Self {
            bytes: None,
            evidence: EffectivePolicy {
                sha256: None,
                provenance,
                metadata: None,
                metadata_sha256: digest(b"null"),
            },
        }
    }

    /// Select exact attachment metadata or its explicit absence before commit.
    ///
    /// When retaining an existing effective policy, also retain its receipt's
    /// `metadata`; parsing only its YAML does not preserve attachment evidence.
    pub fn with_metadata(mut self, metadata: Option<PolicyMetadata>) -> Result<Self> {
        if let Some(metadata) = &metadata {
            metadata.validate()?;
        }
        self.evidence.metadata_sha256 =
            digest(&serde_json::to_vec(&metadata).map_err(anyhow::Error::from)?);
        self.evidence.metadata = metadata;
        Ok(self)
    }

    /// Borrow the captured policy bytes.
    pub fn bytes(&self) -> Option<&[u8]> {
        self.bytes.as_deref()
    }

    /// Borrow the effective policy evidence.
    pub fn evidence(&self) -> &EffectivePolicy {
        &self.evidence
    }
}

/// Inputs to preparation that do not substitute for embedded semantic identity.
#[derive(Debug, Clone)]
pub struct InstallOptions {
    /// Validated, stable private filename stem.
    pub storage_key: StorageKey,
    /// Canonical source-continuity identity.
    pub source: SourceIdentity,
    /// Acquisition and package-resolution evidence.
    pub origin: OriginEvidence,
    /// Installation owner.
    pub owner: InstallOwner,
    /// Post-install intent, not runtime authorization.
    pub intent: InstallIntent,
    /// Effective policy, independently selected from any source sidecar.
    pub policy: PreparedPolicy,
    /// Optional captured source observation.
    pub observation: Option<SourceObservation>,
}

/// Exact captured bytes admitted by L1 and a supplied trusted runtime validator.
#[derive(Debug)]
pub struct PreparedInstall {
    pub(super) wasm: Vec<u8>,
    pub(super) component_id: ComponentId,
    pub(super) artifact_sha256: String,
    pub(super) kind: StoredArtifactKind,
    pub(super) validation: ValidationEvidence,
    pub(super) options: InstallOptions,
}

impl PreparedInstall {
    /// Inspect and validate captured input, without locks, live writes, or execution.
    ///
    /// The callback is trusted to compile these exact bytes with its runtime and
    /// perform the checks named by its returned evidence. Ordinary callbacks must
    /// link/prepare and validate schema/policy; ACP callbacks compile and check
    /// export versions/worlds. Returning success is not permission to run guests.
    pub fn prepare(
        wasm: Vec<u8>,
        options: InstallOptions,
        validator: impl FnOnce(
            &[u8],
            &ArtifactInspection,
            Option<&[u8]>,
        ) -> anyhow::Result<ValidationEvidence>,
    ) -> Result<Self> {
        options.source.validate()?;
        options.origin.validate()?;
        if let InstallOwner::ManagedLocalSource(owner) = &options.owner {
            owner.validate()?;
            if !matches!(options.source, SourceIdentity::File(_)) {
                return Err(StoreError::Invalid(anyhow::anyhow!(
                    "managed-local ownership requires a local source"
                )));
            }
        }
        let inspection = inspect_artifact(&wasm)?;
        let component_id = inspection.identity.clone().map_err(anyhow::Error::from)?;
        let kind = match inspection.shape {
            ArtifactShape::ToolCandidate => StoredArtifactKind::Tool,
            ArtifactShape::AcpProvider => StoredArtifactKind::AcpProvider,
            ArtifactShape::AcpLayer => StoredArtifactKind::AcpLayer,
            ArtifactShape::Unsupported(ref reason) => {
                return Err(StoreError::Invalid(anyhow::anyhow!(
                    "unsupported artifact shape: {reason:?}"
                )));
            }
        };
        let validation = validator(&wasm, &inspection, options.policy.bytes())?;
        let (ordinary, runtime) = match &validation {
            ValidationEvidence::OrdinaryPrepared { runtime } => (true, runtime),
            ValidationEvidence::AcpCompiledAndExportChecked { runtime } => (false, runtime),
        };
        if ordinary != (kind == StoredArtifactKind::Tool) || runtime.is_empty() {
            return Err(StoreError::Invalid(anyhow::anyhow!(
                "validator evidence does not match the inspected artifact route"
            )));
        }
        let artifact_sha256 = digest(&wasm);
        if let Some(observation) = &options.observation {
            if observation.artifact_sha256 != artifact_sha256 {
                return Err(StoreError::Invalid(anyhow::anyhow!(
                    "source observation does not bind the captured artifact"
                )));
            }
        }
        Ok(Self {
            wasm,
            component_id,
            artifact_sha256,
            kind,
            validation,
            options,
        })
    }

    /// The actual unambiguous root name extracted from the captured bytes.
    pub fn component_id(&self) -> &ComponentId {
        &self.component_id
    }

    /// The captured artifact digest, suitable for a conditional derived cache.
    pub fn artifact_sha256(&self) -> &str {
        &self.artifact_sha256
    }
}

/// The authoritative installed binding and its validation/provenance evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallReceipt {
    /// Persisted record schema.
    pub schema: u32,
    /// Exact embedded root component name.
    #[serde(with = "component_id_serde")]
    pub component_id: ComponentId,
    /// Separate portable physical binding.
    #[serde(with = "storage_key_serde")]
    pub storage_key: StorageKey,
    /// Source-continuity identity, not authentication by semantic name.
    pub source: SourceIdentity,
    /// Acquisition evidence.
    pub origin: OriginEvidence,
    /// Exact owner used for compare-and-delete.
    pub owner: InstallOwner,
    /// Hash of the captured Wasm artifact.
    pub artifact_sha256: String,
    /// L1 route admitted by preparation.
    pub kind: StoredArtifactKind,
    /// Runtime checks actually performed.
    pub validation: ValidationEvidence,
    /// Digest and authority of the effective policy.
    pub policy: EffectivePolicy,
    /// Revision of this complete authoritative state.
    pub revision: EntryRevision,
    /// Installation intent, independent of exposure or activation.
    pub intent: InstallIntent,
    /// Last successful optional source capture.
    pub observation: Option<SourceObservation>,
}

impl InstallReceipt {
    /// Whether this ordinary artifact requests tool exposure, not runtime authorization.
    pub fn requests_tool_exposure(&self) -> bool {
        self.kind == StoredArtifactKind::Tool && self.intent == InstallIntent::ExposeTools
    }
}

/// Why an installed entry became a durable reservation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemovalReason {
    /// An explicit operator uninstall.
    ExplicitUninstall,
    /// Exact-owner cleanup after a managed source disappeared.
    SourceMissing,
}

/// A retired binding continues to reserve its semantic name and physical aliases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetiredEntry {
    /// The complete last receipt, including previous owner and source observation.
    pub previous: InstallReceipt,
    /// The retirement revision, distinct from the previous installed revision.
    pub revision: EntryRevision,
    /// Factual cause; source suppression policy belongs to discovery.
    pub reason: RemovalReason,
}

/// An active receipt or a durable retired reservation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoredEntry {
    /// Validated installed state.
    Installed(InstallReceipt),
    /// Removed state retaining continuity and ownership evidence.
    Retired(RetiredEntry),
}

impl StoredEntry {
    /// The reserved semantic identity.
    pub fn component_id(&self) -> &ComponentId {
        &self.binding().component_id
    }

    /// The reserved physical storage key.
    pub fn storage_key(&self) -> &StorageKey {
        &self.binding().storage_key
    }

    /// Current revision, including retirement.
    pub fn revision(&self) -> &EntryRevision {
        match self {
            Self::Installed(receipt) => &receipt.revision,
            Self::Retired(retired) => &retired.revision,
        }
    }

    /// Last known source/owner/policy evidence, even after removal.
    pub fn binding(&self) -> &InstallReceipt {
        match self {
            Self::Installed(receipt) => receipt,
            Self::Retired(retired) => &retired.previous,
        }
    }
}

/// A protected physical slot without a receipt; never automatically prunable.
#[derive(Debug, Clone)]
pub struct ProtectedLegacyEntry {
    /// Observed physical stem, even when it fails current key validation.
    pub physical_key: String,
    /// Valid key and its alias domains, when representable under L1.
    pub storage_key: Option<StorageKey>,
    /// Actual embedded identity when unambiguous; never a filename fallback.
    pub component_id: Option<ComponentId>,
    /// Missing/ambiguous/malformed identity, orphan-file, or I/O diagnostic.
    pub diagnostic: Option<String>,
    /// Hash of captured Wasm, when present and readable.
    pub artifact_sha256: Option<String>,
    /// Hash of captured effective sidecar, when present and readable.
    pub policy_sha256: Option<String>,
}

/// Coherent inventory at one committed cursor.
#[derive(Debug, Clone)]
pub struct StoreSnapshot {
    /// Durable cursor.
    pub cursor: StoreCursor,
    /// Installed and retired receipt bindings.
    pub entries: Vec<StoredEntry>,
    /// Unrecorded files protected against replacement and pruning.
    pub protected: Vec<ProtectedLegacyEntry>,
    /// Inactive transaction directories; not removed merely because they are old.
    pub abandoned_transactions: Vec<String>,
    /// Latest authoritative mutation, if any.
    pub last_change: Option<StoreChange>,
}

/// Captured immutable artifact/policy bytes and the receipt that binds them.
#[derive(Debug)]
pub struct ArtifactSnapshot {
    /// Receipt captured under the same shared lock as the open files.
    pub receipt: InstallReceipt,
    /// Exact Wasm bytes, hash-checked outside the shared lock.
    pub wasm: Vec<u8>,
    /// Exact policy bytes, or explicit absence.
    pub policy: Option<Vec<u8>>,
    /// Cursor at which the files were pinned.
    pub cursor: StoreCursor,
}

/// An opaque expected binding produced by observing a proposed installation.
#[derive(Debug, Clone)]
pub struct ExpectedEntry {
    pub(super) component_id: ComponentId,
    pub(super) storage_key: StorageKey,
    pub(super) source: SourceIdentity,
    pub(super) entry: Option<StoredEntry>,
}

impl ExpectedEntry {
    /// Current installed/retired state, or genuine absence.
    pub fn entry(&self) -> Option<&StoredEntry> {
        self.entry.as_ref()
    }
}

/// Distinct authority for operator removal and exact-owner managed cleanup.
#[derive(Debug, Clone)]
pub enum RemovalAuthority {
    /// Explicit uninstall; the expected revision still must match.
    Explicit,
    /// Remove only the matching managed owner, never an explicit adoption.
    Owned(ManagedLocalSource),
}

/// The operation that changed authoritative state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoreChangeCause {
    /// Installation, replacement, provenance update, or explicit adoption.
    Install,
    /// An effective-policy update.
    PolicyUpdate,
    /// Removal with its factual reason.
    Removal(RemovalReason),
}

/// One authoritative mutation, persisted with its commit decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreChange {
    /// Unique durable journal operation.
    pub operation: String,
    /// Semantic entry that changed.
    #[serde(with = "component_id_serde")]
    pub component_id: ComponentId,
    /// The operation's factual cause.
    pub cause: StoreChangeCause,
    /// Revision before mutation, including retired state.
    pub before: Option<EntryRevision>,
    /// New revision.
    pub after: EntryRevision,
    /// Whether artifact content/presence changed.
    pub artifact_changed: bool,
    /// Whether effective policy bytes/authority changed.
    pub policy_changed: bool,
    /// Whether provenance, validation, intent, or observations changed.
    pub provenance_changed: bool,
    /// Whether ownership changed.
    pub owner_changed: bool,
    /// The commit cursor.
    pub cursor: StoreCursor,
}

/// Successful commit or exact no-op; both return enough state for fresh hydration.
#[derive(Debug, Clone)]
pub struct CommitOutcome {
    /// Current installed or retired record.
    pub entry: StoredEntry,
    /// Committed cursor, including for no-ops.
    pub cursor: StoreCursor,
    /// Absent for an exact no-op installation or policy update.
    pub change: Option<StoreChange>,
}

/// Captured native cache inputs, always subordinate to a receipt revision.
#[derive(Debug)]
pub struct PreparedCache {
    /// Expected artifact hash.
    pub artifact_sha256: String,
    /// Exact engine/configuration compatibility identifier.
    pub engine: String,
    /// Metadata/native serialization compatibility identifier.
    pub schema: String,
    /// Derived JSON metadata.
    pub metadata: serde_json::Value,
    /// Trusted runtime serialization, never arbitrary guest-provided native code.
    pub native: Vec<u8>,
}

/// Hash-checked cache captured together with its authoritative receipt.
#[derive(Debug)]
pub struct CacheSnapshot {
    /// Receipt revision binding.
    pub revision: EntryRevision,
    /// Derived metadata.
    pub metadata: serde_json::Value,
    /// Exact native serialization.
    pub native: Vec<u8>,
}

pub(super) fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

mod component_id_serde {
    use super::*;

    pub fn serialize<S: serde::Serializer>(
        value: &ComponentId,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(value.as_str())
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<ComponentId, D::Error> {
        ComponentId::from_declared_name(&String::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

mod storage_key_serde {
    use super::*;

    pub fn serialize<S: serde::Serializer>(
        value: &StorageKey,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(value.as_str())
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<StorageKey, D::Error> {
        StorageKey::parse(&String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
