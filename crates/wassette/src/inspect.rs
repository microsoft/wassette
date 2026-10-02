// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Engine-free observations of cosmetic producer metadata and root exports.
//!
//! Inspection parses the binary but does not validate its type graph, imports,
//! or compatibility with a runtime. In particular, a tool candidate need not
//! contain callable functions or be supported by the ordinary engine.

use anyhow::{bail, Context, Result};
use wasmparser::{
    BinaryReader, ComponentExternalKind, ComponentName, ComponentNameSectionReader, Encoding,
    Parser, Payload,
};

use crate::identity::{validate_name, IdentityError};

/// Root-export routing evidence, not a guarantee of runtime compatibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactShape {
    /// Non-ACP function or instance exports may be handled by the ordinary runtime.
    ToolCandidate,
    /// Root ACP agent instance exports are present without client instance exports.
    AcpProvider,
    /// Both root ACP agent and client instance exports are present.
    AcpLayer,
    /// The artifact does not have a supported runtime shape.
    Unsupported(UnsupportedArtifact),
}

/// The reason an inspected artifact cannot be routed to a supported runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnsupportedArtifact {
    /// The root binary is a core WebAssembly module, not a component.
    CoreModule,
    /// The root component exports neither functions nor instances.
    NoRuntimeExports,
    /// ACP-family exports are client-only, use a wrong external kind, or are unknown.
    AcpShape,
}

/// Cosmetic name and root-export observations from the same artifact bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactInspection {
    /// Cosmetic declared root name; never admission or logical identity evidence.
    pub identity: Result<String, IdentityError>,
    /// The candidate runtime route inferred from root exports.
    pub shape: ArtifactShape,
    /// Exact root ACP-family export names, including version suffixes.
    pub acp_exports: Vec<String>,
}

/// Inspect cosmetic producer names and routing shape from captured artifact bytes.
///
/// Missing or invalid cosmetic names do not hide the artifact's shape and must
/// never determine whether its source-derived logical identity is admitted.
/// Runtime type checking, import linking, and ACP version checks remain with
/// the selected runtime; successful inspection is not validation.
///
/// # Errors
///
/// Returns an error for malformed binary sections, malformed root-name metadata,
/// or unknown section encodings. Missing, blank, control-containing, or duplicate
/// root-name declarations are reported through [`ArtifactInspection::identity`].
pub fn inspect_artifact(bytes: &[u8]) -> Result<ArtifactInspection> {
    let mut depth = 0usize;
    let mut root_encoding = None;
    let mut declarations = 0usize;
    let mut identity = Err(IdentityError::Missing);
    let mut exports = Vec::new();

    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.context("failed to parse WebAssembly artifact")?;
        match payload {
            Payload::Version { encoding, .. } => {
                if depth == 0 {
                    root_encoding = Some(encoding);
                }
            }
            Payload::ModuleSection { .. } | Payload::ComponentSection { .. } => depth += 1,
            Payload::End(_) => depth = depth.saturating_sub(1),
            Payload::CustomSection(section)
                if depth == 0
                    && root_encoding == Some(Encoding::Component)
                    && section.name() == "component-name" =>
            {
                let names = ComponentNameSectionReader::new(BinaryReader::new(
                    section.data(),
                    section.data_offset(),
                ));
                for name in names {
                    let name = name.context("malformed cosmetic root component-name metadata")?;
                    match name {
                        ComponentName::Component { name, .. } => {
                            declarations += 1;
                            identity = if declarations == 1 {
                                validate_name(name).map(|()| name.to_owned())
                            } else {
                                Err(IdentityError::Ambiguous)
                            };
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
                            parse_entries(names).context("malformed root component-name map")?;
                        }
                        ComponentName::Unknown { .. } => {}
                    }
                }
            }
            Payload::ComponentExportSection(section) => {
                for export in section {
                    let export = export.context("malformed component export section")?;
                    if depth == 0 {
                        exports.push((export.name.name, export.kind));
                    }
                }
            }
            // Section payloads are lazy readers. Consume them even when they do
            // not contribute routing evidence, so malformed data cannot hide
            // behind an otherwise ordinary-looking export.
            Payload::TypeSection(section) => parse_entries(section)?,
            Payload::ImportSection(section) => parse_entries(section.into_imports())?,
            Payload::FunctionSection(section) => parse_entries(section)?,
            Payload::TableSection(section) => parse_entries(section)?,
            Payload::MemorySection(section) => parse_entries(section)?,
            Payload::TagSection(section) => parse_entries(section)?,
            Payload::GlobalSection(section) => parse_entries(section)?,
            Payload::ExportSection(section) => parse_entries(section)?,
            Payload::ElementSection(section) => parse_entries(section)?,
            Payload::DataSection(section) => parse_entries(section)?,
            Payload::InstanceSection(section) => parse_entries(section)?,
            Payload::CoreTypeSection(section) => parse_entries(section)?,
            Payload::ComponentInstanceSection(section) => parse_entries(section)?,
            Payload::ComponentAliasSection(section) => parse_entries(section)?,
            Payload::ComponentTypeSection(section) => parse_entries(section)?,
            Payload::ComponentCanonicalSection(section) => parse_entries(section)?,
            Payload::ComponentImportSection(section) => parse_entries(section)?,
            Payload::CodeSectionEntry(body) => {
                parse_entries(body.get_locals_reader()?)?;
                let mut operators = body.get_operators_reader()?;
                while !operators.eof() {
                    operators.read()?;
                }
                operators.finish()?;
            }
            Payload::CustomSection(_)
            | Payload::StartSection { .. }
            | Payload::DataCountSection { .. }
            | Payload::CodeSectionStart { .. }
            | Payload::ComponentStartSection { .. } => {}
            Payload::UnknownSection { id, .. } => {
                bail!("cannot inspect unknown WebAssembly section {id}");
            }
            _ => bail!("cannot inspect unsupported WebAssembly payload"),
        }
    }

    let shape = match root_encoding {
        Some(Encoding::Module) => ArtifactShape::Unsupported(UnsupportedArtifact::CoreModule),
        Some(Encoding::Component) => classify_exports(exports.iter().copied()),
        None => bail!("missing WebAssembly artifact header"),
    };
    let acp_exports = exports
        .into_iter()
        .filter(|(name, _)| is_acp_export(name))
        .map(|(name, _)| name.to_owned())
        .collect();
    Ok(ArtifactInspection {
        identity,
        shape,
        acp_exports,
    })
}

