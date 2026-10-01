// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::fs::File;
#[cfg(target_os = "linux")]
use std::io::{Read, Write};
use std::io::{Seek, SeekFrom};
#[cfg(target_os = "macos")]
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::fd::FromRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, ensure};
use tokio_util::sync::CancellationToken;

use crate::captured_file_digest;

const MAX_INITRD_BYTES: u64 = 1024 * 1024 * 1024;

/// Owns the exact backing hashed for boot, not merely an open source path.
pub(super) struct BootImage {
    path: PathBuf,
    _file: File,
    sha256: String,
}

impl BootImage {
    pub fn capture(
        source: &Path,
        staging: &Path,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Self> {
        check(cancel, deadline)?;
        let source = File::options()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(source)
            .context("open operator-selected initrd")?;
        let metadata = source.metadata()?;
        ensure!(
            metadata.is_file() && metadata.len() > 0 && metadata.len() <= MAX_INITRD_BYTES,
            "initrd must be a nonempty regular file of at most 1 GiB"
        );
        let (path, mut file) = snapshot(source, staging, cancel, deadline)?;
        file.seek(SeekFrom::Start(0))?;
        let actual = captured_file_digest(&mut file, MAX_INITRD_BYTES, cancel, deadline)?;
        // Darwin's /dev/fd reopening shares the descriptor's file position;
        // the SDK's first CPIO scan must start at byte zero.
        file.seek(SeekFrom::Start(0))?;
        Ok(Self {
            path,
            _file: file,
            sha256: actual,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

fn check(cancel: &CancellationToken, deadline: Instant) -> Result<()> {
    ensure!(!cancel.is_cancelled(), "build cancelled");
    ensure!(
        Instant::now() < deadline,
        "build deadline exceeded capturing initrd"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn snapshot(
    source: File,
    staging: &Path,
    cancel: &CancellationToken,
    deadline: Instant,
) -> Result<(PathBuf, File)> {
    use std::os::unix::fs::PermissionsExt;

    let directory = File::open(staging)?;
    // fclonefileat snapshots the already-open inode atomically using APFS
    // copy-on-write extents. No byte-copy fallback is permitted. The private
    // staging directory is parent-owned until reaping and is not guest-mounted.
    let result = unsafe {
        libc::fclonefileat(
            source.as_raw_fd(),
            directory.as_raw_fd(),
            c"initrd.cpio".as_ptr(),
            0,
        )
    };
    ensure!(
        result == 0,
        "immutable initrd snapshot requires a same-volume copy-on-write filesystem; no image copy fallback: {}",
        std::io::Error::last_os_error()
    );
    check(cancel, deadline)?;
    let path = staging.join("initrd.cpio");
    let file = File::open(&path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o400))?;
    std::fs::remove_file(path)?;
    // Unlike POSIX shared-memory descriptors, an unlinked regular vnode can
    // be reopened by the SDK through /dev/fd. No pathname or writer remains
    // that could replace or modify the hashed snapshot.
    Ok((PathBuf::from(format!("/dev/fd/{}", file.as_raw_fd())), file))
}

#[cfg(target_os = "linux")]
fn snapshot(
    mut source: File,
    _staging: &Path,
    cancel: &CancellationToken,
    deadline: Instant,
) -> Result<(PathBuf, File)> {
    let fd = unsafe {
        libc::memfd_create(
            c"wassette-initrd".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    ensure!(
        fd >= 0,
        "sealed memory initrd unavailable: {}",
        std::io::Error::last_os_error()
    );
    let mut file = unsafe { File::from_raw_fd(fd) };
    let mut bytes = [0; 64 * 1024];
    let mut total = 0u64;
    loop {
        check(cancel, deadline)?;
        let n = source.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        ensure!(total <= MAX_INITRD_BYTES, "initrd exceeds snapshot budget");
        file.write_all(&bytes[..n])?;
    }
    let seals = libc::F_SEAL_WRITE | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL;
    ensure!(
        unsafe { libc::fcntl(fd, libc::F_ADD_SEALS, seals) } == 0,
        "seal immutable initrd: {}",
        std::io::Error::last_os_error()
    );
    Ok((PathBuf::from(format!("/proc/self/fd/{fd}")), file))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn snapshot_survives_source_replacement_and_in_place_writes() {
        let root = tempfile::tempdir().unwrap();
        let stage = tempfile::tempdir_in(root.path()).unwrap();
        let source = root.path().join("source.cpio");
        std::fs::write(&source, b"original-image").unwrap();
        let image = BootImage::capture(
            &source,
            stage.path(),
            &CancellationToken::new(),
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(image.sha256(), crate::sha256(b"original-image"));
        assert!(!stage.path().join("initrd.cpio").exists());
        std::fs::write(&source, b"in-place-change").unwrap();
        let replacement = root.path().join("replacement");
        std::fs::write(&replacement, b"replacement").unwrap();
        std::fs::rename(replacement, source).unwrap();
        assert_eq!(std::fs::read(image.path()).unwrap(), b"original-image");
    }

    #[test]
    fn boot_backing_cannot_be_reopened_for_mutation() {
        use std::io::Write;

        let root = tempfile::tempdir().unwrap();
        let stage = tempfile::tempdir_in(root.path()).unwrap();
        let source = root.path().join("source.cpio");
        std::fs::write(&source, b"original-image").unwrap();
        let image = BootImage::capture(
            &source,
            stage.path(),
            &CancellationToken::new(),
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        if let Ok(mut file) = File::options().write(true).open(image.path()) {
            assert!(file.write_all(b"modified-image").is_err());
        }
        assert_eq!(std::fs::read(image.path()).unwrap(), b"original-image");
    }

    #[test]
    fn hash_is_of_the_captured_image_bytes() {
        let root = tempfile::tempdir().unwrap();
        let stage = tempfile::tempdir_in(root.path()).unwrap();
        let source = root.path().join("source.cpio");
        std::fs::write(&source, b"different-image").unwrap();
        let image = BootImage::capture(
            &source,
            stage.path(),
            &CancellationToken::new(),
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(image.sha256(), crate::sha256(b"different-image"));
    }

    #[test]
    fn empty_or_cancelled_capture_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("empty.cpio");
        std::fs::write(&source, b"").unwrap();
        let cancel = CancellationToken::new();
        assert!(
            BootImage::capture(
                &source,
                root.path(),
                &cancel,
                Instant::now() + Duration::from_secs(5),
            )
            .is_err()
        );
        cancel.cancel();
        assert!(
            BootImage::capture(
                &source,
                root.path(),
                &cancel,
                Instant::now() + Duration::from_secs(5),
            )
            .err()
            .unwrap()
            .to_string()
            .contains("cancelled")
        );
    }

    #[test]
    fn published_sdk_can_scan_the_exact_captured_descriptor() {
        let mut cpio = Vec::new();
        for (name, data) in [
            ("etc/hluk-runtime", b"agent\n".as_slice()),
            ("usr/local/bin/hl_pywarmdriver", b"".as_slice()),
            ("TRAILER!!!", b"".as_slice()),
        ] {
            cpio.extend_from_slice(b"070701");
            for value in [
                1,
                0o100644,
                0,
                0,
                1,
                0,
                data.len(),
                0,
                0,
                0,
                0,
                name.len() + 1,
                0,
            ] {
                cpio.extend_from_slice(format!("{value:08x}").as_bytes());
            }
            cpio.extend_from_slice(name.as_bytes());
            cpio.push(0);
            cpio.resize(cpio.len().next_multiple_of(4), 0);
            cpio.extend_from_slice(data);
            cpio.resize(cpio.len().next_multiple_of(4), 0);
        }
        let root = tempfile::tempdir().unwrap();
        let stage = tempfile::tempdir_in(root.path()).unwrap();
        let source = root.path().join("source.cpio");
        std::fs::write(&source, &cpio).unwrap();
        let image = BootImage::capture(
            &source,
            stage.path(),
            &CancellationToken::new(),
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        std::fs::remove_file(source).unwrap();
        assert_eq!(
            hyperlight_unikraft::default_scratch_mb(image.path()),
            hyperlight_unikraft::runtime_scratch_mb("agent").unwrap(),
        );
    }
}
