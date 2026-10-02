// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Secret management for Wassette components
//!
//! This module provides functionality to manage per-component secrets that are:
//! - Stored in OS-appropriate directories with proper permissions
//! - Persisted across runs without requiring server restart
//! - Easy to edit and audit via CLI
//! - Integrated with component environment variable system
//!
//! Bound APIs associate an admitted semantic name, its original private storage
//! key, and a stable source identity. The configured secrets directory owns these
//! reservations independently of any component store. Deleting values never
//! releases a reservation, and existing unowned YAML is never adopted.
//!
//! The raw APIs are compatibility helpers for private storage keys, not semantic
//! component names. They cannot access a namespace reserved by a bound API.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::identity::{legacy_secret_stem as sanitize_component_id, ComponentId, StorageKey};

const BINDINGS_FILE: &str = ".secret-bindings.json";
const BINDINGS_LOCK: &str = ".secret-bindings.lock";

/// Immutable ownership of a private secret namespace.
///
/// The caller must supply the admitted component identity and a stable source
/// identity from its receipt adapter. A root component name alone does not
/// authenticate the source, and a version/content digest is not a stable source
/// identity if it changes on upgrade.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SecretBindingData")]
pub struct SecretBinding {
    component_name: String,
    storage_key: String,
    legacy_secret_collision_key: String,
    source_identity: String,
}

impl SecretBinding {
    /// Bind an admitted exact component name to its original key and source.
    pub fn new(
        component_id: &ComponentId,
        storage_key: &StorageKey,
        source_identity: impl Into<String>,
    ) -> Result<Self> {
        let source_identity = source_identity.into();
        if source_identity.trim().is_empty() {
            bail!("A secret binding requires a nonempty stable source identity");
        }
        Ok(Self {
            component_name: component_id.as_str().to_owned(),
            storage_key: storage_key.as_str().to_owned(),
            legacy_secret_collision_key: storage_key.collision_keys().legacy_secrets,
            source_identity,
        })
    }

    /// The exact admitted semantic name, without filename sanitization.
    pub fn component_name(&self) -> &str {
        &self.component_name
    }

    /// The original private storage key, not the semantic component name.
    pub fn storage_key(&self) -> &str {
        &self.storage_key
    }

    /// The portable collision key of the unchanged legacy YAML filename.
    pub fn legacy_secret_collision_key(&self) -> &str {
        &self.legacy_secret_collision_key
    }

    /// The stable source identity supplied by the receipt adapter.
    pub fn source_identity(&self) -> &str {
        &self.source_identity
    }
}

impl fmt::Debug for SecretBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretBinding")
            .field("component_name", &self.component_name)
            .field("storage_key", &self.storage_key)
            .field(
                "legacy_secret_collision_key",
                &self.legacy_secret_collision_key,
            )
            .field("source_identity", &"[redacted]")
            .finish()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretBindingData {
    component_name: String,
    storage_key: String,
    legacy_secret_collision_key: String,
    source_identity: String,
}

impl TryFrom<SecretBindingData> for SecretBinding {
    type Error = anyhow::Error;

    fn try_from(data: SecretBindingData) -> Result<Self> {
        let binding = Self::new(
            &ComponentId::from_name(&data.component_name)?,
            &StorageKey::parse(&data.storage_key)?,
            data.source_identity,
        )?;
        if binding.legacy_secret_collision_key != data.legacy_secret_collision_key {
            bail!("Secret binding contains an inconsistent private collision key");
        }
        Ok(binding)
    }
}

/// Secrets manager for components
///
/// Reads check current on-disk ownership and values while holding the namespace
/// lock; modification timestamps are not an ownership or freshness guarantee.
#[derive(Debug)]
pub struct SecretsManager {
    /// Directory where secrets are stored
    secrets_dir: PathBuf,
}

impl SecretsManager {
    /// Create a new secrets manager
    pub fn new(secrets_dir: PathBuf) -> Self {
        Self { secrets_dir }
    }

    /// Get the secrets directory path
    pub fn secrets_dir(&self) -> &Path {
        &self.secrets_dir
    }

    /// Return the legacy YAML path for a private storage key.
    ///
    /// This low-level path helper does not establish ownership. Semantic names
    /// must instead be admitted and used through a [`SecretBinding`].
    pub fn get_component_secrets_path(&self, private_storage_key: &str) -> PathBuf {
        secret_path(&self.secrets_dir, private_storage_key)
    }

    /// Ensure the secrets directory exists with proper permissions
    pub async fn ensure_secrets_dir(&self) -> Result<()> {
        let directory = self.secrets_dir.clone();
        tokio::task::spawn_blocking(move || ensure_directory(&directory)).await?
    }

    /// Check ownership without claiming an empty namespace.
    ///
    /// An absent namespace is permitted; unowned YAML and conflicting or broken
    /// ownership metadata are errors. Reads of an unused directory create no
    /// state, so checking a read-only installation is safe.
    pub async fn check_binding(&self, binding: &SecretBinding) -> Result<()> {
        let directory = self.secrets_dir.clone();
        let binding = binding.clone();
        tokio::task::spawn_blocking(move || match Namespace::read(&directory)? {
            Some(namespace) => namespace.check_bound(&directory, &binding).map(|_| ()),
            None => reject_orphan(&directory, &binding.legacy_secret_collision_key),
        })
        .await?
    }

    /// Load secrets only for the exact admitted component, key, and source.
    pub async fn load_bound_component_secrets(
        &self,
        binding: &SecretBinding,
    ) -> Result<HashMap<String, String>> {
        let directory = self.secrets_dir.clone();
        let binding = binding.clone();
        tokio::task::spawn_blocking(move || {
            let Some(namespace) = Namespace::read(&directory)? else {
                reject_orphan(&directory, &binding.legacy_secret_collision_key)?;
                return Ok(HashMap::new());
            };
            namespace.check_bound(&directory, &binding)?;
            read_secrets(&secret_path(&directory, &binding.storage_key))
                .map(Option::unwrap_or_default)
        })
        .await?
    }

