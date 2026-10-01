// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::fs::{self, File, Metadata};
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

use super::trust;
use crate::store::SourceObservation;

pub(super) struct Capture {
    pub wasm: Vec<u8>,
    pub sidecar: Option<Vec<u8>>,
    pub observation: SourceObservation,
}

pub(super) fn capture(path: &Path, settle: Duration, cap: u64) -> Result<Capture> {
    let sidecar = path.with_extension("policy.yaml");
    trust::check(path)?;
    let first = fs::metadata(path)?;
    let first_sidecar = optional_metadata(&sidecar)?;
    std::thread::sleep(settle);
    trust::check(path)?;
    let second = fs::metadata(path)?;
    let second_sidecar = optional_metadata(&sidecar)?;
    if !same(&first, &second) || !same_option(&first_sidecar, &second_sidecar) {
        bail!("source changed during settle interval");
    }
    let wasm = copy_checked(path, &second, cap)?;
    let policy = if let Some(meta) = second_sidecar {
        trust::check(&sidecar)?;
        Some(copy_checked(&sidecar, &meta, cap)?)
    } else {
        // A newly created sidecar must not turn an absent policy into a successful observation.
        if optional_metadata(&sidecar)?.is_some() {
            bail!("sidecar appeared during capture");
        }
        None
    };
    Ok(Capture {
        observation: SourceObservation {
            token: "local-source-v1".into(),
            artifact_sha256: hex::encode(Sha256::digest(&wasm)),
            sidecar_sha256: policy
                .as_ref()
                .map(|bytes| hex::encode(Sha256::digest(bytes))),
        },
        wasm,
        sidecar: policy,
    })
}

fn optional_metadata(path: &Path) -> Result<Option<Metadata>> {
    match fs::metadata(path) {
        Ok(meta) => Ok(Some(meta)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn same_option(a: &Option<Metadata>, b: &Option<Metadata>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => same(a, b),
        _ => false,
    }
}

fn same(a: &Metadata, b: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        a.dev() == b.dev()
            && a.ino() == b.ino()
            && a.len() == b.len()
            && a.mtime() == b.mtime()
            && a.mtime_nsec() == b.mtime_nsec()
            && a.ctime() == b.ctime()
            && a.ctime_nsec() == b.ctime_nsec()
    }
    #[cfg(not(unix))]
    {
        a.len() == b.len() && a.modified().ok() == b.modified().ok()
    }
}

fn copy_checked(path: &Path, expected: &Metadata, cap: u64) -> Result<Vec<u8>> {
    if !expected.is_file() || expected.len() > cap {
        bail!("source is not a regular file or exceeds capture size cap");
    }
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    if !same(expected, &file.metadata()?) {
        bail!("source changed before capture");
    }
    let mut bytes = Vec::new();
    let mut chunk = [0; 64 * 1024];
    loop {
        let count = file.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        if bytes.len() as u64 + count as u64 > cap {
            bail!("source exceeded capture size cap while reading");
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    if !same(expected, &file.metadata()?) || !same(expected, &fs::metadata(path)?) {
        bail!("source changed during capture");
    }
    Ok(bytes)
}
