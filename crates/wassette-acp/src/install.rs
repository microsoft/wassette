// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Receipt-backed ACP installation and selection.
//!
//! Explicit local paths, `file://`, HTTPS and OCI inputs are captured and
//! installed through the shared transaction store. Non-URI selectors first
//! resolve an exact installed semantic name, even when it contains separators.
//! Physical filenames are private storage keys, never semantic identities.
//! Compilation and policy validation use captured bytes, outside store locks;
//! subsequent sandboxing and stage loading retain that same snapshot.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use tokio::sync::mpsc::Sender;
use wasmtime::Engine;
use wasmtime::component::Component;
use wassette::local_source::LocalValidator;
use wassette::store::{
    ArtifactSnapshot, ComponentStore, ExpectedEntry, InstallIntent, InstallOptions, InstallOwner,
    PolicyMetadata, PolicyProvenance, PreparedInstall, PreparedPolicy, StoreError, StoredEntry,
    ValidationEvidence,
};
use wassette::wasm_directory::{self, PackageId, PackageSelector, WasmDirectoryClient};

/// One admitted snapshot; `path` is informational and must not be reopened.
#[derive(Clone)]
pub struct ResolvedComponent {
    /// Exact embedded root name, not a filename.
    pub component_id: String,
    /// Informational location of the store's current artifact.
    pub path: PathBuf,
    pub snapshot: Arc<ArtifactSnapshot>,
    pub component: Component,
}

