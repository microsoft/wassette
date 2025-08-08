// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! A module for downloading and loading components and policies from various sources.
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use futures::TryStreamExt;
use tokio::fs::metadata;
use tokio::io::AsyncWriteExt;
use tracing::{debug, warn};

/// Represents a downloaded resource, either from a local file or a temporary one.
pub enum DownloadedResource {
    Local(PathBuf),
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
    /// The `name` parameter must be unique across all plugins as it is used to identify the
    /// component.
    pub async fn new_temp_file(
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

    pub fn id(&self) -> Result<String> {
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

    pub async fn copy_to(self, dest: impl AsRef<Path>) -> Result<()> {
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
            DownloadedResource::Temp((tempdir, file)) => {
                let dest = dest.as_ref().join(
                    file.file_name()
                        .context("Path to copy is missing filename")?,
                );
                match tokio::fs::rename(&file, &dest).await {
                    Ok(()) => {}
                    Err(e) if e.raw_os_error() == Some(18) => {
                        // 18 == EXDEV on Unix-like systems (cross-device link).
                        // Fallback to copy + remove.
                        debug!(
                            from = %file.display(),
                            to = %dest.display(),
                            "Cross-device rename detected; falling back to copy"
                        );
                        tokio::fs::copy(&file, &dest).await.with_context(|| {
                            format!(
                                "Failed to copy component from {} to {} during EXDEV fallback",
                                file.display(),
                                dest.display()
                            )
                        })?;
                        if let Err(remove_err) = tokio::fs::remove_file(&file).await {
                            warn!(
                                path = %file.display(),
                                error = %remove_err,
                                "Failed to remove original temp file after copy"
                            );
                        }
                    }
                    Err(e) => return Err(e.into()),
                }
                // Close & cleanup the tempdir (spawn_blocking to mirror previous behavior)
                tokio::task::spawn_blocking(move || tempdir.close())
                    .await?
                    .context("Failed to clean up temporary download file")?;
            }
        }
        Ok(())
    }
}

/// A trait for resources that can be loaded from a URI.
pub trait Loadable: Sized {
    const FILE_EXTENSION: &'static str;
    const RESOURCE_TYPE: &'static str;

    async fn from_local_file(path: &Path) -> Result<DownloadedResource>;
    async fn from_oci_reference(
        reference: &str,
        oci_client: &oci_client::Client,
    ) -> Result<DownloadedResource>;
    async fn from_url(url: &str, http_client: &reqwest::Client) -> Result<DownloadedResource>;
}

/// Loadable implementation for WebAssembly components
pub struct ComponentResource;

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

    async fn from_oci_reference(
        reference: &str,
        oci_client: &oci_client::Client,
    ) -> Result<DownloadedResource> {
        let reference: oci_client::Reference =
            reference.parse().context("Failed to parse OCI reference")?;
        let data = oci_wasm::WasmClient::from(oci_client.clone())
            .pull(&reference, &oci_client::secrets::RegistryAuth::Anonymous)
            .await?;
        let (downloaded_resource, mut file) = DownloadedResource::new_temp_file(
            reference.repository().replace('/', "_"),
            Self::FILE_EXTENSION,
        )
        .await?;
        file.write_all(&data.layers[0].data).await?;

        file.flush().await?;
        file.sync_all().await?;
        drop(file); // Ensure the file handle is closed
        Ok(downloaded_resource)
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
pub struct PolicyResource;

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

    async fn from_oci_reference(
        _reference: &str,
        _oci_client: &oci_client::Client,
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
    let uri = uri.trim();
    let error_message = format!(
        "Invalid {} reference. Should be of the form scheme://reference",
        T::RESOURCE_TYPE
    );
    let (scheme, reference) = uri.split_once("://").context(error_message)?;

    match scheme {
        "file" => T::from_local_file(Path::new(reference)).await,
        "oci" => T::from_oci_reference(reference, oci_client).await,
        "https" => T::from_url(uri, http_client).await,
        _ => bail!("Unsupported {} scheme: {}", T::RESOURCE_TYPE, scheme),
    }
}