fn parse_entries<T>(entries: impl IntoIterator<Item = wasmparser::Result<T>>) -> Result<()> {
    for entry in entries {
        entry?;
    }
    Ok(())
}

/// Apply the same root-export rule to inspected bytes and compiled cache entries.
pub(crate) fn classify_exports<'a>(
    exports: impl IntoIterator<Item = (&'a str, ComponentExternalKind)>,
) -> ArtifactShape {
    let mut has_runtime_exports = false;
    let mut agent = false;
    let mut client = false;
    for (name, kind) in exports {
        has_runtime_exports |= matches!(
            kind,
            ComponentExternalKind::Func | ComponentExternalKind::Instance
        );
        if !is_acp_export(name) {
            continue;
        }
        if kind != ComponentExternalKind::Instance {
            return ArtifactShape::Unsupported(UnsupportedArtifact::AcpShape);
        }
        let unversioned = name.split_once('@').map_or(name, |(name, _)| name);
        match unversioned {
            "wassette:acp/agent" => agent = true,
            "wassette:acp/client" => client = true,
            _ => return ArtifactShape::Unsupported(UnsupportedArtifact::AcpShape),
        }
    }
    match (agent, client, has_runtime_exports) {
        (true, true, _) => ArtifactShape::AcpLayer,
        (true, false, _) => ArtifactShape::AcpProvider,
        (false, true, _) => ArtifactShape::Unsupported(UnsupportedArtifact::AcpShape),
        (false, false, true) => ArtifactShape::ToolCandidate,
        (false, false, false) => ArtifactShape::Unsupported(UnsupportedArtifact::NoRuntimeExports),
    }
}

pub(crate) fn is_acp_export(name: &str) -> bool {
    name.starts_with("wassette:acp/")
}

#[cfg(test)]
mod tests {
    use wasm_encoder::{
        Component, ComponentExportKind, ComponentExportSection, ComponentNameSection,
        CustomSection, Module, ModuleSection, NameMap, NameSection, NestedComponentSection,
        RawSection,
    };

    use super::*;