    /// List bound secret keys, optionally returning their unchanged values.
    pub async fn list_bound_component_secrets(
        &self,
        binding: &SecretBinding,
        show_values: bool,
    ) -> Result<HashMap<String, Option<String>>> {
        Ok(list_secrets(
            self.load_bound_component_secrets(binding).await?,
            show_values,
        ))
    }

    /// Durably reserve ownership before merging any bound secret values.
    ///
    /// A failed value write can leave a reservation. The same binding may retry;
    /// no other identity can take over this private namespace.
    pub async fn set_bound_component_secrets(
        &self,
        binding: &SecretBinding,
        secrets: &[(String, String)],
    ) -> Result<()> {
        let directory = self.secrets_dir.clone();
        let binding = binding.clone();
        let secrets = secrets.to_vec();
        tokio::task::spawn_blocking(move || {
            let mut namespace = Namespace::write(&directory)?;
            if !namespace.check_bound(&directory, &binding)? {
                namespace
                    .owners
                    .insert(binding.legacy_secret_collision_key.clone(), binding.clone());
                namespace.persist(&directory)?;
            }
            set_secrets(&directory, &binding.storage_key, &secrets)
        })
        .await?
    }

    /// Delete bound values without releasing the permanent owner reservation.
    pub async fn delete_bound_component_secrets(
        &self,
        binding: &SecretBinding,
        keys: &[String],
    ) -> Result<()> {
        let directory = self.secrets_dir.clone();
        let binding = binding.clone();
        let keys = keys.to_vec();
        tokio::task::spawn_blocking(move || {
            let namespace = Namespace::write(&directory)?;
            namespace.check_bound(&directory, &binding)?;
            delete_secrets(&directory, &binding.storage_key, &keys)
        })
        .await?
    }

    /// Load legacy secrets by private storage key, rejecting reserved slots.
    ///
    /// This compatibility API must not receive semantic component names.
    pub async fn load_component_secrets(
        &self,
        private_storage_key: &str,
    ) -> Result<HashMap<String, String>> {
        let directory = self.secrets_dir.clone();
        let private_storage_key = private_storage_key.to_owned();
        tokio::task::spawn_blocking(move || {
            let path = secret_path(&directory, &private_storage_key);
            let namespace = match Namespace::read(&directory)? {
                Some(namespace) => namespace,
                None if regular_file_exists(&path)? => Namespace::legacy_read(&directory)?,
                None => return Ok(HashMap::new()),
            };
            namespace.check_raw(&private_storage_key)?;
            read_secrets(&path).map(Option::unwrap_or_default)
        })
        .await?
    }

    /// List legacy secrets by private storage key, rejecting reserved slots.
    pub async fn list_component_secrets(
        &self,
        private_storage_key: &str,
        show_values: bool,
    ) -> Result<HashMap<String, Option<String>>> {
        Ok(list_secrets(
            self.load_component_secrets(private_storage_key).await?,
            show_values,
        ))
    }

    /// Merge legacy secrets by private storage key, rejecting reserved slots.
    ///
    /// Read-modify-write is locked across all managers sharing this directory.
    pub async fn set_component_secrets(
        &self,
        private_storage_key: &str,
        secrets: &[(String, String)],
    ) -> Result<()> {
        let directory = self.secrets_dir.clone();
        let private_storage_key = private_storage_key.to_owned();
        let secrets = secrets.to_vec();
        tokio::task::spawn_blocking(move || {
            let namespace = Namespace::write(&directory)?;
            namespace.check_raw(&private_storage_key)?;
            set_secrets(&directory, &private_storage_key, &secrets)
        })
        .await?
    }

