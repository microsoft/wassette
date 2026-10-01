// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Implementation detail of the packaged, one-job/one-VM executable.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use hyperlight_unikraft::{Mount, SandboxBuilder};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use wit_bindgen_core::{Files, WorldGenerator};

use crate::{BuildError, BuildErrorKind, BuildLimits, artifact, ipc, validate_request};

mod initrd;
mod world;

use initrd::BootImage;
use world::RequestedWorld;

#[cfg(test)]
use crate::{BuildRequest, ComponentKind};

const DRIVER: &str = include_str!("driver.py");

macro_rules! runtime_files {
    ($($path:literal),* $(,)?) => {
        const RUNTIME_FILES: &[(&str, &str)] = &[
            $(($path, include_str!(concat!("../runtime/", $path)))),*
        ];
    }
}

runtime_files! {
    "resource.rs", "support.rs", "rt/mod.rs", "rt/async_support.rs",
    "rt/wit_bindgen_cabi_realloc.rs",
    "rt/async_support/abi_buffer.rs", "rt/async_support/cabi.rs",
    "rt/async_support/error_context.rs", "rt/async_support/future_support.rs",
    "rt/async_support/futures_stream.rs", "rt/async_support/inter_task_wakeup.rs",
    "rt/async_support/inter_task_wakeup_disabled.rs", "rt/async_support/spawn.rs",
    "rt/async_support/spawn_disabled.rs", "rt/async_support/stream_support.rs",
    "rt/async_support/subtask.rs", "rt/async_support/try_lock.rs",
    "rt/async_support/unit_stream.rs", "rt/async_support/waitable.rs",
    "rt/async_support/waitable_set.rs", "rt/async_support/wasip3_context.rs",
}

/// Called only by the packaged binary. Standard output belongs exclusively to
/// IPC, including during Hyperlight boot (whose HostPrint normally uses stdout).
pub fn main(allow_vm_memory: fn(), limit_transforms: fn()) -> Result<()> {
    ensure!(
        std::env::args()
            .collect::<Vec<_>>()
            .get(1)
            .map(String::as_str)
            == Some(ipc::ARGUMENT),
        "expected {}; this helper is not a compiler CLI",
        ipc::ARGUMENT
    );
    let mut output = protocol_stdout()?;
    output.write_all(ipc::MAGIC)?;
    output.flush()?;
    watchdog(Duration::from_secs(120));
    let result = execute(allow_vm_memory, limit_transforms);
    match result {
        Ok(result) => {
            for chunk in result.diagnostics.as_bytes().chunks(ipc::CHUNK) {
                ipc::frame(&mut output, ipc::DIAGNOSTICS, chunk)?;
            }
            for chunk in result.wasm.chunks(ipc::CHUNK) {
                ipc::frame(&mut output, ipc::WASM, chunk)?;
            }
            ipc::frame(&mut output, ipc::SUCCESS, &[])?;
            Ok(())
        }
        Err(error) => {
            let error = BuildError::preserve_or_redact(error, BuildErrorKind::Unavailable);
            let error = error
                .downcast_ref::<BuildError>()
                .expect("redacted builder error");
            ipc::failure(&mut output, error)?;
            // The error is already on the protocol channel, not repeated on
            // stderr (which would consume the same diagnostics budget twice).
            terminate(1)
        }
    }
}

