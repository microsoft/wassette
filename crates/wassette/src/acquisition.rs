// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Private acquisition inputs, without installation or runtime registration.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

use crate::loader::{self, ComponentResource, DownloadedResource};
use crate::store::{OriginEvidence, SourceIdentity};
use crate::{LifecycleConfig, StorageKey};

/// Captured source bytes and acquisition evidence, not a validated installation.
pub struct AcquiredComponent {
    /// Private portable filename stem, never semantic component identity.
    pub storage_key: StorageKey,
    /// Exact captured Wasm bytes to inspect and validate with the selected runtime.
    pub wasm: Vec<u8>,
    /// Captured incoming policy; an explicit stored policy may take precedence.
    pub policy: Option<Vec<u8>>,
    /// Stable source-continuity binding, independent of cosmetic producer names.
    pub source: SourceIdentity,
    /// Acquisition evidence without credentials or unverified version claims.
    pub origin: OriginEvidence,
}

impl AcquiredComponent {
    /// Source-derived logical identity, independent of captured Wasm metadata.
    pub fn component_id(&self) -> Result<crate::ComponentId> {
        self.source.component_id(&self.storage_key, &self.origin)
    }
}

/// Acquire immutable inputs with the caller's configured HTTP and OCI clients.
///
/// This does not construct a runtime, mutate the component store, expose tools,
/// or activate an ACP agent. The returned bytes still require runtime validation.
/// Ordinary loading passes `false` for `include_local_policy`; ACP may pass
/// `true` to retain its existing adjacent-local-policy behavior.
pub async fn acquire_component(
    uri: &str,
    config: &LifecycleConfig,
    include_local_policy: bool,
) -> Result<AcquiredComponent> {
    let uri = uri.trim();
    let (remote_source, mut origin) = source_evidence(uri)?;
    let client = oci_wasm::WasmClient::from(config.oci_client().clone());
    let resource =
        loader::load_resource::<ComponentResource>(uri, &client, config.http_client()).await?;
    let storage_key = resource.storage_key()?;
    let local = matches!(&resource, DownloadedResource::Local(_));
    let path = tokio::fs::canonicalize(resource.as_ref())
        .await
        .context("resolving captured component source")?;
    let source = match remote_source {
        Some(source) => source,
        None => {
            origin.location = format!("file://{}", resource.as_ref().display());
            SourceIdentity::File(path.clone())
        }
    };
    let local_policy = if local && include_local_policy {
        let policy_path = resource
            .as_ref()
            .with_file_name(format!("{}.policy.yaml", storage_key.as_str()));
        loader::read_optional_file(&policy_path).await?
    } else {
        None
    };
    let captured = resource.capture().await?;
    let policy = if local {
        local_policy
    } else {
        captured.bundled_policy
    };
    Ok(AcquiredComponent {
        storage_key: captured.storage_key,
        wasm: captured.wasm,
        policy,
        source,
        origin,
    })
}

