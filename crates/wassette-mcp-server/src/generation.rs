// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Builder image discovery and one-shot CLI adapter for isolated generation.

use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use mcp_server::generation::{error_report, read_request, GenerationJobs};
use tokio_util::sync::CancellationToken;
use wassette::generation::{GenerationConfig, GenerationRequest, GenerationTarget};
use wassette::LifecycleManager;

use crate::commands::Run;
use crate::config::Config;
use crate::format::{print_value, OutputFormat};

/// Configure generation on the same manager used for normal server operations.
pub fn configure(manager: &LifecycleManager) -> Result<()> {
    let Some(mut config) = GenerationConfig::discover()? else {
        tracing::info!(
            image = %GenerationConfig::image_path()?.display(),
            "Component generation unavailable; install the builder image at this path"
        );
        return Ok(());
    };
    config.builder.wit_dependencies = wassette_acp::generation_wit_dependencies();
    let service = config
        .into_service()?
        .with_validator(wassette_acp::generation_validator(
            manager.component_root().to_path_buf(),
        )?);
    manager.enable_generation(service)
}

/// Build and install a bounded request using the discovered builder image.
pub async fn component_build(
    request_path: &Path,
    component_dir: Option<PathBuf>,
    emit_source: Option<&Path>,
) -> Result<()> {
    match build(request_path, component_dir, emit_source).await {
        Ok(report) => print_value(&report, OutputFormat::Json),
        Err(error) => {
            let report = if let Some(failure) = error.downcast_ref::<SourceExportFailure>() {
                json_source_export_failure(failure)
            } else {
                error_report(&error)
            };
            print_value(&report, OutputFormat::Json)?;
            bail!("Component generation did not complete successfully; see the JSON report for the actual commit status")
        }
    }
}

#[derive(Debug)]
struct SourceExportFailure {
    commit_report: serde_json::Value,
    source: anyhow::Error,
}

impl std::fmt::Display for SourceExportFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "source export failed after component commit: {}",
            self.source
        )
    }
}

impl std::error::Error for SourceExportFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

fn json_source_export_failure(failure: &SourceExportFailure) -> serde_json::Value {
    let mut report = failure.commit_report.clone();
    report["status"] = serde_json::json!("committed-source-export-failed");
    report["error"] = serde_json::json!(failure.source.to_string());
    report["next_step"] = serde_json::json!(
        "The component is installed; recover local files with `wassette component source` instead of rebuilding."
    );
    report
}

async fn build(
    request_path: &Path,
    component_dir: Option<PathBuf>,
    emit_source: Option<&Path>,
) -> Result<serde_json::Value> {
    let config = Config::from_run(
        &Run {
            component_dir,
            local_component_dir: None,
            local_components: None,
            env_vars: Vec::new(),
            env_file: None,
            disable_builtin_tools: false,
        },
        None,
    )?;
    let manager = crate::cli_handlers::create_configured_lifecycle_manager(config).await?;
    configure(&manager)?;
    if manager.generation_service().is_err() {
        eprintln!(
            "Component generation unavailable; install the builder image at {}",
            GenerationConfig::image_path()?.display()
        );
    }
    let request = read_request(
        std::fs::File::open(request_path).context("opening component generation request")?,
    )?;
    if let Some(path) = emit_source {
        check_output_directory(path, false)?;
    }
    let emitted = emit_source.map(|_| request.build.clone());
    let jobs = GenerationJobs::default();
    let cancel = CancellationToken::new();
    let operation = jobs.build(&manager, request, cancel.clone());
    tokio::pin!(operation);
    let result = tokio::select! {
        result = &mut operation => result,
        signal = tokio::signal::ctrl_c() => {
            cancel.cancel();
            let result = operation.await;
            signal.context("installing interrupt handler")?;
            result
        }
    };
    jobs.shutdown().await;
    let report = result?;
    if let (Some(path), Some(source)) = (emit_source, emitted) {
        write_source_layout(path, &source, false).map_err(|source| SourceExportFailure {
            commit_report: report.clone(),
            source,
        })?;
    }
    Ok(report)
}

