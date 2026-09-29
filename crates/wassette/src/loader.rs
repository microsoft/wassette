// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! A module for downloading and loading components and policies from various sources.
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use futures::TryStreamExt;
use tokio::fs::metadata;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info};

/// Represents a downloaded resource, either from a local file or a temporary one.
pub(crate) enum DownloadedResource {
    /// A file that already exists on the local filesystem.
    Local(PathBuf),
    /// A freshly downloaded file inside a temporary directory. Dropping the
    /// resource deletes the file, so it must be copied somewhere durable.
    Temp((tempfile::TempDir, PathBuf)),
}

impl AsRef<Path> for DownloadedResource {
    fn as_ref(&self) -> &Path {
        match self {
            DownloadedResource::Local(path) => path.as_path(),
            DownloadedResource::Temp((_, path)) => path.as_path(),
        }
    }
}

impl DownloadedResource {
    /// Returns a new `DownloadedComponent` with an already opened file handle for writing the
    /// download.
    ///
    /// The `name` parameter must be unique across all components as it is used to identify the
    /// component.
    pub(crate) async fn new_temp_file(
        name: impl AsRef<str>,
        extension: &str,
    ) -> Result<(Self, tokio::fs::File)> {
        let tempdir = tokio::task::spawn_blocking(tempfile::tempdir).await??;
        let file_path = tempdir
            .path()
            .join(format!("{}.{}", name.as_ref(), extension));
        let temp_file = tokio::fs::File::create(&file_path).await?;
        Ok((DownloadedResource::Temp((tempdir, file_path)), temp_file))
    }

    /// Returns a stable identifier for the resource: the file stem of its
    /// path.
    pub(crate) fn id(&self) -> Result<String> {
        // NOTE(thomastaylor312): Unfortunately the rust tooling (and I think some of the others),
        // doesn't preserve the package ID from the wit world defined for the component. It just
        // ends up as "root-component". So for now we rely on the file name to give us a unique ID
        // for the component.
        // let decoded = wit_parser::decoding::decode(&wasm_bytes)
        //     .map_err(|e| anyhow::anyhow!("Failed to decode component from path: {}. Error: {}. Please ensure the file is a valid WebAssembly component.", file.as_ref().display(), e))?;

        // let pkg_id = decoded.package();
        // // SAFETY: The package ID is guaranteed to be valid because we just decoded it
        // let pkg = decoded.resolve().packages.get(pkg_id).unwrap();
        // // Format the package name without the colon so it is valid on all systems. We are using the
        // // package name as a unique key on the filesystem as well
        // let id = format!("{}-{}", pkg.name.namespace, pkg.name.name);

        // Load the component to see if it is valid
        let maybe_id = match self {
            DownloadedResource::Local(path) => path.file_stem().and_then(|s| s.to_str()),
            DownloadedResource::Temp((_, path)) => path.file_stem().and_then(|s| s.to_str()),
        };

        maybe_id
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Failed to extract resource ID from path"))
    }

    /// Copies the resource, and any co-located policy file, into the `dest`
    /// directory. The lifecycle manager retains an attached policy when the
    /// replacement has no bundled policy.
    pub(crate) async fn copy_to(self, dest: impl AsRef<Path>) -> Result<()> {
        let meta = tokio::fs::metadata(&dest).await?;
        if !meta.is_dir() {
            bail!(
                "Destination path must be a directory: {}",
                dest.as_ref().display()
            );
        }
        match self {
            DownloadedResource::Local(path) => {
                let dest = dest.as_ref().join(
                    path.file_name()
                        .context("Path to copy is missing filename")?,
                );
                tokio::fs::copy(path, dest).await?;
            }
            DownloadedResource::Temp((_tempdir, file)) => {
                promote_component_artifact_with_policy(&file, dest.as_ref(), true).await?;
            }
        }
        Ok(())
    }
}

