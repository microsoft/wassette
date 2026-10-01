// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Operator-pinned Rust library crates compiled inside the builder guest.
//!
//! Requests never name dependencies. The operator profile lists registry
//! `.crate` archives in dependency-first order; the guest extracts and compiles
//! each with the fixed driver flags before compiling the request source.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
#[cfg(any(test, feature = "hyperlight"))]
use sha2::{Digest, Sha256};

#[cfg(any(test, feature = "hyperlight"))]
use crate::digest_hex;
use crate::validate_digest;

pub(crate) const MAX_CRATES: usize = 64;
#[cfg(any(test, feature = "hyperlight"))]
const MAX_ARCHIVE_BYTES: u64 = 16 * 1024 * 1024;
#[cfg(any(test, feature = "hyperlight"))]
const MAX_TOTAL_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FEATURES: usize = 128;

/// Names the guest compilation already uses for itself or the sysroot.
const RESERVED: &[&str] = &[
    "alloc",
    "core",
    "std",
    "proc_macro",
    "test",
    "wit_bindgen",
    "generated_component",
    "bindings",
    "self",
    "crate",
    "super",
];

/// A library crate the request source may use, compiled from a pinned archive.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RustCrate {
    /// Rust crate name, such as `grep_searcher` (not the package name).
    pub name: String,
    /// Absolute path to a gzip-compressed `.crate` archive.
    pub archive_path: PathBuf,
    /// SHA-256 of the archive; for crates.io this is the `Cargo.lock` checksum.
    pub archive_sha256: String,
    /// Library root inside the archive's single top-level directory.
    pub root: String,
    /// Rust edition: `2015`, `2018`, `2021`, or `2024`.
    pub edition: String,
    /// Enabled Cargo features, passed as `--cfg feature="..."`.
    #[serde(default)]
    pub features: Vec<String>,
    /// Earlier entries this crate links against.
    #[serde(default)]
    pub dependencies: Vec<CrateDependency>,
}

/// A link to an earlier [`RustCrate`], optionally under a renamed extern.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CrateDependency {
    /// `name` of an earlier `rust_crates` entry.
    #[serde(rename = "crate")]
    pub krate: String,
    /// Extern name used by the dependent's source; defaults to `crate`.
    #[serde(default)]
    pub rename: Option<String>,
}

impl CrateDependency {
    pub(crate) fn extern_name(&self) -> &str {
        self.rename.as_deref().unwrap_or(&self.krate)
    }
}

fn identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    value.len() <= 64
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && value != "_"
}

fn relative_source(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && value.len() <= 256
        && value.ends_with(".rs")
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b))
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// Validate host configuration before any job runs.
pub(crate) fn validate(crates: &[RustCrate]) -> Result<()> {
    ensure!(
        crates.len() <= MAX_CRATES,
        "at most {MAX_CRATES} Rust crates may be configured"
    );
    let mut known = BTreeSet::new();
    for krate in crates {
        let name = &krate.name;
        ensure!(
            identifier(name) && !RESERVED.contains(&name.as_str()),
            "Rust crate name `{name}` must be an unreserved identifier"
        );
        ensure!(
            krate.archive_path.is_absolute(),
            "Rust crate `{name}` archive path must be absolute"
        );
        validate_digest(&krate.archive_sha256)?;
        ensure!(
            relative_source(&krate.root),
            "Rust crate `{name}` root must be a relative `.rs` path"
        );
        ensure!(
            ["2015", "2018", "2021", "2024"].contains(&krate.edition.as_str()),
            "Rust crate `{name}` has an unsupported edition"
        );
        ensure!(
            krate.features.len() <= MAX_FEATURES
                && krate.features.iter().all(|feature| {
                    !feature.is_empty()
                        && feature.len() <= 128
                        && feature
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_+.".contains(&b))
                }),
            "Rust crate `{name}` has invalid features"
        );
        let mut externs = BTreeSet::new();
        for dependency in &krate.dependencies {
            ensure!(
                known.contains(dependency.krate.as_str()),
                "Rust crate `{name}` must follow its dependency `{}`",
                dependency.krate
            );
            let alias = dependency.extern_name();
            ensure!(
                identifier(alias) && !RESERVED.contains(&alias) && externs.insert(alias),
                "Rust crate `{name}` has an invalid or duplicate extern `{alias}`"
            );
        }
        ensure!(known.insert(name.as_str()), "duplicate Rust crate `{name}`");
    }
    Ok(())
}

