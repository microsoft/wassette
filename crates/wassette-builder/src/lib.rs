// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Native compilation happens only inside a fresh, packaged Hyperlight helper.
//! This crate neither installs components nor grants ownership or capabilities.
//! Returned bytes still require the parent's L1 and matching runtime validation.

mod artifact;
mod error;
mod ipc;
mod rust_crates;
mod supervise;

pub use error::{BuildError, BuildErrorKind};
pub use rust_crates::{CrateDependency, RustCrate};

#[cfg(feature = "hyperlight")]
#[doc(hidden)]
pub mod helper;

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Semaphore, TryAcquireError};
use tokio_util::sync::CancellationToken;

pub const PROFILE_ID: &str = "wassette-rust-1.98.1-std-wasip2-v3";
pub const COMPILER_VERSION: &str = "1.98.1";
pub const BINDGEN_VERSION: &str = "0.62.0";
pub const RUNTIME_VERSION: &str = "hyperlight-unikraft-0.17.0";
pub const TARGET: &str = "wasm32-wasip2";
const MIB: usize = 1024 * 1024;
/// Operators may raise guest memory above the default for pinned crates: the
/// guest runtime does not reclaim memory from exited compiler processes.
pub const MAX_GUEST_SCRATCH_MIB: usize = 8192;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum ComponentKind {
    Tool,
    AcpLayer,
}

/// Rust source implements generated `bindings` traits and invokes their export
/// macro. It is never interpreted as a command, path, Cargo manifest or script.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildRequest {
    pub component_name: String,
    pub source: String,
    pub wit: String,
    pub world: String,
    pub kind: ComponentKind,
}

/// Host-only configuration. Provision the packaged helper immutably. Each job
/// captures and verifies the exact initrd backing used for boot. Digests ensure
/// integrity; trusting the selected origin/profile remains the operator's job.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuilderConfig {
    pub helper_path: PathBuf,
    pub helper_sha256: String,
    pub initrd_path: PathBuf,
    pub initrd_sha256: String,
    /// Private temporary directories only, never a live component store.
    pub staging_root: PathBuf,
    /// Complete WIT packages, in dependency-first order, chosen by the host.
    /// For ACP, supply the canonical ACP package and its dependencies here.
    pub wit_dependencies: Vec<String>,
    /// Pinned library crates, in dependency-first order, that request source
    /// may use. Each archive is digest-checked and compiled inside the guest.
    #[serde(default)]
    pub rust_crates: Vec<RustCrate>,
}

/// Finite, host-selected budgets. Values can be reduced, but cannot exceed the
/// published profile ceilings. These equal the defaults, except guest scratch,
/// which may be raised to [`MAX_GUEST_SCRATCH_MIB`]. Requests cannot override
/// any budget.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BuildLimits {
    pub source_bytes: usize,
    pub wit_bytes: usize,
    pub wasm_bytes: usize,
    pub diagnostics_bytes: usize,
    pub wall_time_ms: u64,
    pub guest_scratch_mib: usize,
    pub generated_bindings_bytes: usize,
    pub max_parallel_jobs: usize,
}

impl Default for BuildLimits {
    fn default() -> Self {
        Self {
            source_bytes: MIB,
            wit_bytes: 256 * 1024,
            wasm_bytes: 32 * MIB,
            diagnostics_bytes: 256 * 1024,
            wall_time_ms: 120_000,
            guest_scratch_mib: 2048,
            generated_bindings_bytes: 8 * MIB,
            max_parallel_jobs: 1,
        }
    }
}

impl BuildLimits {
    pub fn validate(&self) -> Result<()> {
        let max = Self::default();
        for (name, value, ceiling) in [
            ("source", self.source_bytes, max.source_bytes),
            ("WIT", self.wit_bytes, max.wit_bytes),
            ("Wasm", self.wasm_bytes, max.wasm_bytes),
            ("diagnostics", self.diagnostics_bytes, max.diagnostics_bytes),
            ("scratch", self.guest_scratch_mib, MAX_GUEST_SCRATCH_MIB),
            (
                "bindings",
                self.generated_bindings_bytes,
                max.generated_bindings_bytes,
            ),
            ("parallel jobs", self.max_parallel_jobs, 4),
        ] {
            ensure!(
                value > 0 && value <= ceiling,
                "invalid {name} limit: 1..={ceiling}"
            );
        }
        ensure!(
            self.wall_time_ms > 0 && self.wall_time_ms <= max.wall_time_ms,
            "invalid wall deadline: 1..=120000 milliseconds"
        );
        Ok(())
    }
}

