// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use wassette_builder::{
    BuildError, BuildErrorKind, BuildLimits, BuildRequest, Builder, BuilderConfig, ComponentKind,
    CrateDependency, RustCrate,
};

fn digest(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn config(stage: &Path, dependencies: Vec<String>) -> Result<BuilderConfig> {
    let helper_path =
        PathBuf::from(std::env::var("WASSETTE_BUILDER_HELPER").context("set signed helper path")?);
    let initrd_path = PathBuf::from(
        std::env::var("WASSETTE_BUILDER_INITRD").context("set existing immutable initrd path")?,
    );
    Ok(BuilderConfig {
        helper_sha256: digest(&helper_path)?,
        initrd_sha256: digest(&initrd_path)?,
        helper_path,
        initrd_path,
        staging_root: stage.into(),
        wit_dependencies: dependencies,
        rust_crates: vec![],
    })
}

fn validate(wasm: &[u8]) -> Result<()> {
    wasmparser::Validator::new_with_features(wasmparser::WasmFeatures::all()).validate_all(wasm)?;
    Ok(())
}

#[cfg(feature = "hyperlight")]
#[tokio::test]
async fn helper_returns_safe_wit_and_configuration_failures_without_booting() -> Result<()> {
    let root = tempfile::tempdir()?;
    let staging = tempfile::tempdir_in(root.path())?;
    let image = root.path().join("parser-only-image");
    std::fs::write(&image, b"not a bootable image")?;
    let helper = PathBuf::from(env!("CARGO_BIN_EXE_wassette-builder"));
    let config = BuilderConfig {
        helper_sha256: digest(&helper)?,
        initrd_sha256: digest(&image)?,
        helper_path: helper,
        initrd_path: image,
        staging_root: staging.path().into(),
        wit_dependencies: vec![],
        rust_crates: vec![],
    };
    let limits = BuildLimits {
        diagnostics_bytes: 1024,
        ..BuildLimits::default()
    };
    let builder = Builder::new(config.clone(), limits.clone())?;
    let request = BuildRequest {
        component_name: "test:diagnostic".into(),
        source: "PRIVATE_RUST_SOURCE_BODY".into(),
        wit: "package test:broken@1.0.0;\nworld tool { export run: func() } // PRIVATE_WIT_SOURCE_BODY".into(),
        world: "tool".into(),
        kind: ComponentKind::Tool,
    };
    let error = builder
        .build(request.clone(), CancellationToken::new())
        .await
        .unwrap_err();
    let failure = BuildError::from_error(&error).context("structured WIT failure")?;
    assert_eq!(failure.kind(), BuildErrorKind::InvalidWit);
    let message = failure.diagnostic().context("safe WIT diagnostic")?;
    assert!(
        message.contains("expected") && message.contains("WIT:2:"),
        "{message}"
    );
    assert!(message.len() <= 1024);
    assert!(!message.contains("PRIVATE_"));
    assert!(!message.contains(root.path().to_str().unwrap()));
    assert!(!format!("{error:#}").contains("expected"));
    assert_eq!(std::fs::read_dir(staging.path())?.count(), 0);

    let builder = Builder::new(
        BuilderConfig {
            initrd_sha256: "0".repeat(64),
            ..config
        },
        limits,
    )?;
    let error = builder
        .build(request, CancellationToken::new())
        .await
        .unwrap_err();
    let failure = BuildError::from_error(&error).context("structured configuration failure")?;
    assert_eq!(failure.kind(), BuildErrorKind::Unavailable);
    assert!(failure.diagnostic().is_none());
    assert!(!format!("{error:#}").contains(root.path().to_str().unwrap()));
    assert_eq!(std::fs::read_dir(staging.path())?.count(), 0);
    Ok(())
}

#[cfg(feature = "hyperlight")]
#[test]
fn parent_pipe_loss_terminates_helper_before_vm_boot() -> Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let stage = tempfile::tempdir()?;
    let image = stage.path().join("sparse-test-image");
    std::fs::File::create(&image)?.set_len(64 * 1024 * 1024)?;
    let helper = PathBuf::from(env!("CARGO_BIN_EXE_wassette-builder"));
    let job = serde_json::json!({
        "config": {
            "helper_path": helper,
            "helper_sha256": "0".repeat(64),
            "initrd_path": image,
            "initrd_sha256": "0".repeat(64),
            "staging_root": stage.path(),
            "wit_dependencies": [],
        },
        "limits": BuildLimits::default(),
        "request": {
            "component_name": "test:pipe",
            "source": "struct Component;",
            "wit": "package test:pipe; world tool { export run: func(); }",
            "world": "tool",
            "kind": "Tool",
        },
        "staging": stage.path(),
    });
    let bytes = serde_json::to_vec(&job)?;
    let mut child = Command::new(helper)
        .arg("--job-v3")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut pipe = child.stdin.take().context("parent pipe")?;
    pipe.write_all(b"WSBLD003")?;
    pipe.write_all(&(bytes.len() as u32).to_le_bytes())?;
    pipe.write_all(&bytes)?;
    drop(pipe);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child.try_wait()? {
            assert_eq!(status.code(), Some(125), "{status}");
            break;
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            anyhow::bail!("helper survived parent pipe loss");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires the packaged signed Hyperlight helper and existing Rust 1.98.1 initrd"]
async fn native_tool_multiple_exports_and_strings() -> Result<()> {
    let stage = tempfile::tempdir()?;
    let builder = Builder::new(config(stage.path(), vec![])?, BuildLimits::default())?;
    let result = builder
        .build(
            BuildRequest {
                component_name: "builder:smoke".into(),
                source: r#"
struct Component;
impl bindings::Guest for Component {
    fn add(a: i32, b: i32) -> i32 { a.wrapping_add(b) }
    fn greet(name: String) -> String { format!("hello {name}") }
}
bindings::export!(Component with_types_in bindings);
"#
                .into(),
                wit: "package test:smoke; world tool {
            /// export_name = \"not-an-export\"
            /// export_name = not a string
            export add: func(a: s32, b: s32) -> s32;
            export greet: func(name: string) -> string;
        }"
                .into(),
                world: "tool".into(),
                kind: ComponentKind::Tool,
            },
            CancellationToken::new(),
        )
        .await?;
    validate(&result.wasm)?;
    assert_eq!(result.evidence.component_name, "builder:smoke");
    assert_eq!(std::fs::read_dir(stage.path())?.count(), 0);
    if let Ok(path) = std::env::var("WASSETTE_BUILDER_SMOKE_OUTPUT") {
        std::fs::write(path, result.wasm)?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires the packaged signed Hyperlight helper and existing Rust 1.98.1 initrd"]
async fn native_compiler_errors_are_reported() -> Result<()> {
    let stage = tempfile::tempdir()?;
    let builder = Builder::new(config(stage.path(), vec![])?, BuildLimits::default())?;
    let error = builder
        .build(
            BuildRequest {
                component_name: "builder:error".into(),
                source: "compile_error!(\"guest-compiler-diagnostic-sentinel\");".into(),
                wit: "package test:error; world tool { export run: func(); }".into(),
                world: "tool".into(),
                kind: ComponentKind::Tool,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    let typed = error
        .downcast_ref::<BuildError>()
        .context("typed compiler error")?;
    assert_eq!(typed.kind(), BuildErrorKind::CompilationFailed);
    assert!(
        typed
            .diagnostic()
            .unwrap()
            .contains("guest-compiler-diagnostic-sentinel")
    );
    assert!(!format!("{error:#}").contains("guest-compiler-diagnostic-sentinel"));
    assert_eq!(std::fs::read_dir(stage.path())?.count(), 0);

    let builder = Builder::new(
        config(stage.path(), vec![])?,
        BuildLimits {
            diagnostics_bytes: 4096,
            ..BuildLimits::default()
        },
    )?;
    let error = builder
        .build(
            BuildRequest {
                component_name: "builder:diagnostic-limit".into(),
                source: format!("compile_error!(\"{}\");", "x".repeat(8192)),
                wit: "package test:error; world tool { export run: func(); }".into(),
                world: "tool".into(),
                kind: ComponentKind::Tool,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    let typed = error
        .downcast_ref::<BuildError>()
        .context("typed diagnostic-budget error")?;
    let message = typed.diagnostic().unwrap_or("");
    // This image's vfork can suspend the reader until the compiler exits.
    // A flood can therefore fill the finite guest pipe first; that must be a
    // failed build too, never a partial successful artifact or uncapped output.
    assert!(
        message.contains("budget") || message.contains("guest is deadlocked"),
        "{message}"
    );
    assert!(
        message.len() < 4096,
        "unbounded compiler error: {} bytes",
        message.len()
    );
    assert_eq!(std::fs::read_dir(stage.path())?.count(), 0);
    Ok(())
}

fn wit_package(path: &Path) -> Result<String> {
    let mut files = std::fs::read_dir(path)?
        .map(|file| Ok(file?.path()))
        .collect::<Result<Vec<_>>>()?;
    files.sort();
    let mut package = String::new();
    for file in files
        .into_iter()
        .filter(|file| file.extension().is_some_and(|e| e == "wit"))
    {
        let text = std::fs::read_to_string(file)?;
        if package.is_empty() {
            package.push_str(&text);
        } else if let Some((_, body)) = text.split_once(';') {
            package.push_str(body);
        } else {
            anyhow::bail!("missing WIT package header");
        }
        package.push('\n');
    }
    Ok(package)
}

#[tokio::test]
#[ignore = "requires the packaged signed Hyperlight helper and existing Rust 1.98.1 initrd"]
async fn native_canonical_acp_layer() -> Result<()> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .ancestors()
        .take(5)
        .find(|root| root.join("crates/wassette-acp/wit/acp/world.wit").is_file())
        .context("repository fixture root")?;
    let acp = std::env::var_os("WASSETTE_BUILDER_ACP_WIT")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("crates/wassette-acp/wit/acp"));
    let mut dependencies = vec![
        wit_package(&acp.join("deps/wasmcloud-secrets"))?,
        wit_package(&acp.join("deps/wassette-component-tools"))?,
        wit_package(&root.join("wit/component-generation"))?,
        wit_package(&acp)?,
    ];
    dependencies.retain(|s| !s.is_empty());
    let source = std::fs::read_to_string(root.join("components/acp-uppercase-layer/src/lib.rs"))?
        .replace("#[allow(clippy::all)]\nmod bindings;", "");
    let stage = tempfile::tempdir()?;
    let builder = Builder::new(config(stage.path(), dependencies)?, BuildLimits::default())?;
    let result = builder
        .build(
            BuildRequest {
                component_name: "builder:layer-smoke".into(),
                source,
                wit: "package test:smoke; world generated { include wassette:acp/layer@7.0.0; }"
                    .into(),
                world: "generated".into(),
                kind: ComponentKind::AcpLayer,
            },
            CancellationToken::new(),
        )
        .await?;
    validate(&result.wasm)?;
    assert_eq!(result.evidence.kind, ComponentKind::AcpLayer);
    if let Ok(path) = std::env::var("WASSETTE_BUILDER_LAYER_OUTPUT") {
        std::fs::write(path, result.wasm)?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires the packaged signed Hyperlight helper and Rust 1.98.1 initrd"]
async fn native_wasm_size_limit_is_invalid_output_not_unavailable() -> Result<()> {
    let stage = tempfile::tempdir()?;
    let builder = Builder::new(
        config(stage.path(), vec![])?,
        BuildLimits {
            wasm_bytes: 100,
            ..BuildLimits::default()
        },
    )?;
    let error = builder
        .build(
            BuildRequest {
                component_name: "builder:output-limit".into(),
                source: "struct Component; impl bindings::Guest for Component {
            fn run() -> u32 { 42 }
        } bindings::export!(Component with_types_in bindings);"
                    .into(),
                wit: "package test:limit; world tool { export run: func() -> u32; }".into(),
                world: "tool".into(),
                kind: ComponentKind::Tool,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    let error = wassette_builder::BuildError::from_error(&error).context("typed output error")?;
    assert_eq!(
        error.kind(),
        wassette_builder::BuildErrorKind::InvalidOutput
    );
    assert!(
        error
            .diagnostic()
            .context("output-limit diagnostic")?
            .contains("output budget")
    );
    assert_eq!(std::fs::read_dir(stage.path())?.count(), 0);
    Ok(())
}

fn crate_archive(dir: &Path, package: &str, lib: &str) -> Result<PathBuf> {
    let root = dir.join(package);
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(root.join("src/lib.rs"), lib)?;
    let archive = dir.join(format!("{package}.crate"));
    // Registry archives hold one top-level directory; omit macOS AppleDouble files.
    let status = std::process::Command::new("tar")
        .env("COPYFILE_DISABLE", "1")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(dir)
        .arg(package)
        .status()?;
    anyhow::ensure!(status.success(), "tar failed");
    Ok(archive)
}

#[tokio::test]
#[ignore = "requires the packaged signed Hyperlight helper and existing Rust 1.98.1 initrd"]
async fn native_pinned_rust_crates_link_into_request_source() -> Result<()> {
    let stage = tempfile::tempdir()?;
    let crates = tempfile::tempdir()?;
    let base = crate_archive(
        crates.path(),
        "pinned-base-0.1.0",
        r#"#[cfg(feature = "loud")]
pub fn word() -> &'static str { "PINNED" }"#,
    )?;
    let greeting = crate_archive(
        crates.path(),
        "pinned-greeting-0.1.0",
        "pub fn greet(name: &str) -> String { format!(\"{} {name}\", renamed_base::word()) }",
    )?;
    let mut config = config(stage.path(), vec![])?;
    config.rust_crates = vec![
        RustCrate {
            name: "pinned_base".into(),
            archive_sha256: digest(&base)?,
            archive_path: base,
            root: "src/lib.rs".into(),
            edition: "2021".into(),
            features: vec!["loud".into()],
            dependencies: vec![],
        },
        RustCrate {
            name: "pinned_greeting".into(),
            archive_sha256: digest(&greeting)?,
            archive_path: greeting,
            root: "src/lib.rs".into(),
            edition: "2021".into(),
            features: vec![],
            dependencies: vec![CrateDependency {
                krate: "pinned_base".into(),
                rename: Some("renamed_base".into()),
            }],
        },
    ];
    let builder = Builder::new(config, BuildLimits::default())?;
    let result = builder
        .build(
            BuildRequest {
                component_name: "builder:pinned-crates".into(),
                source: "struct Component;
impl bindings::Guest for Component {
    fn greet(name: String) -> String { pinned_greeting::greet(&name) }
}
bindings::export!(Component with_types_in bindings);"
                    .into(),
                wit: "package test:crates; world tool { export greet: func(name: string) -> string; }"
                    .into(),
                world: "tool".into(),
                kind: ComponentKind::Tool,
            },
            CancellationToken::new(),
        )
        .await?;
    validate(&result.wasm)?;
    assert!(
        result
            .wasm
            .windows(b"PINNED".len())
            .any(|window| window == b"PINNED")
    );
    assert_eq!(std::fs::read_dir(stage.path())?.count(), 0);
    Ok(())
}
