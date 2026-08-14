//! Per-component secret store: host-side `wasmcloud:secrets@0.1.0-draft`
//! backend.
//!
//! Every component that imports `wasmcloud:secrets` transparently gets
//! its own private, persistent secret store, indexed by the component's
//! identity. A `store.get(key)` resolves against *that component's*
//! keyring namespace only, so a component can never read another
//! component's secrets. There is no config file: the host derives the
//! calling component's identity itself (the currently executing stage's
//! `component_id`), so the isolation is *structural* rather than a
//! declared grant.
//!
//! A component's identity is `namespace:component-name` — a registry
//! component's WIT `namespace:package` (e.g. `yosh:ollama-provider`), or
//! `local:<filename-stem>` for one loaded from a file (e.g.
//! `local:ollama_provider`). This distinguishes components that share a
//! bare name but come from different namespaces.
//!
//! Secrets live in a [`keyring-core`] credential store — an OS keychain
//! by default; see [`keyring_store`]. The mapping is:
//!
//! - keyring `service = "{prefix}:{component_id}"` (i.e.
//!   `"{prefix}:{namespace}:{component-name}"`) is the per-component
//!   store. The `prefix` namespaces this host's entries so they don't
//!   collide with unrelated applications in a shared OS keychain.
//! - keyring `user = key` is an entry within that store.
//!
//! Stored bytes are returned as a UTF-8 `string` when they decode
//! cleanly, otherwise as raw `bytes`. Resolved values never appear in
//! logs, and are cached for the host process lifetime to avoid repeated
//! keychain access (and prompts).
//!
//! The WIT interface is read-only. Populate a component's store with the
//! host's `secret set` / `secret delete` admin subcommands ([`set_secret`]
//! / [`delete_secret`]); `secret check` ([`check_secret`]) reports whether
//! an entry exists without revealing it.
//!
//! [`keyring-core`]: https://docs.rs/keyring-core

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{Context, Result};

/// Default `service` prefix for keyring entries. Namespaces this host's
/// credentials within a shared OS keychain; override with
/// `--keyring-service-prefix`.
pub const DEFAULT_SERVICE_PREFIX: &str = "wasm-acp";

/// Spec-aligned error type. Mirrors `wasmcloud:secrets/store.secrets-error`.
#[derive(Debug)]
pub enum SecretsError {
    /// The backing store rejected the request (ambiguous match, bad
    /// encoding, unsupported operation, …).
    Upstream(String),
    /// I/O failure talking to the store (no default store, no access,
    /// platform failure, task join, …).
    Io(String),
    /// No such secret in this component's store.
    NotFound,
}

/// Spec-aligned value type. Mirrors `wasmcloud:secrets/store.secret-value`.
/// `Debug` is redacted so it never leaks via logs.
#[derive(Clone)]
pub enum SecretValue {
    String(String),
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

/// The keyring `service` that backs `component_id`'s private store.
fn service_name(prefix: &str, component_id: &str) -> String {
    format!("{prefix}:{component_id}")
}

/// Resolves `wasmcloud:secrets` lookups against per-component namespaces
/// in the process-global `keyring-core` default store.
///
/// Each component id maps to keyring `service = "{prefix}:{component_id}"`;
/// individual keys are `user`s within it. Resolved values are cached for
/// the process lifetime.
pub struct SecretsRegistry {
    prefix: String,
    cache: Mutex<HashMap<(String, String), SecretValue>>,
}

impl SecretsRegistry {
    /// Build a resolver whose entries live under
    /// `service = "{prefix}:{component_id}"`.
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            cache: Mutex::new(HashMap::new()),
        }
    }

    fn service_for(&self, component_id: &str) -> String {
        service_name(&self.prefix, component_id)
    }

    fn cache_get(&self, component_id: &str, key: &str) -> Option<SecretValue> {
        self.cache
            .lock()
            .ok()?
            .get(&(component_id.to_string(), key.to_string()))
            .cloned()
    }

    fn cache_put(&self, component_id: &str, key: &str, value: SecretValue) {
        if let Ok(mut g) = self.cache.lock() {
            g.insert((component_id.to_string(), key.to_string()), value);
        }
    }

    /// Resolve `key` from `component_id`'s private store. Returns
    /// [`SecretsError::NotFound`] when the component has no such entry.
    pub async fn resolve(
        &self,
        component_id: &str,
        key: &str,
    ) -> Result<SecretValue, SecretsError> {
        if let Some(v) = self.cache_get(component_id, key) {
            return Ok(v);
        }
        // keyring-core is synchronous and may block on IPC to the OS
        // keychain (and can surface a user prompt), so run it off the
        // async executor.
        let service = self.service_for(component_id);
        let key_owned = key.to_string();
        let value = tokio::task::spawn_blocking(move || fetch_keyring(&service, &key_owned))
            .await
            .map_err(|e| SecretsError::Io(format!("keyring task join: {e}")))??;
        self.cache_put(component_id, key, value.clone());
        Ok(value)
    }
}

