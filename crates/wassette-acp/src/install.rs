// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Resolution of `--provider` / `--layer` arguments to on-disk wasm
//! components.
//!
//! ACP components live in the *same* store as Wassette's MCP components:
//! the Wassette component directory (`--component-dir`, defaulting to
//! `$XDG_DATA_HOME/wassette/components`). Downloads go through
//! [`wassette::loader`], so `wassette acp --provider` accepts exactly the
//! references `wassette component load` does:
//!
//! | Argument | Meaning |
//! | --- | --- |
//! | `./agent.wasm`, `/abs/agent.wasm`, `file:///abs/agent.wasm` | a local file, used in place |
//! | `oci://ghcr.io/org/agent:0.1.0` | pulled from a registry into the component dir |
//! | `https://example.com/agent.wasm` | downloaded into the component dir |
//! | `agent` | a component already in the component dir (`agent.wasm`) |
//!
//! The **component id** is the Wassette component id — the `.wasm` file
//! stem — and is what scopes a stage's `/data` directory and its secrets,
//! so `wassette secret set <id> KEY=…` and `wassette acp --provider <id>`
//! agree on the name.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use tokio::sync::mpsc::Sender;

/// A provider or layer argument resolved to a concrete component.
#[derive(Debug, Clone)]
pub struct ResolvedComponent {
    /// Wassette component id (the `.wasm` file stem). Keys the stage's
    /// secret store and its `/data` directory.
    pub component_id: String,
    /// Path to the component on disk.
    pub path: PathBuf,
}

/// How a CLI argument names a component.
#[derive(Debug, PartialEq, Eq)]
enum Reference<'a> {
    /// A remote URI understood by [`wassette::loader`] (`oci://`,
    /// `https://`) or an explicit `file://` path.
    Uri(&'a str),
    /// A filesystem path.
    Path(&'a str),
    /// A component id already present in the component directory.
    Id(&'a str),
}

/// Classify a `--provider` / `--layer` argument.
///
/// Anything with a `<scheme>://` prefix is a URI and handed to the
/// loader. Otherwise an argument that looks like a path (contains a
/// separator, ends in `.wasm`, or exists on disk) is a local file; what
/// remains is a component id to look up in the component directory.
fn classify(arg: &str) -> Result<Reference<'_>> {
    if let Some((scheme, _)) = arg.split_once("://") {
        return match scheme {
            "file" | "oci" | "https" => Ok(Reference::Uri(arg)),
            other => Err(anyhow!(
                "unsupported component scheme `{other}://`; expected `oci://`, `https://`, \
                 `file://`, a filesystem path, or a component id"
            )),
        };
    }
    let looks_like_path = arg.contains(std::path::MAIN_SEPARATOR)
        || arg.contains('/')
        || arg.ends_with(".wasm")
        || Path::new(arg).exists();
    if looks_like_path {
        Ok(Reference::Path(arg))
    } else {
        Ok(Reference::Id(arg))
    }
}

/// Reject ids that would escape the component directory or collide with
/// the secret store's per-component file naming.
fn validate_component_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && id != "."
        && id != "..";
    if !ok {
        anyhow::bail!("`{id}` is not a valid component id (allowed characters: [A-Za-z0-9._-])");
    }
    Ok(())
}

/// Resolves component references against a Wassette component directory.
pub struct Resolver {
    component_dir: PathBuf,
}

impl Resolver {
    /// Resolve against `component_dir`, the Wassette component store.
    pub fn new(component_dir: impl Into<PathBuf>) -> Self {
        Self {
            component_dir: component_dir.into(),
        }
    }

    /// The component directory this resolver reads from and downloads into.
    pub fn component_dir(&self) -> &Path {
        &self.component_dir
    }

    /// Resolve `arg` to a component on disk, downloading it if needed.
    pub async fn resolve(&self, arg: &str) -> Result<ResolvedComponent> {
        self.resolve_with_progress(arg, None).await
    }