    /// Delete legacy secrets by private storage key, rejecting reserved slots.
    pub async fn delete_component_secrets(
        &self,
        private_storage_key: &str,
        keys: &[String],
    ) -> Result<()> {
        let directory = self.secrets_dir.clone();
        let private_storage_key = private_storage_key.to_owned();
        let keys = keys.to_vec();
        tokio::task::spawn_blocking(move || {
            let namespace = Namespace::write(&directory)?;
            namespace.check_raw(&private_storage_key)?;
            delete_secrets(&directory, &private_storage_key, &keys)
        })
        .await?
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NamespaceData {
    version: u32,
    bindings: Vec<SecretBinding>,
}

struct Namespace {
    // The stable lock inode is never replaced or removed, including on deletion.
    _lock: File,
    owners: BTreeMap<String, SecretBinding>,
}

impl Namespace {
    fn read(directory: &Path) -> Result<Option<Self>> {
        if !directory_exists(directory)? {
            return Ok(None);
        }
        let lock_path = directory.join(BINDINGS_LOCK);
        if !regular_file_exists(&lock_path)? {
            reject_missing_lock(directory)?;
            if !regular_file_exists(&lock_path)? {
                return Ok(None);
            }
        }
        let lock = File::open(&lock_path).context("Failed to open secret namespace lock")?;
        lock.lock_shared()
            .context("Failed to lock secret namespace for reading")?;
        Self::load(directory, lock).map(Some)
    }

    fn legacy_read(directory: &Path) -> Result<Self> {
        let lock = open_lock(directory)?;
        lock.lock_shared()
            .context("Failed to lock legacy secret namespace for reading")?;
        Self::load(directory, lock)
    }

    fn write(directory: &Path) -> Result<Self> {
        ensure_directory(directory)?;
        let lock = open_lock(directory)?;
        lock.lock()
            .context("Failed to lock secret namespace for writing")?;
        Self::load(directory, lock)
    }

    fn load(directory: &Path, lock: File) -> Result<Self> {
        let path = directory.join(BINDINGS_FILE);
        let mut owners = BTreeMap::new();
        if regular_file_exists(&path)? {
            let content = fs::read(&path).context("Failed to read secret namespace ownership")?;
            let data: NamespaceData = serde_json::from_slice(&content)
                .map_err(|_| anyhow!("Malformed secret namespace ownership metadata"))?;
            if data.version != 1 {
                bail!("Unsupported secret namespace ownership version");
            }
            let mut names = HashSet::new();
            for binding in data.bindings {
                if !names.insert(binding.component_name.clone())
                    || owners
                        .insert(binding.legacy_secret_collision_key.clone(), binding)
                        .is_some()
                {
                    bail!("Conflicting duplicate secret namespace ownership");
                }
            }
        }
        Ok(Self {
            _lock: lock,
            owners,
        })
    }

    fn check_bound(&self, directory: &Path, binding: &SecretBinding) -> Result<bool> {
        // A separate component store must not authenticate a reused name merely
        // by selecting another filename in this shared secrets directory.
        if self
            .owners
            .values()
            .any(|owner| owner.component_name == binding.component_name && owner != binding)
        {
            bail!("Secret binding conflict: component name is reserved for another source or original private storage key");
        }
        match self.owners.get(&binding.legacy_secret_collision_key) {
            Some(owner) if owner == binding => Ok(true),
            Some(_) => {
                bail!("Secret binding conflict: private namespace is reserved for another component, source, or original storage spelling")
            }
            None => {
                reject_orphan(directory, &binding.legacy_secret_collision_key)?;
                Ok(false)
            }
        }
    }

    fn check_raw(&self, private_storage_key: &str) -> Result<()> {
        let collision_key = sanitize_component_id(private_storage_key).to_ascii_lowercase();
        if self.owners.contains_key(&collision_key) {
            bail!("Secret namespace is reserved; use its admitted component/source binding instead of a raw private storage key");
        }
        Ok(())
    }

    fn persist(&self, directory: &Path) -> Result<()> {
        let data = NamespaceData {
            version: 1,
            bindings: self.owners.values().cloned().collect(),
        };
        let content =
            serde_json::to_vec(&data).context("Failed to serialize secret namespace ownership")?;
        write_atomic(directory, &directory.join(BINDINGS_FILE), &content)
    }
}

fn reject_missing_lock(directory: &Path) -> Result<()> {
    if regular_file_exists(&directory.join(BINDINGS_FILE))?
        && !regular_file_exists(&directory.join(BINDINGS_LOCK))?
    {
        bail!("Broken secret namespace ownership: stable namespace lock is missing");
    }
    Ok(())
}

fn open_lock(directory: &Path) -> Result<File> {
    let path = directory.join(BINDINGS_LOCK);
    if !regular_file_exists(&path)? {
        reject_missing_lock(directory)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    options.mode(0o600);
    let lock = options
        .open(&path)
        .context("Failed to open secret namespace lock")?;
    #[cfg(unix)]
    lock.set_permissions(fs::Permissions::from_mode(0o600))
        .context("Failed to protect secret namespace lock")?;
    Ok(lock)
}

fn ensure_directory(directory: &Path) -> Result<()> {
    fs::create_dir_all(directory).with_context(|| {
        format!(
            "Failed to create secrets directory: {}",
            directory.display()
        )
    })?;
    #[cfg(unix)]
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
        .context("Failed to protect secrets directory")?;
    Ok(())
}

fn directory_exists(directory: &Path) -> Result<bool> {
    match fs::symlink_metadata(directory) {
        Ok(_) => {
            if !fs::metadata(directory)
                .context("Failed to inspect secrets directory")?
                .is_dir()
            {
                bail!("Configured secrets path is not a directory");
            }
            Ok(true)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("Failed to inspect secrets directory"),
    }
}

fn regular_file_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => bail!(
            "Expected a regular secret namespace file: {}",
            path.display()
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| {
            format!(
                "Failed to inspect secret namespace file: {}",
                path.display()
            )
        }),
    }
}

fn reject_orphan(directory: &Path, collision_key: &str) -> Result<()> {
    if !directory_exists(directory)? {
        return Ok(());
    }
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("Failed to inspect unowned secret namespace"),
    };
    for entry in entries {
        let filename = entry
            .context("Failed to inspect unowned secret namespace entry")?
            .file_name();
        if filename
            .to_str()
            .and_then(|name| {
                name.to_ascii_lowercase()
                    .strip_suffix(".yaml")
                    .map(str::to_owned)
            })
            .is_some_and(|stem| stem == collision_key)
        {
            bail!("Protected secret namespace has unknown ownership: existing orphan YAML cannot be adopted or migrated automatically");
        }
    }
    Ok(())
}

fn secret_path(directory: &Path, private_storage_key: &str) -> PathBuf {
    directory.join(format!(
        "{}.yaml",
        sanitize_component_id(private_storage_key)
    ))
}

fn read_secrets(path: &Path) -> Result<Option<HashMap<String, String>>> {
    if !regular_file_exists(path)? {
        return Ok(None);
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read secrets file: {}", path.display()))?;
    // YAML diagnostics can include a scalar's contents, which may be a secret.
    serde_yaml::from_str(&content).map(Some).map_err(|_| {
        anyhow!(
            "Failed to parse secrets file (expected string keys and values): {}",
            path.display()
        )
    })
}

fn list_secrets(
    secrets: HashMap<String, String>,
    show_values: bool,
) -> HashMap<String, Option<String>> {
    secrets
        .into_iter()
        .map(|(key, value)| (key, show_values.then_some(value)))
        .collect()
}

fn set_secrets(
    directory: &Path,
    private_storage_key: &str,
    values: &[(String, String)],
) -> Result<()> {
    let path = secret_path(directory, private_storage_key);
    let mut secrets = read_secrets(&path)?.unwrap_or_default();
    secrets.extend(values.iter().cloned());
    write_secrets(directory, &path, &secrets)
}

fn delete_secrets(directory: &Path, private_storage_key: &str, keys: &[String]) -> Result<()> {
    let path = secret_path(directory, private_storage_key);
    let mut secrets = read_secrets(&path)?
        .ok_or_else(|| anyhow!("No secrets file found for private storage key"))?;
    for key in keys {
        secrets.remove(key);
    }
    if secrets.is_empty() {
        fs::remove_file(&path).context("Failed to remove empty secrets file")?;
        sync_directory(directory)
    } else {
        write_secrets(directory, &path, &secrets)
    }
}

fn write_secrets(directory: &Path, path: &Path, secrets: &HashMap<String, String>) -> Result<()> {
    let content = serde_yaml::to_string(secrets)
        .map_err(|_| anyhow!("Failed to serialize secrets to YAML"))?;
    write_atomic(directory, path, content.as_bytes())
}

fn write_atomic(directory: &Path, path: &Path, content: &[u8]) -> Result<()> {
    let mut file = tempfile::Builder::new()
        .prefix(".secret-write-")
        .tempfile_in(directory)
        .context("Failed to create unique secret namespace staging file")?;
    #[cfg(unix)]
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .context("Failed to protect secret namespace staging file")?;
    file.write_all(content)
        .context("Failed to write secret namespace staging file")?;
    file.as_file()
        .sync_all()
        .context("Failed to sync secret namespace staging file")?;
    file.persist(path)
        .map_err(|error| error.error)
        .context("Failed to atomically replace secret namespace file")?;
    sync_directory(directory)
}

fn sync_directory(directory: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(directory)
        .and_then(|directory| directory.sync_all())
        .context("Failed to sync secrets directory")?;
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tempfile::TempDir;

    use super::*;

    fn binding(name: &str, key: &str, source: &str) -> Result<SecretBinding> {
        SecretBinding::new(
            &ComponentId::from_name(name)?,
            &StorageKey::parse(key)?,
            source,
        )
    }

    #[test]
    fn binding_preserves_exact_names_keys_and_source() -> Result<()> {
        let binding = binding(" météo/東京 ", "Weather__Private", "registry:weather")?;
        assert_eq!(binding.component_name(), " météo/東京 ");
        assert_eq!(binding.storage_key(), "Weather__Private");
        assert_eq!(binding.legacy_secret_collision_key(), "weather_private");
        assert_eq!(binding.source_identity(), "registry:weather");
        let json = serde_json::to_string(&binding)?;
        assert_eq!(serde_json::from_str::<SecretBinding>(&json)?, binding);
        Ok(())
    }

    #[test]
    fn binding_rejects_empty_source_and_invalid_serialized_fields() -> Result<()> {
        for source in ["", " ", "\t\n"] {
            assert!(binding("component", "private", source).is_err());
        }
        let good = serde_json::to_value(binding("component", "private", "source")?)?;
        for (field, value) in [
            ("component_name", ""),
            ("component_name", "bad\nname"),
            ("storage_key", "../other"),
            ("storage_key", "NUL"),
            ("legacy_secret_collision_key", "other"),
            ("source_identity", ""),
        ] {
            let mut invalid = good.clone();
            invalid[field] = value.into();
            assert!(serde_json::from_value::<SecretBinding>(invalid).is_err());
        }
        Ok(())
    }

    #[tokio::test]
    async fn bound_upgrade_reuses_exact_identity_and_preserves_raw_values() -> Result<()> {
        let root = TempDir::new_in(".")?;
        let directory = root.path().join("configured-secrets");
        let first_version = SecretsManager::new(directory.clone());
        let identity = binding("météo/東京", "Weather__Private", "registry:weather")?;
        let value = "  raw:\n日本語\u{0}value\n";
        first_version
            .set_bound_component_secrets(&identity, &[("TOKEN".into(), value.into())])
            .await?;
        assert!(directory.join("Weather_Private.yaml").is_file());
        assert!(!directory.join("m_t_o_.yaml").exists());
        assert!(!directory.join("météo").exists());

        let next_version = SecretsManager::new(directory);
        let restored: SecretBinding = serde_json::from_str(&serde_json::to_string(&identity)?)?;
        assert_eq!(
            next_version.load_bound_component_secrets(&restored).await?["TOKEN"],
            value
        );
        assert_eq!(
            next_version
                .list_bound_component_secrets(&restored, false)
                .await?["TOKEN"],
            None
        );
        assert_eq!(
            next_version
                .list_bound_component_secrets(&restored, true)
                .await?["TOKEN"],
            Some(value.to_owned())
        );
        next_version.check_binding(&restored).await?;
        Ok(())
    }

    #[tokio::test]
    async fn shared_directory_rejects_other_sources_and_original_storage_spellings() -> Result<()> {
        let root = TempDir::new_in(".")?;
        let first = SecretsManager::new(root.path().to_owned());
        let second = SecretsManager::new(root.path().to_owned());
        let owner = binding("weather", "private", "source:a")?;
        first
            .set_bound_component_secrets(&owner, &[("TOKEN".into(), "protected".into())])
            .await?;

        for impostor in [
            binding("weather", "private", "source:b")?,
            binding("weather", "different-key", "source:b")?,
            binding("weather", "different-key", "source:a")?,
            binding("weather", "PRIVATE", "source:a")?,
            binding("other-name", "private", "source:a")?,
        ] {
            assert!(second.check_binding(&impostor).await.is_err());
            assert!(second
                .load_bound_component_secrets(&impostor)
                .await
                .is_err());
            assert!(second
                .list_bound_component_secrets(&impostor, true)
                .await
                .is_err());
            assert!(second
                .set_bound_component_secrets(&impostor, &[("TOKEN".into(), "bad".into())])
                .await
                .is_err());
            assert!(second
                .delete_bound_component_secrets(&impostor, &["TOKEN".into()])
                .await
                .is_err());
        }
        assert_eq!(
            first.load_bound_component_secrets(&owner).await?["TOKEN"],
            "protected"
        );
        let distinct = binding("other-name", "different-key", "source:b")?;
        second
            .set_bound_component_secrets(&distinct, &[("TOKEN".into(), "separate".into())])
            .await?;
        assert_eq!(
            second.load_bound_component_secrets(&distinct).await?["TOKEN"],
            "separate"
        );
        Ok(())
    }

    #[tokio::test]
    async fn legacy_projection_aliases_are_reserved_across_managers() -> Result<()> {
        let prefix = "a".repeat(128);
        for (original, alias) in [
            ("a__b".to_owned(), "a_b".to_owned()),
            ("_a".to_owned(), "a".to_owned()),
            ("_".to_owned(), "unnamed".to_owned()),
            ("Weather".to_owned(), "weather".to_owned()),
            (format!("{prefix}x"), format!("{prefix}y")),
        ] {
            let root = TempDir::new_in(".")?;
            let first = SecretsManager::new(root.path().to_owned());
            let second = SecretsManager::new(root.path().to_owned());
            let owner = binding("first", &original, "source:a")?;
            let impostor = binding("second", &alias, "source:b")?;
            first
                .set_bound_component_secrets(&owner, &[("TOKEN".into(), "protected".into())])
                .await?;
            assert!(second
                .load_bound_component_secrets(&impostor)
                .await
                .is_err());
            assert!(second
                .set_bound_component_secrets(&impostor, &[])
                .await
                .is_err());
            assert!(second.load_component_secrets(&alias).await.is_err());
            assert!(second.list_component_secrets(&alias, true).await.is_err());
            assert!(second.set_component_secrets(&alias, &[]).await.is_err());
            assert!(second
                .delete_component_secrets(&alias, &["TOKEN".into()])
                .await
                .is_err());
            assert_eq!(
                first.load_bound_component_secrets(&owner).await?["TOKEN"],
                "protected"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn orphan_values_are_not_adopted_mutated_or_deleted() -> Result<()> {
        let root = TempDir::new_in(".")?;
        let manager = SecretsManager::new(root.path().to_owned());
        let path = root.path().join("Weather.yaml");
        let content = b"TOKEN: preserved-verbatim\n";
        fs::write(&path, content)?;
        let identity = binding("weather", "weather", "source:a")?;
        let error = manager
            .load_bound_component_secrets(&identity)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unknown ownership"));
        assert!(!root.path().join(BINDINGS_LOCK).exists());
        assert!(!root.path().join(BINDINGS_FILE).exists());
        assert!(manager.check_binding(&identity).await.is_err());
        assert!(manager
            .set_bound_component_secrets(&identity, &[("TOKEN".into(), "replacement".into())])
            .await
            .is_err());
        assert!(manager
            .delete_bound_component_secrets(&identity, &["TOKEN".into()])
            .await
            .is_err());
        assert_eq!(fs::read(&path)?, content);
        assert!(!root.path().join(BINDINGS_FILE).exists());
        assert_eq!(
            manager.load_component_secrets("Weather").await?["TOKEN"],
            "preserved-verbatim"
        );
        Ok(())
    }

    #[tokio::test]
    async fn deleting_last_value_permanently_retains_owner() -> Result<()> {
        let root = TempDir::new_in(".")?;
        let owner = binding("weather", "private", "source:a")?;
        let manager = SecretsManager::new(root.path().to_owned());
        manager
            .set_bound_component_secrets(&owner, &[("TOKEN".into(), "original".into())])
            .await?;
        let ownership = fs::read(root.path().join(BINDINGS_FILE))?;
        manager
            .delete_bound_component_secrets(&owner, &["TOKEN".into()])
            .await?;
        assert!(!root.path().join("private.yaml").exists());
        assert!(root.path().join(BINDINGS_LOCK).is_file());
        assert_eq!(fs::read(root.path().join(BINDINGS_FILE))?, ownership);

        let reopened = SecretsManager::new(root.path().to_owned());
        assert!(reopened
            .load_bound_component_secrets(&owner)
            .await?
            .is_empty());
        for impostor in [
            binding("weather", "private", "source:b")?,
            binding("other", "private", "source:b")?,
            binding("weather", "PRIVATE", "source:a")?,
        ] {
            assert!(reopened
                .load_bound_component_secrets(&impostor)
                .await
                .is_err());
            assert!(reopened
                .set_bound_component_secrets(&impostor, &[])
                .await
                .is_err());
        }
        assert!(reopened.load_component_secrets("private").await.is_err());
        assert!(reopened
            .set_component_secrets("private", &[])
            .await
            .is_err());
        assert!(reopened
            .delete_component_secrets("private", &[])
            .await
            .is_err());
        reopened
            .set_bound_component_secrets(&owner, &[("TOKEN".into(), "retry".into())])
            .await?;
        assert_eq!(
            reopened.load_bound_component_secrets(&owner).await?["TOKEN"],
            "retry"
        );
        Ok(())
    }

    #[tokio::test]
    async fn unused_bound_reads_do_not_create_namespace_state() -> Result<()> {
        let root = TempDir::new_in(".")?;
        let directory = root.path().join("not-created");
        let identity = binding("weather", "private", "source:a")?;
        let manager = SecretsManager::new(directory.clone());
        manager.check_binding(&identity).await?;
        assert!(manager
            .load_bound_component_secrets(&identity)
            .await?
            .is_empty());
        assert!(manager
            .list_bound_component_secrets(&identity, true)
            .await?
            .is_empty());
        assert!(manager.load_component_secrets("private").await?.is_empty());
        assert!(!directory.exists());
        fs::create_dir(&directory)?;
        #[cfg(unix)]
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o500))?;
        manager.check_binding(&identity).await?;
        assert!(manager
            .load_bound_component_secrets(&identity)
            .await?
            .is_empty());
        assert_eq!(fs::read_dir(&directory)?.count(), 0);
        #[cfg(unix)]
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }

    #[tokio::test]
    async fn simultaneous_bound_and_raw_sets_preserve_every_key() -> Result<()> {
        let root = TempDir::new_in(".")?;
        let first = Arc::new(SecretsManager::new(root.path().to_owned()));
        let second = Arc::new(SecretsManager::new(root.path().to_owned()));
        let identity = binding("weather", "private", "source:a")?;
        for bound in [false, true] {
            let barrier = Arc::new(tokio::sync::Barrier::new(24));
            let mut tasks = Vec::new();
            for index in 0..24 {
                let manager = if index % 2 == 0 { &first } else { &second }.clone();
                let barrier = barrier.clone();
                let identity = identity.clone();
                tasks.push(tokio::spawn(async move {
                    let values = [(format!("KEY_{index}"), format!("value_{index}"))];
                    barrier.wait().await;
                    if bound {
                        manager
                            .set_bound_component_secrets(&identity, &values)
                            .await
                    } else {
                        manager.set_component_secrets("legacy", &values).await
                    }
                }));
            }
            for task in tasks {
                task.await??;
            }
            let loaded = if bound {
                first.load_bound_component_secrets(&identity).await?
            } else {
                first.load_component_secrets("legacy").await?
            };
            assert_eq!(loaded.len(), 24);
            for index in 0..24 {
                assert_eq!(loaded[&format!("KEY_{index}")], format!("value_{index}"));
            }
        }
        assert!(!fs::read_dir(root.path())?.any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".secret-write-")
        }));
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_claims_preserve_all_independent_owners() -> Result<()> {
        let root = TempDir::new_in(".")?;
        let first = SecretsManager::new(root.path().to_owned());
        let second = SecretsManager::new(root.path().to_owned());
        let first_owner = binding("first", "first-private", "source:a")?;
        let second_owner = binding("second", "second-private", "source:b")?;
        let first_values = [("TOKEN".into(), "one".into())];
        let second_values = [("TOKEN".into(), "two".into())];
        let (left, right) = tokio::join!(
            first.set_bound_component_secrets(&first_owner, &first_values),
            second.set_bound_component_secrets(&second_owner, &second_values),
        );
        left?;
        right?;
        assert_eq!(
            second.load_bound_component_secrets(&first_owner).await?["TOKEN"],
            "one"
        );
        assert_eq!(
            first.load_bound_component_secrets(&second_owner).await?["TOKEN"],
            "two"
        );
        for key in ["first-private", "second-private"] {
            assert!(first.load_component_secrets(key).await.is_err());
        }
        Ok(())
    }

    #[tokio::test]
    async fn cancelling_a_waiting_set_keeps_its_owned_transaction_alive() -> Result<()> {
        let root = TempDir::new_in(".")?;
        let manager = SecretsManager::new(root.path().to_owned());
        let identity = binding("weather", "private", "source:a")?;
        manager
            .set_bound_component_secrets(&identity, &[("BEFORE".into(), "before".into())])
            .await?;

        let directory = root.path().to_owned();
        let (acquired, waiting) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let blocking_owner = tokio::task::spawn_blocking(move || -> Result<()> {
            let _namespace = Namespace::write(&directory)?;
            acquired.send(()).map_err(|_| anyhow!("Reader dropped"))?;
            released.recv()?;
            Ok(())
        });
        waiting.await?;

        let values = vec![("AFTER".into(), "after".into())];
        let mut pending = Box::pin(manager.set_bound_component_secrets(&identity, &values));
        assert!(futures::poll!(&mut pending).is_pending());
        drop(pending);
        drop(values);
        drop(identity);
        drop(manager);
        release.send(())?;
        blocking_owner.await??;

        let reopened = SecretsManager::new(root.path().to_owned());
        let identity = binding("weather", "private", "source:a")?;
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let secrets = reopened.load_bound_component_secrets(&identity).await?;
                if secrets.contains_key("AFTER") {
                    assert_eq!(secrets["BEFORE"], "before");
                    assert_eq!(secrets["AFTER"], "after");
                    return Ok::<(), anyhow::Error>(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await??;
        Ok(())
    }

    #[tokio::test]
    async fn durable_claim_without_values_is_a_fail_closed_reservation() -> Result<()> {
        let root = TempDir::new_in(".")?;
        let identity = binding("weather", "private", "source:a")?;
        let directory = root.path().to_owned();
        let persisted = identity.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut namespace = Namespace::write(&directory)?;
            namespace
                .owners
                .insert(persisted.legacy_secret_collision_key.clone(), persisted);
            namespace.persist(&directory)
        })
        .await??;

        let reopened = SecretsManager::new(root.path().to_owned());
        assert!(reopened
            .load_bound_component_secrets(&identity)
            .await?
            .is_empty());
        assert!(reopened
            .load_bound_component_secrets(&binding("other", "private", "source:b")?)
            .await
            .is_err());
        assert!(reopened.load_component_secrets("private").await.is_err());
        reopened
            .set_bound_component_secrets(&identity, &[("TOKEN".into(), "retry".into())])
            .await?;
        assert_eq!(
            reopened.load_bound_component_secrets(&identity).await?["TOKEN"],
            "retry"
        );
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_owner_metadata_blocks_bound_and_raw_operations() -> Result<()> {
        let root = TempDir::new_in(".")?;
        let manager = SecretsManager::new(root.path().to_owned());
        let identity = binding("weather", "private", "source:a")?;
        manager
            .set_bound_component_secrets(&identity, &[("TOKEN".into(), "protected".into())])
            .await?;
        let valid = fs::read(root.path().join(BINDINGS_FILE))?;
        let mut duplicate: serde_json::Value = serde_json::from_slice(&valid)?;
        let entry = duplicate["bindings"][0].clone();
        duplicate["bindings"].as_array_mut().unwrap().push(entry);
        let mut bad_key: serde_json::Value = serde_json::from_slice(&valid)?;
        bad_key["bindings"][0]["legacy_secret_collision_key"] = "different".into();
        for corrupt in [
            b"not-json".to_vec(),
            b"{}".to_vec(),
            br#"{"version":2,"bindings":[]}"#.to_vec(),
            serde_json::to_vec(&duplicate)?,
            serde_json::to_vec(&bad_key)?,
        ] {
            fs::write(root.path().join(BINDINGS_FILE), corrupt)?;
            assert!(manager.check_binding(&identity).await.is_err());
            assert!(manager
                .load_bound_component_secrets(&identity)
                .await
                .is_err());
            assert!(manager
                .set_bound_component_secrets(&identity, &[])
                .await
                .is_err());
            assert!(manager
                .delete_bound_component_secrets(&identity, &["TOKEN".into()])
                .await
                .is_err());
            assert!(manager.load_component_secrets("private").await.is_err());
            assert!(manager.set_component_secrets("private", &[]).await.is_err());
            assert!(manager
                .delete_component_secrets("private", &[])
                .await
                .is_err());
        }
        fs::write(root.path().join(BINDINGS_FILE), &valid)?;
        fs::remove_file(root.path().join(BINDINGS_LOCK))?;
        assert!(manager
            .load_bound_component_secrets(&identity)
            .await
            .is_err());
        assert!(manager
            .set_bound_component_secrets(&identity, &[])
            .await
            .is_err());
        assert_eq!(fs::read(root.path().join(BINDINGS_FILE))?, valid);
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bound_files_are_private_and_read_only_claims_remain_readable() -> Result<()> {
        let root = TempDir::new_in(".")?;
        let directory = root.path().join("configured");
        let manager = SecretsManager::new(directory.clone());
        let identity = binding("weather", "private", "source:a")?;
        manager
            .set_bound_component_secrets(&identity, &[("TOKEN".into(), "protected".into())])
            .await?;
        assert_eq!(
            fs::metadata(&directory)?.permissions().mode() & 0o777,
            0o700
        );
        for filename in ["private.yaml", BINDINGS_FILE, BINDINGS_LOCK] {
            assert_eq!(
                fs::metadata(directory.join(filename))?.permissions().mode() & 0o777,
                0o600
            );
            fs::set_permissions(directory.join(filename), fs::Permissions::from_mode(0o400))?;
        }
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o500))?;
        assert_eq!(
            manager.load_bound_component_secrets(&identity).await?["TOKEN"],
            "protected"
        );
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn broken_ownership_links_propagate_instead_of_becoming_empty() -> Result<()> {
        use std::os::unix::fs::symlink;

        let root = TempDir::new_in(".")?;
        let manager = SecretsManager::new(root.path().to_owned());
        let identity = binding("weather", "private", "source:a")?;
        for filename in [BINDINGS_FILE, BINDINGS_LOCK] {
            let path = root.path().join(filename);
            symlink("missing-target", &path)?;
            assert!(manager.check_binding(&identity).await.is_err());
            assert!(manager
                .load_bound_component_secrets(&identity)
                .await
                .is_err());
            assert!(manager.load_component_secrets("private").await.is_err());
            assert!(manager
                .set_bound_component_secrets(&identity, &[])
                .await
                .is_err());
            fs::remove_file(path)?;
        }
        let broken_directory = root.path().join("broken-directory");
        symlink("missing-target", &broken_directory)?;
        let broken = SecretsManager::new(broken_directory);
        assert!(broken.check_binding(&identity).await.is_err());
        assert!(broken
            .load_bound_component_secrets(&identity)
            .await
            .is_err());
        Ok(())
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn values_are_not_exposed_in_diagnostics_or_debug_output() -> Result<()> {
        let root = TempDir::new_in(".")?;
        let manager = SecretsManager::new(root.path().to_owned());
        let value = "unique-secret-never-log-this";
        let identity = binding("weather", "private", value)?;
        manager
            .set_bound_component_secrets(&identity, &[("TOKEN".into(), value.into())])
            .await?;
        manager.load_bound_component_secrets(&identity).await?;
        manager
            .delete_bound_component_secrets(&identity, &["MISSING".into()])
            .await?;
        assert!(!format!("{manager:?}").contains(value));
        assert!(!format!("{identity:?}").contains(value));
        fs::write(root.path().join("private.yaml"), format!("- {value}\n"))?;
        for error in [
            manager
                .load_bound_component_secrets(&identity)
                .await
                .unwrap_err(),
            manager
                .set_bound_component_secrets(&identity, &[])
                .await
                .unwrap_err(),
            manager
                .delete_bound_component_secrets(&identity, &["TOKEN".into()])
                .await
                .unwrap_err(),
        ] {
            assert!(!format!("{error:#} {error:?}").contains(value));
        }
        assert!(!logs_contain(value));
        Ok(())
    }

    #[test]
    fn test_sanitize_component_id() {
        assert_eq!(sanitize_component_id("simple"), "simple");
        assert_eq!(sanitize_component_id("with-dashes"), "with-dashes");
        assert_eq!(sanitize_component_id("with.dots"), "with.dots");
        assert_eq!(
            sanitize_component_id("with_underscores"),
            "with_underscores"
        );
        assert_eq!(sanitize_component_id("with/slashes"), "with_slashes");
        assert_eq!(sanitize_component_id("with spaces"), "with_spaces");
        assert_eq!(sanitize_component_id("with///multiple"), "with_multiple");
        assert_eq!(sanitize_component_id("trailing/"), "trailing");
        assert_eq!(sanitize_component_id("/leading"), "leading");
        assert_eq!(sanitize_component_id(""), "unnamed");

        // Test long string truncation
        let long_id = "a".repeat(200);
        let sanitized = sanitize_component_id(&long_id);
        assert!(sanitized.len() <= 128);
        assert!(sanitized.is_char_boundary(sanitized.len()));
    }

    #[tokio::test]
    async fn test_secrets_manager_basic() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let secrets_dir = temp_dir.path().join("secrets");
        let manager = SecretsManager::new(secrets_dir);

        // Test setting secrets
        let secrets = vec![
            ("API_KEY".to_string(), "secret123".to_string()),
            ("REGION".to_string(), "us-west-2".to_string()),
        ];
        manager
            .set_component_secrets("test-component", &secrets)
            .await?;

        // Test loading secrets
        let loaded = manager.load_component_secrets("test-component").await?;
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get("API_KEY"), Some(&"secret123".to_string()));
        assert_eq!(loaded.get("REGION"), Some(&"us-west-2".to_string()));

        // Test listing secrets
        let listed = manager
            .list_component_secrets("test-component", false)
            .await?;
        assert_eq!(listed.len(), 2);
        assert!(listed.contains_key("API_KEY"));
        assert!(listed.contains_key("REGION"));
        assert_eq!(listed.get("API_KEY"), Some(&None));

        let listed_with_values = manager
            .list_component_secrets("test-component", true)
            .await?;
        assert_eq!(
            listed_with_values.get("API_KEY"),
            Some(&Some("secret123".to_string()))
        );

        // Test deleting secrets
        manager
            .delete_component_secrets("test-component", &["API_KEY".to_string()])
            .await?;
        let after_delete = manager.load_component_secrets("test-component").await?;
        assert_eq!(after_delete.len(), 1);
        assert!(!after_delete.contains_key("API_KEY"));
        assert!(after_delete.contains_key("REGION"));

        Ok(())
    }

    #[tokio::test]
    async fn test_cache_invalidation() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let secrets_dir = temp_dir.path().join("secrets");
        let manager = SecretsManager::new(secrets_dir);

        // Set initial secrets
        let secrets = vec![("KEY1".to_string(), "value1".to_string())];
        manager.set_component_secrets("test", &secrets).await?;

        // Load secrets (should populate cache)
        let loaded1 = manager.load_component_secrets("test").await?;
        assert_eq!(loaded1.get("KEY1"), Some(&"value1".to_string()));

        // Modify secrets directly
        let secrets_path = manager.get_component_secrets_path("test");

        // Sleep to ensure mtime changes
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let new_content = "KEY1: value2\nKEY2: value3\n";
        tokio::fs::write(&secrets_path, new_content).await?;

        // Load again (should detect file change and reload)
        let loaded2 = manager.load_component_secrets("test").await?;
        assert_eq!(loaded2.get("KEY1"), Some(&"value2".to_string()));
        assert_eq!(loaded2.get("KEY2"), Some(&"value3".to_string()));

        Ok(())
    }

