// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Opt-in, two-phase generation over the existing validated component store.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use wassette_builder::BuildArtifact;
pub use wassette_builder::{
    BuildError, BuildErrorKind, BuildLimits, BuildRequest, Builder, BuilderConfig, ComponentKind,
};

use crate::loader::CapturedComponent;
use crate::local_source::LocalValidator;
use crate::store::{
    CommitOutcome, ExpectedEntry, GenerationEvidence, InstallIntent, InstallOptions, InstallOwner,
    OriginEvidence, PolicyProvenance, PreparedInstall, PreparedPolicy, SourceIdentity, StoreError,
    StoredArtifactKind, StoredEntry, ValidationEvidence,
};
use crate::store_support::{source_binding_key, store_operation};
use crate::{
    inspect_artifact, ArtifactShape, LifecycleManager, RefreshReport, SecretBinding, StorageKey,
};

const MAX_REINSTALL_POLICY_BYTES: usize = 128 * 1024;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

mod caller;
pub(crate) use caller::GenerationCaller;
pub(crate) mod host;
pub use caller::GenerationCallerGrant;

/// Trusted operator configuration, separate from every model/guest request.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationConfig {
    /// Explicit packaged helper and trusted, digest-pinned local initrd.
    pub builder: BuilderConfig,
    /// Finite host limits; requests cannot override these.
    #[serde(default)]
    pub limits: BuildLimits,
    /// Explicit opt-in to compilation.
    #[serde(default)]
    pub allow_build: bool,
    /// Explicit opt-in to store mutation.
    #[serde(default)]
    pub allow_install: bool,
    /// Explicit opt-in to ordinary-tool eligibility.
    #[serde(default)]
    pub allow_expose: bool,
    /// Explicit opt-in to replacing an existing generated lineage.
    #[serde(default)]
    pub allow_rebuild: bool,
    /// Optional revision-bound grants for ordinary Wasm callers without a UI route.
    #[serde(default)]
    pub callers: Vec<GenerationCallerGrant>,
}

impl GenerationConfig {
    /// Read a bounded operator-selected JSON file, never a model-selected path.
    ///
    /// Relative helper/image/staging/crate-archive paths are relative to the
    /// configuration file.
    pub fn read(path: &Path) -> Result<Self> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .context("opening trusted component-generation configuration")?
            .take(MAX_CONFIG_BYTES + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= MAX_CONFIG_BYTES,
            "generation configuration exceeds its size limit"
        );
        let mut config: Self = serde_json::from_slice(&bytes)
            .context("parsing trusted component-generation configuration")?;
        let base = std::path::absolute(path)?
            .parent()
            .context("generation configuration has no parent directory")?
            .to_path_buf();
        if config.builder.helper_path.is_relative() {
            config.builder.helper_path = base.join(&config.builder.helper_path);
        }
        if config.builder.initrd_path.is_relative() {
            config.builder.initrd_path = base.join(&config.builder.initrd_path);
        }
        if config.builder.staging_root.is_relative() {
            config.builder.staging_root = base.join(&config.builder.staging_root);
        }
        for krate in &mut config.builder.rust_crates {
            if krate.archive_path.is_relative() {
                krate.archive_path = base.join(&krate.archive_path);
            }
        }
        Ok(config)
    }

    /// Create the configured service without starting a VM.
    pub fn into_service(self) -> Result<GenerationService> {
        let permissions = GenerationPermissions::new(
            self.allow_build,
            self.allow_install,
            self.allow_expose,
            self.allow_rebuild,
        );
        GenerationService::new(Builder::new(self.builder, self.limits)?, permissions)
            .with_callers(self.callers)
    }
}

/// Host-issued authority; it is deliberately not deserializable from a request.
#[derive(Debug, Clone, Copy, Default)]
pub struct GenerationPermissions {
    build: bool,
    install: bool,
    expose: bool,
    rebuild: bool,
}

