// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Client and resolution types for the wasm.directory package API.

use std::net::IpAddr;
use std::time::Duration;
use std::{env, fmt};

use anyhow::{anyhow, bail, Context, Result};
use semver::Version;
use serde::{Deserialize, Serialize};
use url::Url;

/// The default wasm.directory API endpoint.
pub const DEFAULT_API_BASE_URL: &str = "https://api.wasm.directory";

/// Environment variable that overrides the wasm.directory API endpoint.
pub const API_BASE_URL_ENV: &str = "WASSETTE_WASM_DIRECTORY_URL";

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
/// Default number of search records requested from wasm.directory.
pub const DEFAULT_SEARCH_PAGE_SIZE: usize = 20;
/// Maximum search page size accepted by wasm.directory.
pub const MAX_SEARCH_PAGE_SIZE: usize = 100;
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// Stable remote identity for a wasm.directory package.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PackageId {
    /// OCI registry hostname, such as `ghcr.io`.
    pub registry: String,
    /// OCI repository path, such as `owner/component`.
    pub repository: String,
}

impl PackageId {
    /// Parse a canonical `registry/repository` package identity.
    pub fn parse(value: &str) -> Result<Self> {
        let (registry, repository) = value
            .split_once('/')
            .ok_or_else(|| anyhow!("Package identity must be registry/repository"))?;
        Self::new(registry, repository)
    }

    /// Construct and validate a canonical package identity.
    pub fn new(registry: impl Into<String>, repository: impl Into<String>) -> Result<Self> {
        let registry = registry.into();
        let repository = repository.into();
        validate_registry(&registry)?;
        validate_repository(&repository)?;
        Ok(Self {
            registry,
            repository,
        })
    }

    /// Return the canonical `registry/repository` identity.
    pub fn reference(&self) -> String {
        format!("{}/{}", self.registry, self.repository)
    }
}

impl fmt::Display for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.registry, self.repository)
    }
}

/// A package returned by wasm.directory search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredPackage {
    /// Canonical remote package identity.
    pub package_id: String,
    /// Human-readable description, when available.
    pub description: Option<String>,
    /// Advertised registry kind; this metadata does not classify acquired bytes.
    pub advertised_kind: Option<String>,
    /// WIT identity when wasm.directory has one.
    pub wit_identity: Option<String>,
    /// Tags currently indexed by wasm.directory.
    pub tags: Vec<String>,
}

/// One page of search results and the upstream pagination state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchPage {
    /// Search results. Interface packages are excluded; unknown kinds are retained.
    pub packages: Vec<DiscoveredPackage>,
    /// Offset requested from wasm.directory.
    pub offset: usize,
    /// Requested upstream page size.
    pub limit: usize,
    /// Number of raw upstream records consumed, including excluded interface packages.
    pub upstream_count: usize,
    /// Offset for the next request when the upstream page may have more records.
    pub next_offset: Option<usize>,
    /// True when another upstream page may exist.
    pub may_have_more: bool,
}

/// A selected package version pinned to its OCI manifest digest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedPackage {
    /// Canonical remote package identity.
    pub package_id: PackageId,
    /// Version requested by the caller, if explicitly provided.
    pub requested_version: Option<String>,
    /// Exact source tag selected from the indexed version metadata.
    pub selected_version: String,
    /// OCI manifest digest used for immutable acquisition.
    pub manifest_digest: String,
    /// Digest-qualified OCI reference for the configured OCI client.
    pub oci_reference: String,
    /// Package description, when available.
    pub description: Option<String>,
    /// Advertised registry kind; artifact inspection remains authoritative.
    pub advertised_kind: Option<String>,
    /// WIT identity when wasm.directory has one.
    pub wit_identity: Option<String>,
}

/// HTTP client for wasm.directory discovery and package resolution.
///
/// This client is deliberately separate from the lifecycle's HTTP and OCI
/// clients so custom fetch credentials or headers are never sent to the
/// discovery service.
#[derive(Clone)]
pub struct WasmDirectoryClient {
    http_client: reqwest::Client,
    base_url: Url,
}

