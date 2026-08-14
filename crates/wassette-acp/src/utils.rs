//! Small shared helpers for the host binary.

use anyhow::Result;

/// Namespace assigned to components loaded from a local filesystem path
/// (as opposed to a registry WIT name, which carries its own namespace).
/// Mirrors the host's own `local:host` identity.
pub const LOCAL_NAMESPACE: &str = "local";

/// Derive a component *name* (the part after the namespace) from a wasm
/// path. We use the file stem; renaming the binary therefore loses prior
/// data (acceptable for a sample). Restricted to a small alphabet to
/// avoid surprising filesystem behaviour.
pub fn component_name_from_path(path: &std::path::Path) -> Result<String> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("wasm path has no usable file stem: {}", path.display()))?;
    let ok = !stem.is_empty()
        && stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if !ok {
        anyhow::bail!(
            "wasm filename stem {stem:?} contains characters not allowed in a component name (allow [A-Za-z0-9._-])"
        );
    }
    Ok(stem.to_string())
}

/// Derive a component *identity* (`namespace:component-name`) from a
/// provider/layer argument.
///
/// The identity is what a component's private secret store and `/data`
/// preopen are keyed on, so it must be stable and distinguish components
/// that happen to share a name:
///
/// - A registry **WIT name** (`namespace:package[@version]`) keeps its
///   `namespace:package`; the version is stripped so a component's
///   secrets survive upgrades.
/// - A **filesystem path** has no registry namespace, so it becomes
///   `local:<filename-stem>` (see [`LOCAL_NAMESPACE`]).
///
/// `is_wit_name` is the caller's classification of `arg` (the host uses
/// `component_package_manager`'s `looks_like_wit_name`).
pub fn component_id_from_arg(arg: &str, is_wit_name: bool) -> Result<String> {
    if is_wit_name {
        // `namespace:package@version` → `namespace:package`.
        Ok(arg.split_once('@').map_or(arg, |(base, _)| base).to_string())
    } else {
        let name = component_name_from_path(std::path::Path::new(arg))?;
        Ok(format!("{LOCAL_NAMESPACE}:{name}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wit_name_keeps_namespace_and_strips_version() {
        assert_eq!(
            component_id_from_arg("yosh:ollama-provider@1.2.3", true).unwrap(),
            "yosh:ollama-provider"
        );
    }

    #[test]
    fn wit_name_without_version_is_unchanged() {
        assert_eq!(
            component_id_from_arg("wasmcloud:secrets", true).unwrap(),
            "wasmcloud:secrets"
        );
    }

    #[test]
    fn file_path_gets_local_namespace_from_stem() {
        assert_eq!(
            component_id_from_arg("target/wasm32-wasip2/release/ollama_provider.wasm", false)
                .unwrap(),
            "local:ollama_provider"
        );
    }

    #[test]
    fn file_path_bare_filename() {
        assert_eq!(
            component_id_from_arg("uppercase_layer.wasm", false).unwrap(),
            "local:uppercase_layer"
        );
    }

    #[test]
    fn file_path_with_bad_stem_is_rejected() {
        assert!(component_id_from_arg("weird:name.wasm", false).is_err());
    }
}
