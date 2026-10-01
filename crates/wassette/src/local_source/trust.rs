// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::path::Path;

use anyhow::{bail, Result};

pub(super) fn check(path: &Path) -> Result<()> {
    check_inner(path, true)
}

pub(super) fn check_root(path: &Path) -> Result<()> {
    if !std::fs::metadata(path)?.is_dir() {
        bail!("local source root is not a directory: {}", path.display());
    }
    check_inner(path, false)
}

fn check_inner(path: &Path, parent: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let owner = unsafe { libc_geteuid() };
        let mut current = Some(path.to_path_buf());
        // The source root is checked separately; only the direct target parent
        // is security-relevant for a symlink that escapes it.
        for _ in 0..if parent { 2 } else { 1 } {
            let Some(path) = current else { break };
            let link = std::fs::symlink_metadata(&path)?;
            let target = std::fs::metadata(&path)?;
            if link.uid() != owner
                || target.uid() != owner
                || (!link.file_type().is_symlink() && link.mode() & 0o022 != 0)
                || target.mode() & 0o022 != 0
            {
                bail!("untrusted local source path: {}", path.display());
            }
            // Check symlink targets and their parents, not only the link's parent.
            if link.file_type().is_symlink() {
                let resolved = path.canonicalize()?;
                check(&resolved)?;
            }
            current = path.parent().map(Path::to_path_buf);
        }
    }
    // Windows ACL/ownership checking is not available here. Capture still checks
    // identity before and after reading; operators must protect the directory ACL.
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "geteuid"]
    fn libc_geteuid() -> u32;
}