/// Read `key` from `service` in the process-global `keyring-core` default
/// store. Blocking; call from a blocking context (see
/// [`SecretsRegistry::resolve`]). Bytes that decode as UTF-8 become a
/// `String`; otherwise they are returned as raw `Bytes`.
fn fetch_keyring(service: &str, key: &str) -> Result<SecretValue, SecretsError> {
    let entry = keyring_core::Entry::new(service, key).map_err(map_keyring_error)?;
    let bytes = entry.get_secret().map_err(map_keyring_error)?;
    Ok(match String::from_utf8(bytes) {
        Ok(s) => SecretValue::String(s),
        Err(e) => SecretValue::Bytes(e.into_bytes()),
    })
}

/// Map a `keyring-core` error onto the spec-aligned [`SecretsError`].
/// `Display` on these variants never includes the secret bytes, so it is
/// safe to embed in the error message.
fn map_keyring_error(e: keyring_core::Error) -> SecretsError {
    use keyring_core::Error as E;
    match e {
        // A missing credential is the deny-by-default `not-found`.
        E::NoEntry => SecretsError::NotFound,
        // Store misconfiguration / access problems are host-side I/O.
        E::NoDefaultStore => SecretsError::Io("keyring default store not initialized".to_string()),
        E::NoStorageAccess(err) => SecretsError::Io(format!("keyring storage access: {err}")),
        E::PlatformFailure(err) => SecretsError::Io(format!("keyring platform failure: {err}")),
        // Everything else (ambiguous match, bad encoding, invalid input,
        // unsupported op, and future non-exhaustive variants) is upstream.
        other => SecretsError::Upstream(format!("keyring: {other}")),
    }
}

/// Store `value` as `key` in `component_id`'s keyring namespace
/// (`service = "{prefix}:{component_id}"`). Blocking; used by the
/// `secret set` admin subcommand.
pub fn set_secret(prefix: &str, component_id: &str, key: &str, value: &SecretValue) -> Result<()> {
    let service = service_name(prefix, component_id);
    let entry = keyring_core::Entry::new(&service, key)
        .with_context(|| format!("opening keyring entry {service}/{key}"))?;
    match value {
        SecretValue::String(s) => entry.set_password(s),
        SecretValue::Bytes(b) => entry.set_secret(b),
    }
    .with_context(|| format!("writing keyring entry {service}/{key}"))?;
    Ok(())
}

/// Delete `key` from `component_id`'s keyring namespace. Idempotent: a
/// missing entry is treated as success. Blocking; used by the
/// `secret delete` admin subcommand.
pub fn delete_secret(prefix: &str, component_id: &str, key: &str) -> Result<()> {
    let service = service_name(prefix, component_id);
    let entry = keyring_core::Entry::new(&service, key)
        .with_context(|| format!("opening keyring entry {service}/{key}"))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
        Err(e) => Err(anyhow::anyhow!("deleting keyring entry {service}/{key}: {e}")),
    }
}

/// Report whether `component_id` has a secret stored under `key`,
/// *without revealing its value*. Probes existence with
/// [`keyring_core::Entry::get_credential`], which resolves the credential
/// but does not decrypt the secret material — so a `secret check` never
/// reads (or risks prompting for) the secret it is only asking about. A
/// missing entry yields `Ok(false)`; store/access failures propagate as
/// `Err`. Blocking; used by the `secret check` admin subcommand.
pub fn check_secret(prefix: &str, component_id: &str, key: &str) -> Result<bool> {
    let service = service_name(prefix, component_id);
    let entry = keyring_core::Entry::new(&service, key)
        .with_context(|| format!("opening keyring entry {service}/{key}"))?;
    match entry.get_credential() {
        Ok(_) => Ok(true),
        Err(keyring_core::Error::NoEntry) => Ok(false),
        Err(e) => Err(anyhow::anyhow!("checking keyring entry {service}/{key}: {e}")),
    }
}

