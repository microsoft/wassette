// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::{BuildError, BuildErrorKind, BuildLimits, BuildRequest, BuilderConfig};

pub(crate) const MAGIC: &[u8; 8] = b"WSBLD003";
pub(crate) const ARGUMENT: &str = "--job-v3";
pub(crate) const MAX_INPUT: usize = 12 * 1024 * 1024;
pub(crate) const CHUNK: usize = 32 * 1024;
pub(crate) const WASM: u8 = 1;
pub(crate) const DIAGNOSTICS: u8 = 2;
pub(crate) const SUCCESS: u8 = 3;
pub(crate) const ERROR: u8 = 4;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Failure {
    kind: BuildErrorKind,
    diagnostic_truncated: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Job {
    pub config: BuilderConfig,
    pub limits: BuildLimits,
    pub request: BuildRequest,
    pub staging: PathBuf,
}

pub(crate) fn write_job(mut out: impl Write, job: &Job) -> Result<()> {
    let bytes = serde_json::to_vec(job)?;
    ensure!(bytes.len() <= MAX_INPUT, "IPC request exceeds budget");
    out.write_all(MAGIC)?;
    out.write_all(&(bytes.len() as u32).to_le_bytes())?;
    out.write_all(&bytes)?;
    out.flush()?;
    Ok(())
}

#[cfg(feature = "hyperlight")]
pub(crate) fn read_job(mut input: impl Read) -> Result<Job> {
    let mut magic = [0u8; 8];
    input.read_exact(&mut magic)?;
    ensure!(&magic == MAGIC, "unsupported builder IPC version");
    let mut size = [0u8; 4];
    input.read_exact(&mut size)?;
    let size = u32::from_le_bytes(size) as usize;
    ensure!(size <= MAX_INPUT, "IPC request exceeds budget");
    let mut bytes = vec![0; size];
    input.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(any(test, feature = "hyperlight"))]
pub(crate) fn frame(mut out: impl Write, kind: u8, data: &[u8]) -> Result<()> {
    ensure!(data.len() <= CHUNK, "IPC frame exceeds budget");
    out.write_all(&[kind])?;
    out.write_all(&(data.len() as u32).to_le_bytes())?;
    out.write_all(data)?;
    out.flush()?;
    Ok(())
}

#[cfg(any(test, feature = "hyperlight"))]
pub(crate) fn failure(mut out: impl Write, error: &BuildError) -> Result<()> {
    if let Some(diagnostic) = error.diagnostic() {
        for chunk in diagnostic.as_bytes().chunks(CHUNK) {
            frame(&mut out, DIAGNOSTICS, chunk)?;
        }
    }
    let code = serde_json::to_vec(&Failure {
        kind: error.kind(),
        diagnostic_truncated: error.diagnostic_truncated(),
    })?;
    frame(out, ERROR, &code)
}

pub(crate) fn reserve_diagnostics(used: &AtomicUsize, size: usize, cap: usize) -> Result<()> {
    used.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
        used.checked_add(size).filter(|total| *total <= cap)
    })
    .map_err(|_| {
        BuildError::with_diagnostic(
            BuildErrorKind::InvalidOutput,
            "Builder diagnostics exceeded their configured budget.",
            cap,
        )
    })?;
    Ok(())
}