/// Host observations, not authorization, ownership, runtime compatibility or
/// guest attestations. Digests establish integrity, not origin trust.
/// There is deliberately no self-hash or final Wasm hash.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BuildEvidence {
    pub source_sha256: String,
    pub wit_sha256: String,
    pub wit_dependencies_sha256: String,
    #[serde(alias = "initrd_sha256")]
    pub builder_initrd_sha256: String,
    #[serde(alias = "builder_sha256")]
    pub builder_helper_sha256: String,
    /// No OCI manifest is selected by the local-initrd profile.
    #[serde(default)]
    pub builder_manifest_digest: Option<String>,
    #[serde(alias = "profile_id")]
    pub profile: String,
    pub profile_sha256: String,
    #[serde(alias = "compiler_version")]
    pub compiler: String,
    #[serde(alias = "bindgen_version")]
    pub bindgen: String,
    #[serde(alias = "binding_runtime_version")]
    pub binding_runtime: String,
    #[serde(alias = "runtime_version")]
    pub vm_runtime: String,
    pub world: String,
    #[serde(alias = "target_platform")]
    pub target: String,
    pub host_platform: String,
    pub kind: ComponentKind,
    pub component_name: String,
}

pub struct BuildArtifact {
    pub wasm: Vec<u8>,
    pub diagnostics: String,
    pub evidence: BuildEvidence,
}

impl std::fmt::Debug for BuildArtifact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuildArtifact")
            .field("wasm_bytes", &self.wasm.len())
            .field("diagnostic_bytes", &self.diagnostics.len())
            .field("kind", &self.evidence.kind)
            .finish()
    }
}

#[derive(Clone)]
pub struct Builder {
    config: Arc<BuilderConfig>,
    limits: BuildLimits,
    permits: Arc<Semaphore>,
}

impl Builder {
    pub fn new(config: BuilderConfig, limits: BuildLimits) -> Result<Self> {
        Self::new_inner(config, limits)
            .map_err(|error| BuildError::preserve_or_redact(error, BuildErrorKind::Unavailable))
    }

    fn new_inner(config: BuilderConfig, limits: BuildLimits) -> Result<Self> {
        limits.validate()?;
        ensure!(
            cfg!(all(target_os = "macos", target_arch = "aarch64"))
                || cfg!(all(
                    target_os = "linux",
                    any(target_arch = "aarch64", target_arch = "x86_64")
                )),
            "unsupported builder platform; requires Apple silicon HVF or Linux KVM/MSHV"
        );
        for (label, path) in [
            ("helper", &config.helper_path),
            ("initrd", &config.initrd_path),
            ("staging", &config.staging_root),
        ] {
            ensure!(
                path.is_absolute(),
                "{label} path must be host-chosen and absolute"
            );
        }
        validate_digest(&config.helper_sha256)?;
        validate_digest(&config.initrd_sha256)?;
        ensure!(config.helper_path.is_file(), "builder helper is missing");
        ensure!(config.initrd_path.is_file(), "builder initrd is missing");
        ensure!(
            config.staging_root.is_dir(),
            "private staging directory is missing"
        );
        let dependency_bytes = config.wit_dependencies.iter().try_fold(0usize, |n, s| {
            n.checked_add(s.len())
                .context("WIT dependency size overflow")
        })?;
        ensure!(
            config.wit_dependencies.len() <= 32 && dependency_bytes <= limits.wit_bytes,
            "host WIT dependency profile exceeds its budget"
        );
        rust_crates::validate(&config.rust_crates)?;
        for krate in &config.rust_crates {
            ensure!(
                krate.archive_path.is_file(),
                "Rust crate `{}` archive is missing",
                krate.name
            );
        }
        Ok(Self {
            permits: Arc::new(Semaphore::new(limits.max_parallel_jobs)),
            config: Arc::new(config),
            limits,
        })
    }

