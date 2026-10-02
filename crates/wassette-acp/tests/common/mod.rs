// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::ffi::OsStr;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use wassette::store::{
    ComponentStore, InstallOptions, InstallOwner, OriginEvidence, PolicyProvenance,
    PreparedInstall, PreparedPolicy, SourceIdentity, StoredArtifactKind, ValidationEvidence,
};

/// Copy a fixture without changing its component metadata or bytes.
pub struct NamedFixture {
    path: PathBuf,
    _directory: tempfile::TempDir,
}

impl NamedFixture {
    pub fn copy(path: &Path) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let wasm = std::fs::read(path).unwrap();
        let destination = directory.path().join(path.file_name().unwrap());
        std::fs::write(&destination, wasm).unwrap();
        let policy = path.with_extension("policy.yaml");
        if policy.is_file() {
            std::fs::copy(&policy, destination.with_extension("policy.yaml")).unwrap();
        }
        Self {
            path: destination,
            _directory: directory,
        }
    }
}

impl Deref for NamedFixture {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl AsRef<OsStr> for NamedFixture {
    fn as_ref(&self) -> &OsStr {
        self.path.as_os_str()
    }
}

/// Seed a genuinely bound fixture scope, never an unowned legacy YAML file.
pub fn seed_secrets(wasm_path: &Path, secrets_dir: &Path, pairs: &[(&str, &str)]) {
    let root = tempfile::tempdir().unwrap();
    let store = ComponentStore::open(root.path()).unwrap();
    let wasm = std::fs::read(wasm_path).unwrap();
    let source = SourceIdentity::File(wasm_path.canonicalize().unwrap());
    let storage_key =
        wassette::StorageKey::parse(wasm_path.file_stem().unwrap().to_str().unwrap()).unwrap();
    let name = format!("local:{}", storage_key.as_str());
    let expected = store.observe_source(&name, &storage_key, &source).unwrap();
    let prepared = PreparedInstall::prepare(
        wasm,
        InstallOptions {
            storage_key,
            source,
            origin: OriginEvidence {
                location: format!("file://{}", wasm_path.display()),
                requested_version: None,
                selected_version: None,
                manifest_digest: None,
                immutable_uri: None,
                generation: None,
            },
            owner: InstallOwner::Explicit,
            policy: PreparedPolicy::absent(PolicyProvenance::Default),
            observation: None,
        },
        |_, _, _| {
            Ok(ValidationEvidence::AcpCompiledAndExportChecked {
                runtime: "test-fixture".to_owned(),
            })
        },
    )
    .unwrap();
    let receipt = store.commit_install(prepared, expected).unwrap().entry;
    let binding = receipt.binding().secret_binding().unwrap();
    let values = pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect::<Vec<_>>();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        wassette::SecretsManager::new(secrets_dir.to_path_buf())
            .set_bound_component_secrets(&binding, &values)
            .await
            .unwrap();
    });
}

pub fn install_fixture(store_root: &Path, wasm_path: &Path) -> wassette::store::InstallReceipt {
    let store = ComponentStore::open(store_root).unwrap();
    let wasm = std::fs::read(wasm_path).unwrap();
    let inspection = wassette::inspect_artifact(&wasm).unwrap();
    let storage_key =
        wassette::StorageKey::parse(wasm_path.file_stem().unwrap().to_str().unwrap()).unwrap();
    let source = SourceIdentity::File(wasm_path.canonicalize().unwrap());
    let origin = OriginEvidence {
        location: format!("file://{}", wasm_path.display()),
        requested_version: None,
        selected_version: None,
        manifest_digest: None,
        immutable_uri: None,
        generation: None,
    };
    let component_id = source.component_id(&storage_key, &origin).unwrap();
    let expected = store
        .observe_source(component_id.as_str(), &storage_key, &source)
        .unwrap();
    let (kind, validation) = match inspection.shape {
        wassette::ArtifactShape::ToolCandidate => (
            StoredArtifactKind::Tool,
            ValidationEvidence::OrdinaryPrepared {
                runtime: "ACP stdio fixture".to_owned(),
            },
        ),
        wassette::ArtifactShape::AcpProvider => (
            StoredArtifactKind::AcpProvider,
            ValidationEvidence::AcpCompiledAndExportChecked {
                runtime: "ACP stdio fixture".to_owned(),
            },
        ),
        wassette::ArtifactShape::AcpLayer => (
            StoredArtifactKind::AcpLayer,
            ValidationEvidence::AcpCompiledAndExportChecked {
                runtime: "ACP stdio fixture".to_owned(),
            },
        ),
        wassette::ArtifactShape::Unsupported(_) => panic!("unsupported test fixture"),
    };
    let policy_path = wasm_path.with_extension("policy.yaml");
    let policy_bytes = policy_path
        .is_file()
        .then(|| std::fs::read(policy_path).unwrap());
    let policy = policy_bytes
        .map(|bytes| PreparedPolicy::parse(bytes, PolicyProvenance::ExplicitAttachment).unwrap())
        .unwrap_or_else(|| PreparedPolicy::absent(PolicyProvenance::Default));
    let prepared = PreparedInstall::prepare(
        wasm,
        InstallOptions {
            storage_key,
            source,
            origin,
            owner: InstallOwner::Explicit,
            policy,
            observation: None,
        },
        |_, _, _| Ok(validation),
    )
    .unwrap()
    .retain_existing_binding(&expected)
    .unwrap();
    let outcome = store.commit_install(prepared, expected).unwrap();
    match outcome.entry {
        wassette::store::StoredEntry::Installed(receipt) if receipt.kind == kind => receipt,
        entry => panic!("unexpected fixture install result: {entry:?}"),
    }
}