impl WasmDirectoryClient {
    /// Create a client with a dedicated unauthenticated HTTP client and the
    /// configured or default wasm.directory API endpoint.
    pub fn from_environment() -> Result<Self> {
        let base_url = match env::var(API_BASE_URL_ENV) {
            Ok(value) => {
                Url::parse(&value).with_context(|| format!("Invalid {API_BASE_URL_ENV} URL"))?
            }
            Err(env::VarError::NotPresent) => Url::parse(DEFAULT_API_BASE_URL)?,
            Err(error) => return Err(error).context(format!("Failed to read {API_BASE_URL_ENV}")),
        };

        Self::new(base_url)
    }

    /// Create a client with an explicit API base URL.
    ///
    /// The HTTP client is created internally without caller-supplied
    /// credentials, preventing auth intended for another service from leaking
    /// to the discovery endpoint.
    pub fn new(base_url: Url) -> Result<Self> {
        validate_base_url(&base_url)?;
        let http_client = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("Failed to create wasm.directory HTTP client")?;
        Ok(Self {
            http_client,
            base_url,
        })
    }

    /// Search packages in wasm.directory.
    ///
    /// The API caps page sizes at 100. Interface packages are removed after
    /// fetching the upstream page, so pagination advances by raw records
    /// rather than by the filtered result count.
    pub async fn search(
        &self,
        query: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<SearchPage> {
        validate_page_size(limit)?;
        let url = self.endpoint(&["v1", "search"])?;
        let response = self
            .http_client
            .get(url)
            .query(&[
                ("q", query.unwrap_or("")),
                ("offset", &offset.to_string()),
                ("limit", &limit.to_string()),
            ])
            .send()
            .await
            .context("Failed to query wasm.directory search API")?;
        let body = checked_body(response, "wasm.directory search API").await?;
        let upstream: Vec<ApiPackage> = serde_json::from_slice(&body)
            .context("Invalid JSON returned by wasm.directory search API")?;
        let upstream_count = upstream.len();
        let may_have_more = upstream_count == limit;
        let next_offset = if may_have_more {
            Some(
                offset
                    .checked_add(upstream_count)
                    .ok_or_else(|| anyhow!("Search offset overflow"))?,
            )
        } else {
            None
        };

        let packages = upstream
            .into_iter()
            .filter_map(|package| {
                if package.kind.as_deref() == Some("interface") {
                    return None;
                }

                let package_id = match PackageId::new(package.registry, package.repository) {
                    Ok(package_id) => package_id,
                    Err(error) => {
                        return Some(
                            Err(error)
                                .context("wasm.directory returned an invalid package identity"),
                        )
                    }
                };

                Some(Ok(DiscoveredPackage {
                    package_id: package_id.to_string(),
                    description: package.description,
                    advertised_kind: package.kind,
                    wit_identity: wit_identity(package.wit_namespace, package.wit_name),
                    tags: package.tags,
                }))
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(SearchPage {
            packages,
            offset,
            limit,
            upstream_count,
            next_offset,
            may_have_more,
        })
    }

    /// Resolve a package to an exact version and immutable OCI manifest digest.
    pub async fn resolve_package(
        &self,
        package_id: &PackageId,
        requested_version: Option<&str>,
    ) -> Result<ResolvedPackage> {
        let package_id = PackageId::new(&package_id.registry, &package_id.repository)
            .context("Invalid package identity")?;
        let url = self.package_detail_url(&package_id)?;
        let response = self
            .http_client
            .get(url)
            .send()
            .await
            .with_context(|| format!("Failed to query wasm.directory for {package_id}"))?;
        let body = checked_body(response, "wasm.directory package detail API").await?;
        let detail: ApiPackageDetail = serde_json::from_slice(&body)
            .context("Invalid package detail returned by wasm.directory")?;

        if detail.registry != package_id.registry || detail.repository != package_id.repository {
            bail!(
                "wasm.directory returned package {} for requested package {}",
                PackageId::new(detail.registry, detail.repository)
                    .context("wasm.directory returned an invalid package identity")?,
                package_id
            );
        }
        if detail.kind.as_deref() == Some("interface") {
            bail!("Package {package_id} is a WIT interface package, not a runnable component");
        }

        let version = select_version(&detail.versions, requested_version)
            .with_context(|| format!("Failed to select a version for package {package_id}"))?;
        validate_digest(&version.digest)
            .with_context(|| format!("Invalid manifest digest for {package_id}"))?;

        Ok(ResolvedPackage {
            package_id: package_id.clone(),
            requested_version: requested_version.map(str::to_owned),
            selected_version: version.tag.clone().ok_or_else(|| {
                anyhow!("wasm.directory selected an untagged version for {package_id}")
            })?,
            manifest_digest: version.digest.clone(),
            oci_reference: format!("oci://{package_id}@{}", version.digest),
            description: detail.description,
            advertised_kind: detail.kind,
            wit_identity: wit_identity(detail.wit_namespace, detail.wit_name),
        })
    }

    fn endpoint(&self, segments: &[&str]) -> Result<Url> {
        let mut url = self.base_url.clone();
        let mut path = url
            .path_segments_mut()
            .map_err(|_| anyhow!("wasm.directory API URL cannot be a base URL"))?;
        path.pop_if_empty();
        for segment in segments {
            path.push(segment);
        }
        drop(path);
        Ok(url)
    }

    fn package_detail_url(&self, package_id: &PackageId) -> Result<Url> {
        let mut url = self.endpoint(&["v1", "packages", "detail", &package_id.registry])?;
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| anyhow!("wasm.directory API URL cannot be a base URL"))?;
            for segment in package_id.repository.split('/') {
                path.push(segment);
            }
        }
        Ok(url)
    }
}

fn validate_base_url(url: &Url) -> Result<()> {
    let host = url.host_str();
    let local_http = url.scheme() == "http"
        && host.is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        });
    if (url.scheme() != "https" && !local_http)
        || host.is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("wasm.directory API URL must use HTTPS (HTTP is allowed only for loopback tests) and must not contain credentials, query, or fragment");
    }
    Ok(())
}

