// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Per-component secret store: host-side `wasmcloud:secrets@2.1.0`
//! backend, over Wassette's [`SecretsManager`].
//!
//! Secrets are stored persistently by receipt-bound source and private key. A
//! `store.get(key)` normally uses the current stage's component id to
//! select its secrets file, without a guest-supplied config file. In
//! layered chains, concurrent callbacks share a store-wide stage stack
//! and can be attributed to the wrong stage; this is not an isolation
//! guarantee. Layered chains with stored secrets require
//! `--allow-shared-grants`, which acknowledges but does not fix this risk.
//!
//! Secrets are the same ones the rest of Wassette uses — the YAML files
//! under `$XDG_CONFIG_HOME/wassette/secrets/`, managed
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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use anyhow::{Context, Result};
use wassette::{SecretBinding, SecretsManager};

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
    bindings: RwLock<HashMap<String, SecretBinding>>,
}

impl SecretsRegistry {
    /// Build a resolver over the Wassette secrets directory (normally
    /// `$XDG_CONFIG_HOME/wassette/secrets`).
    pub fn new(secrets_dir: impl Into<PathBuf>) -> Self {
        Self {
            manager: SecretsManager::new(secrets_dir.into()),
            bindings: RwLock::new(HashMap::new()),
        }
    }

    /// Build a resolver over an existing [`SecretsManager`].
    pub fn from_manager(manager: SecretsManager) -> Self {
        Self {
            manager,
            bindings: RwLock::new(HashMap::new()),
        }
    }

    /// Register a selected stage's receipt binding, never an arbitrary filename.
    pub fn register(&self, binding: SecretBinding) -> Result<()> {
        let mut bindings = self
            .bindings
            .write()
            .map_err(|_| anyhow::anyhow!("secret registry poisoned"))?;
        if let Some(existing) = bindings.get(binding.component_name()) {
            anyhow::ensure!(
                existing == &binding,
                "component already has a different secret binding"
            );
        } else {
            bindings.insert(binding.component_name().to_owned(), binding);
        }
        Ok(())
    }

    fn binding(&self, component_id: &str) -> Result<SecretBinding> {
        self.bindings
            .read()
            .map_err(|_| anyhow::anyhow!("secret registry poisoned"))?
            .get(component_id)
            .cloned()
            .with_context(|| format!("no registered secret binding for component `{component_id}`"))
    }

    /// The Wassette secrets directory backing this registry.
    pub fn secrets_dir(&self) -> &std::path::Path {
        self.manager.secrets_dir()
    }

    /// Every secret this component owns, as a plain map. Used to seed
    /// policy-declared environment variables (the MCP path does the same
    /// through its bound secret API). Missing values are an empty map;
    /// malformed, unreadable, orphaned and conflicting scopes are errors.
    pub async fn snapshot(&self, component_id: &str) -> Result<HashMap<String, String>> {
        self.manager
            .load_bound_component_secrets(&self.binding(component_id)?)
            .await
            .with_context(|| format!("reading secrets for component `{component_id}`"))
    }

    /// Check for stored secrets even when no policy injects them into WASI.
    /// A malformed or unreadable store must not make a layered chain appear safe.
    pub async fn has_secrets(&self, component_id: &str) -> Result<bool> {
        Ok(!self
            .snapshot(component_id)
            .await
            .with_context(|| format!("checking secrets for component `{component_id}`"))?
            .is_empty())
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
        let secrets = self.snapshot(component_id).await.map_err(|e| {
            SecretsError::Io(format!("reading secrets for `{component_id}`: {e:#}"))
        })?;
        secrets
            .get(key)
            .map(|v| SecretValue::String(v.clone()))
            .ok_or(SecretsError::NotFound)
    }
}

#[cfg(test)]
pub(crate) fn test_binding(component_id: &str) -> SecretBinding {
    SecretBinding::new(
        &test_identity(component_id),
        &wassette::StorageKey::parse(component_id).unwrap(),
        "test-fixture-source",
    )
    .unwrap()
}

