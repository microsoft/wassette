// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::path::PathBuf;
use std::sync::Once;

use anyhow::{Context, Result};

static FETCH_COMPONENT_BUILD: Once = Once::new();
static FILESYSTEM_COMPONENT_BUILD: Once = Once::new();

/// Ensure fetch-rs component is built exactly once for all tests
fn ensure_fetch_component_built() -> Result<()> {
    FETCH_COMPONENT_BUILD.call_once(|| {
        let result = std::panic::catch_unwind(|| {
            let top_level = PathBuf::from(
                std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"),
            )
            .join("..")
            .join("..");

            // Use std::process::Command instead of tokio::process::Command to avoid runtime issues
            let status = std::process::Command::new("cargo")
                .current_dir(top_level.join("examples/fetch-rs"))
                .args(["build", "--release", "--target", "wasm32-wasip2"])
                .status()
                .expect("Failed to execute cargo component build");

            if !status.success() {
                panic!("Failed to compile fetch-rs component");
            }
        });

        if result.is_err() {
            panic!("Failed to build fetch-rs component in Once block");
        }
    });

    Ok(())
}

#[allow(dead_code)]
pub async fn build_fetch_component() -> Result<PathBuf> {
    let top_level =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").context("CARGO_MANIFEST_DIR not set")?)
            .join("..")
            .join("..");

    let component_path =
        top_level.join("examples/fetch-rs/target/wasm32-wasip2/release/fetch_rs.wasm");

    // Ensure component is built exactly once across all tests
    ensure_fetch_component_built()?;

    if !component_path.exists() {
        anyhow::bail!(
            "Component file not found after build: {}",
            component_path.display()
        );
    }

    Ok(component_path)
}

/// Ensure filesystem-rs component is built exactly once for all tests
fn ensure_filesystem_component_built() -> Result<()> {
    FILESYSTEM_COMPONENT_BUILD.call_once(|| {
        let result = std::panic::catch_unwind(|| {
            let top_level = PathBuf::from(
                std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"),
            )
            .join("..")
            .join("..");

            // Use std::process::Command instead of tokio::process::Command to avoid runtime issues
            let status = std::process::Command::new("cargo")
                .current_dir(top_level.join("examples/filesystem-rs"))
                .args(["build", "--release", "--target", "wasm32-wasip2"])
                .status()
                .expect("Failed to execute cargo component build");

            if !status.success() {
                panic!("Failed to compile filesystem component");
            }
        });

        if result.is_err() {
            panic!("Failed to build filesystem component in Once block");
        }
    });

    Ok(())
}

#[allow(dead_code)]
pub async fn build_filesystem_component() -> Result<PathBuf> {
    let top_level =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").context("CARGO_MANIFEST_DIR not set")?)
            .join("..")
            .join("..");

    let component_path =
        top_level.join("examples/filesystem-rs/target/wasm32-wasip2/release/filesystem.wasm");

    // Ensure component is built exactly once across all tests
    ensure_filesystem_component_built()?;

    if !component_path.exists() {
        anyhow::bail!(
            "Component file not found after build: {}",
            component_path.display()
        );
    }

    Ok(component_path)
}
