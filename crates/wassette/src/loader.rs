// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Private acquisition of component and policy inputs, without live publication.
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use futures::TryStreamExt;
use tokio::fs::metadata;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info};

use crate::StorageKey;

/// Represents a downloaded resource, either from a local file or a temporary one.
pub(crate) enum DownloadedResource {
    /// A file that already exists on the local filesystem.
    Local(PathBuf),
    /// A privately downloaded file. Capture its bytes before dropping this
    /// resource; validated installation belongs to `ComponentStore`.
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
    /// `name` is a physical storage key, not the component's semantic name.
    pub(crate) async fn new_temp_file(
        name: impl AsRef<str>,
        extension: &str,
    ) -> Result<(Self, tokio::fs::File)> {
        if extension == ComponentResource::FILE_EXTENSION {
            StorageKey::parse(name.as_ref()).context("Invalid component storage key")?;
        }
        let tempdir = tokio::task::spawn_blocking(tempfile::tempdir).await??;
        let file_path = tempdir
            .path()
            .join(format!("{}.{}", name.as_ref(), extension));
        let temp_file = tokio::fs::File::create(&file_path).await?;
        Ok((DownloadedResource::Temp((tempdir, file_path)), temp_file))
    }

    /// Returns the portable physical key. Embedded identity is inspected separately.
    pub(crate) fn storage_key(&self) -> Result<StorageKey> {
        let stem = self
            .as_ref()
            .file_stem()
            .and_then(|s| s.to_str())
            .context("Failed to extract component storage key from path")?;
        StorageKey::parse(stem).context("Invalid component storage key")
    }

    pub(crate) async fn capture(self) -> Result<CapturedComponent> {
        let storage_key = self.storage_key()?;
        let wasm = tokio::fs::read(self.as_ref())
            .await
            .with_context(|| format!("Failed to capture component {}", self.as_ref().display()))?;
        let bundled_policy = match &self {
            Self::Local(_) => None,
            Self::Temp((_, path)) => {
                read_optional_file(
                    &path.with_file_name(format!("{}.policy.yaml", storage_key.as_str())),
                )
                .await?
            }
        };
        Ok(CapturedComponent {
            storage_key,
            wasm,
            bundled_policy,
        })
    }
}

pub(crate) struct CapturedComponent {
    pub(crate) storage_key: StorageKey,
    pub(crate) wasm: Vec<u8>,
    pub(crate) bundled_policy: Option<Vec<u8>>,
}

pub(crate) async fn read_optional_file(path: &Path) -> Result<Option<Vec<u8>>> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match tokio::fs::symlink_metadata(path).await {
                Err(missing) if missing.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Ok(_) => Err(error)
                    .with_context(|| format!("Failed to read existing policy {}", path.display())),
                Err(error) => Err(error)
                    .with_context(|| format!("Failed to inspect policy {}", path.display())),
            }
        }
        Err(error) => Err(error).with_context(|| format!("Failed to read {}", path.display())),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_directory() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("wassette-loader-test-")
            .tempdir_in(std::env::current_dir().unwrap())
            .unwrap()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dangling_policy_is_not_an_absent_policy() -> Result<()> {
        let root = test_directory();
        let path = root.path().join("component.policy.yaml");
        assert!(read_optional_file(&path).await?.is_none());
        std::os::unix::fs::symlink(root.path().join("missing-policy"), &path)?;
        assert!(read_optional_file(&path).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn capture_freezes_downloaded_wasm_and_policy() -> Result<()> {
        let root = test_directory();
        let source = tempfile::tempdir_in(root.path())?;
        let source_path = source.path().to_owned();
        let wasm = source.path().join("agent.wasm");
        tokio::fs::write(&wasm, b"new wasm").await?;
        tokio::fs::write(source.path().join("agent.policy.yaml"), b"new policy").await?;
        let captured = DownloadedResource::Temp((source, wasm)).capture().await?;
        assert!(!source_path.exists());
        tokio::fs::create_dir(&source_path).await?;
        tokio::fs::write(source_path.join("agent.wasm"), b"mutated source").await?;
        assert_eq!(captured.storage_key.as_str(), "agent");
        assert_eq!(captured.wasm, b"new wasm");
        assert_eq!(
            captured.bundled_policy.as_deref(),
            Some(b"new policy".as_slice())
        );
        Ok(())
    }

    #[tokio::test]
    async fn capture_retains_explicit_source_sidecar_absence() -> Result<()> {
        let root = test_directory();
        let source = tempfile::tempdir_in(root.path())?;
        let wasm = source.path().join("agent.wasm");
        tokio::fs::write(&wasm, b"new wasm").await?;
        let captured = DownloadedResource::Temp((source, wasm)).capture().await?;
        assert_eq!(captured.wasm, b"new wasm");
        assert!(captured.bundled_policy.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn local_capture_does_not_adopt_a_sibling_policy() -> Result<()> {
        let root = test_directory();
        let wasm = root.path().join("agent.wasm");
        tokio::fs::write(&wasm, b"new wasm").await?;
        tokio::fs::write(root.path().join("agent.policy.yaml"), b"malformed: [").await?;
        let captured = DownloadedResource::Local(wasm.clone()).capture().await?;
        assert_eq!(captured.wasm, b"new wasm");
        assert!(captured.bundled_policy.is_none());
        assert_eq!(tokio::fs::read(&wasm).await?, b"new wasm");
        Ok(())
    }

    #[tokio::test]
    async fn capture_rejects_unreadable_source_sidecar() -> Result<()> {
        let root = test_directory();
        let source = tempfile::tempdir_in(root.path())?;
        let wasm = source.path().join("agent.wasm");
        tokio::fs::write(&wasm, b"new wasm").await?;
        tokio::fs::create_dir(source.path().join("agent.policy.yaml")).await?;
        assert!(DownloadedResource::Temp((source, wasm))
            .capture()
            .await
            .is_err());
        Ok(())
    }

    #[tokio::test]
    async fn load_with_clients_preserves_local_file() -> Result<()> {
        let root = test_directory();
        let source = root.path().join("agent.wasm");
        let dest = root.path().join("components");
        tokio::fs::write(&source, b"local wasm").await?;
        let oci = oci_wasm::WasmClient::from(oci_client::Client::default());
        let http = reqwest::Client::builder().build()?;

        let resource = load_resource::<ComponentResource>(
            &format!("file://{}", source.display()),
            &oci,
            &http,
        )
        .await?;
        assert_eq!(resource.storage_key()?.as_str(), "agent");
        assert_eq!(resource.as_ref(), source);
        assert!(!tokio::fs::try_exists(&dest).await?);
        Ok(())
    }

    #[tokio::test]
    async fn acquisition_uses_configured_http_client() -> Result<()> {
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
            crate::acquisition::acquire_component(
                "https://example.invalid/agent.wasm",
                &config,
                false,
            ),
        )
        .await?;
        assert!(result.is_err());
        let request =
            tokio::time::timeout(std::time::Duration::from_secs(10), proxy_request).await???;
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