impl GenerationPermissions {
    /// Set independently authorized operations in trusted host configuration or UI.
    pub fn new(build: bool, install: bool, expose: bool, rebuild: bool) -> Self {
        Self {
            build,
            install,
            expose,
            rebuild,
        }
    }

    /// Whether building is authorized, independently of installation.
    pub fn can_build(self) -> bool {
        self.build
    }

    /// Whether committing a generated artifact is authorized.
    pub fn can_install(self) -> bool {
        self.install
    }

    /// Whether ordinary-tool exposure is authorized.
    pub fn can_expose(self) -> bool {
        self.expose
    }

    /// Whether an existing generated lineage may be rebuilt.
    pub fn can_rebuild(self) -> bool {
        self.rebuild
    }
}

/// New lineage versus an explicitly selected existing revision.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(tag = "mode", rename_all = "kebab-case", deny_unknown_fields)]
pub enum GenerationTarget {
    /// Mint a new host-owned lineage; any existing semantic name conflicts.
    #[default]
    New,
    /// Rebuild only an existing generated binding at this exact revision.
    Rebuild {
        /// Opaque display token previously returned by the store.
        expected_revision: String,
    },
}

impl<'de> Deserialize<'de> for GenerationTarget {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "mode", rename_all = "kebab-case", deny_unknown_fields)]
        enum Input {
            New {},
            Rebuild { expected_revision: String },
        }
        Ok(match Input::deserialize(deserializer)? {
            Input::New {} => Self::New,
            Input::Rebuild { expected_revision } => Self::Rebuild { expected_revision },
        })
    }
}

/// Untrusted build inputs and requested intent, not permission or source ownership.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationRequest {
    /// Bounded source/WIT inputs; builder paths and flags are not request fields.
    pub build: BuildRequest,
    /// Whether to create or explicitly rebuild a lineage.
    #[serde(default)]
    pub target: GenerationTarget,
    /// Install-only by default; ACP layers cannot request ordinary-tool exposure.
    #[serde(default = "install_only")]
    pub intent: InstallIntent,
    /// Optional exact former policy bytes for a retired lineage; never new grants.
    #[serde(default)]
    pub reinstall_policy: Option<String>,
}

fn install_only() -> InstallIntent {
    InstallIntent::InstallOnly
}

/// Captured output information suitable for an install permission prompt.
#[derive(Debug, Clone, Serialize)]
pub struct GenerationPreview {
    /// Actual root identity inspected from captured output.
    pub component_id: String,
    /// Requested and verified output role.
    pub kind: ComponentKind,
    /// Exact finalized Wasm digest.
    pub wasm_sha256: String,
    /// Expected installed/retired revision, absent for a genuinely new lineage.
    pub expected_revision: Option<String>,
    /// Host-observed non-secret generation evidence.
    pub evidence: GenerationEvidence,
    /// Bounded compiler diagnostics; do not place these in logs or receipts.
    pub diagnostics: String,
}

/// Disk and runtime outcomes remain separate; this grants no ACP session exposure.
#[derive(Debug)]
pub struct GenerationOutcome {
    /// Existing authoritative transaction result, including exact no-ops.
    pub commit: CommitOutcome,
    /// Existing catalog reconciliation result.
    pub refresh: RefreshReport,
    /// The exact output the caller approved.
    pub preview: GenerationPreview,
}

impl GenerationOutcome {
    /// Protocol-neutral report using the canonical receipt and commit result.
    pub fn report(&self) -> serde_json::Value {
        serde_json::json!({
            "component_id": self.commit.entry.component_id().as_str(),
            "revision": self.commit.entry.revision().to_string(),
            "commit": self.commit,
            "refresh": { "cursor": self.refresh.cursor, "changed": self.refresh.changed },
            "preview": self.preview,
        })
    }
}