    fn named_component(names: &[&str]) -> Component {
        let mut component = Component::new();
        let mut section = ComponentNameSection::new();
        for name in names {
            section.component(name);
        }
        component.section(&section);
        component
    }

    fn with_exports(mut component: Component, exports: &[(&str, ComponentExportKind)]) -> Vec<u8> {
        let mut section = ComponentExportSection::new();
        for (name, kind) in exports {
            section.export(*name, *kind, 0, None);
        }
        component.section(&section);
        component.finish()
    }

    fn inspect_wat(source: &str) -> ArtifactInspection {
        inspect_artifact(&wat::parse_str(source).unwrap()).unwrap()
    }

    #[test]
    fn missing_root_identity_is_not_synthesized() {
        let inspection = inspect_wat("(component)");
        assert_eq!(inspection.identity, Err(IdentityError::Missing));
        assert_eq!(
            inspection.shape,
            ArtifactShape::Unsupported(UnsupportedArtifact::NoRuntimeExports)
        );
    }

    #[test]
    fn declared_identity_preserves_spelling() {
        for name in [
            "example:Weather/東京@1.0.0",
            "é",
            "e\u{301}",
            " Weather ",
            "root:component",
            "NUL",
            "../not-a-filename",
        ] {
            let bytes = named_component(&[name]).finish();
            let inspection = inspect_artifact(&bytes).unwrap();
            assert_eq!(inspection.identity.unwrap().as_str(), name);
        }
    }

    #[test]
    fn invalid_declared_names_are_not_missing() {
        for name in ["", " ", "\u{2003}", "a\0b", "a\nb", "a\u{7f}b", "a\u{85}b"] {
            let inspection = inspect_artifact(&named_component(&[name]).finish()).unwrap();
            assert_eq!(
                inspection.identity,
                Err(IdentityError::InvalidName),
                "{name:?}"
            );
        }
    }

    #[test]
    fn duplicate_declarations_are_ambiguous_even_when_equal() {
        for names in [
            ["example:one", "example:one"],
            ["example:one", "example:two"],
            ["", "example:one"],
        ] {
            let inspection = inspect_artifact(&named_component(&names).finish()).unwrap();
            assert_eq!(inspection.identity, Err(IdentityError::Ambiguous));

            let mut component = named_component(&names[..1]);
            let mut second = ComponentNameSection::new();
            second.component(names[1]);
            component.section(&second);
            assert_eq!(
                inspect_artifact(&component.finish()).unwrap().identity,
                Err(IdentityError::Ambiguous)
            );
        }
    }

    #[test]
    fn malformed_root_name_metadata_is_an_explicit_error() {
        for data in [
            &[1, 0xff][..],    // Invalid UTF-8.
            &[2, b'a'][..],    // Truncated name.
            &[1, b'a', 0][..], // Trailing bytes.
        ] {
            let mut component = named_component(&["valid"]);
            let mut names = ComponentNameSection::new();
            names.raw(0, data);
            component.section(&names);
            let error = inspect_artifact(&component.finish()).unwrap_err();
            assert!(error
                .to_string()
                .contains("malformed cosmetic root component-name"));
        }
        let mut component = Component::new();
        component.section(&CustomSection {
            name: "component-name".into(),
            data: (&[0, 0x80][..]).into(),
        });
        assert!(inspect_artifact(&component.finish()).is_err());
    }

    #[test]
    fn module_names_maps_and_producers_do_not_declare_root_identity() {
        let mut module = Module::new();
        let mut module_name = NameSection::new();
        module_name.module("downloaded-file.wasm");
        module.section(&module_name);
        let mut component = Component::new();
        component.section(&ModuleSection(&module));
        component.section(&module_name.as_custom());
        let mut names = ComponentNameSection::new();
        let mut map = NameMap::new();
        map.append(0, "map-name");
        names.funcs(&map);
        names.components(&map);
        names.core_modules(&map);
        component.section(&names);
        component.section(&CustomSection {
            name: "producers".into(),
            data: (&b"example:producer"[..]).into(),
        });
        assert_eq!(
            inspect_artifact(component.as_slice()).unwrap().identity,
            Err(IdentityError::Missing)
        );
        let mut root_name = ComponentNameSection::new();
        root_name.component("example:actual");
        component.section(&root_name);
        assert_eq!(
            inspect_artifact(&component.finish())
                .unwrap()
                .identity
                .unwrap()
                .as_str(),
            "example:actual"
        );
    }