/// Copy each archive into the guest's read-only input while hashing the copy,
/// so the guest compiles exactly the bytes that matched the pinned digest.
#[cfg(any(test, feature = "hyperlight"))]
pub(crate) fn stage(
    crates: &[RustCrate],
    input: &Path,
    cancel: &tokio_util::sync::CancellationToken,
    deadline: std::time::Instant,
) -> Result<()> {
    use std::io::{Read, Write};

    if crates.is_empty() {
        return Ok(());
    }
    let directory = input.join("crates");
    std::fs::create_dir(&directory)?;
    let mut total = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    for (index, krate) in crates.iter().enumerate() {
        let mut source = std::fs::File::open(&krate.archive_path)?;
        ensure!(
            source.metadata()?.is_file(),
            "Rust crate archive is not a file"
        );
        let mut copy = std::fs::File::create_new(directory.join(format!("{index}.crate")))?;
        let mut hash = Sha256::new();
        let mut size = 0u64;
        loop {
            ensure!(!cancel.is_cancelled(), "build cancelled");
            ensure!(
                std::time::Instant::now() < deadline,
                "build deadline exceeded"
            );
            let n = source.read(&mut buf)?;
            if n == 0 {
                break;
            }
            size += n as u64;
            total += n as u64;
            ensure!(
                size <= MAX_ARCHIVE_BYTES && total <= MAX_TOTAL_ARCHIVE_BYTES,
                "Rust crate archives exceed their budget"
            );
            hash.update(&buf[..n]);
            copy.write_all(&buf[..n])?;
        }
        ensure!(
            digest_hex(&hash.finalize()) == krate.archive_sha256,
            "Rust crate archive digest mismatch"
        );
        copy.sync_all()?;
    }
    Ok(())
}

/// Parameters the fixed guest driver needs, without host paths.
#[cfg(any(test, feature = "hyperlight"))]
pub(crate) fn driver_config(crates: &[RustCrate]) -> serde_json::Value {
    crates
        .iter()
        .map(|krate| {
            serde_json::json!({
                "name": krate.name,
                "root": krate.root,
                "edition": krate.edition,
                "features": krate.features,
                "metadata": &krate.archive_sha256[..16],
                "externs": krate
                    .dependencies
                    .iter()
                    .map(|dependency| [dependency.extern_name(), &dependency.krate])
                    .collect::<Vec<_>>(),
            })
        })
        .collect()
}