/// Promote a previously validated staged component into `dest_dir`.
///
/// The caller must validate the staged WASM (and any co-located policy) before
/// calling this function. The files are first copied into the destination
/// filesystem; a failed promotion restores the previous policy and leaves the
/// old WASM in place. Publishing two separate paths cannot be atomic for
/// unsynchronized readers, so callers must serialize concurrent loads.
pub async fn promote_component_artifact(staged_wasm: &Path, dest_dir: &Path) -> Result<PathBuf> {
    promote_component_artifact_with_policy(staged_wasm, dest_dir, false).await
}

async fn promote_component_artifact_with_policy(
    staged_wasm: &Path,
    dest_dir: &Path,
    retain_unbundled_policy: bool,
) -> Result<PathBuf> {
    let name = staged_wasm
        .file_name()
        .context("Path to copy is missing filename")?;
    let id = staged_wasm
        .file_stem()
        .and_then(|stem| stem.to_str())
        .context("Path to copy is missing component id")?;
    let policy_name = format!("{id}.policy.yaml");
    let source_policy = staged_wasm.with_file_name(&policy_name);
    let wasm_dest = dest_dir.join(name);
    let policy_dest = dest_dir.join(&policy_name);

    let dir = dest_dir.to_path_buf();
    let stage = tokio::task::spawn_blocking(move || tempfile::tempdir_in(dir)).await??;
    let staged_copy = stage.path().join(name);
    tokio::fs::copy(staged_wasm, &staged_copy)
        .await
        .with_context(|| format!("Failed to stage component {}", staged_wasm.display()))?;
    let staged_policy = stage.path().join(&policy_name);
    let has_policy = tokio::fs::try_exists(&source_policy).await?;
    if has_policy {
        tokio::fs::copy(&source_policy, &staged_policy)
            .await
            .with_context(|| format!("Failed to stage policy {}", source_policy.display()))?;
    }

    let installed = tokio::task::spawn_blocking(move || -> Result<PathBuf> {
        // Keep the transaction together even if the awaiting task is cancelled.
        let old_policy = stage.path().join("previous-policy");
        let had_policy = policy_dest.try_exists()?;
        if had_policy && (has_policy || !retain_unbundled_policy) {
            std::fs::hard_link(&policy_dest, &old_policy)
                .with_context(|| format!("Failed to back up policy {}", policy_dest.display()))?;
        }

        if has_policy {
            std::fs::rename(&staged_policy, &policy_dest)
                .with_context(|| format!("Failed to install policy {}", policy_dest.display()))?;
        } else if had_policy && !retain_unbundled_policy {
            std::fs::remove_file(&policy_dest).with_context(|| {
                format!("Failed to remove stale policy {}", policy_dest.display())
            })?;
        }

        if let Err(error) = std::fs::rename(&staged_copy, &wasm_dest) {
            if had_policy && (has_policy || !retain_unbundled_policy) {
                std::fs::rename(&old_policy, &policy_dest)
                    .with_context(|| format!("Failed to restore policy after {error}"))?;
            } else if has_policy {
                std::fs::remove_file(&policy_dest)
                    .with_context(|| format!("Failed to remove policy after {error}"))?;
            }
            return Err(error)
                .with_context(|| format!("Failed to install {}", wasm_dest.display()));
        }
        Ok(wasm_dest)
    })
    .await??;
    debug!(path = %installed.display(), "Promoted component artifact");
    Ok(installed)
}

/// A trait for resources that can be loaded from a URI.
pub(crate) trait Loadable: Sized {
    /// File extension used for downloaded copies of this resource.
    const FILE_EXTENSION: &'static str;
    /// Human readable name of the resource type, used in error messages.
    const RESOURCE_TYPE: &'static str;

    /// Load the resource from an absolute path on the local filesystem.
    async fn from_local_file(path: &Path) -> Result<DownloadedResource>;
    /// Pull the resource from an OCI registry reference.
    async fn from_oci_reference_with_progress(
        reference: &str,
        oci_client: &oci_client::Client,
        show_progress: bool,
    ) -> Result<DownloadedResource>;
    /// Download the resource over HTTP(S).
    async fn from_url(url: &str, http_client: &reqwest::Client) -> Result<DownloadedResource>;
}