    /// Cancellation (including dropping this future) kills and reaps the helper.
    /// A dedicated supervisor owns its permit and staging until `wait` completes,
    /// independently of the async executor's lifetime.
    ///
    /// Admission is immediate: a saturated builder returns [`BuildErrorKind::Busy`]
    /// without queuing or retaining the request for a later permit.
    pub async fn build(
        &self,
        request: BuildRequest,
        cancel: CancellationToken,
    ) -> Result<BuildArtifact> {
        if cancel.is_cancelled() {
            return Err(BuildError::new(BuildErrorKind::Cancelled).into());
        }
        let permit = self.permits.clone().try_acquire_owned().map_err(|error| {
            BuildError::new(match error {
                TryAcquireError::NoPermits => BuildErrorKind::Busy,
                TryAcquireError::Closed => BuildErrorKind::Unavailable,
            })
        })?;
        validate_request(&request, &self.limits).map_err(|error| {
            BuildError::explain(
                &error,
                BuildErrorKind::InvalidRequest,
                self.limits.diagnostics_bytes,
            )
        })?;
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(self.limits.wall_time_ms);
        let local_cancel = cancel.child_token();
        let guard = local_cancel.clone().drop_guard();
        let config = self.config.clone();
        let limits = self.limits.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .name("wassette-build-supervisor".into())
            .spawn(move || {
                let _permit = permit;
                let result = supervise::run(&config, &limits, request, &local_cancel, deadline)
                    .map_err(|error| {
                        if local_cancel.is_cancelled() {
                            BuildError::new(BuildErrorKind::Cancelled).into()
                        } else if std::time::Instant::now() >= deadline {
                            BuildError::new(BuildErrorKind::DeadlineExceeded).into()
                        } else {
                            BuildError::preserve_or_redact(error, BuildErrorKind::Unavailable)
                        }
                    });
                drop(_permit);
                let _ = tx.send(result);
            })
            .map_err(|_| BuildError::new(BuildErrorKind::Internal))?;
        let result = rx
            .await
            .map_err(|_| BuildError::new(BuildErrorKind::Internal))?;
        guard.disarm();
        result
    }
}

fn validate_request(request: &BuildRequest, limits: &BuildLimits) -> Result<()> {
    ensure!(
        !request.source.is_empty() && request.source.len() <= limits.source_bytes,
        "Rust source must contain 1..={} UTF-8 bytes; received {} bytes",
        limits.source_bytes,
        request.source.len()
    );
    ensure!(
        !request.wit.is_empty() && request.wit.len() <= limits.wit_bytes,
        "WIT must contain 1..={} UTF-8 bytes; received {} bytes",
        limits.wit_bytes,
        request.wit.len()
    );
    ensure!(
        request.component_name.len() <= 512
            && !request.component_name.trim().is_empty()
            && !request.component_name.chars().any(char::is_control),
        "component name must be nonblank, control-free and at most 512 bytes"
    );
    ensure!(
        !request.world.is_empty()
            && request.world.len() <= 256
            && request
                .world
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_:/.@".contains(&b)),
        "WIT world must contain 1..=256 ASCII letters, digits, or -_:/.@"
    );
    Ok(())
}

fn validate_digest(value: &str) -> Result<()> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "expected a lowercase SHA-256 digest"
    );
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    digest_hex(&Sha256::digest(bytes))
}

fn digest_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn file_digest(
    path: &Path,
    cap: u64,
    cancel: &CancellationToken,
    deadline: std::time::Instant,
) -> Result<String> {
    let mut file = File::open(path).context("open pinned builder input")?;
    captured_file_digest(&mut file, cap, cancel, deadline)
}

fn captured_file_digest(
    file: &mut File,
    cap: u64,
    cancel: &CancellationToken,
    deadline: std::time::Instant,
) -> Result<String> {
    ensure!(
        file.metadata()?.is_file() && file.metadata()?.len() <= cap,
        "pinned builder input exceeds profile size"
    );
    let mut hash = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        ensure!(!cancel.is_cancelled(), "build cancelled");
        ensure!(
            std::time::Instant::now() < deadline,
            "build deadline exceeded"
        );
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        ensure!(
            total <= cap,
            "pinned builder input grew beyond profile size"
        );
        hash.update(&buf[..n]);
    }
    Ok(digest_hex(&hash.finalize()))
}