/// Digest of every crate setting that affects compilation, excluding paths.
#[cfg(any(test, feature = "hyperlight"))]
pub(crate) fn digest(crates: &[RustCrate]) -> String {
    let mut hash = Sha256::new();
    let mut field = |value: &str| {
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value.as_bytes());
    };
    for krate in crates {
        field(&krate.name);
        field(&krate.archive_sha256);
        field(&krate.root);
        field(&krate.edition);
        field(&krate.features.len().to_string());
        krate.features.iter().for_each(|feature| field(feature));
        field(&krate.dependencies.len().to_string());
        for dependency in &krate.dependencies {
            field(&dependency.krate);
            field(dependency.extern_name());
        }
    }
    digest_hex(&hash.finalize())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn fixture() -> Vec<RustCrate> {
        vec![
            RustCrate {
                name: "memmap2".into(),
                archive_path: "/crates/memmap2.crate".into(),
                archive_sha256: "c".repeat(64),
                root: "src/lib.rs".into(),
                edition: "2021".into(),
                features: vec![],
                dependencies: vec![],
            },
            RustCrate {
                name: "grep_searcher".into(),
                archive_path: "/crates/grep-searcher.crate".into(),
                archive_sha256: "d".repeat(64),
                root: "src/lib.rs".into(),
                edition: "2024".into(),
                features: vec!["default".into()],
                dependencies: vec![CrateDependency {
                    krate: "memmap2".into(),
                    rename: Some("memmap".into()),
                }],
            },
        ]
    }

    #[test]
    fn accepts_dependency_first_graph_with_renames() {
        validate(&fixture()).unwrap();
        validate(&[]).unwrap();
    }

    #[test]
    fn rejects_unsafe_or_ambiguous_entries() {
        type Mutation = Box<dyn Fn(&mut Vec<RustCrate>)>;
        let cases: Vec<(&str, Mutation)> = vec![
            ("forward dependency", Box::new(|c| c.swap(0, 1))),
            ("duplicate", Box::new(|c| c[1].name = "memmap2".into())),
            ("reserved", Box::new(|c| c[0].name = "wit_bindgen".into())),
            (
                "not identifier",
                Box::new(|c| c[0].name = "grep-searcher".into()),
            ),
            (
                "relative archive",
                Box::new(|c| c[0].archive_path = "x.crate".into()),
            ),
            ("digest", Box::new(|c| c[0].archive_sha256 = "C".repeat(64))),
            ("parent root", Box::new(|c| c[0].root = "../lib.rs".into())),
            (
                "absolute root",
                Box::new(|c| c[0].root = "/src/lib.rs".into()),
            ),
            ("non-rust root", Box::new(|c| c[0].root = "build.sh".into())),
            ("edition", Box::new(|c| c[0].edition = "2027".into())),
            (
                "feature quote",
                Box::new(|c| c[1].features = vec!["a\"".into()]),
            ),
            (
                "duplicate extern",
                Box::new(|c| {
                    let dependency = c[1].dependencies[0].clone();
                    c[1].dependencies.push(dependency);
                }),
            ),
            (
                "reserved extern",
                Box::new(|c| c[1].dependencies[0].rename = Some("std".into())),
            ),
        ];
        for (label, mutate) in cases {
            let mut crates = fixture();
            mutate(&mut crates);
            assert!(validate(&crates).is_err(), "{label} must be rejected");
        }
        let too_many = (0..=MAX_CRATES)
            .map(|index| RustCrate {
                name: format!("c{index}"),
                ..fixture()[0].clone()
            })
            .collect::<Vec<_>>();
        assert!(validate(&too_many).is_err());
    }

    #[test]
    fn digest_binds_compilation_inputs_but_not_paths() {
        let base = digest(&fixture());
        let mut moved = fixture();
        moved[0].archive_path = "/elsewhere/memmap2.crate".into();
        assert_eq!(digest(&moved), base);
        for mutate in [
            |c: &mut Vec<RustCrate>| c[0].archive_sha256 = "e".repeat(64),
            |c: &mut Vec<RustCrate>| c[1].features.clear(),
            |c: &mut Vec<RustCrate>| c[1].dependencies[0].rename = None,
            |c: &mut Vec<RustCrate>| c[1].edition = "2021".into(),
            |c: &mut Vec<RustCrate>| c[1].root = "src/main.rs".into(),
        ] {
            let mut changed = fixture();
            mutate(&mut changed);
            assert_ne!(digest(&changed), base);
        }
    }

    #[test]
    fn staging_copies_only_matching_archives_within_budget() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("memmap2.crate");
        std::fs::write(&archive, b"archive bytes").unwrap();
        let mut crates = fixture();
        crates.truncate(1);
        crates[0].archive_path = archive.clone();
        crates[0].archive_sha256 = crate::sha256(b"archive bytes");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let cancel = tokio_util::sync::CancellationToken::new();
        let input = dir.path().join("input");
        std::fs::create_dir(&input).unwrap();
        stage(&crates, &input, &cancel, deadline).unwrap();
        assert_eq!(
            std::fs::read(input.join("crates/0.crate")).unwrap(),
            b"archive bytes"
        );

        let input = dir.path().join("tampered");
        std::fs::create_dir(&input).unwrap();
        std::fs::write(&archive, b"replaced bytes").unwrap();
        let error = stage(&crates, &input, &cancel, deadline).unwrap_err();
        assert!(error.to_string().contains("digest mismatch"));

        let input = dir.path().join("empty");
        std::fs::create_dir(&input).unwrap();
        stage(&[], &input, &cancel, deadline).unwrap();
        assert!(!input.join("crates").exists());
    }

    #[test]
    fn driver_config_has_no_host_paths() {
        let value = driver_config(&fixture());
        let text = value.to_string();
        assert!(!text.contains("/crates/"));
        assert_eq!(
            value[1]["externs"][0],
            serde_json::json!(["memmap", "memmap2"])
        );
        assert_eq!(value[1]["metadata"], "d".repeat(16));
    }

    #[test]
    fn profile_json_uses_crate_and_rename_fields() {
        let value = serde_json::to_value(&fixture()[1]).unwrap();
        assert_eq!(value["dependencies"][0]["crate"], "memmap2");
        assert_eq!(value["dependencies"][0]["rename"], "memmap");
        let mut unknown = value;
        unknown["build_script"] = "build.rs".into();
        assert!(serde_json::from_value::<RustCrate>(unknown).is_err());
    }
}