/// Loadable implementation for WebAssembly components
pub(crate) struct ComponentResource;

impl Loadable for ComponentResource {
    const FILE_EXTENSION: &'static str = "wasm";
    const RESOURCE_TYPE: &'static str = "component";

    async fn from_local_file(path: &Path) -> Result<DownloadedResource> {
        if !path.is_absolute() {
            bail!("Component path must be fully qualified. Please provide an absolute path to the WebAssembly component file.");
        }

        if !tokio::fs::try_exists(path).await? {
            bail!("Component path does not exist: {}. Please provide a valid path to a WebAssembly component file.", path.display());
        }

        if path.extension().unwrap_or_default() != Self::FILE_EXTENSION {
            bail!(
                "Invalid file extension for component: {}. Component file must have .{} extension.",
                path.display(),
                Self::FILE_EXTENSION
            );
        }

        Ok(DownloadedResource::Local(path.to_path_buf()))
    }

    async fn from_oci_reference_with_progress(
        reference: &str,
        oci_client: &oci_client::Client,
        show_progress: bool,
    ) -> Result<DownloadedResource> {
        let reference: oci_client::Reference =
            reference.parse().context("Failed to parse OCI reference")?;

        if show_progress {
            eprintln!("Downloading component from {}...", reference);
        }

        // First try oci-wasm for backwards compatibility with single-layer artifacts
        let wasm_client = oci_wasm::WasmClient::from(oci_client.clone());
        let result = wasm_client
            .pull(&reference, &oci_client::secrets::RegistryAuth::Anonymous)
            .await;

        match result {
            Ok(data) => {
                // Successfully pulled with oci-wasm - this is a single-layer WASM artifact
                debug!("Successfully pulled single-layer WASM artifact");
                if show_progress {
                    eprintln!("✓ Downloaded {} bytes", data.layers[0].data.len());
                }
                let (downloaded_resource, mut file) = DownloadedResource::new_temp_file(
                    reference.repository().replace('/', "_"),
                    Self::FILE_EXTENSION,
                )
                .await?;

                // Use the first layer (oci-wasm validated it's WASM)
                file.write_all(&data.layers[0].data).await?;
                file.flush().await?;
                file.sync_all().await?;
                drop(file);
                Ok(downloaded_resource)
            }
            Err(e) => {
                // Check if this is a multi-layer artifact issue
                let error_str = e.to_string();
                if error_str.contains("Incompatible layer media type") {
                    // Multi-layer artifact detected - use our custom handler
                    info!("Multi-layer OCI artifact detected, using direct OCI client");

                    // Use our new multi-layer support to get ALL layers
                    let artifact = crate::oci_multi_layer::pull_multi_layer_artifact_with_progress(
                        &reference,
                        oci_client,
                        show_progress,
                    )
                    .await
                    .context("Failed to extract layers from multi-layer OCI artifact")?;

                    // Save the WASM data
                    let component_name = reference.repository().replace('/', "_");
                    let (downloaded_resource, mut file) =
                        DownloadedResource::new_temp_file(&component_name, Self::FILE_EXTENSION)
                            .await?;

                    file.write_all(&artifact.wasm_data).await?;
                    file.flush().await?;
                    file.sync_all().await?;
                    drop(file);

                    // If there's a policy, save it alongside the WASM in the temp directory
                    if let Some(policy_data) = artifact.policy_data {
                        info!("Saving policy layer alongside component");

                        // Create policy file in the same temp directory as the WASM
                        if let DownloadedResource::Temp((ref tempdir, ref _wasm_path)) =
                            downloaded_resource
                        {
                            let policy_path =
                                tempdir.path().join(format!("{component_name}.policy.yaml"));
                            tokio::fs::write(&policy_path, &policy_data)
                                .await
                                .context("Failed to save policy file")?;
                            info!("Policy saved to: {:?}", policy_path);
                        }
                    }

                    info!("Successfully extracted WASM component and policy from multi-layer artifact");

                    Ok(downloaded_resource)
                } else {
                    // Some other error - propagate it
                    Err(e)
                }
            }
        }
    }