#[derive(Default)]
struct GuestOutput {
    wasm: Vec<u8>,
    diagnostics: String,
    rust_diagnostics: Vec<u8>,
    link_diagnostics: Vec<u8>,
    failure: Option<GuestFailure>,
    failed: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum GuestFailure {
    Rust,
    Link,
    Profile,
    Dependency,
    Diagnostics,
    Output,
}

fn execute(allow_vm_memory: fn(), limit_transforms: fn()) -> Result<GuestOutput> {
    let job = ipc::read_job(std::io::stdin().lock())?;
    job.limits.validate()?;
    validate_request(&job.request, &job.limits).map_err(|error| {
        BuildError::explain(
            &error,
            BuildErrorKind::InvalidRequest,
            job.limits.diagnostics_bytes,
        )
    })?;
    ensure!(job.staging.is_absolute(), "invalid host staging path");
    watchdog(Duration::from_millis(job.limits.wall_time_ms));
    std::thread::Builder::new()
        .name("builder-parent-pipe".into())
        .spawn(|| {
            let mut byte = [0];
            // Any EOF, unexpected extra input, or broken pipe ends this process,
            // including its VM, even if the async host has disappeared.
            let _ = std::io::stdin().read(&mut byte);
            terminate(125);
        })?;
    let cancel = CancellationToken::new();
    let deadline = Instant::now() + Duration::from_millis(job.limits.wall_time_ms);
    let image = BootImage::capture(
        &job.config.initrd_path,
        &job.config.initrd_sha256,
        &job.staging,
        &cancel,
        deadline,
    )?;
    let mut requested =
        RequestedWorld::resolve(&job.request, &job.config.wit_dependencies, &job.limits)?;
    let bindings = generate_resolved_bindings(&mut requested, &job.limits).map_err(|error| {
        BuildError::explain(
            &error,
            BuildErrorKind::InvalidWit,
            job.limits.diagnostics_bytes,
        )
    })?;
    let exports = linker_exports(&bindings).map_err(|error| {
        BuildError::explain(
            &error,
            BuildErrorKind::InvalidWit,
            job.limits.diagnostics_bytes,
        )
    })?;
    let input = job.staging.join("input");
    std::fs::create_dir(&input)?;
    prepare_sources(&input, &job.request.source, &bindings)?;
    crate::rust_crates::stage(&job.config.rust_crates, &input, &cancel, deadline)?;
    let driver_config = serde_json::json!({
        "exports": exports,
        "rust_crates": crate::rust_crates::driver_config(&job.config.rust_crates),
        "wasm_bytes": job.limits.wasm_bytes,
        "diagnostics_bytes": job.limits.diagnostics_bytes,
        "compiler_arch": std::env::consts::ARCH,
    });
    std::fs::write(
        input.join("driver.json"),
        serde_json::to_vec(&driver_config)?,
    )?;
    allow_vm_memory();
    let output = Arc::new(Mutex::new(GuestOutput::default()));
    let sink = output.clone();
    let limits = job.limits.clone();
    let mut sandbox = SandboxBuilder::from_initrd(image.path())
        .scratch_mb(job.limits.guest_scratch_mib)
        .mount(Mount::ro(&input, "/input"))
        .profile(false)
        // This is a bounded write-only extraction channel, not an application
        // tool import. There are no writable mounts, network, or other tools.
        .host_function("builder-output", move |input| {
            let mut output = sink.lock().map_err(|_| "output sink poisoned".to_owned())?;
            let result = extract_chunk(&mut output, input, &limits);
            if let Err(error) = &result { output.failed = true; return Err(error.to_string()); }
            Ok("null".into())
        })
        .boot()
        .context("Hyperlight boot failed: unsupported hypervisor/platform, missing macOS com.apple.security.hypervisor signing, or incompatible builder profile/initrd")?;
    let run_result = sandbox.run(DRIVER);
    drop(sandbox);
    drop(image);
    limit_transforms();
    let mut output = Arc::try_unwrap(output)
        .map_err(|_| anyhow::anyhow!("VM retained output capability"))?
        .into_inner()
        .map_err(|_| anyhow::anyhow!("output sink poisoned"))?;
    let raw = [
        String::from_utf8_lossy(&output.rust_diagnostics),
        String::from_utf8_lossy(&output.link_diagnostics),
    ]
    .join("\n");
    let (diagnostics, diagnostic_truncated) =
        crate::error::sanitize(&raw, job.limits.diagnostics_bytes);
    output.diagnostics = diagnostics;
    if let Err(error) = run_result {
        if output.failed || matches!(output.failure, Some(GuestFailure::Output)) {
            return Err(BuildError::with_diagnostic(
                BuildErrorKind::InvalidOutput,
                "Compiled Wasm is empty or exceeds its configured output budget.",
                job.limits.diagnostics_bytes,
            )
            .into());
        }
        let message = match output.failure {
            // Pinned crates are operator configuration, not requester input.
            Some(GuestFailure::Profile | GuestFailure::Dependency) => {
                return Err(BuildError::new(BuildErrorKind::Unavailable).into());
            }
            Some(GuestFailure::Diagnostics) => {
                "Compiler diagnostics exceeded their configured budget."
            }
            Some(GuestFailure::Rust) => "Rust compilation failed.",
            Some(GuestFailure::Link) => "Component linking failed.",
            Some(GuestFailure::Output) => unreachable!("output failure handled above"),
            None if matches!(error, hyperlight_unikraft::Error::Deadlocked) => {
                "The compiler guest is deadlocked before delivering a complete diagnostic; reduce source or diagnostic volume."
            }
            // Earlier diagnostics (often warnings) did not cause a guest crash.
            None if guest_crashed(&error) => {
                return Err(BuildError::with_diagnostic(
                    BuildErrorKind::CompilationFailed,
                    GUEST_CRASHED,
                    job.limits.diagnostics_bytes,
                )
                .into());
            }
            None if !output.diagnostics.is_empty() => "Compilation failed.",
            None => return Err(BuildError::new(BuildErrorKind::Unavailable).into()),
        };
        let mut failure = BuildError::with_diagnostic(
            BuildErrorKind::CompilationFailed,
            &format!("{message}\n{}", output.diagnostics),
            job.limits.diagnostics_bytes,
        );
        failure.set_truncated(diagnostic_truncated);
        return Err(failure.into());
    }
    if diagnostic_truncated {
        return Err(BuildError::with_diagnostic(
            BuildErrorKind::InvalidOutput,
            "Compiler diagnostics exceeded the configured safe-response budget.",
            job.limits.diagnostics_bytes,
        )
        .into());
    }
    if output.failed || output.failure.is_some() || output.wasm.is_empty() {
        return Err(BuildError::with_diagnostic(
            BuildErrorKind::InvalidOutput,
            "The compiler returned invalid or oversized output.",
            job.limits.diagnostics_bytes,
        )
        .into());
    }
    output.wasm =
        artifact::name_component(output.wasm, &job.request, job.limits.wasm_bytes, || {
            ensure!(
                Instant::now() < deadline,
                "deadline exceeded naming captured component"
            );
            Ok(())
        })
        .map_err(|error| {
            BuildError::explain(
                &error,
                BuildErrorKind::InvalidOutput,
                job.limits.diagnostics_bytes,
            )
        })?;
    requested
        .validate(&output.wasm, &job.limits, deadline)
        .map_err(|error| {
            BuildError::explain(
                &error,
                BuildErrorKind::InvalidOutput,
                job.limits.diagnostics_bytes,
            )
        })?;
    Ok(output)
}

const GUEST_CRASHED: &str = "The compiler guest crashed before reporting a result, usually because compilation exhausted its configured guest scratch memory (limits.guest_scratch_mib).";

fn guest_crashed(error: &hyperlight_unikraft::Error) -> bool {
    matches!(
        error,
        hyperlight_unikraft::Error::Hyperlight(
            hyperlight_unikraft::hyperlight_host::HyperlightError::GuestAborted(..)
        )
    )
}

fn extract_chunk(output: &mut GuestOutput, input: &str, limits: &BuildLimits) -> Result<()> {
    ensure!(!output.failed, "extraction already failed");
    ensure!(
        input.len() <= 48 * 1024,
        "guest extraction frame exceeds budget"
    );
    let [kind, encoded]: [String; 2] =
        serde_json::from_str(input).context("invalid guest extraction message")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded)?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= ipc::CHUNK,
        "invalid guest extraction chunk"
    );
    match kind.as_str() {
        "wasm" => {
            ensure!(
                bytes.len() <= limits.wasm_bytes.saturating_sub(output.wasm.len()),
                "guest Wasm exceeds budget"
            );
            output.wasm.extend_from_slice(&bytes);
        }
        "rust-diagnostics" | "link-diagnostics" => {
            ensure!(
                bytes.len()
                    <= limits.diagnostics_bytes.saturating_sub(
                        output.rust_diagnostics.len() + output.link_diagnostics.len()
                    ),
                "guest diagnostics exceed budget"
            );
            let destination = if kind == "rust-diagnostics" {
                &mut output.rust_diagnostics
            } else {
                &mut output.link_diagnostics
            };
            destination.extend_from_slice(&bytes);
        }
        "failure" => {
            ensure!(output.failure.is_none(), "duplicate guest failure");
            output.failure = Some(serde_json::from_slice(&bytes)?);
        }
        _ => bail!("unknown guest extraction channel; metadata is host-owned"),
    }
    Ok(())
}

