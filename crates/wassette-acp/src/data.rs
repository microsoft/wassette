// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Receipt-bound ownership of the existing project/private-key `/data` path.
//!
//! Ownership records and permanent cooperating locks live beside project
//! directories, outside guest preopens. An unowned nonempty directory is never
//! adopted. Claims are durable before a directory can be mounted, and remain
//! reserved even if its contents or the directory itself are later removed.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use wassette::{SecretBinding, StorageKey};

const OWNERS_DIRECTORY: &str = ".acp-data-ownership";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Owners {
    schema: u32,
    bindings: BTreeMap<String, SecretBinding>,
}

/// Claim only when the chain's existing data-mount guard permits a preopen.
pub(crate) async fn stage_data_dir(
    project_dir: Option<&Path>,
    storage_key: &StorageKey,
    binding: &SecretBinding,
    mount: bool,
) -> Result<Option<PathBuf>> {
    let Some(project_dir) = project_dir.filter(|_| mount) else {
        return Ok(None);
    };
    let project_dir = project_dir.to_path_buf();
    let storage_key = storage_key.clone();
    let binding = binding.clone();
    tokio::task::spawn_blocking(move || claim(&project_dir, &storage_key, &binding))
        .await?
        .map(Some)
}

fn claim(project_dir: &Path, storage_key: &StorageKey, binding: &SecretBinding) -> Result<PathBuf> {
    ensure!(
        storage_key.as_str() == binding.storage_key(),
        "ACP data binding does not match its private storage key"
    );
    let root = project_dir
        .parent()
        .context("ACP project data has no parent")?;
    let project = project_dir
        .file_name()
        .and_then(|name| name.to_str())
        .context("ACP project data has no valid project key")?;
    ensure!(
        !project.is_empty() && project.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid ACP project data key"
    );
    directory(root)?;
    let owners_dir = root.join(OWNERS_DIRECTORY);
    directory(&owners_dir)?;
    sync_directory(root)?;
    let record_path = owners_dir.join(format!("{project}.json"));
    let lock_path = owners_dir.join(format!("{project}.lock"));
    regular_file_or_absent(&lock_path)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .context("opening ACP data ownership lock")?;
    lock.lock().context("locking ACP data ownership")?;

    let mut owners = match regular_file_or_absent(&record_path)? {
        true => {
            let file = File::open(&record_path)?;
            ensure!(
                file.metadata()?.len() <= 16 * 1024 * 1024,
                "ACP data ownership record is too large"
            );
            let owners: Owners =
                serde_json::from_reader(file).context("invalid ACP data ownership record")?;
            ensure!(owners.schema == 1, "unsupported ACP data ownership schema");
            for (key, owner) in &owners.bindings {
                ensure!(
                    StorageKey::parse(owner.storage_key())?
                        .collision_keys()
                        .artifact
                        == *key,
                    "ACP data ownership record has an invalid private key"
                );
            }
            owners
        }
        false => Owners {
            schema: 1,
            bindings: BTreeMap::new(),
        },
    };
    directory(project_dir)?;
    sync_directory(root)?;
    let alias = storage_key.collision_keys().artifact;
    for entry in fs::read_dir(project_dir)? {
        let name = entry?.file_name();
        if let Some(name) = name.to_str() {
            ensure!(
                !name.eq_ignore_ascii_case(storage_key.as_str()) || name == storage_key.as_str(),
                "ACP data private key conflicts with an existing directory alias"
            );
        }
    }
    let data_dir = project_dir.join(storage_key.as_str());
    let existing_data = directory_or_absent(&data_dir)?;
    match owners.bindings.get(&alias) {
        Some(owner) => ensure!(
            owner == binding,
            "ACP data ownership conflicts for private key `{}`",
            storage_key.as_str()
        ),
        None => {
            ensure!(
                !existing_data || fs::read_dir(&data_dir)?.next().transpose()?.is_none(),
                "unowned nonempty ACP data directory `{}` requires explicit operator recovery",
                data_dir.display()
            );
            owners.bindings.insert(alias, binding.clone());
            persist_owners(&owners_dir, project, &record_path, &owners)?;
        }
    }
    // Recheck durability even on reads, so an unsupported filesystem cannot
    // mount data after a previous claim failed during directory synchronization.
    sync_directory(&owners_dir)?;
    directory(&data_dir)?;
    sync_directory(project_dir)?;
    Ok(data_dir)
}