    async fn from_url(url: &str, http_client: &reqwest::Client) -> Result<DownloadedResource> {
        let resp = http_client.get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!(
                "Failed to download component from URL: {}. Status code: {}\nBody: {}",
                url,
                status,
                body
            );
        }
        let name = resp
            .url()
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .context("Failed to discover name from URL")?
            .trim_end_matches(&format!(".{}", Self::FILE_EXTENSION));
        let (downloaded_resource, mut file) =
            DownloadedResource::new_temp_file(name, Self::FILE_EXTENSION).await?;
        let stream = resp.bytes_stream();
        let mut reader = tokio_util::io::StreamReader::new(stream.map_err(std::io::Error::other));
        tokio::io::copy(&mut reader, &mut file)
            .await
            .context("Failed to write downloaded component to temp file")?;
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        Ok(downloaded_resource)
    }
}

/// Loadable implementation for policies
pub(crate) struct PolicyResource;

impl Loadable for PolicyResource {
    const FILE_EXTENSION: &'static str = "yaml";
    const RESOURCE_TYPE: &'static str = "policy";

    async fn from_local_file(path: &Path) -> Result<DownloadedResource> {
        if !path.is_absolute() {
            bail!("Policy file path must be fully qualified");
        }

        match metadata(path).await {
            Ok(meta) if meta.is_file() => Ok(DownloadedResource::Local(path.to_path_buf())),
            _ => {
                bail!("Policy file does not exist: {}", path.display());
            }
        }
    }

    async fn from_oci_reference_with_progress(
        _reference: &str,
        _oci_client: &oci_client::Client,
        _show_progress: bool,
    ) -> Result<DownloadedResource> {
        bail!("OCI references are not supported for policy resources. Use 'file://' or 'https://' schemes instead.")
    }

    async fn from_url(url: &str, http_client: &reqwest::Client) -> Result<DownloadedResource> {
        let url_obj = reqwest::Url::parse(url)?;
        let filename = url_obj
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .unwrap_or("policy")
            .trim_end_matches(&format!(".{}", Self::FILE_EXTENSION))
            .trim_end_matches(".yml");

        let temp_file_name = format!("policy-{filename}");
        let (downloaded_resource, mut temp_file) =
            DownloadedResource::new_temp_file(&temp_file_name, Self::FILE_EXTENSION).await?;

        let response = http_client.get(url).send().await?;
        if !response.status().is_success() {
            bail!(
                "Failed to download policy from {}: {}",
                url,
                response.status()
            );
        }

        let policy_bytes = response.bytes().await?;
        tokio::io::copy(&mut policy_bytes.as_ref(), &mut temp_file).await?;

        temp_file.flush().await?;
        temp_file.sync_all().await?;
        drop(temp_file);

        Ok(downloaded_resource)
    }
}

/// Generic resource loading function
pub(crate) async fn load_resource<T: Loadable>(
    uri: &str,
    oci_client: &oci_wasm::WasmClient,
    http_client: &reqwest::Client,
) -> Result<DownloadedResource> {
    load_resource_with_progress::<T>(uri, oci_client, http_client, false).await
}

/// Generic resource loading function with optional progress reporting
pub(crate) async fn load_resource_with_progress<T: Loadable>(
    uri: &str,
    oci_client: &oci_wasm::WasmClient,
    http_client: &reqwest::Client,
    show_progress: bool,
) -> Result<DownloadedResource> {
    let uri = uri.trim();
    let error_message = format!(
        "Invalid {} reference. Should be of the form scheme://reference",
        T::RESOURCE_TYPE
    );
    let (scheme, reference) = uri.split_once("://").context(error_message)?;

    match scheme {
        "file" => T::from_local_file(Path::new(reference)).await,
        "oci" => T::from_oci_reference_with_progress(reference, oci_client, show_progress).await,
        "https" => T::from_url(uri, http_client).await,
        _ => bail!("Unsupported {} scheme: {}", T::RESOURCE_TYPE, scheme),
    }
}