#[cfg(test)]
fn generate_bindings(
    request: &BuildRequest,
    dependencies: &[String],
    limits: &BuildLimits,
) -> Result<String> {
    generate_resolved_bindings(
        &mut RequestedWorld::resolve(request, dependencies, limits)?,
        limits,
    )
}

fn generate_resolved_bindings(
    requested: &mut RequestedWorld,
    limits: &BuildLimits,
) -> Result<String> {
    let opts = wit_bindgen_rust::Opts {
        runtime_path: Some("crate::rt".into()),
        generate_all: true,
        ..Default::default()
    };
    let mut files = Files::default();
    opts.build()
        .generate(&mut requested.resolve, requested.world, &mut files)
        .context("Rust binding generation failed")?;
    let mut files = files.iter();
    let (_, bindings) = files.next().context("bindgen produced no source")?;
    ensure!(files.next().is_none(), "unexpected bindgen output layout");
    ensure!(
        bindings.len() <= limits.generated_bindings_bytes,
        "generated bindings exceed budget"
    );
    Ok(std::str::from_utf8(bindings)?.to_owned())
}

fn linker_exports(bindings: &str) -> Result<Vec<String>> {
    let mut exports = BTreeSet::new();
    let tokens: proc_macro2::TokenStream = bindings
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid generated Rust tokens"))?;
    collect_export_attributes(tokens, 0, &mut exports)?;
    ensure!(
        !exports.is_empty() && exports.len() <= 4096,
        "invalid generated export count"
    );
    Ok(exports.into_iter().collect())
}