    /// Like [`Resolver::resolve`] but emits coarse phase messages on
    /// `progress` when set. Used by the host-side `/install` slash
    /// command to drive an ACP tool-call progress card; send failures are
    /// ignored so reporting never blocks resolution.
    pub async fn resolve_with_progress(
        &self,
        arg: &str,
        progress: Option<Sender<String>>,
    ) -> Result<ResolvedComponent> {
        let report = |msg: String| {
            if let Some(tx) = progress.as_ref() {
                let _ = tx.try_send(msg);
            }
        };

        match classify(arg)? {
            Reference::Id(id) => {
                validate_component_id(id)?;
                let path = self.component_dir.join(format!("{id}.wasm"));
                if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
                    anyhow::bail!(
                        "no component `{id}` in {}; load it first (`wassette component load \
                         <oci://…|https://…|file://…>`) or pass a path or URI",
                        self.component_dir.display()
                    );
                }
                report(format!("Using `{id}`."));
                Ok(ResolvedComponent {
                    component_id: id.to_string(),
                    path,
                })
            }
            Reference::Path(path) => {
                // The loader requires absolute paths; relative CLI
                // arguments are the common case, so anchor them at the
                // current directory before handing them over.
                let abs = std::path::absolute(path)
                    .with_context(|| format!("resolving component path `{path}`"))?;
                report(format!("Loading `{}`…", abs.display()));
                self.fetch(&format!("file://{}", abs.display())).await
            }
            Reference::Uri(uri) => {
                report(format!("Fetching `{uri}`…"));
                let resolved = self.fetch(uri).await?;
                report(format!("Fetched `{}`.", resolved.component_id));
                Ok(resolved)
            }
        }
    }

    /// Hand `uri` to [`wassette::loader`]. Remote artifacts land in the
    /// component directory; local files are used where they are.
    async fn fetch(&self, uri: &str) -> Result<ResolvedComponent> {
        let (component_id, path) = wassette::loader::fetch_component(uri, &self.component_dir)
            .await
            .with_context(|| format!("fetching component `{uri}`"))?;
        validate_component_id(&component_id)
            .with_context(|| format!("deriving a component id from `{uri}`"))?;
        Ok(ResolvedComponent { component_id, path })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemes_are_uris() {
        assert_eq!(
            classify("oci://ghcr.io/org/agent:0.1.0").unwrap(),
            Reference::Uri("oci://ghcr.io/org/agent:0.1.0")
        );
        assert_eq!(
            classify("https://example.com/agent.wasm").unwrap(),
            Reference::Uri("https://example.com/agent.wasm")
        );
        assert_eq!(
            classify("file:///tmp/agent.wasm").unwrap(),
            Reference::Uri("file:///tmp/agent.wasm")
        );
    }

    #[test]
    fn unknown_scheme_is_rejected() {
        assert!(classify("ftp://example.com/agent.wasm").is_err());
    }

    #[test]
    fn paths_are_paths() {
        assert_eq!(
            classify("./target/agent.wasm").unwrap(),
            Reference::Path("./target/agent.wasm")
        );
        assert_eq!(
            classify("agent.wasm").unwrap(),
            Reference::Path("agent.wasm")
        );
    }

    #[test]
    fn bare_names_are_ids() {
        assert_eq!(
            classify("acp-echo-provider").unwrap(),
            Reference::Id("acp-echo-provider")
        );
    }

    #[test]
    fn traversal_ids_are_rejected() {
        assert!(validate_component_id("../etc/passwd").is_err());
        assert!(validate_component_id("..").is_err());
        assert!(validate_component_id("").is_err());
    }

    #[tokio::test]
    async fn missing_id_reports_the_component_dir() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = Resolver::new(dir.path());
        let err = resolver.resolve("nope").await.unwrap_err().to_string();
        assert!(err.contains("no component `nope`"), "{err}");
        assert!(err.contains(&dir.path().display().to_string()), "{err}");
    }

    #[tokio::test]
    async fn id_resolves_against_the_component_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.wasm");
        tokio::fs::write(&path, b"\0asm").await.unwrap();
        let resolver = Resolver::new(dir.path());
        let resolved = resolver.resolve("agent").await.unwrap();
        assert_eq!(resolved.component_id, "agent");
        assert_eq!(resolved.path, path);
    }
}