#[cfg(test)]
fn test_identity(name: &str) -> wassette::ComponentId {
    wassette::ComponentId::from_name(name).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn seed(dir: &std::path::Path, component_id: &str, pairs: &[(&str, &str)]) {
        let manager = SecretsManager::new(dir.to_path_buf());
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        manager
            .set_bound_component_secrets(&test_binding(component_id), &owned)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn missing_secret_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let registry = SecretsRegistry::new(dir.path());
        registry.register(test_binding("missing-comp")).unwrap();
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
        registry.register(test_binding("comp-str")).unwrap();
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
        registry.register(test_binding("owner")).unwrap();
        registry.register(test_binding("other")).unwrap();
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
    async fn unknown_component_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let registry = SecretsRegistry::new(dir.path());
        assert!(matches!(
            registry.resolve("never-provisioned", "k").await,
            Err(SecretsError::Io(_))
        ));
    }

    #[tokio::test]
    async fn semantic_names_do_not_select_secret_filenames_or_other_sources() {
        let dir = tempfile::tempdir().unwrap();
        let semantic = test_identity("../namespace:agent/tool");
        let key = wassette::StorageKey::parse("private-key").unwrap();
        let binding = SecretBinding::new(&semantic, &key, "source:a").unwrap();
        let manager = SecretsManager::new(dir.path().to_path_buf());
        manager
            .set_bound_component_secrets(&binding, &[("TOKEN".into(), "owner".into())])
            .await
            .unwrap();
        let registry = SecretsRegistry::new(dir.path());
        registry.register(binding.clone()).unwrap();
        assert!(
            matches!(registry.resolve(semantic.as_str(), "TOKEN").await, Ok(SecretValue::String(value)) if value == "owner")
        );
        let impostor = SecretBinding::new(&semantic, &key, "source:b").unwrap();
        assert!(registry.register(impostor.clone()).is_err());
        let other_registry = SecretsRegistry::new(dir.path());
        other_registry.register(impostor).unwrap();
        assert!(other_registry.snapshot(semantic.as_str()).await.is_err());
        let alias = SecretBinding::new(
            &test_identity("other-semantic"),
            &wassette::StorageKey::parse("PRIVATE-KEY").unwrap(),
            "source:a",
        )
        .unwrap();
        other_registry.register(alias).unwrap();
        assert!(other_registry.snapshot("other-semantic").await.is_err());
    }

    #[tokio::test]
    async fn orphan_secret_yaml_is_not_adopted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("orphan.yaml"), "TOKEN: secret\n").unwrap();
        let registry = SecretsRegistry::new(dir.path());
        registry.register(test_binding("orphan")).unwrap();
        assert!(registry.snapshot("orphan").await.is_err());
        assert!(registry.has_secrets("orphan").await.is_err());
        assert!(matches!(
            registry.resolve("orphan", "TOKEN").await,
            Err(SecretsError::Io(_))
        ));
    }

    #[tokio::test]
    async fn value_debug_is_redacted() {
        let v = SecretValue::String("hunter2".into());
        assert!(!format!("{v:?}").contains("hunter2"));

        let v = SecretValue::Bytes(b"hunter2".to_vec());
        let debug = format!("{v:?}");
        assert!(!debug.contains("hunter2"));
        assert!(!debug.contains("[104, 117, 110, 116, 101, 114, 50]"));
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    async fn resolving_secret_does_not_log_its_value() {
        const VALUE: &str = "never-log-this-secret-value";
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "comp-log", &[("api_key", VALUE)]).await;
        let registry = SecretsRegistry::new(dir.path());
        registry.register(test_binding("comp-log")).unwrap();

        assert!(matches!(
            registry.resolve("comp-log", "api_key").await,
            Ok(SecretValue::String(value)) if value == VALUE
        ));
        assert!(!logs_contain(VALUE));
    }
}