/// Generation-specific host failures; lower-level typed causes remain intact.
#[derive(Debug, thiserror::Error)]
pub enum GenerationError {
    /// Neither Cargo feature availability nor a request enables this capability.
    #[error("component generation is not enabled by the host")]
    Disabled,
    /// The operator's ceiling or the caller's host-issued permission denied an operation.
    #[error("component generation permission denied: {0}")]
    PermissionDenied(&'static str),
    /// Cancellation occurred before a store commit was accepted.
    #[error("component generation cancelled before commit")]
    Cancelled,
    /// The shared journal could not conclusively complete the commit operation.
    #[error("generated-component commit requires recovery for operation {operation}")]
    CommitRecoveryRequired {
        /// Existing store operation identifier, not a new generation lineage.
        operation: String,
        /// Canonical receipt if the exact operation was observed committed.
        observed_commit: Option<Box<CommitOutcome>>,
        /// Failure encountered while observing/recovering, if any.
        observation_error: Option<StoreError>,
        /// Original shared-store error; no string-based classification.
        #[source]
        source: StoreError,
    },
    /// The installed artifact is durable but runtime reconciliation failed.
    #[error("generated component was committed, but runtime publication or catalog refresh failed: {source}")]
    CommittedButRefreshFailed {
        /// Actual receipt/cursor; do not retry by creating another lineage.
        commit: Box<CommitOutcome>,
        /// Refresh failure, retaining the core typed source chain.
        #[source]
        source: anyhow::Error,
    },
}

impl GenerationError {
    /// Preserve the canonical durable outcome when runtime publication failed.
    pub fn committed_report(&self) -> Option<serde_json::Value> {
        match self {
            Self::CommittedButRefreshFailed { commit, .. } => Some(serde_json::json!({
                "component_id": commit.entry.component_id().as_str(),
                "revision": commit.entry.revision().to_string(),
                "commit": commit,
                "refresh": null,
            })),
            _ => None,
        }
    }

    /// Distinguish uncertain journal completion from a failed precommit build.
    pub fn recovery_report(&self) -> Option<serde_json::Value> {
        match self {
            Self::CommitRecoveryRequired {
                operation,
                observed_commit,
                ..
            } => Some(serde_json::json!({
                "status": if observed_commit.is_some() {
                    "committed-recovery-required"
                } else {
                    "commit-unknown"
                },
                "operation": operation,
                "commit": observed_commit,
                "refresh": null,
            })),
            _ => None,
        }
    }
}

/// A configured isolated builder; it never implicitly authorizes a caller.
pub struct GenerationService {
    builder: Arc<Builder>,
    permissions: GenerationPermissions,
    validator: Option<Arc<dyn LocalValidator>>,
    callers: Vec<GenerationCallerGrant>,
}

struct SelectedTarget {
    key: StorageKey,
    source: SourceIdentity,
    expected: ExpectedEntry,
    policy: PreparedPolicy,
}

/// Owned output awaiting separate installation authorization. Dropping it installs nothing.
pub struct PreparedGeneration {
    manager: LifecycleManager,
    selected: SelectedTarget,
    artifact: BuildArtifact,
    preview: GenerationPreview,
    intent: InstallIntent,
    permissions: GenerationPermissions,
    validator: Option<Arc<dyn LocalValidator>>,
    is_rebuild: bool,
}

impl LifecycleManager {
    /// Enable generation once using trusted operator configuration.
    ///
    /// Clones share this service. No image is acquired and no VM starts here.
    pub fn enable_generation(&self, service: GenerationService) -> Result<()> {
        self.generation
            .set(Arc::new(service))
            .map_err(|_| anyhow!("component generation was already configured"))
    }

    /// Retrieve the explicitly configured generation capability.
    pub fn generation_service(&self) -> Result<Arc<GenerationService>> {
        self.generation
            .get()
            .cloned()
            .ok_or_else(|| GenerationError::Disabled.into())
    }
}

impl GenerationService {
    /// Bind an isolated builder to an operator-controlled permission ceiling.
    pub fn new(builder: Builder, permissions: GenerationPermissions) -> Self {
        Self {
            builder: Arc::new(builder),
            permissions,
            validator: None,
            callers: Vec::new(),
        }
    }

