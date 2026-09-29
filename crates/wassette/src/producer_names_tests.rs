// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result};
use wasm_encoder::{Component, ComponentNameSection};

use crate::{inspect_artifact, ArtifactShape};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn declarations() -> BTreeMap<String, String> {
    serde_json::from_str(include_str!("../../../scripts/component-names.json")).unwrap()
}

fn name(project: &str, artifact: &Path) -> Output {
    Command::new("python3")
        .arg(root().join("scripts/name-component.py"))
        .arg(root().join(project))
        .arg(artifact)
        .output()
        .expect("producer tests require Python 3 and wasm-tools")
}

fn fixture() -> Vec<u8> {
    wat::parse_str(
        r#"(component
            (core module $nested-module
                (func (export "run")))
            (core instance $i (instantiate $nested-module))
            (func (export "run") (canon lift (core func $i "run")))
            (component $nested-component)
            (@custom "package-docs" "\01{}")
        )"#,
    )
    .unwrap()
}

fn nested_binaries(bytes: &[u8]) -> Vec<&[u8]> {
    wasmparser::Parser::new(0)
        .parse_all(bytes)
        .filter_map(|payload| match payload.unwrap() {
            wasmparser::Payload::ModuleSection {
                unchecked_range, ..
            }
            | wasmparser::Payload::ComponentSection {
                unchecked_range, ..
            } => Some(&bytes[unchecked_range]),
            _ => None,
        })
        .collect()
}

fn check_built_name(project: &str, artifact: &str, shape: ArtifactShape) -> Result<()> {
    let path = root().join(project).join(artifact);
    let bytes = std::fs::read(&path)
        .with_context(|| format!("build named fixtures before testing: {}", path.display()))?;
    let inspection = inspect_artifact(&bytes)?;
    assert_eq!(inspection.identity?.as_str(), declarations()[project]);
    assert_eq!(inspection.shape, shape);
    Ok(())
}

#[test]
fn producer_names_are_explicit_unique_and_independent_of_output_paths() -> Result<()> {
    let declared = declarations();
    let unique: std::collections::BTreeSet<_> = declared.values().collect();
    assert_eq!(declared.len(), unique.len());
    for (project, expected) in declared {
        let temp = tempfile::tempdir()?;
        let output = temp.path().join("unrelated filename.wasm");
        let original = fixture();
        std::fs::write(&output, &original)?;
        let result = name(&project, &output);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let named = std::fs::read(&output)?;
        assert_eq!(nested_binaries(&original), nested_binaries(&named));
        let inspection = inspect_artifact(&named)?;
        assert_eq!(inspection.identity?.as_str(), expected);
        assert_eq!(inspection.shape, ArtifactShape::ToolCandidate);
        for marker in [
            b"nested-module".as_slice(),
            b"nested-component",
            b"package-docs",
        ] {
            assert!(named.windows(marker.len()).any(|bytes| bytes == marker));
        }

        let result = name(&project, &output);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(std::fs::read(&output)?, named, "naming must be idempotent");
        assert_eq!(
            inspect_artifact(&std::fs::read(&output)?)?
                .identity?
                .as_str(),
            expected
        );
    }
    Ok(())
}

#[test]
fn producer_names_replace_compiler_names_without_ambiguity() -> Result<()> {
    for count in [1, 2] {
        let temp = tempfile::tempdir()?;
        let output = temp.path().join("component.wasm");
        let mut component = Component::new();
        let mut names = ComponentNameSection::new();
        for _ in 0..count {
            names.component("compiler-name");
        }
        component.section(&names);
        std::fs::write(&output, component.finish())?;
        let result = name("examples/fetch-rs", &output);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            inspect_artifact(&std::fs::read(&output)?)?
                .identity?
                .as_str(),
            "microsoft:fetch-rs"
        );
    }
    Ok(())
}

#[test]
fn producer_names_refuse_undeclared_projects_and_non_components() -> Result<()> {
    let mut ambiguous = Component::new();
    let mut names = ComponentNameSection::new();
    names.component("compiler-name");
    ambiguous.section(&names).section(&names);
    for (project, bytes) in [
        ("crates/component2json/testdata", fixture()),
        ("examples/fetch-rs", wat::parse_str("(module)")?),
        ("examples/fetch-rs", b"not a component".to_vec()),
        (
            "examples/fetch-rs",
            b"\0asm\x0d\0\x01\0\x01\x08invalid!".to_vec(),
        ),
        ("examples/fetch-rs", b"\0asm\x0d\0\x01\0\x00\xff".to_vec()),
        ("examples/fetch-rs", ambiguous.finish()),
    ] {
        let temp = tempfile::tempdir()?;
        let output = temp.path().join("opaque.wasm");
        std::fs::write(&output, &bytes)?;
        let result = name(project, &output);
        assert!(!result.status.success());
        assert_eq!(std::fs::read(&output)?, bytes);
        assert_eq!(std::fs::read_dir(temp.path())?.count(), 1);
    }
    Ok(())
}

#[test]
fn producer_names_on_built_rust_and_acp_fixtures_are_inspectable() -> Result<()> {
    let outputs = [
        (
            "examples/fetch-rs",
            "fetch_rs",
            ArtifactShape::ToolCandidate,
        ),
        (
            "examples/filesystem-rs",
            "filesystem",
            ArtifactShape::ToolCandidate,
        ),
        (
            "components/acp-echo-provider",
            "acp_echo_provider",
            ArtifactShape::AcpProvider,
        ),
        (
            "components/acp-uppercase-layer",
            "acp_uppercase_layer",
            ArtifactShape::AcpLayer,
        ),
        (
            "components/acp-ollama-provider",
            "acp_ollama_provider",
            ArtifactShape::AcpProvider,
        ),
        (
            "components/acp-copilot-provider",
            "acp_copilot_provider",
            ArtifactShape::AcpProvider,
        ),
    ];
    for (project, filename, shape) in outputs {
        check_built_name(
            project,
            &format!("target/wasm32-wasip2/release/{filename}.wasm"),
            shape,
        )?;
    }
    Ok(())
}

#[test]
#[ignore = "requires npm ci && npm run build in examples/time-server-js"]
fn producer_names_on_javascript_output_are_inspectable() -> Result<()> {
    check_built_name(
        "examples/time-server-js",
        "time.wasm",
        ArtifactShape::ToolCandidate,
    )
}

#[test]
#[ignore = "requires just build in examples/eval-py"]
fn producer_names_on_python_output_are_inspectable() -> Result<()> {
    check_built_name(
        "examples/eval-py",
        "eval.wasm",
        ArtifactShape::ToolCandidate,
    )
}

#[test]
#[ignore = "requires just build in examples/gomodule-go"]
fn producer_names_on_go_output_are_inspectable() -> Result<()> {
    check_built_name(
        "examples/gomodule-go",
        "gomodule.wasm",
        ArtifactShape::ToolCandidate,
    )
}
