// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{BuildArtifact, BuildError, BuildErrorKind, BuildLimits, BuildRequest, BuilderConfig};

pub(crate) const ARGUMENT: &str = "--wassette-internal-builder-v1";
const MAGIC: &[u8; 8] = b"WSBLD004";
const MAX_JOB_BYTES: usize = 16 * 1024 * 1024;
const RESPONSE_HEADER_BYTES: usize = 512 * 1024;
const MAX_WASM_BYTES: usize = 32 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Job {
    pub config: BuilderConfig,
    pub limits: BuildLimits,
    pub request: BuildRequest,
    pub staging: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    ok: bool,
    #[serde(default)]
    kind: Option<BuildErrorKind>,
    #[serde(default)]
    diagnostic: Option<String>,
    #[serde(default)]
    diagnostic_truncated: bool,
    #[serde(default)]
    diagnostics: String,
    #[serde(default)]
    evidence: Option<crate::BuildEvidence>,
    wasm_bytes: usize,
}

pub(crate) fn write_job(mut output: impl Write, job: &Job) -> Result<()> {
    let bytes = serde_json::to_vec(job)?;
    ensure!(
        bytes.len() <= MAX_JOB_BYTES,
        "builder job exceeds IPC budget"
    );
    output.write_all(MAGIC)?;
    output.write_all(&(bytes.len() as u32).to_le_bytes())?;
    output.write_all(&bytes)?;
    output.flush()?;
    Ok(())
}

pub(crate) fn read_job(mut input: impl Read) -> Result<Job> {
    let mut magic = [0; 8];
    input.read_exact(&mut magic)?;
    ensure!(&magic == MAGIC, "unsupported builder IPC version");
    let mut size = [0; 4];
    input.read_exact(&mut size)?;
    let size = u32::from_le_bytes(size) as usize;
    ensure!(size <= MAX_JOB_BYTES, "builder job exceeds IPC budget");
    let mut bytes = vec![0; size];
    input.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub(crate) fn write_result(mut output: impl Write, result: Result<BuildArtifact>) -> Result<()> {
    match result {
        Ok(artifact) => {
            let response = Response {
                ok: true,
                kind: None,
                diagnostic: None,
                diagnostic_truncated: false,
                diagnostics: artifact.diagnostics,
                evidence: Some(artifact.evidence),
                wasm_bytes: artifact.wasm.len(),
            };
            write_header(&mut output, &response)?;
            output.write_all(&artifact.wasm)?;
        }
        Err(error) => {
            let error = BuildError::preserve_or_redact(error, BuildErrorKind::Unavailable);
            let typed = BuildError::from_error(&error).context("redacted build error")?;
            let response = Response {
                ok: false,
                kind: Some(typed.kind()),
                diagnostic: typed.diagnostic().map(str::to_owned),
                diagnostic_truncated: typed.diagnostic_truncated(),
                diagnostics: String::new(),
                evidence: None,
                wasm_bytes: 0,
            };
            write_header(&mut output, &response)?;
        }
    }
    output.flush()?;
    Ok(())
}

fn write_header(output: &mut impl Write, response: &Response) -> Result<()> {
    let bytes = serde_json::to_vec(response)?;
    ensure!(
        bytes.len() <= RESPONSE_HEADER_BYTES,
        "builder response header exceeds budget"
    );
    output.write_all(MAGIC)?;
    output.write_all(&(bytes.len() as u32).to_le_bytes())?;
    output.write_all(&bytes)?;
    Ok(())
}

pub(crate) fn read_result(mut input: impl Read, limits: &BuildLimits) -> Result<BuildArtifact> {
    let mut magic = [0; 8];
    input
        .read_exact(&mut magic)
        .context("missing builder response")?;
    ensure!(&magic == MAGIC, "unsupported builder IPC version");
    let mut size = [0; 4];
    input.read_exact(&mut size)?;
    let size = u32::from_le_bytes(size) as usize;
    ensure!(
        size <= RESPONSE_HEADER_BYTES,
        "builder response header exceeds budget"
    );
    let mut bytes = vec![0; size];
    input.read_exact(&mut bytes)?;
    let response: Response = serde_json::from_slice(&bytes)?;
    if !response.ok {
        ensure!(
            response.wasm_bytes == 0 && response.evidence.is_none(),
            "invalid builder failure response"
        );
        let mut error = BuildError::with_diagnostic(
            response.kind.unwrap_or(BuildErrorKind::Unavailable),
            response.diagnostic.as_deref().unwrap_or_default(),
            limits.diagnostics_bytes,
        );
        error.set_truncated(response.diagnostic_truncated);
        let mut trailing = [0];
        ensure!(input.read(&mut trailing)? == 0, "trailing builder response");
        return Err(error.into());
    }
    ensure!(
        response.kind.is_none() && response.diagnostic.is_none(),
        "invalid builder success response"
    );
    ensure!(
        response.wasm_bytes > 0
            && response.wasm_bytes <= limits.wasm_bytes
            && response.wasm_bytes <= MAX_WASM_BYTES,
        "builder output exceeds configured budget"
    );
    ensure!(
        response.diagnostics.len() <= limits.diagnostics_bytes,
        "builder diagnostics exceed configured budget"
    );
    let evidence = response.evidence.context("missing builder evidence")?;
    let mut wasm = vec![0; response.wasm_bytes];
    input.read_exact(&mut wasm)?;
    let mut trailing = [0];
    ensure!(input.read(&mut trailing)? == 0, "trailing builder response");
    Ok(BuildArtifact {
        wasm,
        diagnostics: response.diagnostics,
        evidence,
    })
}