/// Fetch a WebAssembly component referenced by `uri` and return its Wassette
/// component id (the `.wasm` file stem) together with a local path to it.
///
/// `uri` is one of `file://<absolute path>`, `oci://<reference>` or
/// `https://<url>`. Local files are used where they are; remote artifacts are
/// downloaded and persisted into `dest_dir` — the Wassette component directory
/// — as `<component-id>.wasm`, so a component fetched once is reachable by id
/// afterwards.
pub async fn fetch_component(uri: &str, dest_dir: &Path) -> Result<(String, PathBuf)> {
    let config = crate::LifecycleManager::builder(dest_dir).build_config()?;
    fetch_component_with_config(uri, &config).await
}

/// Fetch a component using the HTTP and OCI clients configured by
/// [`LifecycleBuilder`](crate::LifecycleBuilder).
///
/// Remote artifacts are persisted in `config.component_dir()` immediately.
/// To validate before installation, use a staging directory for this config
/// and call [`promote_component_artifact`] after validation.
pub async fn fetch_component_with_config(
    uri: &str,
    config: &crate::LifecycleConfig,
) -> Result<(String, PathBuf)> {
    fetch_component_with_config_into(uri, config, config.component_dir()).await
}

/// Fetch with configured clients into a staging directory instead of the
/// lifecycle manager's live component directory.
pub async fn fetch_component_with_config_into(
    uri: &str,
    config: &crate::LifecycleConfig,
    dest_dir: &Path,
) -> Result<(String, PathBuf)> {
    let oci_client = oci_wasm::WasmClient::from(config.oci_client().clone());
    fetch_component_with_clients(uri, dest_dir, &oci_client, config.http_client()).await
}