fn validate_page_size(limit: usize) -> Result<()> {
    if !(1..=MAX_SEARCH_PAGE_SIZE).contains(&limit) {
        bail!("Search limit must be between 1 and {MAX_SEARCH_PAGE_SIZE}");
    }
    Ok(())
}

fn validate_registry(registry: &str) -> Result<()> {
    if registry.is_empty()
        || !registry.is_ascii()
        || registry != registry.to_ascii_lowercase()
        || registry.chars().any(|character| {
            character.is_ascii_whitespace() || matches!(character, '/' | '\\' | '@' | '?' | '#')
        })
    {
        bail!("Invalid OCI registry hostname");
    }

    let parsed =
        Url::parse(&format!("https://{registry}")).context("Invalid OCI registry hostname")?;
    if parsed.host_str().is_none()
        || !parsed.path().is_empty() && parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        bail!("Invalid OCI registry hostname");
    }
    Ok(())
}

fn validate_repository(repository: &str) -> Result<()> {
    if repository.is_empty()
        || repository
            .split('/')
            .any(|segment| !is_valid_repository_segment(segment))
    {
        bail!("Invalid OCI repository path");
    }
    Ok(())
}

fn is_valid_repository_segment(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    let Some((&first, &last)) = bytes.first().zip(bytes.last()) else {
        return false;
    };
    if !is_repository_alphanumeric(first) || !is_repository_alphanumeric(last) {
        return false;
    }

    let mut index = 0;
    while index < bytes.len() {
        if is_repository_alphanumeric(bytes[index]) {
            index += 1;
            continue;
        }

        match bytes[index] {
            b'.' => index += 1,
            b'_' => {
                index += 1;
                if bytes.get(index) == Some(&b'_') {
                    index += 1;
                }
            }
            b'-' => {
                while bytes.get(index) == Some(&b'-') {
                    index += 1;
                }
            }
            _ => return false,
        }

        if !bytes
            .get(index)
            .is_some_and(|byte| is_repository_alphanumeric(*byte))
        {
            return false;
        }
    }
    true
}

