// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Builder and configuration helpers for constructing
//! [`LifecycleManager`](crate::LifecycleManager).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::{
    get_default_secrets_dir, LifecycleManager, DEFAULT_HTTP_TIMEOUT_SECS, DEFAULT_OCI_TIMEOUT_SECS,
};

/// Fully-specified configuration for constructing a [`LifecycleManager`].
#[derive(Clone)]
pub struct LifecycleConfig {
    plugin_dir: PathBuf,
    secrets_dir: PathBuf,
    environment_vars: HashMap<String, String>,
    http_client: reqwest::Client,
    oci_client: oci_client::Client,
    eager_load: bool,
}

impl LifecycleConfig {
    /// Location where components live.
    pub fn plugin_dir(&self) -> &Path {
        &self.plugin_dir
    }

    /// Directory where component secrets are stored.
    pub fn secrets_dir(&self) -> &Path {
        &self.secrets_dir
    }

    /// Environment variables exposed to components.
    pub fn environment_vars(&self) -> &HashMap<String, String> {
        &self.environment_vars
    }

    /// HTTP client used for remote fetches.
    pub fn http_client(&self) -> &reqwest::Client {
        &self.http_client
    }

    /// OCI client used for registry interactions.
    pub fn oci_client(&self) -> &oci_client::Client {
        &self.oci_client
    }

    /// Whether eager loading was requested.
    pub fn eager_load(&self) -> bool {
        self.eager_load
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        PathBuf,
        PathBuf,
        HashMap<String, String>,
        reqwest::Client,
        oci_client::Client,
        bool,
    ) {
        (
            self.plugin_dir,
            self.secrets_dir,
            self.environment_vars,
            self.http_client,
            self.oci_client,
            self.eager_load,
        )
    }
}

/// Builder that validates inputs and produces a [`LifecycleConfig`] or [`LifecycleManager`].
#[derive(Clone)]
pub struct LifecycleBuilder {
    plugin_dir: PathBuf,
    secrets_dir: Option<PathBuf>,
    environment_vars: HashMap<String, String>,
    http_client: Option<reqwest::Client>,
    oci_client: Option<oci_client::Client>,
    eager_load: bool,
}

impl LifecycleBuilder {
    /// Create a builder with sensible defaults for the provided plugin
    /// directory.
    pub(crate) fn new(plugin_dir: PathBuf) -> Self {
        Self {
            plugin_dir,
            secrets_dir: None,
            environment_vars: HashMap::new(),
            http_client: None,
            oci_client: None,
            eager_load: true,
        }
    }

    /// Replace the entire environment variable map the components receive.
    pub fn with_environment_vars(mut self, environment: HashMap<String, String>) -> Self {
        self.environment_vars = environment;
        self
    }

    /// Set an individual environment variable.
    pub fn with_environment_var(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.environment_vars.insert(key.into(), value.into());
        self
    }

    /// Override the secrets directory.
    pub fn with_secrets_dir(mut self, secrets_dir: impl Into<PathBuf>) -> Self {
        self.secrets_dir = Some(secrets_dir.into());
        self
    }

    /// Override the HTTP client.
    pub fn with_http_client(mut self, client: reqwest::Client) -> Self {
        self.http_client = Some(client);
        self
    }

    /// Override the OCI client.
    pub fn with_oci_client(mut self, client: oci_client::Client) -> Self {
        self.oci_client = Some(client);
        self
    }

    /// Control whether the manager eagerly loads components during build.
    pub fn with_eager_loading(mut self, eager: bool) -> Self {
        self.eager_load = eager;
        self
    }

    /// Produce a validated [`LifecycleConfig`] without constructing a manager.
    pub fn build_config(self) -> Result<LifecycleConfig> {
        let plugin_dir = match self.plugin_dir.canonicalize() {
            Ok(path) => path,
            Err(_) => self.plugin_dir.clone(),
        };

        let secrets_dir = self.secrets_dir.unwrap_or_else(get_default_secrets_dir);

        let http_client = match self.http_client {
            Some(client) => client,
            None => default_http_client()?,
        };

        let oci_client = match self.oci_client {
            Some(client) => client,
            None => default_oci_client()?,
        };

        Ok(LifecycleConfig {
            plugin_dir,
            secrets_dir,
            environment_vars: self.environment_vars,
            http_client,
            oci_client,
            eager_load: self.eager_load,
        })
    }

    /// Construct a [`LifecycleManager`] using the current builder settings.
    ///
    /// If eager loading is enabled the plugin directory is scanned
    /// immediately; otherwise the caller can defer loading until a later
    /// [`LifecycleManager::load_all_components`](crate::LifecycleManager::load_all_components)
    /// invocation.
    pub async fn build(self) -> Result<LifecycleManager> {
        let config = self.build_config()?;
        let eager = config.eager_load();
        let manager = LifecycleManager::from_config(config).await?;
        if eager {
            manager.load_all_components().await?;
        }
        Ok(manager)
    }
}

/// Create the default HTTP client used when none is supplied.
fn default_http_client() -> Result<reqwest::Client> {
    let http_timeout = std::env::var("HTTP_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_HTTP_TIMEOUT_SECS);

    reqwest::Client::builder()
        .timeout(Duration::from_secs(http_timeout))
        .build()
        .context("Failed to create default HTTP client")
}

/// Create the default OCI client used when none is supplied.
fn default_oci_client() -> Result<oci_client::Client> {
    let oci_timeout = std::env::var("OCI_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_OCI_TIMEOUT_SECS);

    Ok(oci_client::Client::new(oci_client::client::ClientConfig {
        read_timeout: Some(Duration::from_secs(oci_timeout)),
        ..Default::default()
    }))
}
