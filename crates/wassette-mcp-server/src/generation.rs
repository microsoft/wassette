// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Operator configuration and one-shot CLI adapter for isolated generation.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use mcp_server::generation::{error_report, read_request, GenerationJobs};
use tokio_util::sync::CancellationToken;
use wassette::generation::GenerationConfig;
use wassette::LifecycleManager;

use crate::commands::Run;
use crate::config::Config;
use crate::format::{print_value, OutputFormat};

/// Configure generation on the same manager used for normal server operations.
pub fn configure(manager: &LifecycleManager, profile: Option<&Path>) -> Result<()> {
    let Some(profile) = profile else {
        return Ok(());
    };
    let service = GenerationConfig::read(profile)?
        .into_service()?
        .with_validator(wassette_acp::generation_validator(
            manager.component_root().to_path_buf(),
        )?);
    manager.enable_generation(service)
}

/// Build and install a bounded request using only the resolved operator profile.
pub async fn component_build(
    request_path: &Path,
    component_dir: Option<PathBuf>,
    generation_config: Option<PathBuf>,
) -> Result<()> {
    match build(request_path, component_dir, generation_config).await {
        Ok(report) => print_value(&report, OutputFormat::Json),
        Err(error) => {
            print_value(&error_report(&error), OutputFormat::Json)?;
            bail!("Component generation did not complete successfully; see the JSON report for the actual commit status")
        }
    }
}

async fn build(
    request_path: &Path,
    component_dir: Option<PathBuf>,
    generation_config: Option<PathBuf>,
) -> Result<serde_json::Value> {
    let config = Config::from_run(
        &Run {
            component_dir,
            generation_config,
            local_component_dir: None,
            local_components: None,
            env_vars: Vec::new(),
            env_file: None,
            disable_builtin_tools: false,
        },
        None,
    )?;
    let profile = config
        .generation_config
        .clone()
        .context("component build requires --generation-config, WASSETTE_GENERATION_CONFIG, or generation_config in config.toml")?;
    let manager = crate::cli_handlers::create_configured_lifecycle_manager(config).await?;
    configure(&manager, Some(&profile))?;
    let request = read_request(
        std::fs::File::open(request_path).context("opening component generation request")?,
    )?;
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
    result
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::{json, Value};
    use wassette::generation::BuildLimits;

    use super::*;

    fn reference_profile() -> Value {
        let reference = include_str!("../../../docs/reference/configuration-files.md");
        let section = reference
            .split_once("**Operator profile example:**")
            .unwrap()
            .1;
        let example = section.split_once("```json\n").unwrap().1;
        serde_json::from_str(example.split_once("\n```").unwrap().0).unwrap()
    }

    #[test]
    fn reference_profile_requires_every_host_owned_builder_field() {
        let profile = reference_profile();
        let config: GenerationConfig = serde_json::from_value(profile.clone()).unwrap();
        assert_eq!(config.limits, BuildLimits::default());
        assert!(config.callers.is_empty());
        assert!(config.allow_build && config.allow_install);
        assert!(!config.allow_expose && !config.allow_rebuild);
        for field in [
            "helper_path",
            "helper_sha256",
            "initrd_path",
            "initrd_sha256",
            "staging_root",
            "wit_dependencies",
        ] {
            let mut missing = profile.clone();
            missing["builder"].as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<GenerationConfig>(missing).is_err(),
                "{field} must remain required"
            );
        }
    }

    #[test]
    fn operator_paths_resolve_relative_to_profile_and_dependencies_remain_inline() {
        let root = tempfile::tempdir_in(".").unwrap();
        let profile_dir = root.path().join("operator");
        fs::create_dir(&profile_dir).unwrap();
        let path = profile_dir.join("generation.json");
        let mut profile = reference_profile();
        let dependency = format!(
            "package trusted:bindings;\n// {}\ninterface helper {{}}\n",
            "p".repeat(70 * 1024)
        );
        profile["builder"]["wit_dependencies"] = json!([dependency]);
        fs::write(&path, serde_json::to_vec(&profile).unwrap()).unwrap();
        let config = GenerationConfig::read(&path).unwrap();
        let base = std::path::absolute(&profile_dir).unwrap();
        assert_eq!(
            config.builder.helper_path,
            base.join("helper/wassette-builder")
        );
        assert_eq!(
            config.builder.initrd_path,
            base.join("images/rust-builder.initrd")
        );
        assert_eq!(config.builder.staging_root, base.join("staging"));
        assert_eq!(config.builder.wit_dependencies, vec![dependency]);

        profile["builder"]["helper_path"] = json!(base.join("absolute-helper"));
        profile["builder"]["initrd_path"] = json!(base.join("absolute-initrd"));
        profile["builder"]["staging_root"] = json!(base.join("absolute-staging"));
        fs::write(&path, serde_json::to_vec(&profile).unwrap()).unwrap();
        let config = GenerationConfig::read(&path).unwrap();
        assert_eq!(config.builder.helper_path, base.join("absolute-helper"));
        assert_eq!(config.builder.initrd_path, base.join("absolute-initrd"));
        assert_eq!(config.builder.staging_root, base.join("absolute-staging"));
    }

    #[test]
    fn operator_profile_accepts_one_mib_but_not_an_extra_byte() {
        const MAX_PROFILE_BYTES: usize = 1024 * 1024;
        let root = tempfile::tempdir_in(".").unwrap();
        let path = root.path().join("generation.json");
        let mut bytes = serde_json::to_vec(&reference_profile()).unwrap();
        bytes.resize(MAX_PROFILE_BYTES, b' ');
        fs::write(&path, &bytes).unwrap();
        assert!(GenerationConfig::read(&path).is_ok());
        bytes.push(b' ');
        fs::write(&path, bytes).unwrap();
        assert!(GenerationConfig::read(&path).is_err());
    }

    #[test]
    fn explicit_limits_require_the_complete_eight_field_object() {
        let mut profile = reference_profile();
        profile["limits"] = serde_json::to_value(BuildLimits::default()).unwrap();
        let config: GenerationConfig = serde_json::from_value(profile.clone()).unwrap();
        config.limits.validate().unwrap();
        for field in [
            "source_bytes",
            "wit_bytes",
            "wasm_bytes",
            "diagnostics_bytes",
            "wall_time_ms",
            "guest_scratch_mib",
            "generated_bindings_bytes",
            "max_parallel_jobs",
        ] {
            let mut missing = profile.clone();
            missing["limits"].as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<GenerationConfig>(missing).is_err(),
                "{field} must remain required when limits is present"
            );
        }
    }
}