impl std::fmt::Debug for ResolvedComponent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolvedComponent")
            .field("component_id", &self.component_id)
            .field("path", &self.path)
            .field("receipt", &self.snapshot.receipt)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Reference<'a> {
    Uri(&'a str),
    Path(&'a str),
    Id(&'a str),
}

fn classify(arg: &str) -> Result<Reference<'_>> {
    if let Some((scheme, _)) = arg.split_once("://") {
        return match scheme {
            "file" | "oci" | "https" => Ok(Reference::Uri(arg)),
            other => Err(anyhow!(
                "unsupported component scheme `{other}://`; expected `oci://`, `https://`, \
                 `file://`, a filesystem path, or a component id"
            )),
        };
    }
    if arg.contains(std::path::MAIN_SEPARATOR)
        || arg.contains('/')
        || arg.ends_with(".wasm")
        || Path::new(arg).exists()
    {
        Ok(Reference::Path(arg))
    } else {
        Ok(Reference::Id(arg))
    }
}

/// Classify a non-URI selector that names a wasm.directory package.
///
/// Installed names and existing paths take precedence. `namespace:package`
/// selectors always resolve through wasm.directory; `registry/repository`
/// only does when no such local path exists.
fn package_selector(arg: &str) -> Result<Option<PackageSelector>> {
    Ok(match classify(arg)? {
        Reference::Id(id) if id.contains(':') => {
            Some(PackageSelector::parse(id).with_context(|| {
                format!(
                    "no installed component `{id}`, and it is not a valid wasm.directory \
                     `namespace:package[@version]` selector"
                )
            })?)
        }
        Reference::Path(path)
            if !path.ends_with(".wasm")
                && looks_like_registry(path)
                && !Path::new(path).exists() =>
        {
            PackageId::parse(path).ok().map(PackageSelector::Package)
        }
        _ => None,
    })
}

/// Follow the OCI convention: the first segment names a registry only when it
/// is `localhost` or contains a `.` or `:`, and it never starts with a `.`.
fn looks_like_registry(arg: &str) -> bool {
    arg.split_once('/').is_some_and(|(registry, _)| {
        !registry.starts_with('.')
            && (registry == "localhost" || registry.contains('.') || registry.contains(':'))
    })
}

/// Resolves with the caller's configured clients, without an ordinary MCP engine.
pub struct Resolver {
    config: wassette::LifecycleConfig,
    directory: Option<WasmDirectoryClient>,
}

/// ACP runtime validation injected into local-source discovery.
pub(crate) struct AcpLocalValidator {
    engine: Engine,
    component_dir: PathBuf,
}

impl AcpLocalValidator {
    pub(crate) fn new(engine: Engine, component_dir: PathBuf) -> Self {
        Self {
            engine,
            component_dir,
        }
    }
}

impl LocalValidator for AcpLocalValidator {
    fn validate(
        &self,
        wasm: &[u8],
        inspection: &wassette::ArtifactInspection,
        policy: Option<&[u8]>,
    ) -> Result<ValidationEvidence> {
        crate::classify_acp_component(inspection)?;
        crate::sandbox::validate_policy(policy, &self.component_dir)?;
        Component::new(&self.engine, wasm)
            .map_err(anyhow::Error::from)
            .context("compiling local ACP component")?;
        Ok(ValidationEvidence::AcpCompiledAndExportChecked {
            runtime: format!("wassette-acp/{}", crate::HOST_ACP_VERSION),
        })
    }
}

impl Resolver {
    #[cfg(test)]
    pub fn new(component_dir: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self::with_config(
            wassette::LifecycleManager::builder(component_dir.into()).build_config()?,
        ))
    }

    pub fn with_config(config: wassette::LifecycleConfig) -> Self {
        Self {
            config,
            directory: None,
        }
    }

    /// Use an explicit wasm.directory client instead of the environment's.
    #[cfg(test)]
    pub fn with_directory(mut self, directory: WasmDirectoryClient) -> Self {
        self.directory = Some(directory);
        self
    }

    fn directory(&self) -> Result<WasmDirectoryClient> {
        match &self.directory {
            Some(directory) => Ok(directory.clone()),
            None => WasmDirectoryClient::from_environment(),
        }
    }

    pub fn component_dir(&self) -> &Path {
        self.config.component_dir()
    }

    pub fn secrets_dir(&self) -> &Path {
        self.config.secrets_dir()
    }

    /// Persist only: this never starts a guest or replaces a selected stage.
    pub async fn install_validated(
        &self,
        arg: &str,
        progress: Option<Sender<String>>,
        engine: &Engine,
    ) -> Result<ResolvedComponent> {
        self.resolve(arg, progress, engine, None, InstallIntent::InstallOnly)
            .await
    }

    pub async fn resolve_validated(
        &self,
        arg: &str,
        progress: Option<Sender<String>>,
        engine: &Engine,
        expected_kind: Option<crate::state::StageKind>,
    ) -> Result<ResolvedComponent> {
        self.resolve(
            arg,
            progress,
            engine,
            expected_kind,
            InstallIntent::AcpSelection,
        )
        .await
    }

    async fn resolve(
        &self,
        arg: &str,
        progress: Option<Sender<String>>,
        engine: &Engine,
        expected_kind: Option<crate::state::StageKind>,
        intent: InstallIntent,
    ) -> Result<ResolvedComponent> {
        let root = self.component_dir().to_path_buf();
        let store = tokio::task::spawn_blocking(move || ComponentStore::open(root)).await??;
        if !arg.contains("://") {
            let reader = store.clone();
            let name = arg.to_string();
            match tokio::task::spawn_blocking(move || reader.read(&name)).await? {
                Ok(snapshot) => {
                    let component = self.validate(
                        engine,
                        &snapshot.wasm,
                        &wassette::inspect_artifact(&snapshot.wasm)?,
                        snapshot.policy.as_deref(),
                        expected_kind,
                    )?;
                    return Ok(self.resolved(snapshot, component));
                }
                Err(StoreError::NotFound(_)) => {}
                Err(error) => return Err(error.into()),
            }
        }
        let acquired = match package_selector(arg)? {
            Some(selector) => {
                if let Some(tx) = &progress {
                    let _ = tx.try_send(format!("Resolving `{selector}` on wasm.directory…"));
                }
                let (resolved, acquired) = wasm_directory::acquire_package(
                    &self.directory()?,
                    &selector,
                    None,
                    &self.config,
                )
                .await?;
                if let Some(tx) = &progress {
                    let _ = tx.try_send(format!(
                        "Captured {} {} ({})",
                        resolved.package_id, resolved.selected_version, resolved.manifest_digest
                    ));
                }
                acquired
            }
            None => self.acquire_uri(arg, progress.as_ref()).await?,
        };
        let inspection = wassette::inspect_artifact(&acquired.wasm)?;
        let component_id = inspection.identity.map_err(anyhow::Error::from)?;
        let observer = store.clone();
        let name = component_id.as_str().to_owned();
        let key = acquired.storage_key.clone();
        let source = acquired.source.clone();
        let policy_source = match &source {
            wassette::store::SourceIdentity::File(path) => format!(
                "file://{}",
                path.with_file_name(format!("{}.policy.yaml", key.as_str()))
                    .display()
            ),
            _ => acquired.origin.location.clone(),
        };
        let (expected, policy) = tokio::task::spawn_blocking(move || {
            let expected = observer.observe(&name, &key, &source)?;
            let policy = select_policy(&observer, &expected, acquired.policy, policy_source)?;
            Ok::<_, anyhow::Error>((expected, policy))
        })
        .await??;
        if let Some(tx) = &progress {
            let _ = tx.try_send("Validating component…".to_string());
        }
        let policy_bytes = policy.bytes().map(<[u8]>::to_vec);
        let wasm = acquired.wasm;
        let mut compiled = None;
        let prepared = PreparedInstall::prepare(
            wasm.clone(),
            InstallOptions {
                storage_key: acquired.storage_key,
                source: acquired.source,
                origin: acquired.origin,
                owner: InstallOwner::Explicit,
                intent,
                policy,
                observation: None,
            },
            |bytes, inspection, policy| {
                compiled = Some(self.validate(engine, bytes, inspection, policy, expected_kind)?);
                Ok(ValidationEvidence::AcpCompiledAndExportChecked {
                    runtime: format!("wassette-acp/{}", crate::HOST_ACP_VERSION),
                })
            },
        )?;
        // Keep the exact prepared bytes, rather than reopening a possibly newer
        // receipt after commit. The owned closure also finishes on cancellation.
        let outcome =
            tokio::task::spawn_blocking(move || store.commit_install(prepared, expected)).await??;
        let StoredEntry::Installed(receipt) = outcome.entry else {
            anyhow::bail!("install returned a retired component");
        };
        Ok(self.resolved(
            ArtifactSnapshot {
                receipt,
                wasm,
                policy: policy_bytes,
                cursor: outcome.cursor,
            },
            compiled.context("ACP validator did not compile the component")?,
        ))
    }

    async fn acquire_uri(
        &self,
        arg: &str,
        progress: Option<&Sender<String>>,
    ) -> Result<wassette::acquisition::AcquiredComponent> {
        let uri = match classify(arg)? {
            Reference::Id(id) => anyhow::bail!(
                "no component `{id}` in {}; install a named ACP artifact from a path, URI or \
                 wasm.directory package (`namespace:package[@version]`) first",
                self.component_dir().display()
            ),
            Reference::Path(path) => {
                let absolute = std::path::absolute(path)
                    .with_context(|| format!("resolving component path `{path}`"))?;
                format!("file://{}", absolute.display())
            }
            Reference::Uri(uri) => uri.to_string(),
        };
        if let Some(tx) = progress {
            let _ = tx.try_send("Capturing component…".to_string());
        }
        wassette::acquisition::acquire_component(&uri, &self.config, true)
            .await
            .with_context(|| format!("fetching component `{uri}`"))
    }

    fn validate(
        &self,
        engine: &Engine,
        bytes: &[u8],
        inspection: &wassette::ArtifactInspection,
        policy: Option<&[u8]>,
        expected_kind: Option<crate::state::StageKind>,
    ) -> Result<Component> {
        match expected_kind {
            Some(kind) => crate::validate_stage(inspection, kind)?,
            None => {
                crate::classify_acp_component(inspection)?;
            }
        }
        crate::sandbox::validate_policy(policy, self.component_dir())?;
        Component::new(engine, bytes)
            .map_err(anyhow::Error::from)
            .context("compiling ACP component")
    }

    fn resolved(&self, snapshot: ArtifactSnapshot, component: Component) -> ResolvedComponent {
        ResolvedComponent {
            component_id: snapshot.receipt.component_id.as_str().to_owned(),
            path: self
                .component_dir()
                .join(format!("{}.wasm", snapshot.receipt.storage_key.as_str())),
            snapshot: Arc::new(snapshot),
            component,
        }
    }
}