    /// Inject the existing matching-runtime validator for ACP-layer outputs.
    pub fn with_validator(mut self, validator: Arc<dyn LocalValidator>) -> Self {
        self.validator = Some(validator);
        self
    }

    /// The operator ceiling, not authorization inferred from request intent.
    pub fn permissions(&self) -> GenerationPermissions {
        self.permissions
    }

    /// Build privately without store writes, locks held during compilation, or exposure.
    pub async fn prepare(
        &self,
        manager: &LifecycleManager,
        request: GenerationRequest,
        authorization: GenerationPermissions,
        cancel: CancellationToken,
    ) -> Result<PreparedGeneration> {
        require(self.permissions.build && authorization.build, "build")?;
        let is_rebuild = matches!(request.target, GenerationTarget::Rebuild { .. });
        if is_rebuild {
            require(self.permissions.rebuild && authorization.rebuild, "rebuild")?;
        }
        ensure!(
            matches!(
                request.intent,
                InstallIntent::InstallOnly | InstallIntent::ExposeTools
            ),
            "generation does not select or activate ACP components"
        );
        ensure!(
            request.build.kind != ComponentKind::AcpLayer
                || request.intent == InstallIntent::InstallOnly,
            "generated ACP layers must be installed without ordinary-tool exposure"
        );
        if request.build.kind == ComponentKind::AcpLayer {
            ensure!(
                self.validator.is_some(),
                "ACP layer generation requires an injected ACP validator"
            );
        }
        ensure!(
            request
                .reinstall_policy
                .as_ref()
                .is_none_or(|policy| policy.len() <= MAX_REINSTALL_POLICY_BYTES),
            "reinstall policy exceeds the generation input limit"
        );
        crate::ComponentId::from_declared_name(&request.build.component_name)?;
        check_cancelled(&cancel)?;
        let selected = select_target(manager, &request).await?;
        let requested_name = request.build.component_name.clone();
        let requested_kind = request.build.kind;
        let source_sha256 = hex::encode(Sha256::digest(request.build.source.as_bytes()));
        let wit_sha256 = hex::encode(Sha256::digest(request.build.wit.as_bytes()));
        let artifact = self.builder.build(request.build, cancel.clone()).await;
        check_cancelled(&cancel)?;
        let artifact = artifact?;
        ensure!(
            artifact.evidence.source_sha256 == source_sha256
                && artifact.evidence.wit_sha256 == wit_sha256
                && artifact.evidence.kind == requested_kind
                && artifact.evidence.component_name == requested_name,
            "builder evidence does not match the captured source and WIT"
        );
        let inspection = inspect_artifact(&artifact.wasm)?;
        let component_id = inspection.identity?;
        ensure!(
            component_id.as_str() == requested_name,
            "builder output has a different actual component name"
        );
        ensure!(
            shape_matches(&inspection.shape, &requested_kind),
            "builder output has a different component role"
        );
        let evidence = generation_evidence(&artifact);
        let preview = GenerationPreview {
            component_id: component_id.as_str().to_owned(),
            kind: requested_kind,
            wasm_sha256: hex::encode(Sha256::digest(&artifact.wasm)),
            expected_revision: selected
                .expected
                .entry()
                .map(|entry| entry.revision().to_string()),
            evidence,
            diagnostics: artifact.diagnostics.clone(),
        };
        Ok(PreparedGeneration {
            manager: manager.clone(),
            selected,
            artifact,
            preview,
            intent: request.intent,
            permissions: self.permissions,
            validator: self.validator.clone(),
            is_rebuild,
        })
    }
}

impl PreparedGeneration {
    /// Inspect the immutable output before deciding whether to install it.
    pub fn preview(&self) -> &GenerationPreview {
        &self.preview
    }

