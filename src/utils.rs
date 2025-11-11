// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Utility functions for the wassette command

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use crate::registry;

mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

/// Parse environment variable in KEY=VALUE format
pub fn parse_env_var(s: &str) -> Result<(String, String), String> {
    match s.split_once('=') {
        Some((key, value)) => {
            if key.is_empty() {
                Err("Environment variable key cannot be empty".to_string())
            } else {
                Ok((key.to_string(), value.to_string()))
            }
        }
        None => Err("Environment variable must be in KEY=VALUE format".to_string()),
    }
}

/// Load environment variables from a file (supports .env format)
pub fn load_env_file(path: &PathBuf) -> Result<HashMap<String, String>, anyhow::Error> {
    use std::fs;

    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read environment file: {}", path.display()))?;

    let mut env_vars = HashMap::new();

    for (line_num, line) in content.lines().enumerate() {
        let line = line.trim();

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Parse KEY=VALUE format
        match line.split_once('=') {
            Some((key, value)) => {
                let key = key.trim();
                let value = value.trim();

                if key.is_empty() {
                    bail!("Empty environment variable key at line {}", line_num + 1);
                }

                // Handle quoted values
                let value = if (value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\''))
                {
                    &value[1..value.len() - 1]
                } else {
                    value
                };

                env_vars.insert(key.to_string(), value.to_string());
            }
            None => {
                bail!(
                    "Invalid environment variable format at line {}: {}",
                    line_num + 1,
                    line
                );
            }
        }
    }

    Ok(env_vars)
}

/// Load and parse the component registry JSON
pub fn load_component_registry() -> Result<Vec<registry::RegistryComponent>> {
    const COMPONENT_REGISTRY: &str = include_str!("../component-registry.json");
    registry::parse_registry(COMPONENT_REGISTRY).context("Failed to parse component registry")
}

/// Formats build information similar to agentgateway's version output
pub fn format_build_info() -> String {
    // Parse Rust version more robustly by looking for version pattern
    // Expected format: "rustc 1.88.0 (extra info)"
    let rust_version = built_info::RUSTC_VERSION
        .split_whitespace()
        .find(|part| part.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .unwrap_or("unknown");

    let build_profile = built_info::PROFILE;

    let build_status = if built_info::GIT_DIRTY.unwrap_or(false) {
        "Modified"
    } else {
        "Clean"
    };

    let git_tag = built_info::GIT_VERSION.unwrap_or("unknown");

    let git_revision = built_info::GIT_COMMIT_HASH.unwrap_or("unknown");
    let version = if built_info::GIT_DIRTY.unwrap_or(false) {
        format!("{git_revision}-dirty")
    } else {
        git_revision.to_string()
    };

    format!(
        "{} version.BuildInfo{{RustVersion:\"{}\", BuildProfile:\"{}\", BuildStatus:\"{}\", GitTag:\"{}\", Version:\"{}\", GitRevision:\"{}\"}}",
        built_info::PKG_VERSION,
        rust_version,
        build_profile,
        build_status,
        git_tag,
        version,
        git_revision
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_format_contains_required_fields() {
        let version_info = format_build_info();

        // Check that the version output contains expected components
        assert!(version_info.contains("version.BuildInfo"));
        assert!(version_info.contains("RustVersion"));
        assert!(version_info.contains("BuildProfile"));
        assert!(version_info.contains("BuildStatus"));
        assert!(version_info.contains("GitTag"));
        assert!(version_info.contains("Version"));
        assert!(version_info.contains("GitRevision"));
    }

    #[test]
    fn test_version_contains_cargo_version() {
        let version_info = format_build_info();
        // This test ensures the Homebrew formula test will pass by checking the version info contains package version
        assert!(version_info.contains(built_info::PKG_VERSION));
    }
}
