// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Result, StoreChange, StoreCursor, StoreError};

pub(super) const HEAD: &str = ".store-state.json";
pub(super) const ACTIVE: &str = ".active-transaction";
pub(super) const TRANSACTIONS: &str = ".transactions";
pub(super) const SUFFIXES: &[&str] = &[
    ".install.json",
    ".wasm",
    ".policy.yaml",
    ".policy.meta.json",
    ".metadata.json",
    ".cwasm",
];
const RECORD_LIMIT: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Head {
    pub schema: u32,
    pub cursor: StoreCursor,
    pub operation: Option<String>,
    pub change: Option<StoreChange>,
}

impl Head {
    pub fn next_cursor(&self) -> Result<StoreCursor> {
        Ok(StoreCursor {
            epoch: self.cursor.epoch.clone(),
            sequence: self
                .cursor
                .sequence
                .checked_add(1)
                .ok_or_else(|| StoreError::Integrity("store sequence exhausted".into()))?,
        })
    }
}

pub(super) fn open_lock(root: &Path) -> Result<File> {
    let path = root.join(".store.lock");
    reject_symlink(&path)?;
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?)
}

pub(super) fn exclusive(root: &Path) -> Result<File> {
    let file = open_lock(root)?;
    file.lock()?;
    recover(root)?;
    Ok(file)
}

pub(super) fn shared(root: &Path) -> Result<File> {
    loop {
        let file = open_lock(root)?;
        file.lock_shared()?;
        if !exists(&root.join(ACTIVE))? {
            return Ok(file);
        }
        drop(file);
        drop(exclusive(root)?);
    }
}

pub(super) fn initialize(root: &Path) -> Result<()> {
    let _lock = exclusive(root)?;
    if exists(&root.join(HEAD))? {
        read_head(root)?;
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        if entry?
            .file_name()
            .to_string_lossy()
            .ends_with(".install.json")
        {
            return Err(StoreError::Integrity(
                "missing store head beside existing receipts".into(),
            ));
        }
    }
    let epoch = tempfile::Builder::new()
        .prefix(".epoch-")
        .rand_bytes(24)
        .tempfile_in(root)?;
    let token = epoch
        .path()
        .file_name()
        .expect("temporary file has a name")
        .to_string_lossy()
        .into_owned();
    let head = Head {
        schema: 1,
        cursor: StoreCursor {
            epoch: token,
            sequence: 0,
        },
        operation: None,
        change: None,
    };
    serde_json::to_writer(epoch.as_file(), &head).map_err(anyhow::Error::from)?;
    epoch.as_file().sync_all()?;
    epoch
        .persist(root.join(HEAD))
        .map_err(|error| error.error)?;
    sync_dir(root)?;
    Ok(())
}

pub(super) fn read_head(root: &Path) -> Result<Head> {
    let head: Head = read_json(&root.join(HEAD))?;
    if head.schema != 1
        || head.cursor.epoch.is_empty()
        || head
            .change
            .as_ref()
            .is_some_and(|change| change.cursor != head.cursor)
    {
        return Err(StoreError::Integrity(
            "unsupported or invalid store head".into(),
        ));
    }
    Ok(head)
}

pub(super) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let file = open_optional(path)?.ok_or_else(|| {
        StoreError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("missing record {}", path.display()),
        ))
    })?;
    read_json_file(file)
}

pub(super) fn read_json_file<T: DeserializeOwned>(file: File) -> Result<T> {
    if file.metadata()?.len() > RECORD_LIMIT {
        return Err(StoreError::Integrity(
            "store record exceeds size limit".into(),
        ));
    }
    serde_json::from_reader(file)
        .map_err(|error| StoreError::Integrity(format!("invalid persisted store record: {error}")))
}

