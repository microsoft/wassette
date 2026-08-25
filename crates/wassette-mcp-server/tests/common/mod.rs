// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::path::PathBuf;
use std::sync::Once;
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use anyhow::{bail, ensure};
use anyhow::{Context, Result};
#[cfg(unix)]
use tokio::process::Child;

#[cfg(unix)]
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

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

#[cfg(unix)]
async fn force_shutdown(child: &mut Child) -> Result<()> {
    child
        .start_kill()
        .context("failed to force-kill the server process")?;
    child
        .wait()
        .await
        .context("failed to wait for the force-killed server process")?;
    Ok(())
}

#[cfg(unix)]
#[allow(dead_code)]
pub async fn shutdown_child(child: &mut Child) -> Result<()> {
    let pid = child.id().context("server process has no process ID")?;
    let signal_status = std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .context("failed to send SIGTERM to the server process")?;

    if !signal_status.success() {
        force_shutdown(child).await?;
        bail!("failed to send SIGTERM to server process {pid}: {signal_status}");
    }

    let status = match tokio::time::timeout(SHUTDOWN_TIMEOUT, child.wait()).await {
        Ok(result) => result.context("failed to wait for the server process")?,
        Err(_) => {
            force_shutdown(child).await?;
            bail!(
                "server process {pid} did not exit within {} seconds after SIGTERM",
                SHUTDOWN_TIMEOUT.as_secs()
            );
        }
    };

    ensure!(
        status.success(),
        "server process {pid} exited unsuccessfully after SIGTERM: {status}"
    );
    Ok(())
}

#[cfg(not(unix))]
#[allow(dead_code)]
pub async fn shutdown_child(child: &mut tokio::process::Child) -> Result<()> {
    child
        .kill()
        .await
        .context("failed to stop the server process")
}