    #[test]
    fn nested_names_and_acp_exports_do_not_escape_their_depth() {
        let nested_bytes = with_exports(
            named_component(&["nested:agent"]),
            &[("wassette:acp/agent@0.1.0", ComponentExportKind::Instance)],
        );
        let mut middle = named_component(&["middle"]);
        middle.section(&RawSection {
            id: wasm_encoder::ComponentSectionId::Component.into(),
            data: &nested_bytes,
        });
        let mut root = Component::new();
        root.section(&NestedComponentSection(&middle));
        root.section(&ModuleSection(&Module::new()));
        assert_eq!(
            inspect_artifact(root.as_slice()).unwrap().identity,
            Err(IdentityError::Missing)
        );
        let mut name = ComponentNameSection::new();
        name.component("actual:root");
        root.section(&name);
        let bytes = with_exports(root, &[("run", ComponentExportKind::Func)]);
        let inspection = inspect_artifact(&bytes).unwrap();
        assert_eq!(inspection.identity.unwrap().as_str(), "actual:root");
        assert_eq!(inspection.shape, ArtifactShape::ToolCandidate);
        assert!(inspection.acp_exports.is_empty());
    }

    #[test]
    fn nested_malformed_name_metadata_does_not_replace_root_identity() {
        let mut nested = Component::new();
        let mut names = ComponentNameSection::new();
        names.raw(0, &[1, 0xff]);
        nested.section(&names);
        let mut root = named_component(&["root:declared"]);
        root.section(&NestedComponentSection(&nested));
        assert_eq!(
            inspect_artifact(&root.finish())
                .unwrap()
                .identity
                .unwrap()
                .as_str(),
            "root:declared"
        );
    }

    #[test]
    fn name_and_shape_are_observed_independently_in_the_same_bytes() {
        let bytes = with_exports(
            named_component(&["unrelated:semantic/name"]),
            &[("wassette:acp/agent", ComponentExportKind::Instance)],
        );
        let inspection = inspect_artifact(&bytes).unwrap();
        assert_eq!(
            inspection.identity.unwrap().as_str(),
            "unrelated:semantic/name"
        );
        assert_eq!(inspection.shape, ArtifactShape::AcpProvider);
        let unnamed = with_exports(
            Component::new(),
            &[("wassette:acp/agent", ComponentExportKind::Instance)],
        );
        let inspection = inspect_artifact(&unnamed).unwrap();
        assert_eq!(inspection.identity, Err(IdentityError::Missing));
        assert_eq!(inspection.shape, ArtifactShape::AcpProvider);
    }

    #[test]
    fn core_modules_are_not_components_even_with_component_name_metadata() {
        let mut module = Module::new();
        let mut names = ComponentNameSection::new();
        names.component("not-a-component");
        module.section(&names.as_custom());
        let inspection = inspect_artifact(&module.finish()).unwrap();
        assert_eq!(
            inspection.shape,
            ArtifactShape::Unsupported(UnsupportedArtifact::CoreModule)
        );
        assert_eq!(inspection.identity, Err(IdentityError::Missing));
    }

