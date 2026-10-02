// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Loopback wasm.directory API and OCI registry fixtures for tests.
//!
//! One plain-HTTP listener serves both the `/v1` discovery API and the `/v2`
//! OCI distribution routes. Callers must configure their OCI client with
//! `ClientProtocol::Http` to pull from it.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use oci_client::client::{Config, ImageLayer};
use oci_client::manifest::OciImageManifest;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use url::Url;

use super::WasmDirectoryClient;

/// One indexed package served by [`WasmDirectoryFixture`].
pub struct FixturePackage {
    /// OCI repository path, such as `owner/tool`.
    pub repository: String,
    /// Indexed `namespace:package` WIT identity, if any.
    pub wit_identity: Option<String>,
    /// Indexed `(tag, component bytes)` versions.
    pub versions: Vec<(String, Vec<u8>)>,
}

impl FixturePackage {
    /// Create a component package with the given WIT identity and versions.
    pub fn new(
        repository: &str,
        wit_identity: Option<&str>,
        versions: impl IntoIterator<Item = (&'static str, Vec<u8>)>,
    ) -> Self {
        Self {
            repository: repository.to_owned(),
            wit_identity: wit_identity.map(str::to_owned),
            versions: versions
                .into_iter()
                .map(|(tag, wasm)| (tag.to_owned(), wasm))
                .collect(),
        }
    }
}

type Routes = HashMap<String, (String, Vec<u8>)>;

/// A running loopback wasm.directory API and OCI registry.
pub struct WasmDirectoryFixture {
    /// Registry host and port, such as `127.0.0.1:1234`.
    pub registry: String,
    /// Base URL of the fixture's wasm.directory API.
    pub api_url: Url,
    digests: HashMap<(String, String), String>,
    task: JoinHandle<()>,
}

impl WasmDirectoryFixture {
    /// Bind a loopback listener and serve the given packages.
    pub async fn start(packages: Vec<FixturePackage>) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let registry = listener.local_addr()?.to_string();
        let mut routes = Routes::new();
        routes.insert(
            "/v2/".to_owned(),
            ("application/json".to_owned(), b"{}".to_vec()),
        );
        let mut digests = HashMap::new();
        let mut search = Vec::new();
        for package in &packages {
            let mut versions = Vec::new();
            for (tag, wasm) in &package.versions {
                let digest = publish(&mut routes, &package.repository, wasm.clone())?;
                let manifest = routes
                    .get(&format!("/v2/{}/manifests/{digest}", package.repository))
                    .context("published fixture manifest is missing")?
                    .clone();
                routes.insert(
                    format!("/v2/{}/manifests/{tag}", package.repository),
                    manifest,
                );
                digests.insert((package.repository.clone(), tag.clone()), digest.clone());
                versions.push(serde_json::json!({"tag": tag, "digest": digest}));
            }
            let (namespace, name) = match package.wit_identity.as_deref() {
                Some(identity) => {
                    let (namespace, name) = identity
                        .split_once(':')
                        .context("fixture WIT identity must be namespace:name")?;
                    (Some(namespace), Some(name))
                }
                None => (None, None),
            };
            let tags = package
                .versions
                .iter()
                .map(|(tag, _)| tag.clone())
                .collect::<Vec<_>>();
            search.push(serde_json::json!({
                "registry": registry,
                "repository": package.repository,
                "kind": "component",
                "tags": tags,
                "wit_namespace": namespace,
                "wit_name": name,
            }));
            routes.insert(
                format!("/v1/packages/detail/{registry}/{}", package.repository),
                (
                    "application/json".to_owned(),
                    serde_json::to_vec(&serde_json::json!({
                        "registry": registry,
                        "repository": package.repository,
                        "kind": "component",
                        "wit_namespace": namespace,
                        "wit_name": name,
                        "versions": versions,
                    }))?,
                ),
            );
        }
        routes.insert(
            "/v1/search".to_owned(),
            ("application/json".to_owned(), serde_json::to_vec(&search)?),
        );
        let routes = Arc::new(routes);
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let routes = routes.clone();
                tokio::spawn(async move {
                    let _ = reply(socket, &routes).await;
                });
            }
        });
        Ok(Self {
            api_url: Url::parse(&format!("http://{registry}"))?,
            registry,
            digests,
            task,
        })
    }

    /// Canonical `registry/repository` identity for a served repository.
    pub fn package_id(&self, repository: &str) -> String {
        format!("{}/{repository}", self.registry)
    }

    /// OCI manifest digest served for `repository` at `tag`.
    pub fn digest(&self, repository: &str, tag: &str) -> &str {
        &self.digests[&(repository.to_owned(), tag.to_owned())]
    }

    /// An OCI client that pulls from this plain-HTTP fixture registry.
    pub fn oci_client(&self) -> oci_client::Client {
        oci_client::Client::new(oci_client::client::ClientConfig {
            protocol: oci_client::client::ClientProtocol::Http,
            ..Default::default()
        })
    }

    /// A discovery client pointed at this fixture.
    pub fn directory(&self) -> Result<WasmDirectoryClient> {
        WasmDirectoryClient::new(self.api_url.clone())
    }
}