/// Selection and initialization of the process-global `keyring-core`
/// default store that backs all secret lookups.
///
/// `keyring-core` keeps a single global default store; entries created
/// with `Entry::new` resolve against it. The host sets it once at
/// startup based on `--keyring-store` (see [`Backend`]).
pub mod keyring_store {
    use anyhow::Result;

    /// Which `keyring-core` credential store backs secret lookups.
    #[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
    pub enum Backend {
        /// The platform's native OS credential store (macOS Keychain,
        /// Windows Credential Manager, or the Linux Secret Service).
        Native,
        /// An in-memory store with no persistence. For tests and CI: a
        /// fresh process starts empty.
        Mock,
    }

    /// Install the chosen store as `keyring-core`'s process-global
    /// default. Call once, before resolving or provisioning any secret.
    pub fn init_default_store(backend: Backend) -> Result<()> {
        let store: std::sync::Arc<keyring_core::CredentialStore> = match backend {
            Backend::Mock => keyring_core::mock::Store::new()
                .map_err(|e| anyhow::anyhow!("building mock keyring store: {e}"))?,
            Backend::Native => native_store()?,
        };
        keyring_core::set_default_store(store);
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn native_store() -> Result<std::sync::Arc<keyring_core::CredentialStore>> {
        // The User (login) keychain — the general-purpose store a CLI can
        // reach without a provisioning profile.
        let store: std::sync::Arc<keyring_core::CredentialStore> =
            apple_native_keyring_store::keychain::Store::new()
                .map_err(|e| anyhow::anyhow!("building macOS keychain store: {e}"))?;
        Ok(store)
    }

    #[cfg(all(target_os = "linux", feature = "__never"))]
    fn native_store() -> Result<std::sync::Arc<keyring_core::CredentialStore>> {
        let store: std::sync::Arc<keyring_core::CredentialStore> =
            dbus_secret_service_keyring_store::Store::new()
                .map_err(|e| anyhow::anyhow!("building Secret Service keyring store: {e}"))?;
        Ok(store)
    }

    #[cfg(target_os = "windows")]
    fn native_store() -> Result<std::sync::Arc<keyring_core::CredentialStore>> {
        let store: std::sync::Arc<keyring_core::CredentialStore> =
            windows_native_keyring_store::Store::new()
                .map_err(|e| anyhow::anyhow!("building Windows credential store: {e}"))?;
        Ok(store)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn native_store() -> Result<std::sync::Arc<keyring_core::CredentialStore>> {
        anyhow::bail!(
            "no native keyring store is available for this target OS; \
             pass `--keyring-store mock`"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;

    // `set_default_store` is process-global, so every test in this binary
    // shares one in-memory mock store. Tests use unique component ids to
    // stay isolated from one another.
    static MOCK_STORE: Once = Once::new();
    fn ensure_mock_store() {
        MOCK_STORE.call_once(|| {
            keyring_core::set_default_store(keyring_core::mock::Store::new().unwrap());
        });
    }

    const PREFIX: &str = "test-acp";

    #[tokio::test]
    async fn missing_secret_is_not_found() {
        ensure_mock_store();
        let r = SecretsRegistry::new(PREFIX);
        assert!(matches!(
            r.resolve("missing_comp", "nope").await,
            Err(SecretsError::NotFound)
        ));
    }

    #[tokio::test]
    async fn string_secret_round_trips() {
        ensure_mock_store();
        set_secret(PREFIX, "comp_str", "api_key", &SecretValue::String("hunter2".into())).unwrap();
        let r = SecretsRegistry::new(PREFIX);
        match r.resolve("comp_str", "api_key").await.unwrap() {
            SecretValue::String(s) => assert_eq!(s, "hunter2"),
            other => panic!("expected string, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn non_utf8_secret_is_returned_as_bytes() {
        ensure_mock_store();
        let raw = vec![0xff, 0x00, 0xfe];
        set_secret(PREFIX, "comp_bytes", "blob", &SecretValue::Bytes(raw.clone())).unwrap();
        let r = SecretsRegistry::new(PREFIX);
        match r.resolve("comp_bytes", "blob").await.unwrap() {
            SecretValue::Bytes(b) => assert_eq!(b, raw),
            other => panic!("expected bytes, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn utf8_bytes_secret_is_returned_as_string() {
        ensure_mock_store();
        // Written via the bytes path but valid UTF-8: read back as string.
        set_secret(PREFIX, "comp_utf8", "k", &SecretValue::Bytes(b"hello".to_vec())).unwrap();
        let r = SecretsRegistry::new(PREFIX);
        match r.resolve("comp_utf8", "k").await.unwrap() {
            SecretValue::String(s) => assert_eq!(s, "hello"),
            other => panic!("expected string, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn per_component_isolation() {
        ensure_mock_store();
        // Two components share the bare name `app` but differ by
        // namespace; the identity is `namespace:component-name`, so a
        // secret under one must be invisible to the other.
        set_secret(PREFIX, "local:app", "shared", &SecretValue::String("owned".into())).unwrap();
        let r = SecretsRegistry::new(PREFIX);
        match r.resolve("local:app", "shared").await.unwrap() {
            SecretValue::String(s) => assert_eq!(s, "owned"),
            other => panic!("expected string, got {other:?}"),
        }
        assert!(matches!(
            r.resolve("yosh:app", "shared").await,
            Err(SecretsError::NotFound)
        ));
    }

    #[tokio::test]
    async fn resolution_is_cached() {
        ensure_mock_store();
        set_secret(PREFIX, "comp_cache", "k", &SecretValue::String("v1".into())).unwrap();
        let r = SecretsRegistry::new(PREFIX);
        match r.resolve("comp_cache", "k").await.unwrap() {
            SecretValue::String(s) => assert_eq!(s, "v1"),
            other => panic!("expected string, got {other:?}"),
        }
        // Mutate the underlying store; the cached value must win.
        set_secret(PREFIX, "comp_cache", "k", &SecretValue::String("v2".into())).unwrap();
        match r.resolve("comp_cache", "k").await.unwrap() {
            SecretValue::String(s) => assert_eq!(s, "v1", "expected cached value"),
            other => panic!("expected string, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn service_uses_prefix_and_component_id() {
        ensure_mock_store();
        // Seed by writing the raw keyring entry the resolver should target,
        // proving the `{prefix}:{namespace}:{component-name}` service /
        // `user = key` mapping for a namespaced identity.
        let service = service_name(PREFIX, "yosh:tablemark");
        assert_eq!(service, "test-acp:yosh:tablemark");
        keyring_core::Entry::new(&service, "tok")
            .unwrap()
            .set_password("mapped")
            .unwrap();
        let r = SecretsRegistry::new(PREFIX);
        match r.resolve("yosh:tablemark", "tok").await.unwrap() {
            SecretValue::String(s) => assert_eq!(s, "mapped"),
            other => panic!("expected string, got {other:?}"),
        }
    }

    #[test]
    fn delete_removes_secret() {
        ensure_mock_store();
        set_secret(PREFIX, "comp_del", "k", &SecretValue::String("bye".into())).unwrap();
        let service = service_name(PREFIX, "comp_del");
        assert!(keyring_core::Entry::new(&service, "k").unwrap().get_secret().is_ok());
        delete_secret(PREFIX, "comp_del", "k").unwrap();
        assert!(matches!(
            keyring_core::Entry::new(&service, "k").unwrap().get_secret(),
            Err(keyring_core::Error::NoEntry)
        ));
    }

    #[test]
    fn delete_missing_is_ok() {
        ensure_mock_store();
        // Idempotent: deleting a never-set entry succeeds.
        delete_secret(PREFIX, "comp_del_missing", "nope").unwrap();
    }

    #[test]
    fn check_reports_presence_without_revealing() {
        ensure_mock_store();
        // Absent → false.
        assert!(!check_secret(PREFIX, "comp_check", "k").unwrap());
        // Present → true.
        set_secret(PREFIX, "comp_check", "k", &SecretValue::String("hunter2".into())).unwrap();
        assert!(check_secret(PREFIX, "comp_check", "k").unwrap());
        // Deleted → false again.
        delete_secret(PREFIX, "comp_check", "k").unwrap();
        assert!(!check_secret(PREFIX, "comp_check", "k").unwrap());
    }
}
