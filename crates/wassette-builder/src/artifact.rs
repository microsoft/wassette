// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#[cfg(any(test, feature = "hyperlight"))]
use anyhow::{Context, Result, ensure};
#[cfg(any(test, feature = "hyperlight"))]
use wasm_encoder::{ComponentNameSection, ComponentSection, Encode};
#[cfg(any(test, feature = "hyperlight"))]
use wasmparser::{
    BinaryReader, ComponentExternalKind, ComponentName, ComponentNameSectionReader, Encoding,
    Parser, Payload,
};

#[cfg(any(test, feature = "hyperlight"))]
use crate::{BuildRequest, ComponentKind};

/// Only root metadata and routing shape are observed here. This is NOT L1
/// validation or Wasmtime preparation, which must be performed by the parent.
#[cfg(any(test, feature = "hyperlight"))]
pub(crate) fn name_component(
    mut wasm: Vec<u8>,
    request: &BuildRequest,
    cap: usize,
    check: impl Fn() -> Result<()>,
) -> Result<Vec<u8>> {
    ensure!(wasm.len() <= cap, "component exceeds output budget");
    let mut depth = 0usize;
    let mut name_count = 0usize;
    let mut agent = false;
    let mut client = false;
    let mut runtime_exports = 0usize;
    for payload in Parser::new(0).parse_all(&wasm) {
        check()?;
        match payload.context("parse generated component")? {
            Payload::Version { encoding, .. } if depth == 0 => {
                ensure!(
                    encoding == Encoding::Component,
                    "compiler returned a core module, not a component"
                );
            }
            Payload::ComponentSection { .. } | Payload::ModuleSection { .. } => {
                depth += 1;
                ensure!(depth <= 64, "component nesting exceeds metadata budget");
            }
            Payload::End(_) => depth = depth.saturating_sub(1),
            Payload::CustomSection(section) if depth == 0 && section.name() == "component-name" => {
                for entry in ComponentNameSectionReader::new(BinaryReader::new(
                    section.data(),
                    section.data_offset(),
                )) {
                    check()?;
                    match entry.context("malformed root component-name")? {
                        ComponentName::Component { name, .. } => {
                            name_count += 1;
                            ensure!(name_count == 1, "duplicate root component-name declaration");
                            ensure!(
                                name == request.component_name,
                                "conflicting root component-name declaration"
                            );
                        }
                        ComponentName::CoreFuncs(names)
                        | ComponentName::CoreGlobals(names)
                        | ComponentName::CoreMemories(names)
                        | ComponentName::CoreTables(names)
                        | ComponentName::CoreTags(names)
                        | ComponentName::CoreModules(names)
                        | ComponentName::CoreInstances(names)
                        | ComponentName::CoreTypes(names)
                        | ComponentName::Types(names)
                        | ComponentName::Instances(names)
                        | ComponentName::Components(names)
                        | ComponentName::Funcs(names)
                        | ComponentName::Values(names) => {
                            for entry in names {
                                check()?;
                                entry.context("malformed root component-name map")?;
                            }
                        }
                        ComponentName::Unknown { .. } => {}
                    }
                }
            }
            Payload::ComponentExportSection(section) if depth == 0 => {
                for export in section {
                    check()?;
                    let export = export?;
                    let name = export.name.name;
                    if name.starts_with("wassette:acp/") {
                        ensure!(
                            request.kind == ComponentKind::AcpLayer,
                            "tool output has ACP exports"
                        );
                        ensure!(
                            export.kind == ComponentExternalKind::Instance,
                            "ACP output has a non-instance export"
                        );
                        match name.split('@').next() {
                            Some("wassette:acp/agent") => agent = true,
                            Some("wassette:acp/client") => client = true,
                            _ => anyhow::bail!("unsupported ACP output export"),
                        }
                    }
                    if matches!(
                        export.kind,
                        ComponentExternalKind::Func | ComponentExternalKind::Instance
                    ) {
                        runtime_exports += 1;
                    }
                }
            }
            _ => {}
        }
    }
    ensure!(runtime_exports > 0, "component has no runtime exports");
    if request.kind == ComponentKind::AcpLayer {
        ensure!(
            agent && client,
            "ACP layer must export both agent and client; providers are not supported"
        );
    }
    if name_count == 0 {
        let mut names = ComponentNameSection::new();
        names.component(&request.component_name);
        let mut encoded = Vec::new();
        encoded.push(names.id());
        names.encode(&mut encoded);
        ensure!(
            encoded.len() <= cap.saturating_sub(wasm.len()),
            "named component exceeds output budget"
        );
        wasm.extend_from_slice(&encoded);
    }
    Ok(wasm)
}

#[cfg(test)]
mod tests {
    use wasm_encoder::{Component, ComponentExportKind, ComponentExportSection};

    use super::*;
    use crate::tests::request;

    fn name_component(wasm: Vec<u8>, request: &BuildRequest, cap: usize) -> Result<Vec<u8>> {
        super::name_component(wasm, request, cap, || Ok(()))
    }

    fn component(names: &[&str]) -> Vec<u8> {
        let mut component = Component::new();
        let mut exports = ComponentExportSection::new();
        exports.export("add", ComponentExportKind::Func, 0, None);
        component.section(&exports);
        for name in names {
            let mut section = ComponentNameSection::new();
            section.component(name);
            component.section(&section);
        }
        component.finish()
    }

    #[test]
    fn emits_explicit_name_and_preserves_matching_name() {
        let req = request();
        let named = name_component(component(&[]), &req, 4096).unwrap();
        assert!(
            named
                .windows(req.component_name.len())
                .any(|b| b == req.component_name.as_bytes())
        );
        assert_eq!(name_component(named.clone(), &req, 4096).unwrap(), named);
        let existing = component(&[&req.component_name]);
        assert_eq!(
            name_component(existing.clone(), &req, 4096).unwrap(),
            existing
        );
    }

    #[test]
    fn rejects_conflicts_duplicates_core_modules_and_oversize() {
        for bytes in [
            component(&["other"]),
            component(&["example:add", "example:add"]),
            wasm_encoder::Module::new().finish(),
        ] {
            assert!(name_component(bytes, &request(), 4096).is_err());
        }
        let bytes = component(&[]);
        let cap = bytes.len();
        assert!(name_component(bytes, &request(), cap).is_err());
    }

    #[test]
    fn nested_names_cannot_supply_root_identity() {
        let nested = component(&["nested"]);
        let mut root = Component::new();
        root.section(&wasm_encoder::RawSection {
            id: 4,
            data: &nested,
        });
        let mut exports = ComponentExportSection::new();
        exports.export("add", ComponentExportKind::Func, 0, None);
        root.section(&exports);
        let named = name_component(root.finish(), &request(), 4096).unwrap();
        assert!(named.windows(11).any(|b| b == b"example:add"));
    }
}
