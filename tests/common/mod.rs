// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::path::PathBuf;

use anyhow::{Context, Result};

#[allow(dead_code)]
pub async fn build_fetch_component() -> Result<PathBuf> {
    let top_level =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").context("CARGO_MANIFEST_DIR not set")?);

    let component_path =
        top_level.join("examples/fetch-rs/target/wasm32-wasip2/release/fetch_rs.wasm");

    let status = tokio::process::Command::new("cargo")
        .current_dir(top_level.join("examples/fetch-rs"))
        .args(["build", "--release", "--target", "wasm32-wasip2"])
        .status()
        .await
        .context("Failed to execute cargo component build")?;

    if !status.success() {
        anyhow::bail!("Failed to compile fetch-rs component");
    }

    if !component_path.exists() {
        anyhow::bail!(
            "Component file not found after build: {}",
            component_path.display()
        );
    }

    Ok(component_path)
}

#[allow(dead_code)]
pub async fn build_filesystem_component() -> Result<PathBuf> {
    let top_level =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").context("CARGO_MANIFEST_DIR not set")?);

    let component_path =
        top_level.join("examples/filesystem-rs/target/wasm32-wasip2/release/filesystem.wasm");

    let status = tokio::process::Command::new("cargo")
        .current_dir(top_level.join("examples/filesystem-rs"))
        .args(["build", "--release", "--target", "wasm32-wasip2"])
        .status()
        .await
        .context("Failed to execute cargo component build")?;

    if !status.success() {
        anyhow::bail!("Failed to compile filesystem component");
    }

    if !component_path.exists() {
        anyhow::bail!(
            "Component file not found after build: {}",
            component_path.display()
        );
    }

    Ok(component_path)
}