fn source_evidence(uri: &str) -> Result<(Option<SourceIdentity>, OriginEvidence)> {
    let (scheme, reference) = uri
        .split_once("://")
        .context("Invalid component reference: expected file://, oci://, or https://")?;
    let mut origin = OriginEvidence {
        location: String::new(),
        requested_version: None,
        selected_version: None,
        manifest_digest: None,
        immutable_uri: None,
        generation: None,
    };
    let source = match scheme {
        "file" => None,
        "oci" => {
            let reference: oci_client::Reference = reference
                .parse()
                .context("invalid OCI component reference")?;
            let repository = format!(
                "{}/{}",
                reference.registry().to_ascii_lowercase(),
                reference.repository()
            );
            origin.location = format!("oci://{reference}");
            origin.requested_version = reference.tag().map(str::to_owned);
            origin.manifest_digest = reference.digest().map(str::to_owned);
            origin.immutable_uri = reference.digest().map(|_| origin.location.clone());
            Some(SourceIdentity::OciRepository(repository))
        }
        "https" => {
            let mut url = url::Url::parse(uri).context("invalid HTTPS component URL")?;
            if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
                bail!("component URLs must not include user information or a fragment");
            }
            let request_sha256 = hex::encode(Sha256::digest(url.as_str().as_bytes()));
            url.set_query(None);
            origin.location = url.to_string();
            Some(SourceIdentity::Https {
                location: origin.location.clone(),
                request_sha256,
            })
        }
        _ => bail!("Unsupported component scheme: {scheme}"),
    };
    Ok((source, origin))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_versions_do_not_replace_source_identity() -> Result<()> {
        let (first, first_origin) = source_evidence("oci://ghcr.io/example/tool:1.0")?;
        let (second, second_origin) = source_evidence("oci://ghcr.io/example/tool:2.0")?;
        assert_eq!(first, second);
        assert_ne!(
            first_origin.requested_version,
            second_origin.requested_version
        );
        assert!(first_origin.manifest_digest.is_none());
        assert!(first_origin.selected_version.is_none());
        let key = StorageKey::parse("example_tool")?;
        assert_eq!(
            first
                .as_ref()
                .unwrap()
                .component_id(&key, &first_origin)?
                .as_str(),
            "ghcr.io/example/tool",
        );
        assert_eq!(
            second
                .as_ref()
                .unwrap()
                .component_id(&key, &second_origin)?,
            first.as_ref().unwrap().component_id(&key, &first_origin)?,
        );
        let (pinned, pinned_origin) = source_evidence(&format!(
            "oci://ghcr.io/example/tool@sha256:{}",
            "a".repeat(64)
        ))?;
        assert_eq!(pinned, first);
        assert_eq!(
            pinned.unwrap().component_id(&key, &pinned_origin)?.as_str(),
            "ghcr.io/example/tool",
        );
        Ok(())
    }

    #[test]
    fn https_queries_are_significant_but_not_persisted() -> Result<()> {
        let (first, origin) = source_evidence("https://example.test/tool.wasm?token=first-secret")?;
        let (second, _) = source_evidence("https://example.test/tool.wasm?token=second-secret")?;
        assert_ne!(first, second);
        assert_eq!(origin.location, "https://example.test/tool.wasm");
        assert_eq!(
            first
                .as_ref()
                .unwrap()
                .component_id(&StorageKey::parse("tool")?, &origin)?
                .as_str(),
            "tool",
        );
        assert!(!serde_json::to_string(&first)?.contains("first-secret"));
        assert!(!serde_json::to_string(&origin)?.contains("first-secret"));
        assert!(source_evidence("https://user:password@example.test/tool.wasm").is_err());
        Ok(())
    }

    #[tokio::test]
    async fn direct_oci_loads_unnamed_and_mismatched_names_with_stable_registry_identity(
    ) -> Result<()> {
        use crate::wasm_directory::fixtures::{FixturePackage, WasmDirectoryFixture};
        let unnamed =
            wat::parse_str(r#"(component (instance $empty) (export "empty" (instance $empty)))"#)?;
        let mismatched = wat::parse_str(
            r#"(component $"unrelated:producer" (instance $empty) (export "empty" (instance $empty)))"#,
        )?;
        let fixture = WasmDirectoryFixture::start(vec![FixturePackage::new(
            "owner/tool",
            None,
            [("1.0.0", unnamed), ("2.0.0", mismatched)],
        )])
        .await?;
        let root = tempfile::tempdir()?;
        let manager = crate::LifecycleManager::builder(root.path().join("store"))
            .with_secrets_dir(root.path().join("secrets"))
            .with_oci_client(fixture.oci_client())
            .build()
            .await?;
        let id = fixture.package_id("owner/tool");
        let mut previous = None;
        for tag in ["1.0.0", "2.0.0"] {
            let outcome = manager.load_component(&format!("oci://{id}:{tag}")).await?;
            assert_eq!(outcome.component_id, id);
            let snapshot = manager.component_store().read(&id)?;
            assert_eq!(
                snapshot.receipt.origin.requested_version.as_deref(),
                Some(tag)
            );
            if let Some((key, binding)) = previous {
                assert_eq!(snapshot.receipt.storage_key, key);
                assert_eq!(snapshot.receipt.secret_binding()?, binding);
            }
            previous = Some((
                snapshot.receipt.storage_key.clone(),
                snapshot.receipt.secret_binding()?,
            ));
        }
        Ok(())
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn local_symlink_uses_visible_filename_not_canonical_target_name() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("producer-output.wasm");
        tokio::fs::write(&target, b"captured bytes").await?;
        let visible = root.path().join("visible-name.wasm");
        std::os::unix::fs::symlink(&target, &visible)?;
        let config = crate::LifecycleManager::builder(root.path().join("store"))
            .with_secrets_dir(root.path().join("secrets"))
            .build_config()?;
        let captured =
            acquire_component(&format!("file://{}", visible.display()), &config, false).await?;
        assert_eq!(captured.component_id()?.as_str(), "local:visible-name");
        assert_eq!(
            captured.source,
            SourceIdentity::File(target.canonicalize()?)
        );
        Ok(())
    }

    #[tokio::test]
    async fn local_capture_never_installs_and_respects_policy_selection() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("private-key.wasm");
        tokio::fs::write(&source, b"captured artifact").await?;
        tokio::fs::write(
            directory.path().join("private-key.policy.yaml"),
            b"source policy",
        )
        .await?;
        let store_path = directory.path().join("store");
        let config = crate::LifecycleManager::builder(&store_path)
            .with_secrets_dir(directory.path().join("secrets"))
            .build_config()?;
        let uri = format!("file://{}", source.display());
        let ordinary = acquire_component(&uri, &config, false).await?;
        let acp = acquire_component(&uri, &config, true).await?;
        assert_eq!(ordinary.wasm, b"captured artifact");
        assert!(ordinary.policy.is_none());
        assert_eq!(acp.policy.as_deref(), Some(b"source policy".as_slice()));
        assert_eq!(ordinary.component_id()?.as_str(), "local:private-key");
        assert_eq!(
            ordinary.source,
            SourceIdentity::File(source.canonicalize()?)
        );
        assert!(!store_path.exists());
        Ok(())
    }
}