    #[test]
    fn only_function_and_instance_exports_establish_tool_candidates() {
        for kind in [ComponentExportKind::Func, ComponentExportKind::Instance] {
            let bytes = with_exports(Component::new(), &[("ordinary", kind)]);
            assert_eq!(
                inspect_artifact(&bytes).unwrap().shape,
                ArtifactShape::ToolCandidate
            );
        }
        for kind in [
            ComponentExportKind::Module,
            ComponentExportKind::Type,
            ComponentExportKind::Value,
            ComponentExportKind::Component,
        ] {
            let bytes = with_exports(Component::new(), &[("not-runtime", kind)]);
            assert_eq!(
                inspect_artifact(&bytes).unwrap().shape,
                ArtifactShape::Unsupported(UnsupportedArtifact::NoRuntimeExports)
            );
        }
        assert_eq!(
            inspect_wat(r#"(component (instance $empty) (export "empty" (instance $empty)))"#)
                .shape,
            ArtifactShape::ToolCandidate
        );
    }

    #[test]
    fn providers_and_layers_preserve_exact_acp_export_names() {
        let agent = "wassette:acp/agent@0.1.0";
        let client = "wassette:acp/client@0.2.0-rc.1+build";
        let provider = with_exports(Component::new(), &[(agent, ComponentExportKind::Instance)]);
        let inspection = inspect_artifact(&provider).unwrap();
        assert_eq!(inspection.shape, ArtifactShape::AcpProvider);
        assert_eq!(inspection.acp_exports, [agent]);
        let layer = with_exports(
            Component::new(),
            &[
                (client, ComponentExportKind::Instance),
                (agent, ComponentExportKind::Instance),
                ("ordinary", ComponentExportKind::Func),
            ],
        );
        let inspection = inspect_artifact(&layer).unwrap();
        assert_eq!(inspection.shape, ArtifactShape::AcpLayer);
        assert_eq!(inspection.acp_exports, [client, agent]);
    }

    #[test]
    fn acp_version_validation_remains_with_the_acp_runtime() {
        for name in [
            "wassette:acp/agent",
            "wassette:acp/agent@",
            "wassette:acp/agent@not-semver",
            "wassette:acp/agent@99.0.0",
        ] {
            let bytes = with_exports(Component::new(), &[(name, ComponentExportKind::Instance)]);
            let inspection = inspect_artifact(&bytes).unwrap();
            assert_eq!(inspection.shape, ArtifactShape::AcpProvider);
            assert_eq!(inspection.acp_exports, [name]);
        }
    }

    #[test]
    fn client_imports_do_not_determine_shape() {
        let provider = inspect_wat(
            r#"(component
                (import "wassette:acp/client@0.1.0" (instance))
                (instance $agent)
                (export "wassette:acp/agent@0.1.0" (instance $agent)))"#,
        );
        assert_eq!(provider.shape, ArtifactShape::AcpProvider);
        assert_eq!(provider.acp_exports, ["wassette:acp/agent@0.1.0"]);
        let ordinary = inspect_wat(
            r#"(component
                (import "wassette:acp/client" (instance))
                (instance $ordinary)
                (export "ordinary" (instance $ordinary)))"#,
        );
        assert_eq!(ordinary.shape, ArtifactShape::ToolCandidate);
        assert!(ordinary.acp_exports.is_empty());
        let imports_only = inspect_wat(r#"(component (import "wassette:acp/client" (instance)))"#);
        assert_eq!(
            imports_only.shape,
            ArtifactShape::Unsupported(UnsupportedArtifact::NoRuntimeExports)
        );
    }

    #[test]
    fn client_only_and_unknown_acp_families_never_become_tools() {
        for name in [
            "wassette:acp/client",
            "wassette:acp/unknown@0.1.0",
            "wassette:acp/agent-extra",
            "wassette:acp/agent/session",
            "wassette:acp/",
        ] {
            let bytes = with_exports(
                Component::new(),
                &[
                    ("ordinary", ComponentExportKind::Func),
                    (name, ComponentExportKind::Instance),
                ],
            );
            let inspection = inspect_artifact(&bytes).unwrap();
            assert_eq!(
                inspection.shape,
                ArtifactShape::Unsupported(UnsupportedArtifact::AcpShape),
                "{name}"
            );
            assert_eq!(inspection.acp_exports, [name]);
        }
        let bytes = with_exports(
            Component::new(),
            &[
                ("wassette:acp/agent", ComponentExportKind::Instance),
                ("wassette:acp/unknown", ComponentExportKind::Instance),
            ],
        );
        assert_eq!(
            inspect_artifact(&bytes).unwrap().shape,
            ArtifactShape::Unsupported(UnsupportedArtifact::AcpShape)
        );
    }

    #[test]
    fn acp_exports_require_instance_external_kind() {
        for kind in [
            ComponentExportKind::Module,
            ComponentExportKind::Func,
            ComponentExportKind::Value,
            ComponentExportKind::Type,
            ComponentExportKind::Component,
        ] {
            for name in ["wassette:acp/agent", "wassette:acp/client"] {
                let bytes = with_exports(
                    Component::new(),
                    &[("ordinary", ComponentExportKind::Instance), (name, kind)],
                );
                let inspection = inspect_artifact(&bytes).unwrap();
                assert_eq!(
                    inspection.shape,
                    ArtifactShape::Unsupported(UnsupportedArtifact::AcpShape)
                );
                assert_eq!(inspection.acp_exports, [name]);
            }
        }
    }