impl Drop for WasmDirectoryFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn publish(routes: &mut Routes, repository: &str, wasm: Vec<u8>) -> Result<String> {
    let layer = ImageLayer::new(wasm, oci_wasm::WASM_LAYER_MEDIA_TYPE.into(), None);
    let config = Config::new(
        serde_json::to_vec(&serde_json::json!({
            "created": "1970-01-01T00:00:00Z",
            "architecture": oci_wasm::WASM_ARCHITECTURE,
            "os": oci_wasm::COMPONENT_OS,
            "layerDigests": [layer.sha256_digest()],
            "component": {"exports": [], "imports": [], "target": null}
        }))?,
        oci_wasm::WASM_MANIFEST_CONFIG_MEDIA_TYPE.into(),
        None,
    );
    let mut manifest = OciImageManifest::build(std::slice::from_ref(&layer), &config, None);
    manifest.media_type = Some(oci_wasm::WASM_MANIFEST_MEDIA_TYPE.into());
    let manifest_bytes = serde_json::to_vec(&manifest)?;
    let digest = format!("sha256:{}", hex::encode(Sha256::digest(&manifest_bytes)));
    routes.insert(
        format!("/v2/{repository}/manifests/{digest}"),
        (
            oci_wasm::WASM_MANIFEST_MEDIA_TYPE.to_owned(),
            manifest_bytes,
        ),
    );
    routes.insert(
        format!("/v2/{repository}/blobs/{}", manifest.config.digest),
        (config.media_type, config.data.to_vec()),
    );
    routes.insert(
        format!("/v2/{repository}/blobs/{}", layer.sha256_digest()),
        (layer.media_type, layer.data.to_vec()),
    );
    Ok(digest)
}

async fn reply(mut socket: TcpStream, routes: &Routes) -> Result<()> {
    let mut request = Vec::new();
    let mut buffer = [0; 1024];
    while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
        let count = socket.read(&mut buffer).await?;
        anyhow::ensure!(count > 0 && request.len() + count < 8192, "bad request");
        request.extend_from_slice(&buffer[..count]);
    }
    let request = String::from_utf8(request)?;
    let mut words = request.split_whitespace();
    let method = words.next().context("missing method")?;
    let target = words.next().context("missing path")?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let path = path.replace("%3A", ":").replace("%3a", ":");
    let paged_out = path == "/v1/search"
        && query
            .split('&')
            .any(|pair| pair.starts_with("offset=") && pair != "offset=0");
    let (status, media_type, body) = match routes.get(&path) {
        _ if paged_out => ("200 OK", "application/json", b"[]".to_vec()),
        Some((media_type, body)) => ("200 OK", media_type.as_str(), body.clone()),
        None => ("404 Not Found", "application/json", b"{}".to_vec()),
    };
    let headers = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: {media_type}\r\n\
         Docker-Content-Digest: sha256:{}\r\n\
         Docker-Distribution-Api-Version: registry/2.0\r\nConnection: close\r\n\r\n",
        body.len(),
        hex::encode(Sha256::digest(&body))
    );
    socket.write_all(headers.as_bytes()).await?;
    if method != "HEAD" {
        socket.write_all(&body).await?;
    }
    Ok(())
}