pub(crate) fn read_result(
    mut input: impl Read,
    limits: &BuildLimits,
    diagnostics_used: &AtomicUsize,
) -> Result<(Vec<u8>, String)> {
    let mut magic = [0u8; 8];
    input
        .read_exact(&mut magic)
        .context("missing builder handshake; wrong helper or unsupported hypervisor/signing")?;
    ensure!(&magic == MAGIC, "unsupported builder IPC version");
    let mut wasm = Vec::new();
    let mut diagnostics = Vec::new();
    let mut frames = 0usize;
    loop {
        let mut header = [0; 5];
        input
            .read_exact(&mut header)
            .context("truncated builder response")?;
        let size = u32::from_le_bytes(header[1..].try_into()?) as usize;
        ensure!(size <= CHUNK, "IPC response frame exceeds budget");
        frames += 1;
        ensure!(frames <= 4096, "too many IPC response frames");
        match header[0] {
            WASM | DIAGNOSTICS => {
                ensure!(size > 0, "empty data frame");
                if header[0] == DIAGNOSTICS {
                    reserve_diagnostics(diagnostics_used, size, limits.diagnostics_bytes)?;
                }
                let (output, cap) = if header[0] == WASM {
                    (&mut wasm, limits.wasm_bytes)
                } else {
                    (&mut diagnostics, limits.diagnostics_bytes)
                };
                if size > cap.saturating_sub(output.len()) {
                    return Err(BuildError::with_diagnostic(
                        BuildErrorKind::InvalidOutput,
                        "Builder output exceeded its configured budget.",
                        limits.diagnostics_bytes,
                    )
                    .into());
                }
                let old = output.len();
                output.resize(old + size, 0);
                input.read_exact(&mut output[old..])?;
            }
            SUCCESS => {
                ensure!(size == 0 && !wasm.is_empty(), "invalid success frame");
                let mut trailing = [0];
                ensure!(input.read(&mut trailing)? == 0, "trailing builder response");
                return Ok((
                    wasm,
                    String::from_utf8(diagnostics).context("diagnostics are not UTF-8")?,
                ));
            }
            ERROR => {
                ensure!(size <= 256, "builder failure control frame exceeds budget");
                let mut error = vec![0; size];
                input.read_exact(&mut error)?;
                let failure: Failure = serde_json::from_slice(&error)?;
                let mut error = BuildError::with_diagnostic(
                    failure.kind,
                    std::str::from_utf8(&diagnostics)?,
                    limits.diagnostics_bytes,
                );
                error.set_truncated(failure.diagnostic_truncated);
                return Err(error.into());
            }
            _ => bail!("unknown builder response frame"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_forged_lengths_versions_and_success() {
        for bytes in [
            b"wrong000".to_vec(),
            b"WSBLD001".to_vec(),
            b"WSBLD002".to_vec(),
            [MAGIC.as_slice(), &[WASM], &u32::MAX.to_le_bytes()].concat(),
            [MAGIC.as_slice(), &[SUCCESS, 0, 0, 0, 0]].concat(),
            [
                MAGIC.as_slice(),
                &[WASM, 1, 0, 0, 0, 9],
                &[SUCCESS, 0, 0, 0, 0, 9],
            ]
            .concat(),
        ] {
            assert!(
                read_result(
                    bytes.as_slice(),
                    &BuildLimits::default(),
                    &AtomicUsize::new(0)
                )
                .is_err()
            );
        }
    }

    #[test]
    fn caps_output_before_reading_payload() {
        let bytes = [MAGIC.as_slice(), &[WASM, 2, 0, 0, 0]].concat();
        let limits = BuildLimits {
            wasm_bytes: 1,
            ..BuildLimits::default()
        };
        let error = read_result(bytes.as_slice(), &limits, &AtomicUsize::new(0)).unwrap_err();
        assert_eq!(
            error.downcast_ref::<BuildError>().unwrap().kind(),
            BuildErrorKind::InvalidOutput
        );
    }

    #[test]
    fn console_and_protocol_share_diagnostics_budget() {
        let used = AtomicUsize::new(0);
        reserve_diagnostics(&used, 3, 4).unwrap();
        assert!(reserve_diagnostics(&used, 2, 4).is_err());
        assert_eq!(used.load(Ordering::Relaxed), 3);
        reserve_diagnostics(&used, 1, 4).unwrap();
    }

    #[test]
    fn typed_failure_round_trips_without_logging_its_body() {
        let original = BuildError::with_diagnostic(
            BuildErrorKind::CompilationFailed,
            "source:4:2: error[E0308]: PRIVATE_DIAGNOSTIC expected u32, found string",
            256,
        );
        let mut bytes = MAGIC.to_vec();
        failure(&mut bytes, &original).unwrap();
        let error = read_result(
            bytes.as_slice(),
            &BuildLimits::default(),
            &AtomicUsize::new(0),
        )
        .unwrap_err();
        let typed = error.downcast_ref::<BuildError>().unwrap();
        assert_eq!(typed, &original);
        assert!(!format!("{error:#}").contains("PRIVATE_DIAGNOSTIC"));
        assert!(!format!("{typed:?}").contains("PRIVATE_DIAGNOSTIC"));
        assert!(typed.diagnostic().unwrap().contains("E0308"));
    }

    #[test]
    fn tiny_diagnostic_budget_still_preserves_the_failure_kind() {
        let original =
            BuildError::with_diagnostic(BuildErrorKind::InvalidWit, "expected semicolon", 3);
        let mut bytes = MAGIC.to_vec();
        failure(&mut bytes, &original).unwrap();
        let limits = BuildLimits {
            diagnostics_bytes: 3,
            ..BuildLimits::default()
        };
        let error = read_result(bytes.as_slice(), &limits, &AtomicUsize::new(0)).unwrap_err();
        let typed = error.downcast_ref::<BuildError>().unwrap();
        assert_eq!(typed.kind(), BuildErrorKind::InvalidWit);
        assert_eq!(typed.diagnostic(), Some("exp"));
        assert!(typed.diagnostic_truncated());
    }
}