    #[tokio::test]
    async fn test_secrets_with_environment_precedence() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let secrets_dir = temp_dir.path().join("secrets");
        let manager = SecretsManager::new(secrets_dir);

        // Set secrets
        let secrets = vec![
            ("SECRET_KEY".to_string(), "from_secrets".to_string()),
            ("ONLY_IN_SECRETS".to_string(), "secret_value".to_string()),
        ];
        manager.set_component_secrets("test", &secrets).await?;

        // Test environment precedence using extract_env_vars function
        use policy::PolicyParser;

        use crate::wasistate::extract_env_vars;

        let yaml_content = r#"
version: "1.0"
description: "Test policy"
permissions:
  environment:
    allow:
      - key: "SECRET_KEY"
      - key: "ONLY_IN_SECRETS" 
      - key: "ONLY_IN_ENV"
"#;
        let policy = PolicyParser::parse_str(yaml_content)?;

        // Environment vars (highest precedence)
        let mut env_vars = std::collections::HashMap::new();
        env_vars.insert("SECRET_KEY".to_string(), "from_env".to_string());
        env_vars.insert("ONLY_IN_ENV".to_string(), "env_value".to_string());

        // Load secrets
        let loaded_secrets = manager.load_component_secrets("test").await?;

        // Test precedence
        let result = extract_env_vars(&policy, &env_vars, Some(&loaded_secrets))?;

        // SECRET_KEY should come from env (highest precedence)
        assert_eq!(result.get("SECRET_KEY"), Some(&"from_env".to_string()));

        // ONLY_IN_SECRETS should come from secrets
        assert_eq!(
            result.get("ONLY_IN_SECRETS"),
            Some(&"secret_value".to_string())
        );

        // ONLY_IN_ENV should come from env
        assert_eq!(result.get("ONLY_IN_ENV"), Some(&"env_value".to_string()));

        Ok(())
    }
}