fn collect_export_attributes(
    tokens: proc_macro2::TokenStream,
    depth: usize,
    exports: &mut BTreeSet<String>,
) -> Result<()> {
    use proc_macro2::{Delimiter, TokenTree};
    ensure!(depth <= 128, "generated Rust token nesting exceeds budget");
    let mut tokens = tokens.into_iter().peekable();
    while let Some(token) = tokens.next() {
        if matches!(&token, TokenTree::Punct(punctuation) if punctuation.as_char() == '#')
            && matches!(tokens.peek(), Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Bracket)
        {
            let Some(TokenTree::Group(attribute)) = tokens.next() else {
                unreachable!("peeked attribute group");
            };
            if let Some(name) = export_attribute(attribute.stream())? {
                ensure!(
                    name.len() <= 1024 && !name.chars().any(char::is_control),
                    "invalid generated export name"
                );
                exports.insert(name);
                ensure!(
                    exports.len() <= 4096,
                    "generated export count exceeds budget"
                );
            }
            continue;
        }
        if let TokenTree::Group(group) = token {
            collect_export_attributes(group.stream(), depth + 1, exports)?;
        }
    }
    Ok(())
}

fn export_attribute(tokens: proc_macro2::TokenStream) -> Result<Option<String>> {
    use proc_macro2::{Delimiter, TokenTree};
    let tokens: Vec<_> = tokens.into_iter().collect();
    if let [TokenTree::Ident(name), TokenTree::Group(inner)] = tokens.as_slice()
        && name == "unsafe"
        && inner.delimiter() == Delimiter::Parenthesis
    {
        return export_attribute(inner.stream());
    }
    if !matches!(tokens.first(), Some(TokenTree::Ident(name)) if name == "export_name") {
        return Ok(None);
    }
    let [_, TokenTree::Punct(equal), TokenTree::Literal(value)] = tokens.as_slice() else {
        bail!("unexpected generated export attribute");
    };
    ensure!(
        equal.as_char() == '=',
        "unexpected generated export attribute"
    );
    let literal = proc_macro2::TokenStream::from(TokenTree::Literal(value.clone()));
    Ok(Some(syn::parse2::<syn::LitStr>(literal)?.value()))
}

fn prepare_sources(staging: &Path, source: &str, bindings: &str) -> Result<()> {
    for (path, source) in RUNTIME_FILES {
        let path = staging.join(path);
        std::fs::create_dir_all(path.parent().context("runtime parent directory")?)?;
        std::fs::write(path, source)?;
    }
    std::fs::write(staging.join("bindings.rs"), bindings)?;
    let mut source = source.to_owned();
    source.push_str("\nextern crate alloc;\nextern crate self as wit_bindgen;\n#[allow(warnings)] pub mod rt;\n#[allow(warnings)] mod resource;\n#[allow(warnings)] mod bindings;\nmod support;\n");
    std::fs::write(staging.join("component.rs"), source)?;
    Ok(())
}

fn protocol_stdout() -> Result<File> {
    // SAFETY: dup creates an owned descriptor; dup2 redirects only this helper's
    // fd 1. Hyperlight's prints then flow to the parent's bounded stderr reader.
    unsafe {
        let fd: RawFd = libc::dup(libc::STDOUT_FILENO);
        ensure!(
            fd >= 0,
            "duplicate protocol stdout: {}",
            std::io::Error::last_os_error()
        );
        let output = File::from_raw_fd(fd);
        ensure!(
            libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) >= 0,
            "redirect guest stdout: {}",
            std::io::Error::last_os_error()
        );
        Ok(output)
    }
}