fn is_repository_alphanumeric(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit()
}

fn validate_digest(digest: &str) -> Result<()> {
    let Some(hex) = digest.strip_prefix("sha256:") else {
        bail!("Only sha256 OCI manifest digests are supported");
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("Invalid sha256 OCI manifest digest");
    }
    Ok(())
}

fn validate_tag(tag: &str) -> Result<()> {
    let mut bytes = tag.bytes();
    let Some(first) = bytes.next() else {
        bail!("Version tag must not be empty");
    };
    if tag.len() > 128
        || !(first.is_ascii_alphanumeric() || first == b'_')
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
    {
        bail!("Invalid OCI version tag");
    }
    Ok(())
}

fn select_version<'a>(
    versions: &'a [ApiPackageVersion],
    requested_version: Option<&str>,
) -> Result<&'a ApiPackageVersion> {
    if let Some(requested_version) = requested_version {
        validate_tag(requested_version)?;
        return versions
            .iter()
            .find(|version| version.tag.as_deref() == Some(requested_version))
            .ok_or_else(|| anyhow!("Version tag {requested_version:?} is not indexed"));
    }

    let mut candidates = versions
        .iter()
        .filter_map(|version| {
            let tag = version.tag.as_deref()?;
            let semver = Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()?;
            Some((semver, tag, version))
        })
        .collect::<Vec<_>>();
    let has_stable_release = candidates
        .iter()
        .any(|(version, _, _)| version.pre.is_empty());
    if has_stable_release {
        candidates.retain(|(version, _, _)| version.pre.is_empty());
    }
    candidates.sort_by(
        |(left_version, left_tag, _), (right_version, right_tag, _)| {
            right_version
                .cmp(left_version)
                .then_with(|| left_tag.cmp(right_tag))
        },
    );
    candidates
        .first()
        .map(|(_, _, version)| *version)
        .ok_or_else(|| anyhow!("Package has no indexed semver versions"))
}

fn wit_identity(namespace: Option<String>, name: Option<String>) -> Option<String> {
    match (namespace, name) {
        (Some(namespace), Some(name)) => Some(format!("{namespace}:{name}")),
        _ => None,
    }
}

async fn checked_body(mut response: reqwest::Response, api_name: &str) -> Result<Vec<u8>> {
    let status = response.status();
    if !status.is_success() {
        bail!("{api_name} returned HTTP {status}");
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        bail!("{api_name} response exceeds the size limit");
    }

    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("Failed to read response from {api_name}"))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            bail!("{api_name} response exceeds the size limit");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[derive(Debug, Deserialize)]
struct ApiPackage {
    registry: String,
    repository: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    wit_namespace: Option<String>,
    #[serde(default)]
    wit_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApiPackageDetail {
    registry: String,
    repository: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    wit_namespace: Option<String>,
    #[serde(default)]
    wit_name: Option<String>,
    #[serde(default)]
    versions: Vec<ApiPackageVersion>,
}

#[derive(Debug, Deserialize)]
struct ApiPackageVersion {
    #[serde(default)]
    tag: Option<String>,
    digest: String,
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    use super::*;

    async fn mock_api(body: String) -> (Url, oneshot::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (request_tx, request_rx) = oneshot::channel();

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let bytes_read = socket.read(&mut request).await.unwrap();
            let request_line = String::from_utf8_lossy(&request[..bytes_read])
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            let _ = request_tx.send(request_line);

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        (
            Url::parse(&format!("http://{address}")).unwrap(),
            request_rx,
        )
    }