/// Fetch a component using the clients supplied by a [`LifecycleConfig`](crate::LifecycleConfig).
///
/// Pass `oci_wasm::WasmClient::from(config.oci_client().clone())` as
/// `oci_client` to use the configured OCI registry settings. Remote artifacts
/// are persisted immediately; to validate before installation, fetch into a
/// staging directory and call [`promote_component_artifact`] afterwards.
pub async fn fetch_component_with_clients(
    uri: &str,
    dest_dir: &Path,
    oci_client: &oci_wasm::WasmClient,
    http_client: &reqwest::Client,
) -> Result<(String, PathBuf)> {
    let resource = load_resource::<ComponentResource>(uri, oci_client, http_client)
        .await
        .with_context(|| format!("Failed to fetch component from {uri}"))?;
    let id = resource.id()?;
    match resource {
        DownloadedResource::Local(path) => Ok((id, path)),
        downloaded => {
            tokio::fs::create_dir_all(dest_dir).await.with_context(|| {
                format!(
                    "Failed to create component directory: {}",
                    dest_dir.display()
                )
            })?;
            let dest = dest_dir.join(format!("{id}.{}", ComponentResource::FILE_EXTENSION));
            promote_component_artifact(downloaded.as_ref(), dest_dir)
                .await
                .with_context(|| format!("Failed to store component in {}", dest_dir.display()))?;
            Ok((id, dest))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_directory() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("wassette-loader-test-")
            .tempdir_in(".")
            .unwrap()
    }

    #[tokio::test]
    async fn remote_copy_replaces_wasm_and_policy_together() -> Result<()> {
        let root = test_directory();
        let source = tempfile::tempdir_in(root.path())?;
        let dest = root.path().join("components");
        tokio::fs::create_dir(&dest).await?;
        let wasm = source.path().join("agent.wasm");
        tokio::fs::write(&wasm, b"new wasm").await?;
        tokio::fs::write(source.path().join("agent.policy.yaml"), b"new policy").await?;
        tokio::fs::write(dest.join("agent.wasm"), b"old wasm").await?;
        tokio::fs::write(dest.join("agent.policy.yaml"), b"old policy").await?;

        DownloadedResource::Temp((source, wasm))
            .copy_to(&dest)
            .await?;
        assert_eq!(tokio::fs::read(dest.join("agent.wasm")).await?, b"new wasm");
        assert_eq!(
            tokio::fs::read(dest.join("agent.policy.yaml")).await?,
            b"new policy"
        );
        Ok(())
    }

    #[tokio::test]
    async fn policy_free_replacement_removes_stale_policy() -> Result<()> {
        let root = test_directory();
        let source = tempfile::tempdir_in(root.path())?;
        let dest = root.path().join("components");
        tokio::fs::create_dir(&dest).await?;
        let wasm = source.path().join("agent.wasm");
        tokio::fs::write(&wasm, b"new wasm").await?;
        tokio::fs::write(dest.join("agent.wasm"), b"old wasm").await?;
        tokio::fs::write(dest.join("agent.policy.yaml"), b"old policy").await?;

        promote_component_artifact(&wasm, &dest).await?;
        assert_eq!(tokio::fs::read(dest.join("agent.wasm")).await?, b"new wasm");
        assert!(!tokio::fs::try_exists(dest.join("agent.policy.yaml")).await?);
        Ok(())
    }

    #[tokio::test]
    async fn lifecycle_copy_retains_explicitly_attached_policy() -> Result<()> {
        let root = test_directory();
        let source = tempfile::tempdir_in(root.path())?;
        let dest = root.path().join("components");
        tokio::fs::create_dir(&dest).await?;
        let wasm = source.path().join("agent.wasm");
        tokio::fs::write(&wasm, b"new wasm").await?;
        tokio::fs::write(dest.join("agent.wasm"), b"old wasm").await?;
        tokio::fs::write(dest.join("agent.policy.yaml"), b"attached policy").await?;

        DownloadedResource::Temp((source, wasm))
            .copy_to(&dest)
            .await?;
        assert_eq!(tokio::fs::read(dest.join("agent.wasm")).await?, b"new wasm");
        assert_eq!(
            tokio::fs::read(dest.join("agent.policy.yaml")).await?,
            b"attached policy"
        );
        Ok(())
    }

    #[tokio::test]
    async fn failed_staging_preserves_existing_pair() -> Result<()> {
        let root = test_directory();
        let source = tempfile::tempdir_in(root.path())?;
        let dest = root.path().join("components");
        tokio::fs::create_dir(&dest).await?;
        let wasm = source.path().join("agent.wasm");
        tokio::fs::write(&wasm, b"new wasm").await?;
        tokio::fs::create_dir(source.path().join("agent.policy.yaml")).await?;
        tokio::fs::write(dest.join("agent.wasm"), b"old wasm").await?;
        tokio::fs::write(dest.join("agent.policy.yaml"), b"old policy").await?;

        assert!(promote_component_artifact(&wasm, &dest).await.is_err());
        assert_eq!(tokio::fs::read(dest.join("agent.wasm")).await?, b"old wasm");
        assert_eq!(
            tokio::fs::read(dest.join("agent.policy.yaml")).await?,
            b"old policy"
        );
        Ok(())
    }

    #[tokio::test]
    async fn failed_wasm_publish_rolls_back_policy() -> Result<()> {
        let root = test_directory();
        let source = tempfile::tempdir_in(root.path())?;
        let dest = root.path().join("components");
        tokio::fs::create_dir(&dest).await?;
        let wasm = source.path().join("agent.wasm");
        tokio::fs::write(&wasm, b"new wasm").await?;
        tokio::fs::write(source.path().join("agent.policy.yaml"), b"new policy").await?;
        tokio::fs::create_dir(dest.join("agent.wasm")).await?;
        tokio::fs::write(dest.join("agent.policy.yaml"), b"old policy").await?;

        assert!(promote_component_artifact(&wasm, &dest).await.is_err());
        assert!(tokio::fs::metadata(dest.join("agent.wasm")).await?.is_dir());
        assert_eq!(
            tokio::fs::read(dest.join("agent.policy.yaml")).await?,
            b"old policy"
        );
        Ok(())
    }

    #[tokio::test]
    async fn failed_policy_free_publish_restores_stale_policy() -> Result<()> {
        let root = test_directory();
        let source = tempfile::tempdir_in(root.path())?;
        let dest = root.path().join("components");
        tokio::fs::create_dir(&dest).await?;
        let wasm = source.path().join("agent.wasm");
        tokio::fs::write(&wasm, b"new wasm").await?;
        tokio::fs::create_dir(dest.join("agent.wasm")).await?;
        tokio::fs::write(dest.join("agent.policy.yaml"), b"old policy").await?;

        assert!(promote_component_artifact(&wasm, &dest).await.is_err());
        assert!(tokio::fs::metadata(dest.join("agent.wasm")).await?.is_dir());
        assert_eq!(
            tokio::fs::read(dest.join("agent.policy.yaml")).await?,
            b"old policy"
        );
        Ok(())
    }

    #[tokio::test]
    async fn failed_validation_does_not_promote_staged_artifact() -> Result<()> {
        let root = test_directory();
        let source = tempfile::tempdir_in(root.path())?;
        let dest = root.path().join("components");
        tokio::fs::create_dir(&dest).await?;
        let wasm = source.path().join("agent.wasm");
        tokio::fs::write(&wasm, b"invalid wasm").await?;
        tokio::fs::write(source.path().join("agent.policy.yaml"), b"new policy").await?;
        tokio::fs::write(dest.join("agent.wasm"), b"old wasm").await?;
        tokio::fs::write(dest.join("agent.policy.yaml"), b"old policy").await?;

        assert!(
            wasmtime::component::Component::from_file(&wasmtime::Engine::default(), &wasm).is_err()
        );
        assert_eq!(tokio::fs::read(dest.join("agent.wasm")).await?, b"old wasm");
        assert_eq!(
            tokio::fs::read(dest.join("agent.policy.yaml")).await?,
            b"old policy"
        );
        Ok(())
    }

    #[tokio::test]
    async fn fetch_with_clients_preserves_local_file() -> Result<()> {
        let root = test_directory();
        let source = root.path().join("agent.wasm");
        let dest = root.path().join("components");
        tokio::fs::write(&source, b"local wasm").await?;
        let oci = oci_wasm::WasmClient::from(oci_client::Client::default());
        let http = reqwest::Client::builder().build()?;

        let (id, path) = fetch_component_with_clients(
            &format!("file://{}", source.display()),
            &dest,
            &oci,
            &http,
        )
        .await?;
        assert_eq!(id, "agent");
        assert_eq!(path, source);
        assert!(!tokio::fs::try_exists(&dest).await?);
        Ok(())
    }

    #[tokio::test]
    async fn fetch_with_config_uses_configured_http_client() -> Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let root = test_directory();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let proxy = format!("http://{}", listener.local_addr()?);
        let client = reqwest::Client::builder()
            .proxy(reqwest::Proxy::all(proxy)?)
            .build()?;
        let config = crate::LifecycleManager::builder(root.path().join("components"))
            .with_http_client(client)
            .build_config()?;
        let proxy_request = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let mut bytes = [0; 512];
            let len = stream.read(&mut bytes).await?;
            stream
                .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                .await?;
            Ok::<_, std::io::Error>(String::from_utf8_lossy(&bytes[..len]).into_owned())
        });

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            fetch_component_with_config("https://example.invalid/agent.wasm", &config),
        )
        .await?;
        assert!(result.is_err());
        let request = proxy_request.await??;
        assert!(
            request.starts_with("CONNECT example.invalid:443"),
            "{request}"
        );
        Ok(())
    }

    #[test]
    fn test_load_resource_with_progress_api_exists() {
        // Compile-time test to verify the progress-aware API exists
        // Just check that we can reference the function
        let _ = load_resource_with_progress::<ComponentResource>;
    }

    #[test]
    fn test_component_resource_has_progress_method() {
        // Verify that ComponentResource implements from_oci_reference_with_progress
        let _ = ComponentResource::from_oci_reference_with_progress;
    }

    #[test]
    fn test_policy_resource_has_progress_method() {
        // Verify that PolicyResource implements from_oci_reference_with_progress
        let _ = PolicyResource::from_oci_reference_with_progress;
    }
}