fn select_policy(
    store: &ComponentStore,
    expected: &ExpectedEntry,
    incoming: Option<Vec<u8>>,
    source_uri: String,
) -> Result<PreparedPolicy> {
    if let Some(entry) = expected.entry() {
        let receipt = entry.binding();
        if let StoredEntry::Installed(_) = entry {
            let snapshot = store.read(receipt.component_id.as_str())?;
            anyhow::ensure!(
                snapshot.receipt.revision == receipt.revision,
                "component changed while selecting its effective policy"
            );
            // Conservatively retain the complete current policy, including an
            // explicit clear, until policy replacement is explicitly requested.
            return Ok(
                prepared_policy(snapshot.policy, receipt.policy.provenance.clone())?
                    .with_metadata(receipt.policy.metadata.clone())?,
            );
        }
        if matches!(
            receipt.policy.provenance,
            PolicyProvenance::ExplicitAttachment
                | PolicyProvenance::PermissionEdit
                | PolicyProvenance::Legacy
        ) {
            if receipt.policy.sha256.is_none() {
                return Ok(PreparedPolicy::absent(receipt.policy.provenance.clone())
                    .with_metadata(receipt.policy.metadata.clone())?);
            }
            let restored = prepared_policy(incoming, receipt.policy.provenance.clone())?
                .with_metadata(receipt.policy.metadata.clone())?;
            anyhow::ensure!(
                restored.evidence() == &receipt.policy,
                "retired component requires the exact previously protected policy"
            );
            return Ok(restored);
        }
    }
    let provenance = if incoming.is_some() {
        PolicyProvenance::Bundled
    } else {
        PolicyProvenance::Default
    };
    let metadata = incoming.as_ref().map(|_| PolicyMetadata {
        source_uri,
        attached_at: None,
    });
    Ok(prepared_policy(incoming, provenance)?.with_metadata(metadata)?)
}

