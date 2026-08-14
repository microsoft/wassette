// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Per-component secret store: host-side `wasmcloud:secrets@0.1.0-draft`
//! backend, over Wassette's [`SecretsManager`].
//!
//! Every component that imports `wasmcloud:secrets` transparently gets
//! its own private, persistent secret store, indexed by the component's
//! Wassette component id. A `store.get(key)` resolves against *that
//! component's* secrets file only, so a component can never read another
//! component's secrets. There is no config file: the host derives the
//! calling component's identity itself (the currently executing stage's
//! `component_id`), so the isolation is *structural* rather than a
//! declared grant.
//!
//! Secrets are the same ones the rest of Wassette uses — the YAML files
//! under `$XDG_CONFIG_HOME/wassette/secrets/<component-id>.yaml`, managed
//! with:
//!
//! ```text
//! wassette secret set <component-id> KEY=value
//! wassette secret list <component-id>
//! wassette secret delete <component-id> KEY
//! ```
//!
//! so a component's ACP secrets and its MCP environment secrets are one
//! and the same. The WIT interface is read-only; resolved values never
//! appear in logs.

use std::path::PathBuf;

use wassette::SecretsManager;

/// Spec-aligned error type. Mirrors `wasmcloud:secrets/store.secrets-error`.
#[derive(Debug)]
pub enum SecretsError {
    /// The backing store rejected the request (unparsable secrets file,
    /// bad encoding, unsupported operation, …).
    Upstream(String),
    /// I/O failure talking to the store (unreadable file, bad
    /// permissions, …).
    Io(String),
    /// No such secret in this component's store.
    NotFound,
}

/// Spec-aligned value type. Mirrors `wasmcloud:secrets/store.secret-value`.
/// `Debug` is redacted so it never leaks via logs.
#[derive(Clone)]
pub enum SecretValue {
    /// A UTF-8 secret. Everything Wassette's YAML store holds is a string.
    String(String),
    /// Raw bytes. Kept for parity with the WIT interface.
    Bytes(Vec<u8>),
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretValue::String(_) => f.write_str("SecretValue::String(<redacted>)"),
            SecretValue::Bytes(_) => f.write_str("SecretValue::Bytes(<redacted>)"),
        }
    }
}

/// Resolves `wasmcloud:secrets` lookups against Wassette's per-component
/// secret files.
pub struct SecretsRegistry {
    manager: SecretsManager,
}

impl SecretsRegistry {
    /// Build a resolver over the Wassette secrets directory (normally
    /// `$XDG_CONFIG_HOME/wassette/secrets`).
    pub fn new(secrets_dir: impl Into<PathBuf>) -> Self {
        Self {
            manager: SecretsManager::new(secrets_dir.into()),
        }
    }

    /// Build a resolver over an existing [`SecretsManager`].
    pub fn from_manager(manager: SecretsManager) -> Self {
        Self { manager }
    }

    /// The Wassette secrets directory backing this registry.
    pub fn secrets_dir(&self) -> &std::path::Path {
        self.manager.secrets_dir()
    }

    /// Every secret this component owns, as a plain map. Used to seed
    /// policy-declared environment variables (the MCP path does the same
    /// through `SecretsManager::load_component_secrets`). `None` when the
    /// store cannot be read — a missing file is an empty map, not an
    /// error.
    pub async fn snapshot(
        &self,
        component_id: &str,
    ) -> Option<std::collections::HashMap<String, String>> {
        self.manager.load_component_secrets(component_id).await.ok()
    }

    /// Resolve `key` from `component_id`'s private store. Returns
    /// [`SecretsError::NotFound`] when the component has no such entry.
    ///
    /// [`SecretsManager`] caches each component's file until its mtime
    /// changes, so editing a secrets file takes effect without a restart
    /// while repeated lookups stay cheap.
    pub async fn resolve(
        &self,
        component_id: &str,
        key: &str,
    ) -> Result<SecretValue, SecretsError> {
        let secrets = self
            .manager
            .load_component_secrets(component_id)
            .await
            .map_err(|e| {
                // `load_component_secrets` reports a missing *file* as an
                // empty map, so anything that fails here is a real store
                // problem: unreadable file, bad YAML, bad permissions.
                SecretsError::Io(format!("reading secrets for `{component_id}`: {e:#}"))
            })?;
        secrets
            .get(key)
            .map(|v| SecretValue::String(v.clone()))
            .ok_or(SecretsError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a component's secrets file the way `wassette secret set`
    /// does, so the tests exercise the on-disk contract rather than an
    /// internal API.
    async fn seed(dir: &std::path::Path, component_id: &str, pairs: &[(&str, &str)]) {
        let manager = SecretsManager::new(dir.to_path_buf());
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        manager
            .set_component_secrets(component_id, &owned)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn missing_secret_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let registry = SecretsRegistry::new(dir.path());
        assert!(matches!(
            registry.resolve("missing-comp", "nope").await,
            Err(SecretsError::NotFound)
        ));
    }

    #[tokio::test]
    async fn string_secret_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "comp-str", &[("api_key", "hunter2")]).await;
        let registry = SecretsRegistry::new(dir.path());
        match registry.resolve("comp-str", "api_key").await.unwrap() {
            SecretValue::String(s) => assert_eq!(s, "hunter2"),
            other => panic!("expected string, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn per_component_isolation() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "owner", &[("shared", "owned")]).await;
        seed(dir.path(), "other", &[("unrelated", "x")]).await;
        let registry = SecretsRegistry::new(dir.path());
        match registry.resolve("owner", "shared").await.unwrap() {
            SecretValue::String(s) => assert_eq!(s, "owned"),
            other => panic!("expected string, got {other:?}"),
        }
        // A component only ever sees its own file.
        assert!(matches!(
            registry.resolve("other", "shared").await,
            Err(SecretsError::NotFound)
        ));
    }

    #[tokio::test]
    async fn unknown_component_has_an_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let registry = SecretsRegistry::new(dir.path());
        assert!(matches!(
            registry.resolve("never-provisioned", "k").await,
            Err(SecretsError::NotFound)
        ));
    }

    #[tokio::test]
    async fn value_debug_is_redacted() {
        let v = SecretValue::String("hunter2".into());
        assert!(!format!("{v:?}").contains("hunter2"));
    }
}