    /// Validate and commit the approved captured output through the shared store.
    ///
    /// Before commit, cancellation or dropping this future publishes nothing.
    /// Once handed off, the existing store transaction owns completion/recovery.
    pub async fn install(
        self,
        authorization: GenerationPermissions,
        cancel: CancellationToken,
    ) -> Result<GenerationOutcome> {
        require(self.permissions.install && authorization.install, "install")?;
        if self.is_rebuild {
            require(self.permissions.rebuild && authorization.rebuild, "rebuild")?;
        }
        if self.intent == InstallIntent::ExposeTools {
            require(self.permissions.expose && authorization.expose, "expose")?;
        }
        check_cancelled(&cancel)?;
        let manager = self.manager;
        let id = self.preview.component_id.clone();
        let guard = manager.load_guard(&id).await.lock_owned().await;
        let selected = self.selected;
        let observed_id = id.clone();
        let key = selected.key.clone();
        let source = selected.source.clone();
        let current = store_operation(manager.component_store(), move |store| {
            Ok(store.observe(&observed_id, &key, &source)?)
        })
        .await?;
        ensure_expected(&current, &selected.expected)?;
        check_cancelled(&cancel)?;
        let inspection = inspect_artifact(&self.artifact.wasm)?;
        let binding = SecretBinding::new(
            &inspection.identity.clone()?,
            &selected.key,
            source_binding_key(&selected.source)?,
        )?;
        manager.secrets_manager.check_binding(&binding).await?;
        let (evidence, prepared_runtime) = match inspection.shape {
            ArtifactShape::ToolCandidate => {
                let prepared = manager
                    .prepare_component_load(
                        CapturedComponent {
                            storage_key: selected.key.clone(),
                            wasm: self.artifact.wasm.clone(),
                            bundled_policy: None,
                        },
                        &binding,
                        selected.policy.bytes().map(<[u8]>::to_vec),
                    )
                    .await?;
                (
                    ValidationEvidence::OrdinaryPrepared {
                        runtime: manager.cache_engine(),
                    },
                    Some(prepared),
                )
            }
            ArtifactShape::AcpLayer => (
                self.validator
                    .as_ref()
                    .context("ACP layer validator is unavailable")?
                    .validate(&self.artifact.wasm, &inspection, selected.policy.bytes())?,
                None,
            ),
            _ => {
                return Err(anyhow!(
                    "generation only installs ordinary tools or ACP layers"
                ))
            }
        };
        let SourceIdentity::Generated { id: source_id } = &selected.source else {
            return Err(anyhow!(
                "generated artifact lost its admitted source binding"
            ));
        };
        let expose_tools = self.intent == InstallIntent::ExposeTools;
        let install = PreparedInstall::prepare(
            self.artifact.wasm,
            InstallOptions {
                storage_key: selected.key,
                origin: OriginEvidence {
                    location: format!("generated://{source_id}"),
                    requested_version: None,
                    selected_version: None,
                    manifest_digest: None,
                    immutable_uri: None,
                    generation: Some(self.preview.evidence.clone()),
                },
                source: selected.source,
                owner: InstallOwner::Explicit,
                intent: self.intent,
                policy: selected.policy,
                observation: None,
            },
            |_, _, _| Ok(evidence),
        )?;
        check_cancelled(&cancel)?;
        let preview = self.preview;
        tokio::spawn(async move {
            let outcome = store_operation(manager.component_store(), move |store| {
                commit_generated(&store, install, selected.expected)
            })
            .await?;
            if let Some(prepared) = prepared_runtime.filter(|_| expose_tools) {
                manager
                    .publish_prepared(prepared, outcome.clone())
                    .await
                    .map_err(|source| GenerationError::CommittedButRefreshFailed {
                        commit: Box::new(outcome.clone()),
                        source,
                    })?;
            }
            drop(guard);
            let refresh = manager.refresh_from_store().await.map_err(|source| {
                GenerationError::CommittedButRefreshFailed {
                    commit: Box::new(outcome.clone()),
                    source,
                }
            })?;
            Ok(GenerationOutcome {
                commit: outcome,
                refresh,
                preview,
            })
        })
        .await
        .context("generated-component commit worker failed")?
    }
}

fn commit_generated(
    store: &crate::store::ComponentStore,
    install: PreparedInstall,
    expected: ExpectedEntry,
) -> Result<CommitOutcome> {
    let id = install.component_id().as_str().to_owned();
    let artifact_sha256 = install.artifact_sha256().to_owned();
    match store.commit_install(install, expected) {
        Ok(outcome) => Ok(outcome),
        Err(source @ StoreError::RecoveryRequired { .. }) => {
            let StoreError::RecoveryRequired { operation, .. } = &source else {
                unreachable!("matched recovery error");
            };
            let operation = operation.clone();
            let observed = observe_generated_commit(store, &operation, &id, &artifact_sha256);
            let (observed_commit, observation_error) = match observed {
                Ok(outcome) => (outcome.map(Box::new), None),
                Err(error) => (None, Some(error)),
            };
            Err(GenerationError::CommitRecoveryRequired {
                operation,
                observed_commit,
                observation_error,
                source,
            }
            .into())
        }
        Err(error) => Err(error.into()),
    }
}

fn observe_generated_commit(
    store: &crate::store::ComponentStore,
    operation: &str,
    id: &str,
    artifact_sha256: &str,
) -> crate::store::Result<Option<CommitOutcome>> {
    let snapshot = store
        .snapshot_if_changed(None)?
        .ok_or_else(|| StoreError::Integrity("missing full recovery observation".into()))?;
    let Some(change) = snapshot.last_change.filter(|change| {
        change.operation == operation
            && change.component_id.as_str() == id
            && change.cause == crate::store::StoreChangeCause::Install
    }) else {
        return Ok(None);
    };
    let current = store.read(id)?;
    if current.receipt.revision != change.after
        || current.receipt.artifact_sha256 != artifact_sha256
    {
        return Ok(None);
    }
    let _scope = store.checked_read(&snapshot.cursor, Some((id, &current.receipt.revision)))?;
    Ok(Some(CommitOutcome {
        entry: StoredEntry::Installed(current.receipt),
        cursor: snapshot.cursor,
        change: Some(change),
    }))
}

fn require(allowed: bool, operation: &'static str) -> Result<()> {
    if allowed {
        Ok(())
    } else {
        Err(GenerationError::PermissionDenied(operation).into())
    }
}

fn check_cancelled(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(GenerationError::Cancelled.into())
    } else {
        Ok(())
    }
}