fn prepared_policy(bytes: Option<Vec<u8>>, provenance: PolicyProvenance) -> Result<PreparedPolicy> {
    Ok(match bytes {
        Some(bytes) => PreparedPolicy::parse(bytes, provenance)?,
        None => PreparedPolicy::absent(provenance),
    })
}

#[cfg(test)]
pub(crate) fn named_fixture(name: &str, layer: bool) -> Vec<u8> {
    let name = serde_json::to_string(name).unwrap();
    let client = if layer {
        r#"(export "wassette:acp/client@7.0.0" (instance $empty))"#
    } else {
        ""
    };
    wat::parse_str(format!(
        r#"(component ${name}
            (instance $empty)
            (export "wassette:acp/agent@7.0.0" (instance $empty))
            {client})"#
    ))
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_distinguish_uris_paths_and_names() {
        for uri in [
            "oci://ghcr.io/org/agent:0.1.0",
            "https://example.com/agent.wasm",
            "file:///fixtures/agent.wasm",
        ] {
            assert_eq!(classify(uri).unwrap(), Reference::Uri(uri));
        }
        assert!(classify("ftp://example.com/agent.wasm").is_err());
        for path in ["./target/agent.wasm", "agent.wasm"] {
            assert_eq!(classify(path).unwrap(), Reference::Path(path));
        }
        assert_eq!(classify("agent").unwrap(), Reference::Id("agent"));
    }

    #[test]
    fn package_selectors_are_recognized_after_paths_and_names() {
        assert!(matches!(
            package_selector("yosh:wordmark@2.0.6").unwrap(),
            Some(PackageSelector::Wit(_))
        ));
        assert!(matches!(
            package_selector("ghcr.io/yoshuawuyts/components/wordmark").unwrap(),
            Some(PackageSelector::Package(_))
        ));
        assert!(package_selector("Yosh:wordmark").is_err());
        for other in [
            "agent",
            "agent.wasm",
            "./target/agent",
            "../target/agent",
            "target/agent",
            "/abs/agent",
            "oci://ghcr.io/org/agent:0.1.0",
        ] {
            assert!(package_selector(other).unwrap().is_none(), "{other}");
        }
    }

    fn directory_resolver(
        fixture: &wassette::wasm_directory::fixtures::WasmDirectoryFixture,
        dir: &Path,
    ) -> Resolver {
        Resolver::with_config(
            wassette::LifecycleManager::builder(dir)
                .with_oci_client(fixture.oci_client())
                .build_config()
                .unwrap(),
        )
        .with_directory(fixture.directory().unwrap())
    }

    #[tokio::test]
    async fn install_resolves_wit_selector_through_wasm_directory() {
        use wassette::wasm_directory::fixtures::{FixturePackage, WasmDirectoryFixture};

        let fixture = WasmDirectoryFixture::start(vec![
            FixturePackage::new(
                "owner/agent",
                Some("demo:agent"),
                [
                    ("1.0.0", named_fixture("demo-agent", false)),
                    ("1.1.0", named_fixture("demo-agent", false)),
                ],
            ),
            FixturePackage::new(
                "owner/agent-extra",
                Some("demo:agent-extra"),
                [("1.0.0", named_fixture("other", false))],
            ),
        ])
        .await
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let resolver = directory_resolver(&fixture, dir.path());
        let engine = Engine::default();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);

        let installed = resolver
            .install_validated("demo:agent", Some(tx), &engine)
            .await
            .unwrap();
        assert_eq!(installed.component_id, "demo-agent");
        let origin = &installed.snapshot.receipt.origin;
        assert_eq!(origin.selected_version.as_deref(), Some("1.1.0"));
        assert_eq!(
            origin.manifest_digest.as_deref(),
            Some(fixture.digest("owner/agent", "1.1.0"))
        );
        let mut messages = Vec::new();
        while let Ok(message) = rx.try_recv() {
            messages.push(message);
        }
        assert!(
            messages
                .iter()
                .any(|message| message.contains(fixture.digest("owner/agent", "1.1.0"))),
            "{messages:?}"
        );

        let pinned = resolver
            .install_validated("demo:agent@1.0.0", None, &engine)
            .await
            .unwrap();
        assert_eq!(
            pinned.snapshot.receipt.origin.manifest_digest.as_deref(),
            Some(fixture.digest("owner/agent", "1.0.0"))
        );

        let canonical = resolver
            .install_validated(&fixture.package_id("owner/agent"), None, &engine)
            .await
            .unwrap();
        assert_eq!(canonical.component_id, "demo-agent");

        let missing = resolver
            .install_validated("demo:absent", None, &engine)
            .await
            .unwrap_err();
        assert!(
            format!("{missing:#}").contains("No wasm.directory component package"),
            "{missing:#}"
        );
    }

    #[tokio::test]
    async fn install_reports_nameless_wasm_directory_artifacts() {
        use wassette::wasm_directory::fixtures::{FixturePackage, WasmDirectoryFixture};

        let nameless = wat::parse_str(
            r#"(component
                (instance (;0;))
                (export "wassette:acp/agent@7.0.0" (instance 0)))"#,
        )
        .unwrap();
        let fixture = WasmDirectoryFixture::start(vec![FixturePackage::new(
            "owner/nameless",
            Some("demo:nameless"),
            [("2.0.6", nameless)],
        )])
        .await
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let resolver = directory_resolver(&fixture, dir.path());
        let error = resolver
            .install_validated("demo:nameless", None, &Engine::default())
            .await
            .unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains(&fixture.package_id("owner/nameless")),
            "{message}"
        );
        assert!(
            message.contains(fixture.digest("owner/nameless", "2.0.6")),
            "{message}"
        );
        assert!(
            message.contains("wasm-tools metadata add --name"),
            "{message}"
        );
    }

    #[tokio::test]
    async fn missing_id_reports_the_component_dir() {
        let dir = tempfile::tempdir().unwrap();
        let error = Resolver::new(dir.path())
            .unwrap()
            .install_validated("nope", None, &Engine::default())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no component `nope`"), "{error}");
        assert!(
            error
                .to_string()
                .contains(&dir.path().display().to_string())
        );
    }

    #[tokio::test]
    async fn local_install_and_semantic_selection_pin_the_same_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let path = source.path().join("private-key.wasm");
        let bytes = named_fixture("../namespace:agent/semantic", false);
        std::fs::write(&path, &bytes).unwrap();
        let resolver = Resolver::new(dir.path()).unwrap();
        let engine = Engine::default();
        let installed = resolver
            .install_validated(path.to_str().unwrap(), None, &engine)
            .await
            .unwrap();
        assert_eq!(installed.component_id, "../namespace:agent/semantic");
        assert_eq!(
            installed.snapshot.receipt.storage_key.as_str(),
            "private-key"
        );
        assert_eq!(
            installed.snapshot.receipt.intent,
            InstallIntent::InstallOnly
        );
        let selected = resolver
            .resolve_validated(
                "../namespace:agent/semantic",
                None,
                &engine,
                Some(crate::state::StageKind::Provider),
            )
            .await
            .unwrap();
        std::fs::write(&selected.path, b"replaced").unwrap();
        assert_eq!(selected.snapshot.wasm, bytes);
        assert_eq!(selected.snapshot.receipt, installed.snapshot.receipt);
        let sandbox = crate::sandbox::Sandbox::Policy(Box::new(crate::sandbox::PolicyGrants {
            policy_path: None,
            has_policy_grants: false,
            template: wassette::WasiStateTemplate::default(),
        }));
        let stage = crate::load_stage(&selected, sandbox).unwrap();
        assert_eq!(stage.component_id, "../namespace:agent/semantic");
        assert_eq!(stage.storage_key.as_str(), "private-key");
    }

    #[tokio::test]
    async fn invalid_or_wrong_role_replacement_preserves_last_known_good() {
        let dir = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let path = source.path().join("agent.wasm");
        let original = named_fixture("semantic", false);
        std::fs::write(&path, &original).unwrap();
        let resolver = Resolver::new(dir.path()).unwrap();
        let engine = Engine::default();
        let installed = resolver
            .install_validated(path.to_str().unwrap(), None, &engine)
            .await
            .unwrap();
        for replacement in [
            b"invalid".to_vec(),
            named_fixture("semantic", true),
            wat::parse_str(
                r#"(component $semantic (instance $empty)
                (export "wassette:acp/agent@8.0.0" (instance $empty)))"#,
            )
            .unwrap(),
            wat::parse_str(
                r#"(component $semantic (instance $empty)
                (export "tool" (instance $empty)))"#,
            )
            .unwrap(),
        ] {
            std::fs::write(&path, replacement).unwrap();
            assert!(
                resolver
                    .resolve_validated(
                        path.to_str().unwrap(),
                        None,
                        &engine,
                        Some(crate::state::StageKind::Provider)
                    )
                    .await
                    .is_err()
            );
            let snapshot = ComponentStore::open(dir.path())
                .unwrap()
                .read("semantic")
                .unwrap();
            assert_eq!(snapshot.wasm, original);
            assert_eq!(snapshot.receipt, installed.snapshot.receipt);
        }
    }

    #[tokio::test]
    async fn unnamed_and_legacy_artifacts_are_not_runnable_by_filename() {
        let dir = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let path = source.path().join("agent.wasm");
        let unnamed = wat::parse_str(
            r#"(component (instance $empty) (export "wassette:acp/agent@7.0.0" (instance $empty)))"#,
        ).unwrap();
        std::fs::write(&path, &unnamed).unwrap();
        std::fs::write(
            dir.path().join("agent.wasm"),
            named_fixture("semantic", false),
        )
        .unwrap();
        let resolver = Resolver::new(dir.path()).unwrap();
        let engine = Engine::default();
        assert!(
            resolver
                .install_validated(path.to_str().unwrap(), None, &engine)
                .await
                .is_err()
        );
        assert!(
            resolver
                .install_validated("agent", None, &engine)
                .await
                .is_err()
        );
        assert!(
            resolver
                .install_validated("semantic", None, &engine)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn ambiguous_root_names_are_rejected_without_reserving_a_slot() {
        use wasm_encoder::{ComponentSection, Encode};

        let dir = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let path = source.path().join("agent.wasm");
        let mut wasm = named_fixture("first", false);
        let mut names = wasm_encoder::ComponentNameSection::new();
        names.component("second");
        wasm.push(names.id());
        names.encode(&mut wasm);
        std::fs::write(&path, wasm).unwrap();
        let resolver = Resolver::new(dir.path()).unwrap();
        assert!(
            resolver
                .install_validated(path.to_str().unwrap(), None, &Engine::default())
                .await
                .is_err()
        );
        let snapshot = ComponentStore::open(dir.path())
            .unwrap()
            .snapshot_if_changed(None)
            .unwrap()
            .unwrap();
        assert!(snapshot.entries.is_empty());
        assert!(!dir.path().join("agent.wasm").exists());
    }

    #[tokio::test]
    async fn explicit_policy_clear_is_not_replaced_by_a_source_policy() {
        let dir = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let path = source.path().join("agent.wasm");
        std::fs::write(&path, named_fixture("semantic", false)).unwrap();
        let resolver = Resolver::new(dir.path()).unwrap();
        let engine = Engine::default();
        let installed = resolver
            .install_validated(path.to_str().unwrap(), None, &engine)
            .await
            .unwrap();
        assert_eq!(
            installed.snapshot.receipt.policy.provenance,
            PolicyProvenance::Default
        );
        let store = ComponentStore::open(dir.path()).unwrap();
        store
            .update_policy(
                "semantic",
                &installed.snapshot.receipt.revision,
                PreparedPolicy::absent(PolicyProvenance::ExplicitAttachment),
            )
            .unwrap();
        std::fs::write(
            source.path().join("agent.policy.yaml"),
            "version: '1.0'\npermissions:\n  network:\n    allow:\n      - host: example.com\n",
        )
        .unwrap();
        let next = resolver
            .install_validated(path.to_str().unwrap(), None, &engine)
            .await
            .unwrap();
        assert_eq!(next.snapshot.policy, None);
        assert_eq!(
            next.snapshot.receipt.policy.provenance,
            PolicyProvenance::ExplicitAttachment
        );
        assert!(!dir.path().join("agent.policy.yaml").exists());
        let attached = store
            .update_policy(
                "semantic",
                &next.snapshot.receipt.revision,
                PreparedPolicy::parse(
                    b"version: '1.0'\npermissions: {}\n".to_vec(),
                    PolicyProvenance::ExplicitAttachment,
                )
                .unwrap()
                .with_metadata(Some(PolicyMetadata {
                    source_uri: "file:///operator-policy.yaml".to_owned(),
                    attached_at: Some(42),
                }))
                .unwrap(),
            )
            .unwrap();
        let reinstalled = resolver
            .install_validated(path.to_str().unwrap(), None, &engine)
            .await
            .unwrap();
        assert_eq!(
            reinstalled.snapshot.receipt.policy,
            attached.entry.binding().policy
        );
    }

    #[tokio::test]
    async fn malformed_policy_and_operator_edits_are_never_silently_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let path = source.path().join("agent.wasm");
        std::fs::write(&path, named_fixture("semantic", false)).unwrap();
        let resolver = Resolver::new(dir.path()).unwrap();
        let engine = Engine::default();
        std::fs::write(path.with_extension("policy.yaml"), "not: [valid").unwrap();
        assert!(
            resolver
                .install_validated(path.to_str().unwrap(), None, &engine)
                .await
                .is_err()
        );
        assert!(!dir.path().join("agent.wasm").exists());
        std::fs::write(
            path.with_extension("policy.yaml"),
            "version: '1.0'\npermissions: {}\n",
        )
        .unwrap();
        let installed = resolver
            .install_validated(path.to_str().unwrap(), None, &engine)
            .await
            .unwrap();
        std::fs::write(dir.path().join("agent.policy.yaml"), "operator-edit").unwrap();
        assert!(
            resolver
                .install_validated(path.to_str().unwrap(), None, &engine)
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(dir.path().join("agent.policy.yaml")).unwrap(),
            b"operator-edit"
        );
        assert_eq!(
            std::fs::read(&installed.path).unwrap(),
            installed.snapshot.wasm
        );
    }

    #[tokio::test]
    async fn install_uses_supplied_acp_engine_without_host_linking() {
        let dir = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let path = source.path().join("async-agent.wasm");
        let wasm = wat::parse_str(
            r#"(component $async-provider
            (import "unlinked" (func))
            (core func $return (canon task.return))
            (core module $m
                (import "" "return" (func $return))
                (func (export "run") (call $return)))
            (core instance $i (instantiate $m (with "" (instance
                (export "return" (func $return))))))
            (func $pending async (canon lift (core func $i "run") async))
            (instance $agent (export "pending" (func $pending)))
            (export "wassette:acp/agent@7.0.0" (instance $agent)))"#,
        )
        .unwrap();
        assert!(Component::new(&Engine::default(), &wasm).is_err());
        std::fs::write(&path, wasm).unwrap();
        let mut config = wasmtime::Config::new();
        config.wasm_component_model_async(true);
        config.wasm_component_model_async_stackful(true);
        let engine = Engine::new(&config).unwrap();
        let installed = Resolver::new(dir.path())
            .unwrap()
            .install_validated(path.to_str().unwrap(), None, &engine)
            .await
            .unwrap();
        assert!(matches!(
            installed.snapshot.receipt.validation,
            ValidationEvidence::AcpCompiledAndExportChecked { .. }
        ));
    }

    #[tokio::test]
    async fn configured_secrets_path_reaches_runtime_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let custom_secrets = tempfile::tempdir().unwrap();
        let path = source.path().join("private-key.wasm");
        std::fs::write(&path, named_fixture("namespace:agent/semantic", false)).unwrap();
        std::fs::write(path.with_extension("policy.yaml"), "version: '1.0'\npermissions:\n  environment:\n    allow:\n      - key: WASSETTE_ACP_BOUND_TEST_TOKEN\n").unwrap();
        let config = wassette::LifecycleManager::builder(dir.path())
            .with_secrets_dir(custom_secrets.path())
            .build_config()
            .unwrap();
        let resolver = Resolver::with_config(config);
        let resolved = resolver
            .install_validated(path.to_str().unwrap(), None, &Engine::default())
            .await
            .unwrap();
        let binding = resolved.snapshot.receipt.secret_binding().unwrap();
        wassette::SecretsManager::new(custom_secrets.path().to_path_buf())
            .set_bound_component_secrets(
                &binding,
                &[("WASSETTE_ACP_BOUND_TEST_TOKEN".into(), "configured".into())],
            )
            .await
            .unwrap();
        let registry = crate::secrets::SecretsRegistry::new(resolver.secrets_dir());
        registry.register(binding).unwrap();
        let sandbox =
            crate::sandbox::Sandbox::load(false, &resolved, resolver.component_dir(), &registry)
                .await
                .unwrap();
        let crate::sandbox::Sandbox::Policy(grants) = sandbox else {
            panic!("expected policy")
        };
        assert_eq!(
            grants.template.config_vars["WASSETTE_ACP_BOUND_TEST_TOKEN"],
            "configured"
        );
        assert!(!dir.path().join("private-key.yaml").exists());
    }

    #[tokio::test]
    async fn resolver_preserves_configured_http_client() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy =
            reqwest::Proxy::all(format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let config = wassette::LifecycleManager::builder(dir.path())
            .with_http_client(reqwest::Client::builder().proxy(proxy).build().unwrap())
            .build_config()
            .unwrap();
        let resolver = Resolver::with_config(config);
        let request = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 512];
            let n = stream.read(&mut bytes).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
            String::from_utf8_lossy(&bytes[..n]).into_owned()
        });
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            resolver.install_validated(
                "https://example.invalid/agent.wasm",
                None,
                &Engine::default(),
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(error.to_string().contains("fetching"), "{error:#}");
        assert!(
            request
                .await
                .unwrap()
                .starts_with("CONNECT example.invalid:443")
        );
    }
}