/// Retrieve a retained request from the authoritative component store.
pub async fn component_source(
    id: &str,
    revision: Option<&str>,
    out: Option<&Path>,
    force: bool,
    component_dir: Option<PathBuf>,
) -> Result<()> {
    let config = Config::from_run(
        &Run {
            component_dir,
            local_component_dir: None,
            local_components: None,
            env_vars: Vec::new(),
            env_file: None,
            disable_builtin_tools: false,
        },
        None,
    )?;
    let store = wassette::store::ComponentStore::open(config.component_dir)?;
    let id = id.to_owned();
    let revision = revision.map(str::to_owned);
    let source = tokio::task::spawn_blocking(move || {
        let receipt = store.read(&id)?.receipt;
        if let Some(expected) = revision {
            ensure!(
                receipt.revision.to_string() == expected,
                "component revision does not match"
            );
        }
        Ok::<_, anyhow::Error>(store.read_source(&id, Some(&receipt.revision))?)
    })
    .await??;
    if let Some(out) = out {
        write_source_layout(out, &source, force)?;
    } else {
        let request = rebuildable_request(source);
        print_value(&serde_json::to_value(request)?, OutputFormat::Json)?;
    }
    Ok(())
}

fn rebuildable_request(build: wassette::generation::BuildRequest) -> GenerationRequest {
    GenerationRequest {
        build,
        target: GenerationTarget::New,
        reinstall_policy: None,
    }
}