fn persist_owners(directory: &Path, project: &str, record: &Path, owners: &Owners) -> Result<()> {
    let pending = directory.join(format!("{project}.pending"));
    if regular_file_or_absent(&pending)? {
        // A pre-rename crash cannot have mounted data for this claim: publication
        // and directory sync both precede returning a path to the WASI builder.
        fs::remove_file(&pending)?;
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pending)?;
    serde_json::to_writer(&file, owners).context("writing ACP data ownership")?;
    file.sync_all()?;
    drop(file);
    fs::rename(pending, record).context("publishing ACP data ownership")?;
    sync_directory(directory)
}

fn regular_file_or_absent(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file(),
                "ACP data ownership path is not a regular file: {}",
                path.display()
            );
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn directory_or_absent(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                metadata.is_dir(),
                "ACP data path is not a real directory: {}",
                path.display()
            );
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn directory(path: &Path) -> Result<()> {
    if !directory_or_absent(path)? {
        match fs::create_dir(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                ensure!(directory_or_absent(path)?, "ACP data directory disappeared");
            }
            Err(error) => return Err(error).context("creating ACP data directory"),
        }
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?
        .sync_all()
        .context("synchronizing ACP data directory")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(name: &str, key: &str, source: &str) -> SecretBinding {
        let identity = wassette::ComponentId::from_name(name).unwrap();
        SecretBinding::new(&identity, &StorageKey::parse(key).unwrap(), source).unwrap()
    }

    fn mount(project: &Path, binding: &SecretBinding) -> Result<PathBuf> {
        claim(project, &StorageKey::parse(binding.storage_key())?, binding)
    }

    #[test]
    fn same_owner_reopens_original_private_data_path() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("0123456789abcdef");
        let binding = binding("../namespace:semantic/name", "private-key", "source:a");
        let data = mount(&project, &binding).unwrap();
        assert_eq!(data, project.join("private-key"));
        assert!(fs::read_dir(&data).unwrap().next().is_none());
        fs::write(data.join("history"), "private").unwrap();
        assert_eq!(mount(&project, &binding).unwrap(), data);
        assert_eq!(fs::read(data.join("history")).unwrap(), b"private");
        assert!(
            root.path()
                .join(OWNERS_DIRECTORY)
                .join("0123456789abcdef.json")
                .is_file()
        );
        assert_eq!(fs::read_dir(&data).unwrap().count(), 1);
    }

    #[test]
    fn different_store_cannot_claim_same_key_for_another_identity() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("0123456789abcdef");
        let owner = binding("semantic", "private-key", "source:a");
        let data = mount(&project, &owner).unwrap();
        fs::write(data.join("history"), "private").unwrap();
        for other in [
            binding("other-semantic", "private-key", "source:a"),
            binding("semantic", "private-key", "source:b"),
            binding("semantic", "PRIVATE-KEY", "source:a"),
        ] {
            assert!(mount(&project, &other).is_err());
        }
        assert_eq!(mount(&project, &owner).unwrap(), data);
        fs::remove_file(data.join("history")).unwrap();
        fs::remove_dir(&data).unwrap();
        assert!(mount(&project, &binding("other", "private-key", "source:b")).is_err());
        assert_eq!(mount(&project, &owner).unwrap(), data);
    }

    #[test]
    fn unowned_nonempty_legacy_data_is_not_adopted() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("0123456789abcdef");
        let data = project.join("private-key");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("history"), "legacy").unwrap();
        let binding = binding("semantic", "private-key", "source:a");
        let error = mount(&project, &binding).unwrap_err();
        assert!(error.to_string().contains("unowned nonempty"));
        assert_eq!(fs::read(data.join("history")).unwrap(), b"legacy");
        assert!(
            !root
                .path()
                .join(OWNERS_DIRECTORY)
                .join("0123456789abcdef.json")
                .exists()
        );
    }

    #[test]
    fn unowned_empty_legacy_directory_can_be_claimed() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("0123456789abcdef");
        let data = project.join("private-key");
        fs::create_dir_all(&data).unwrap();
        assert_eq!(
            mount(&project, &binding("semantic", "private-key", "source:a")).unwrap(),
            data
        );
    }

    #[test]
    fn malformed_or_missing_owner_record_never_exposes_existing_data() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("0123456789abcdef");
        let owner = binding("semantic", "private-key", "source:a");
        let data = mount(&project, &owner).unwrap();
        fs::write(data.join("history"), "private").unwrap();
        let record = root
            .path()
            .join(OWNERS_DIRECTORY)
            .join("0123456789abcdef.json");
        fs::write(&record, b"invalid").unwrap();
        assert!(mount(&project, &owner).is_err());
        fs::remove_file(record).unwrap();
        assert!(mount(&project, &owner).is_err());
        assert_eq!(fs::read(data.join("history")).unwrap(), b"private");
    }

    #[test]
    fn interrupted_prepublication_record_is_never_treated_as_ownership() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("0123456789abcdef");
        let owners = root.path().join(OWNERS_DIRECTORY);
        fs::create_dir(&owners).unwrap();
        let pending = owners.join("0123456789abcdef.pending");
        fs::write(&pending, b"unfinished claim").unwrap();
        let owner = binding("semantic", "private-key", "source:a");
        let data = mount(&project, &owner).unwrap();
        assert_eq!(data, project.join("private-key"));
        assert!(!pending.exists());
        assert!(fs::read_dir(&data).unwrap().next().is_none());
    }

    #[test]
    fn concurrent_claims_for_different_private_keys_are_both_retained() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("0123456789abcdef");
        let first = binding("semantic-a", "private-a", "source:a");
        let second = binding("semantic-b", "private-b", "source:b");
        std::thread::scope(|scope| {
            let first = scope.spawn(|| mount(&project, &first).unwrap());
            let second = scope.spawn(|| mount(&project, &second).unwrap());
            first.join().unwrap();
            second.join().unwrap();
        });
        let record = root
            .path()
            .join(OWNERS_DIRECTORY)
            .join("0123456789abcdef.json");
        let owners: Owners = serde_json::from_slice(&fs::read(record).unwrap()).unwrap();
        assert_eq!(owners.bindings.len(), 2);
        assert_eq!(owners.bindings["private-a"], first);
        assert_eq!(owners.bindings["private-b"], second);
    }

    #[tokio::test]
    async fn non_opted_layered_chain_does_not_mount_or_claim_provider_data() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("0123456789abcdef");
        let owner = binding("semantic", "private-key", "source:a");
        let key = StorageKey::parse("private-key").unwrap();
        assert_eq!(
            stage_data_dir(Some(&project), &key, &owner, false)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            stage_data_dir(None, &key, &owner, true).await.unwrap(),
            None
        );
        assert!(fs::read_dir(root.path()).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_data_and_owner_paths_fail_closed() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("0123456789abcdef");
        fs::create_dir(&project).unwrap();
        let outside = root.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("history"), "private").unwrap();
        let owner = binding("semantic", "private-key", "source:a");
        symlink(&outside, project.join("private-key")).unwrap();
        assert!(mount(&project, &owner).is_err());
        fs::remove_file(project.join("private-key")).unwrap();
        let record = root
            .path()
            .join(OWNERS_DIRECTORY)
            .join("0123456789abcdef.json");
        symlink(outside.join("history"), &record).unwrap();
        assert!(mount(&project, &owner).is_err());
        assert_eq!(fs::read(outside.join("history")).unwrap(), b"private");
    }

    #[test]
    fn concurrent_processes_cannot_claim_another_owners_directory() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("0123456789abcdef");
        let child = |source: &str| {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "data::tests::ownership_child"])
                .env("WASSETTE_ACP_DATA_TEST_PROJECT", &project)
                .env("WASSETTE_ACP_DATA_TEST_SOURCE", source)
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap()
        };
        let mut first = child("source:a");
        let mut second = child("source:b");
        let statuses = [first.wait().unwrap().code(), second.wait().unwrap().code()];
        assert!(
            statuses == [Some(0), Some(42)] || statuses == [Some(42), Some(0)],
            "{statuses:?}"
        );
        let source = if statuses[0] == Some(0) {
            "source:a"
        } else {
            "source:b"
        };
        assert!(mount(&project, &binding("semantic", "private-key", source)).is_ok());
    }

    #[test]
    fn ownership_child() {
        let Some(project) = std::env::var_os("WASSETTE_ACP_DATA_TEST_PROJECT") else {
            return;
        };
        let source = std::env::var("WASSETTE_ACP_DATA_TEST_SOURCE").unwrap();
        if let Err(error) = mount(
            Path::new(&project),
            &binding("semantic", "private-key", &source),
        ) {
            if error.to_string().contains("ownership conflicts") {
                std::process::exit(42);
            }
            panic!("unexpected ownership error: {error:#}");
        }
    }
}