fn evidence(config: &BuilderConfig, request: &BuildRequest) -> BuildEvidence {
    let mut dependencies = Sha256::new();
    for source in &config.wit_dependencies {
        dependencies.update((source.len() as u64).to_le_bytes());
        dependencies.update(source.as_bytes());
    }
    BuildEvidence {
        source_sha256: sha256(request.source.as_bytes()),
        wit_sha256: sha256(request.wit.as_bytes()),
        wit_dependencies_sha256: digest_hex(&dependencies.finalize()),
        builder_initrd_sha256: config.initrd_sha256.clone(),
        builder_helper_sha256: config.helper_sha256.clone(),
        builder_manifest_digest: None,
        profile: PROFILE_ID.into(),
        // The executable digest covers the fixed driver, runtime and bindgen
        // implementation as well as all host transformation code. Pinned
        // crates extend the profile; profiles without them keep their digest.
        profile_sha256: sha256(
            if config.rust_crates.is_empty() {
                format!(
                    "{PROFILE_ID}\0{}\0{}",
                    config.helper_sha256, config.initrd_sha256
                )
            } else {
                format!(
                    "{PROFILE_ID}\0{}\0{}\0{}",
                    config.helper_sha256,
                    config.initrd_sha256,
                    rust_crates::digest(&config.rust_crates)
                )
            }
            .as_bytes(),
        ),
        compiler: COMPILER_VERSION.into(),
        bindgen: BINDGEN_VERSION.into(),
        binding_runtime: BINDGEN_VERSION.into(),
        vm_runtime: RUNTIME_VERSION.into(),
        world: request.world.clone(),
        target: TARGET.into(),
        host_platform: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
        kind: request.kind,
        component_name: request.component_name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn request() -> BuildRequest {
        BuildRequest {
            component_name: "example:add".into(),
            source: "struct Component;".into(),
            wit: "package example:add; world tool { export add: func(); }".into(),
            world: "tool".into(),
            kind: ComponentKind::Tool,
        }
    }

    #[test]
    fn input_is_strict_and_bounded() {
        let mut value = serde_json::to_value(request()).unwrap();
        value["command"] = "rustc".into();
        assert!(serde_json::from_value::<BuildRequest>(value).is_err());
        let limits = BuildLimits::default();
        validate_request(&request(), &limits).unwrap();
        let mut bad = request();
        bad.component_name = "\n".into();
        assert!(validate_request(&bad, &limits).is_err());
        bad = request();
        bad.source = "x".repeat(limits.source_bytes + 1);
        assert!(validate_request(&bad, &limits).is_err());
        assert!(
            BuildLimits {
                wall_time_ms: 0,
                ..limits.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            BuildLimits {
                wasm_bytes: usize::MAX,
                ..limits.clone()
            }
            .validate()
            .is_err()
        );
        let scratch = |guest_scratch_mib| BuildLimits {
            guest_scratch_mib,
            ..limits.clone()
        };
        assert_eq!(limits.guest_scratch_mib, 2048);
        scratch(MAX_GUEST_SCRATCH_MIB).validate().unwrap();
        assert!(scratch(MAX_GUEST_SCRATCH_MIB + 1).validate().is_err());
        assert!(scratch(0).validate().is_err());
    }

    fn fixture_evidence() -> BuildEvidence {
        let mut request = request();
        request.source = "PRIVATE_SOURCE_SENTINEL".into();
        let config = BuilderConfig {
            helper_path: "/helper".into(),
            helper_sha256: "a".repeat(64),
            initrd_path: "/image".into(),
            initrd_sha256: "b".repeat(64),
            staging_root: "/stage".into(),
            wit_dependencies: vec!["PRIVATE_DEPENDENCY".into()],
            rust_crates: vec![],
        };
        let evidence = evidence(&config, &request);
        assert_eq!(evidence.source_sha256, sha256(request.source.as_bytes()));
        evidence
    }

    #[test]
    fn evidence_has_no_bodies_or_self_hash() {
        let evidence = fixture_evidence();
        let serialized = serde_json::to_string(&evidence).unwrap();
        assert!(!serialized.contains("PRIVATE_"));
        assert!(!serialized.contains("wasm_sha256"));
        assert!(!serialized.contains("evidence_sha256"));
        assert_eq!(evidence, serde_json::from_str(&serialized).unwrap());
    }

    #[test]
    fn evidence_matches_store_provenance_fields_without_inventing_a_manifest() {
        let value = serde_json::to_value(fixture_evidence()).unwrap();
        for (field, expected) in [
            ("source_sha256", sha256(b"PRIVATE_SOURCE_SENTINEL")),
            ("wit_sha256", sha256(request().wit.as_bytes())),
            ("builder_initrd_sha256", "b".repeat(64)),
            ("builder_helper_sha256", "a".repeat(64)),
            ("profile", PROFILE_ID.into()),
            ("compiler", COMPILER_VERSION.into()),
            ("bindgen", BINDGEN_VERSION.into()),
            ("binding_runtime", BINDGEN_VERSION.into()),
            ("vm_runtime", RUNTIME_VERSION.into()),
            ("world", "tool".into()),
            ("target", TARGET.into()),
        ] {
            assert_eq!(value[field], expected, "{field}");
        }
        assert_eq!(value["builder_manifest_digest"], serde_json::Value::Null);
        assert!(!value.as_object().unwrap().contains_key("wasm_sha256"));
    }

    #[test]
    fn evidence_reads_prototype_field_names_but_writes_only_store_names() {
        let expected = fixture_evidence();
        let mut value = serde_json::to_value(&expected).unwrap();
        let fields = value.as_object_mut().unwrap();
        fields.remove("builder_manifest_digest");
        for (current, prototype) in [
            ("builder_initrd_sha256", "initrd_sha256"),
            ("builder_helper_sha256", "builder_sha256"),
            ("profile", "profile_id"),
            ("compiler", "compiler_version"),
            ("bindgen", "bindgen_version"),
            ("binding_runtime", "binding_runtime_version"),
            ("vm_runtime", "runtime_version"),
            ("target", "target_platform"),
        ] {
            let field = fields.remove(current).unwrap();
            fields.insert(prototype.into(), field);
        }
        let parsed: BuildEvidence = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(serde_json::to_value(parsed).unwrap()["profile"], PROFILE_ID);
        value["profile"] = "conflicting-profile".into();
        assert!(serde_json::from_value::<BuildEvidence>(value).is_err());
        let mut unknown = serde_json::to_value(expected).unwrap();
        unknown["wasm_sha256"] = "f".repeat(64).into();
        assert!(serde_json::from_value::<BuildEvidence>(unknown).is_err());
    }

    #[test]
    fn pinned_crates_extend_profile_digest_without_changing_crate_free_profiles() {
        let base = fixture_evidence();
        assert_eq!(
            base.profile_sha256,
            sha256(format!("{PROFILE_ID}\0{}\0{}", "a".repeat(64), "b".repeat(64)).as_bytes())
        );
        let config = BuilderConfig {
            helper_path: "/helper".into(),
            helper_sha256: "a".repeat(64),
            initrd_path: "/image".into(),
            initrd_sha256: "b".repeat(64),
            staging_root: "/stage".into(),
            wit_dependencies: vec!["PRIVATE_DEPENDENCY".into()],
            rust_crates: rust_crates::tests::fixture(),
        };
        let mut request = request();
        request.source = "PRIVATE_SOURCE_SENTINEL".into();
        let with_crates = evidence(&config, &request);
        assert_ne!(with_crates.profile_sha256, base.profile_sha256);
        assert_eq!(
            BuildEvidence {
                profile_sha256: base.profile_sha256.clone(),
                ..with_crates
            },
            base
        );
    }

    #[test]
    fn artifact_debug_does_not_log_code_or_compiler_messages() {
        let artifact = BuildArtifact {
            wasm: b"PRIVATE_WASM_BODY".to_vec(),
            diagnostics: "PRIVATE_COMPILER_MESSAGE".into(),
            evidence: fixture_evidence(),
        };
        assert!(!format!("{artifact:?}").contains("PRIVATE_"));
    }
}