    #[test]
    fn acp_matching_is_exact_not_a_normalized_name_prefix() {
        for name in [
            "wassette:acp-other/agent",
            "other:acp/agent",
            "wassette-acp-agent",
            "Wassette:acp/agent",
        ] {
            assert!(!is_acp_export(name));
            assert_eq!(
                classify_exports([(name, ComponentExternalKind::Instance)]),
                ArtifactShape::ToolCandidate
            );
        }
        assert!(is_acp_export("wassette:acp/agent@0.1.0"));
        assert!(is_acp_export("wassette:acp/unknown"));
    }

    #[test]
    fn async_component_types_are_inspected_without_engine_validation() {
        use wasm_encoder::{
            ComponentImportSection, ComponentInstanceSection, ComponentTypeRef,
            ComponentTypeSection, ComponentValType,
        };

        let mut component = named_component(&["example:async-tool"]);
        let mut types = ComponentTypeSection::new();
        types
            .function()
            .async_(true)
            .params([] as [(&str, ComponentValType); 0])
            .result(None);
        component.section(&types);
        let mut imports = ComponentImportSection::new();
        imports.import("run", ComponentTypeRef::Func(0));
        component.section(&imports);
        let bytes = with_exports(component.clone(), &[("run", ComponentExportKind::Func)]);
        let inspection = inspect_artifact(&bytes).unwrap();
        assert_eq!(inspection.identity.unwrap().as_str(), "example:async-tool");
        assert_eq!(inspection.shape, ArtifactShape::ToolCandidate);

        let mut instances = ComponentInstanceSection::new();
        instances.export_items([("run", ComponentExportKind::Func, 0)]);
        component.section(&instances);
        let bytes = with_exports(
            component,
            &[("wassette:acp/agent@0.1.0", ComponentExportKind::Instance)],
        );
        assert_eq!(
            inspect_artifact(&bytes).unwrap().shape,
            ArtifactShape::AcpProvider
        );
    }

    #[test]
    fn malformed_and_unknown_sections_never_become_candidates() {
        assert!(inspect_artifact(b"not wasm").is_err());
        let mut bad_version = Component::new().finish();
        bad_version[4] = 0xff;
        assert!(inspect_artifact(&bad_version).is_err());
        for (id, data) in [
            (7, &[1, 0xff][..]),                    // Invalid component type.
            (8, &[1, 0xff][..]),                    // Unknown canonical operation.
            (10, &[1][..]),                         // Truncated import.
            (11, &[1, 0, 1, b'x', 0xff, 0, 0][..]), // Unknown export kind.
            (11, &[0, 0][..]),                      // Trailing export data.
            (99, &[][..]),                          // Unknown section.
        ] {
            let mut component = Component::new();
            component.section(&RawSection { id, data });
            let bytes = with_exports(component, &[("run", ComponentExportKind::Func)]);
            assert!(inspect_artifact(&bytes).is_err(), "section {id}");
        }
        let mut truncated = with_exports(Component::new(), &[("run", ComponentExportKind::Func)]);
        truncated.pop();
        assert!(inspect_artifact(&truncated).is_err());
    }

    #[test]
    fn nested_core_sections_and_function_bodies_are_parsed() {
        for (id, data) in [
            (1, &[1, 0xff][..]),              // Invalid core type.
            (2, &[1][..]),                    // Truncated core import.
            (10, &[1, 3, 0, 0xff, 0x0b][..]), // Unknown opcode.
            (10, &[1, 1, 0][..]),             // Missing end opcode.
            (99, &[][..]),                    // Unknown core section.
        ] {
            let mut module = Module::new();
            module.section(&RawSection { id, data });
            let mut component = Component::new();
            component.section(&ModuleSection(&module));
            let bytes = with_exports(component, &[("run", ComponentExportKind::Func)]);
            assert!(inspect_artifact(&bytes).is_err(), "core section {id}");
        }
        let ordinary = inspect_wat(
            r#"(component
                (core module $m (func (export "run")))
                (core instance $i (instantiate $m))
                (func (export "run") (canon lift (core func $i "run"))))"#,
        );
        assert_eq!(ordinary.shape, ArtifactShape::ToolCandidate);
    }
}