    async fn mock_redirect() -> Url {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let _ = socket.read(&mut request).await.unwrap();
            let response = "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:0/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        Url::parse(&format!("http://{address}")).unwrap()
    }

    #[test]
    fn package_id_parses_canonical_reference() {
        let package_id = PackageId::parse("ghcr.io/owner/tool").unwrap();
        assert_eq!(package_id.registry, "ghcr.io");
        assert_eq!(package_id.repository, "owner/tool");
        assert_eq!(package_id.to_string(), "ghcr.io/owner/tool");
    }

    #[test]
    fn package_id_rejects_non_identity_references_and_traversal() {
        for invalid in [
            "oci://ghcr.io/owner/tool:latest",
            "ghcr.io/owner/../tool",
            "ghcr.io//tool",
            "ghcr.io/Owner/tool",
            "ghcr.io/owner/tool?query",
            "ghcr.io/owner/a..b",
        ] {
            assert!(PackageId::parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn explicit_version_selects_exact_non_semver_tag() {
        let versions = vec![
            ApiPackageVersion {
                tag: Some("latest".to_owned()),
                digest: format!("sha256:{}", "a".repeat(64)),
            },
            ApiPackageVersion {
                tag: Some("1.0.0".to_owned()),
                digest: format!("sha256:{}", "b".repeat(64)),
            },
        ];

        assert_eq!(
            select_version(&versions, Some("latest"))
                .unwrap()
                .tag
                .as_deref(),
            Some("latest")
        );
        assert!(select_version(&versions, Some("unknown")).is_err());
    }

    #[test]
    fn default_version_prefers_latest_stable_and_accepts_leading_v() {
        let versions = vec![
            ApiPackageVersion {
                tag: Some("v2.0.0-rc.1".to_owned()),
                digest: format!("sha256:{}", "a".repeat(64)),
            },
            ApiPackageVersion {
                tag: Some("v1.9.0".to_owned()),
                digest: format!("sha256:{}", "b".repeat(64)),
            },
            ApiPackageVersion {
                tag: Some("2.0.0".to_owned()),
                digest: format!("sha256:{}", "c".repeat(64)),
            },
        ];

        assert_eq!(
            select_version(&versions, None).unwrap().tag.as_deref(),
            Some("2.0.0")
        );
    }

    #[test]
    fn default_version_falls_back_to_latest_prerelease_and_breaks_ties() {
        let versions = vec![
            ApiPackageVersion {
                tag: Some("v2.0.0-rc.1".to_owned()),
                digest: format!("sha256:{}", "a".repeat(64)),
            },
            ApiPackageVersion {
                tag: Some("2.0.0-rc.1".to_owned()),
                digest: format!("sha256:{}", "b".repeat(64)),
            },
            ApiPackageVersion {
                tag: Some("2.0.0-beta.1".to_owned()),
                digest: format!("sha256:{}", "c".repeat(64)),
            },
        ];

        assert_eq!(
            select_version(&versions, None).unwrap().tag.as_deref(),
            Some("2.0.0-rc.1")
        );
    }

    #[test]
    fn validates_manifest_digest_and_oci_tags() {
        assert!(validate_digest(&format!("sha256:{}", "a".repeat(64))).is_ok());
        assert!(validate_digest("sha256:abcd").is_err());
        assert!(validate_digest(&format!("sha256:{}", "A".repeat(64))).is_err());
        assert!(validate_digest(&format!("sha512:{}", "a".repeat(128))).is_err());
        assert!(validate_tag("v1.2.3-rc.1").is_ok());
        assert!(validate_tag("latest").is_ok());
        assert!(validate_tag("bad/tag").is_err());
        assert!(validate_tag(&"a".repeat(129)).is_err());
    }

    #[test]
    fn base_url_rejects_credentials_and_fragments() {
        for value in [
            "https://user:pass@example.com",
            "https://example.com/path?token=secret",
            "https://example.com/#fragment",
            "file:///tmp/api",
            "http://example.com",
        ] {
            let url = Url::parse(value).unwrap();
            assert!(validate_base_url(&url).is_err(), "{value}");
        }
    }

    #[tokio::test]
    async fn search_filters_interfaces_and_advances_by_raw_records() -> Result<()> {
        let (base_url, request_rx) = mock_api(
            r#"[
                {
                    "registry": "ghcr.io",
                    "repository": "owner/tool",
                    "kind": "component",
                    "description": "A tool",
                    "tags": ["1.0.0"],
                    "wit_namespace": "demo",
                    "wit_name": "tool"
                },
                {
                    "registry": "ghcr.io",
                    "repository": "owner/interfaces",
                    "kind": "interface",
                    "description": "WIT definitions",
                    "tags": []
                }
            ]"#
            .to_owned(),
        )
        .await;
        let client = WasmDirectoryClient::new(base_url)?;

        let page = client.search(Some("weather"), 5, 2).await?;
        let request = request_rx.await?;

        assert!(request.starts_with("GET /v1/search?"));
        assert!(request.contains("q=weather"));
        assert!(request.contains("offset=5"));
        assert!(request.contains("limit=2"));
        assert_eq!(page.upstream_count, 2);
        assert_eq!(page.packages.len(), 1);
        assert_eq!(page.packages[0].package_id, "ghcr.io/owner/tool");
        assert_eq!(
            page.packages[0].advertised_kind.as_deref(),
            Some("component")
        );
        assert_eq!(page.packages[0].wit_identity.as_deref(), Some("demo:tool"));
        assert_eq!(page.next_offset, Some(7));
        assert!(page.may_have_more);
        Ok(())
    }

    #[tokio::test]
    async fn resolves_version_to_immutable_manifest_reference() -> Result<()> {
        let digest = format!("sha256:{}", "a".repeat(64));
        let body = format!(
            r#"{{
                "registry": "ghcr.io",
                "repository": "owner/tool",
                "kind": "component",
                "description": "A tool",
                "wit_namespace": "demo",
                "wit_name": "tool",
                "versions": [{{"tag": "v1.2.3", "digest": "{digest}"}}]
            }}"#
        );
        let (base_url, request_rx) = mock_api(body).await;
        let client = WasmDirectoryClient::new(base_url)?;
        let package_id = PackageId::parse("ghcr.io/owner/tool")?;

        let resolved = client.resolve_package(&package_id, Some("v1.2.3")).await?;
        let request = request_rx.await?;

        assert_eq!(
            request,
            "GET /v1/packages/detail/ghcr.io/owner/tool HTTP/1.1"
        );
        assert_eq!(resolved.selected_version, "v1.2.3");
        assert_eq!(resolved.manifest_digest, digest);
        assert_eq!(
            resolved.oci_reference,
            format!("oci://ghcr.io/owner/tool@{}", resolved.manifest_digest)
        );
        Ok(())
    }

    #[tokio::test]
    async fn package_resolution_rejects_advertised_interface_packages() -> Result<()> {
        let (base_url, _) = mock_api(
            r#"{
                "registry": "ghcr.io",
                "repository": "owner/interfaces",
                "kind": "interface",
                "versions": []
            }"#
            .to_owned(),
        )
        .await;
        let client = WasmDirectoryClient::new(base_url)?;
        let package_id = PackageId::parse("ghcr.io/owner/interfaces")?;

        let error = client.resolve_package(&package_id, None).await.unwrap_err();
        assert!(error.to_string().contains("interface package"));
        Ok(())
    }

    #[tokio::test]
    async fn directory_requests_do_not_follow_redirects() -> Result<()> {
        let client = WasmDirectoryClient::new(mock_redirect().await)?;
        let error = client.search(None, 0, 1).await.unwrap_err();
        assert!(error.to_string().contains("302"));
        Ok(())
    }
}