fn watchdog(duration: Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(duration);
        // Process termination also destroys an unresponsive VM. There is no
        // background guest to detach and no writable host guest output to flush.
        terminate(124);
    });
}

fn terminate(code: i32) -> ! {
    // No exit handlers or stdio locks: they may be held by a blocked SDK call.
    unsafe { libc::_exit(code) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_roots_include_every_function_and_post_return() {
        let mut request = crate::tests::request();
        request.wit = "package example:tool; world tool { export first: func() -> string; export second: func(a: u32) -> u32; }".into();
        let source = generate_bindings(&request, &[], &BuildLimits::default()).unwrap();
        let exports = linker_exports(&source).unwrap();
        assert!(exports.iter().any(|s| s == "first"));
        assert!(exports.iter().any(|s| s == "second"));
        assert!(
            exports
                .iter()
                .any(|s| s.contains("post") && s.contains("first"))
        );
    }

    #[test]
    fn documentation_and_string_examples_are_not_linker_export_roots() {
        let source = r##"
            /// export_name = "not-an-export"
            /// export_name = not a string
            #[doc = "#[unsafe(export_name = \"also-not-an-export\")]"]
            const EXAMPLE: &str = "export_name = \"not-real\"";
            macro_rules! export {
                () => { #[unsafe(export_name = "real-export")] fn run() {} };
            }
            #[export_name = r#"other-export"#] fn other() {}
        "##;
        assert_eq!(
            linker_exports(source).unwrap(),
            ["other-export", "real-export"]
        );
        let mut request = crate::tests::request();
        request.wit = "package example:tool; world tool {
            /// export_name = \"not-an-export\"
            /// export_name = not a string
            export run: func();
        }"
        .into();
        let generated = generate_bindings(&request, &[], &BuildLimits::default()).unwrap();
        assert_eq!(linker_exports(&generated).unwrap(), ["run"]);
    }

    #[test]
    fn only_guest_kernel_aborts_are_reported_as_guest_crashes() {
        use hyperlight_unikraft::hyperlight_host::HyperlightError;

        assert!(guest_crashed(&hyperlight_unikraft::Error::Hyperlight(
            HyperlightError::GuestAborted(0, String::new())
        )));
        assert!(!guest_crashed(&hyperlight_unikraft::Error::Deadlocked));
        assert!(!guest_crashed(&hyperlight_unikraft::Error::CallFailed {
            status: 1
        }));
        assert!(GUEST_CRASHED.contains("guest_scratch_mib"));
    }

    #[test]
    fn extraction_is_bounded_and_cannot_assert_evidence() {
        let mut output = GuestOutput::default();
        let limits = BuildLimits {
            wasm_bytes: 2,
            ..BuildLimits::default()
        };
        assert!(extract_chunk(&mut output, r#"["wasm","AQID"]"#, &limits).is_err());
        assert!(output.wasm.is_empty());
        assert!(extract_chunk(&mut output, r#"["evidence","e30="]"#, &limits).is_err());
        extract_chunk(&mut output, r#"["wasm","AQI="]"#, &limits).unwrap();
        assert!(extract_chunk(&mut output, r#"["wasm","AQ=="]"#, &limits).is_err());
        assert_eq!(output.wasm, vec![1, 2]);
    }

    #[test]
    fn async_layer_bindings_include_streams_and_callbacks() {
        let request = BuildRequest {
            component_name: "test:layer".into(),
            source: "struct Layer;".into(),
            wit: "package wassette:acp@7.0.0;
                interface agent { prompt: async func() -> stream<string>; }
                interface client { update: async func(); }
                world layer {
                    import agent; import client;
                    export agent; export client;
                }"
            .into(),
            world: "layer".into(),
            kind: ComponentKind::AcpLayer,
        };
        let bindings = generate_bindings(&request, &[], &BuildLimits::default()).unwrap();
        assert!(bindings.contains("async fn"));
        assert!(bindings.contains("StreamReader"));
        assert!(
            linker_exports(&bindings)
                .unwrap()
                .iter()
                .any(|name| name.contains("[callback]"))
        );
        let mut provider = request;
        provider.wit = provider.wit.replace("export client;", "");
        assert!(generate_bindings(&provider, &[], &BuildLimits::default()).is_err());
    }
}