fn shape_matches(shape: &ArtifactShape, kind: &ComponentKind) -> bool {
    matches!(
        (shape, kind),
        (ArtifactShape::ToolCandidate, ComponentKind::Tool)
            | (ArtifactShape::AcpLayer, ComponentKind::AcpLayer)
    )
}

fn ensure_expected(current: &ExpectedEntry, expected: &ExpectedEntry) -> Result<()> {
    if current.entry() != expected.entry() {
        return Err(StoreError::Conflict(
            "generated component changed since build admission".into(),
        )
        .into());
    }
    Ok(())
}

async fn select_target(
    manager: &LifecycleManager,
    request: &GenerationRequest,
) -> Result<SelectedTarget> {
    let id = request.build.component_name.clone();
    let target = request.target.clone();
    let kind = request.build.kind;
    let restore = request.reinstall_policy.clone();
    store_operation(manager.component_store(), move |store| {
        let snapshot = store
            .snapshot_if_changed(None)?
            .context("missing component inventory")?;
        let previous = snapshot
            .entries
            .into_iter()
            .find(|entry| entry.component_id().as_str() == id);
        let (key, source) = match target {
            GenerationTarget::New => {
                ensure!(
                    restore.is_none(),
                    "new generation cannot supply an initial policy"
                );
                let mut entropy = [0; 16];
                getrandom::fill(&mut entropy)
                    .map_err(|error| anyhow!("cannot mint generated lineage: {error}"))?;
                let source_id = hex::encode(entropy);
                (
                    StorageKey::parse(&format!("generated_{source_id}"))?,
                    SourceIdentity::Generated { id: source_id },
                )
            }
            GenerationTarget::Rebuild { expected_revision } => {
                let previous = previous
                    .as_ref()
                    .context("generated component is not installed or reserved")?;
                ensure!(
                    matches!(previous.binding().source, SourceIdentity::Generated { .. }),
                    "rebuild cannot adopt another source"
                );
                if previous.revision().to_string() != expected_revision {
                    return Err(
                        StoreError::Conflict("generated rebuild revision is stale".into()).into(),
                    );
                }
                ensure!(
                    matches!(
                        (&previous.binding().kind, &kind),
                        (StoredArtifactKind::Tool, ComponentKind::Tool)
                            | (StoredArtifactKind::AcpLayer, ComponentKind::AcpLayer)
                    ),
                    "rebuild cannot change the component role"
                );
                (
                    previous.storage_key().clone(),
                    previous.binding().source.clone(),
                )
            }
        };
        let expected = store.observe(&id, &key, &source)?;
        if expected.entry() != previous.as_ref() {
            return Err(StoreError::Conflict(
                "component changed during generation admission".into(),
            )
            .into());
        }
        let policy = match expected.entry() {
            Some(StoredEntry::Installed(receipt)) => {
                ensure!(
                    restore.is_none(),
                    "an installed component retains its current policy"
                );
                let captured = store.read(&id)?;
                if captured.receipt.revision != receipt.revision {
                    return Err(StoreError::Conflict(
                        "policy changed during generation admission".into(),
                    )
                    .into());
                }
                crate::store_runtime::policy_from_snapshot(&captured)?
            }
            Some(StoredEntry::Retired(retired)) => {
                let old = &retired.previous.policy;
                let restored = match restore {
                    Some(yaml) => PreparedPolicy::parse(yaml.into_bytes(), old.provenance.clone())?,
                    None => PreparedPolicy::absent(old.provenance.clone()),
                }
                .with_metadata(old.metadata.clone())?;
                ensure!(
                    restored.evidence() == old,
                    "retired generated component requires its exact previous policy"
                );
                restored
            }
            None => PreparedPolicy::absent(PolicyProvenance::Default),
        };
        Ok(SelectedTarget {
            key,
            source,
            expected,
            policy,
        })
    })
    .await
}

fn generation_evidence(artifact: &BuildArtifact) -> GenerationEvidence {
    let evidence = &artifact.evidence;
    GenerationEvidence {
        source_sha256: evidence.source_sha256.clone(),
        wit_sha256: evidence.wit_sha256.clone(),
        wit_dependencies_sha256: evidence.wit_dependencies_sha256.clone(),
        builder_initrd_sha256: evidence.builder_initrd_sha256.clone(),
        builder_helper_sha256: evidence.builder_helper_sha256.clone(),
        builder_manifest_digest: evidence.builder_manifest_digest.clone(),
        profile: evidence.profile.clone(),
        profile_sha256: evidence.profile_sha256.clone(),
        compiler: evidence.compiler.clone(),
        bindgen: evidence.bindgen.clone(),
        binding_runtime: evidence.binding_runtime.clone(),
        vm_runtime: evidence.vm_runtime.clone(),
        world: evidence.world.clone(),
        target: evidence.target.clone(),
        host_platform: evidence.host_platform.clone(),
    }
}

#[cfg(test)]
mod tests;