pub(super) fn open_optional(path: &Path) -> Result<Option<File>> {
    reject_symlink(path)?;
    match File::open(path) {
        Ok(file) if file.metadata()?.is_file() => Ok(Some(file)),
        Ok(_) => Err(StoreError::Integrity(format!(
            "not a regular store file: {}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn reject_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(StoreError::Integrity(format!(
            "store paths must not be symbolic links: {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct Image {
    name: String,
    sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Replacement {
    path: String,
    old: Option<Image>,
    new: Option<Image>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    schema: u32,
    operation: String,
    old_head: Head,
    new_head: Head,
    replacements: Vec<Replacement>,
}

/// Before activation Drop removes only private, unpublished preparation.
/// Activation relinquishes TempDir ownership before publishing the marker.
pub(super) struct Transaction {
    directory: Option<tempfile::TempDir>,
    path: PathBuf,
    pub operation: String,
    replacements: Vec<Replacement>,
}

impl Transaction {
    pub fn new(root: &Path) -> Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("op-")
            .rand_bytes(24)
            .tempdir_in(root.join(TRANSACTIONS))?;
        let path = directory.path().to_owned();
        let operation = path
            .file_name()
            .expect("transaction directory has a name")
            .to_string_lossy()
            .into_owned();
        Ok(Self {
            directory: Some(directory),
            path,
            operation,
            replacements: Vec::new(),
        })
    }

    /// Copy/hash both images outside the global lock.
    pub fn replace(&mut self, path: String, old: Option<File>, new: Option<&[u8]>) -> Result<()> {
        validate_target(&path)?;
        let index = self.replacements.len();
        let old = old
            .map(|file| self.save_image(format!("old-{index}"), file))
            .transpose()?;
        let new = new
            .map(|bytes| self.save_image(format!("new-{index}"), bytes))
            .transpose()?;
        self.replacements.push(Replacement { path, old, new });
        Ok(())
    }

    fn save_image(&self, name: String, mut reader: impl Read) -> Result<Image> {
        let mut file = File::create(self.path.join(&name))?;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            file.write_all(&buffer[..count])?;
            hash.update(&buffer[..count]);
        }
        file.sync_all()?;
        Ok(Image {
            name,
            sha256: hex::encode(hash.finalize()),
        })
    }

    pub fn seal(self, old_head: Head, new_head: Head) -> Result<SealedTransaction> {
        let manifest = Manifest {
            schema: 1,
            operation: self.operation.clone(),
            old_head,
            new_head,
            replacements: self.replacements,
        };
        write_json(&self.path.join("manifest.json"), &manifest)?;
        write_json(&self.path.join("head-new"), &manifest.new_head)?;
        write_json(&self.path.join("head-old"), &manifest.old_head)?;
        write_json(&self.path.join("active"), &manifest.operation)?;
        sync_dir(&self.path)?;
        sync_dir(self.path.parent().expect("stage has parent"))?;
        Ok(SealedTransaction {
            directory: self.directory,
            path: self.path,
            manifest,
        })
    }
}

pub(super) struct SealedTransaction {
    directory: Option<tempfile::TempDir>,
    path: PathBuf,
    manifest: Manifest,
}

impl SealedTransaction {
    /// Caller holds the exclusive lock and has rechecked the complete CAS.
    pub fn commit(mut self, root: &Path, failpoint: impl Fn(&str) -> Result<()>) -> Result<()> {
        failpoint("before-active")?;
        let _durable = self.directory.take().expect("unactivated stage").keep();
        let result = (|| {
            publish_image(root, &self.path, &self.manifest.operation, "active", ACTIVE)?;
            sync_dir(root)?;
            failpoint("after-active")?;
            for (index, replacement) in self.manifest.replacements.iter().enumerate() {
                apply(
                    root,
                    &self.path,
                    &self.manifest.operation,
                    replacement,
                    true,
                )?;
                failpoint("after-publish")?;
                failpoint(&format!("after-file-{index}"))?;
            }
            sync_dir(root)?;
            failpoint("before-head")?;
            publish_image(root, &self.path, &self.manifest.operation, "head-new", HEAD)?;
            sync_dir(root)?;
            failpoint("after-head")?;
            finish(root, &self.path, &self.manifest.operation)?;
            Ok(())
        })();
        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                let decided = read_head(root)
                    .map(|head| head == self.manifest.new_head)
                    .unwrap_or(true);
                let active = exists(&root.join(ACTIVE)).map_err(|inspection| {
                    StoreError::RecoveryRequired {
                        operation: self.manifest.operation.clone(),
                        detail: format!("{error}; cannot inspect recovery marker: {inspection}"),
                    }
                })?;
                if !active {
                    return if decided {
                        Err(StoreError::RecoveryRequired {
                            operation: self.manifest.operation,
                            detail: format!("commit decision may be durable: {error}"),
                        })
                    } else {
                        Err(error)
                    };
                }
                match recover(root) {
                    Ok(()) if !decided => Err(error),
                    Ok(()) => Err(StoreError::RecoveryRequired {
                        operation: self.manifest.operation,
                        detail: format!("decision committed and recovery completed: {error}"),
                    }),
                    Err(recovery) => Err(StoreError::RecoveryRequired {
                        operation: self.manifest.operation,
                        detail: format!("{error}; recovery failed: {recovery}"),
                    }),
                }
            }
        }
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut file = File::create(path)?;
    serde_json::to_writer(&mut file, value).map_err(anyhow::Error::from)?;
    file.sync_all()?;
    Ok(())
}

fn publish_image(
    root: &Path,
    stage: &Path,
    operation: &str,
    image: &str,
    target: &str,
) -> Result<()> {
    let scratch = root.join(format!(".store-publish-{operation}"));
    if exists(&scratch)? {
        fs::remove_file(&scratch)?;
    }
    fs::hard_link(stage.join(image), &scratch)?;
    fs::rename(scratch, root.join(target))?;
    Ok(())
}

fn apply(
    root: &Path,
    stage: &Path,
    operation: &str,
    replacement: &Replacement,
    forward: bool,
) -> Result<()> {
    let image = if forward {
        &replacement.new
    } else {
        &replacement.old
    };
    match image {
        Some(image) => publish_image(root, stage, operation, &image.name, &replacement.path)?,
        None => {
            let target = root.join(&replacement.path);
            if exists(&target)? {
                fs::remove_file(target)?;
            }
        }
    }
    Ok(())
}

fn finish(root: &Path, stage: &Path, operation: &str) -> Result<()> {
    let scratch = root.join(format!(".store-publish-{operation}"));
    if exists(&scratch)? {
        fs::remove_file(scratch)?;
    }
    fs::remove_file(root.join(ACTIVE))?;
    sync_dir(root)?;
    // Once the marker removal is durable, no recovery can reference these files.
    fs::remove_dir_all(stage)?;
    sync_dir(&root.join(TRANSACTIONS))?;
    Ok(())
}

/// Only recovery may perform large image reads under the exclusive lock.
pub(super) fn recover(root: &Path) -> Result<()> {
    if !exists(&root.join(ACTIVE))? {
        return Ok(());
    }
    let operation: String =
        read_json(&root.join(ACTIVE)).map_err(|error| StoreError::RecoveryRequired {
            operation: "unknown".into(),
            detail: error.to_string(),
        })?;
    let result = (|| {
        if !operation.starts_with("op-")
            || !operation
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(StoreError::Integrity(
                "invalid active operation name".into(),
            ));
        }
        let stage = root.join(TRANSACTIONS).join(&operation);
        reject_symlink(&stage)?;
        let manifest: Manifest = read_json(&stage.join("manifest.json"))?;
        if manifest.schema != 1
            || manifest.operation != operation
            || manifest.new_head.operation.as_ref() != Some(&operation)
        {
            return Err(StoreError::Integrity("invalid transaction manifest".into()));
        }
        let head = read_head(root)?;
        let forward = if head == manifest.new_head {
            true
        } else if head == manifest.old_head {
            false
        } else {
            return Err(StoreError::Integrity(
                "head matches neither side of the pending transaction".into(),
            ));
        };
        let mut targets = std::collections::HashSet::new();
        for replacement in &manifest.replacements {
            validate_target(&replacement.path)?;
            if !targets.insert(&replacement.path) {
                return Err(StoreError::Integrity("duplicate journal target".into()));
            }
            let image = if forward {
                &replacement.new
            } else {
                &replacement.old
            };
            if let Some(image) = image {
                if !(image.name.starts_with("new-") || image.name.starts_with("old-"))
                    || !image
                        .name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                {
                    return Err(StoreError::Integrity("invalid recovery image path".into()));
                }
                let mut file = open_optional(&stage.join(&image.name))?
                    .ok_or_else(|| StoreError::Integrity("missing recovery image".into()))?;
                let mut hash = Sha256::new();
                let mut buffer = [0u8; 64 * 1024];
                loop {
                    let count = file.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    hash.update(&buffer[..count]);
                }
                if hex::encode(hash.finalize()) != image.sha256 {
                    return Err(StoreError::Integrity("corrupt recovery image".into()));
                }
            }
        }
        for replacement in &manifest.replacements {
            apply(root, &stage, &operation, replacement, forward)?;
        }
        sync_dir(root)?;
        // The head is already the decision; recovery never invents a new one.
        finish(root, &stage, &operation)?;
        Ok(())
    })();
    result.map_err(|error: StoreError| StoreError::RecoveryRequired {
        operation,
        detail: error.to_string(),
    })
}

fn validate_target(path: &str) -> Result<()> {
    let key = SUFFIXES.iter().find_map(|suffix| path.strip_suffix(suffix));
    match key {
        Some(key) if crate::StorageKey::parse(key).is_ok() => Ok(()),
        _ => Err(StoreError::Integrity(format!(
            "invalid journal target: {path}"
        ))),
    }
}
