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
    /// Stable source-continuity binding, separate from the artifact's own name.
    pub source: SourceIdentity,
    /// Acquisition evidence without credentials or unverified version claims.
    pub origin: OriginEvidence,
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
            origin.location = format!("file://{}", path.display());
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
        Ok(())
    }

    #[test]
    fn https_queries_are_significant_but_not_persisted() -> Result<()> {
        let (first, origin) = source_evidence("https://example.test/tool.wasm?token=first-secret")?;
        let (second, _) = source_evidence("https://example.test/tool.wasm?token=second-secret")?;
        assert_ne!(first, second);
        assert_eq!(origin.location, "https://example.test/tool.wasm");
        assert!(!serde_json::to_string(&first)?.contains("first-secret"));
        assert!(!serde_json::to_string(&origin)?.contains("first-secret"));
        assert!(source_evidence("https://user:password@example.test/tool.wasm").is_err());
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
        assert_eq!(
            ordinary.source,
            SourceIdentity::File(source.canonicalize()?)
        );
        assert!(!store_path.exists());
        Ok(())
    }
}