fn write_source_layout(
    directory: &Path,
    source: &wassette::generation::BuildRequest,
    force: bool,
) -> Result<()> {
    check_output_directory(directory, force)?;
    let src = directory.join("src");
    let wit = directory.join("wit");
    let files = [
        src.join("lib.rs"),
        wit.join("world.wit"),
        directory.join("request.json"),
    ];
    for subdir in [&src, &wit] {
        if subdir.exists() || subdir.is_symlink() {
            ensure!(
                subdir.is_dir() && !subdir.is_symlink(),
                "source output subdirectory is not a real directory"
            );
        }
    }
    for path in &files {
        if path.exists() || path.is_symlink() {
            ensure!(
                force && path.is_file() && !path.is_symlink(),
                "refusing to overwrite source output {}",
                path.display()
            );
        }
    }
    if !directory.exists() {
        std::fs::create_dir_all(directory).context("creating source output directory")?;
    }
    for subdir in [&src, &wit] {
        if !subdir.exists() {
            std::fs::create_dir(subdir).context("creating source output subdirectory")?;
        }
    }
    let request = serde_json::to_vec_pretty(&rebuildable_request(source.clone()))?;
    for (path, bytes) in [
        (&files[0], source.source.as_bytes()),
        (&files[1], source.wit.as_bytes()),
        (&files[2], request.as_slice()),
    ] {
        std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

fn check_output_directory(directory: &Path, force: bool) -> Result<()> {
    if directory.exists() || directory.is_symlink() {
        ensure!(
            directory.is_dir() && !directory.is_symlink(),
            "output must be a real directory"
        );
        ensure!(
            force || std::fs::read_dir(directory)?.next().is_none(),
            "output directory is not empty (pass --force to overwrite)"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use sha2::Digest;
    use wassette::store::{
        ComponentStore, GenerationEvidence, InstallOptions, InstallOwner, OriginEvidence,
        PolicyProvenance, PreparedInstall, PreparedPolicy, SourceIdentity, ValidationEvidence,
    };
    use wassette::StorageKey;

    use super::*;

    #[test]
    fn source_layout_round_trips_and_requires_force_for_nonempty_directory() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("source");
        let build = wassette::generation::BuildRequest {
            component_name: "example:generated".into(),
            source: "struct Component;".into(),
            wit: "package example:generated; world tool {}".into(),
            world: "tool".into(),
            kind: wassette::generation::ComponentKind::Tool,
        };
        write_source_layout(&output, &build, false).unwrap();
        assert_eq!(
            fs::read_to_string(output.join("src/lib.rs")).unwrap(),
            build.source
        );
        assert_eq!(
            fs::read_to_string(output.join("wit/world.wit")).unwrap(),
            build.wit
        );
        let request = read_request(fs::File::open(output.join("request.json")).unwrap()).unwrap();
        assert_eq!(request.build.source, build.source);
        assert_eq!(request.build.wit, build.wit);
        assert_eq!(
            sha2::Sha256::digest(request.build.source.as_bytes()),
            sha2::Sha256::digest(build.source.as_bytes()),
        );
        assert!(write_source_layout(&output, &build, false).is_err());
        write_source_layout(&output, &build, true).unwrap();
        let outside = root.path().join("outside");
        fs::write(&outside, "safe").unwrap();
        fs::remove_file(output.join("src/lib.rs")).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, output.join("src/lib.rs")).unwrap();
            assert!(write_source_layout(&output, &build, true).is_err());
            assert_eq!(fs::read_to_string(outside).unwrap(), "safe");
        }
    }

    #[test]
    fn failed_local_export_keeps_the_committed_receipt_in_the_report() {
        let failure = SourceExportFailure {
            commit_report: json!({
                "status": "installed",
                "component_id": "example:generated",
                "revision": "exact-revision",
                "commit": { "cursor": 1 }
            }),
            source: anyhow::anyhow!("cannot write source"),
        };
        let report = json_source_export_failure(&failure);
        assert_eq!(report["status"], "committed-source-export-failed");
        assert_eq!(report["component_id"], "example:generated");
        assert_eq!(report["revision"], "exact-revision");
        assert_eq!(report["commit"], json!({ "cursor": 1 }));
        assert!(report["error"].as_str().unwrap().contains("cannot write"));
    }

    #[tokio::test]
    async fn source_command_checks_revision_and_output_overwrite() {
        let root = tempfile::tempdir().unwrap();
        let component_dir = root.path().join("components");
        let store = ComponentStore::open(&component_dir).unwrap();
        let build = wassette::generation::BuildRequest {
            component_name: "example:generated".into(),
            source: "struct Component;".into(),
            wit: "package example:generated; world tool {}".into(),
            world: "tool".into(),
            kind: wassette::generation::ComponentKind::Tool,
        };
        let sha = |bytes: &[u8]| hex::encode(sha2::Sha256::digest(bytes));
        let evidence: GenerationEvidence = serde_json::from_value(json!({
            "component_name": build.component_name,
            "source_sha256": sha(build.source.as_bytes()),
            "wit_sha256": sha(build.wit.as_bytes()),
            "wit_dependencies_sha256": "a".repeat(64),
            "builder_initrd_sha256": "a".repeat(64),
            "builder_helper_sha256": "a".repeat(64),
            "builder_manifest_digest": null,
            "profile": "test-profile",
            "profile_sha256": "a".repeat(64),
            "compiler": "rustc",
            "bindgen": "bindgen",
            "binding_runtime": "runtime",
            "vm_runtime": "vm",
            "world": build.world,
            "target": "wasm32-wasip2",
            "host_platform": "test"
        }))
        .unwrap();
        let source = SourceIdentity::Generated { id: "a".repeat(32) };
        let key = StorageKey::parse(&format!("generated_{}", "a".repeat(32))).unwrap();
        let expected = store.observe(&build.component_name, &key, &source).unwrap();
        let prepared = PreparedInstall::prepare(
                wat::parse_str(r#"(component $"example:generated" (instance $empty) (export "empty" (instance $empty)))"#).unwrap(),
                InstallOptions {
                    storage_key: key,
                    origin: OriginEvidence {
                        location: format!("generated://{}", "a".repeat(32)),
                        requested_version: None,
                        selected_version: None,
                        manifest_digest: None,
                        immutable_uri: None,
                        generation: Some(evidence),
                    },
                    source,
                    owner: InstallOwner::Explicit,
                    policy: PreparedPolicy::absent(PolicyProvenance::Default),
                    observation: None,
                },
                |_, _, _| Ok(ValidationEvidence::OrdinaryPrepared { runtime: "test".into() }),
            ).unwrap();
        let committed = store
            .commit_generated_install(prepared, expected, Some(build.clone()))
            .unwrap();
        let output = root.path().join("export");
        assert!(component_source(
            &build.component_name,
            Some("stale"),
            Some(&output),
            false,
            Some(component_dir.clone())
        )
        .await
        .is_err());
        component_source(
            &build.component_name,
            Some(&committed.entry.revision().to_string()),
            Some(&output),
            false,
            Some(component_dir.clone()),
        )
        .await
        .unwrap();
        assert!(component_source(
            &build.component_name,
            None,
            Some(&output),
            false,
            Some(component_dir.clone())
        )
        .await
        .is_err());
        component_source(
            &build.component_name,
            None,
            Some(&output),
            true,
            Some(component_dir),
        )
        .await
        .unwrap();
        let request = read_request(fs::File::open(output.join("request.json")).unwrap()).unwrap();
        assert_eq!(
            sha(request.build.source.as_bytes()),
            sha(build.source.as_bytes())
        );
    }
}
