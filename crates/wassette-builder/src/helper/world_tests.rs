// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::time::Duration;

use wit_bindgen_core::wit_parser::{LiftLowerAbi, ManglingAndAbi};
use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};

use super::*;

fn request(wit: &str) -> BuildRequest {
    BuildRequest {
        component_name: "test:contract".into(),
        source: "struct Component;".into(),
        wit: wit.into(),
        world: "tool".into(),
        kind: ComponentKind::Tool,
    }
}

fn component(wit: &str) -> Vec<u8> {
    let mut resolve = Resolve::default();
    let package = resolve.push_str("actual.wit", wit).unwrap();
    let world = resolve.select_world(&[package], Some("tool")).unwrap();
    encode_component(&resolve, world)
}

fn encode_component(resolve: &Resolve, world: WorldId) -> Vec<u8> {
    let mut module = dummy_module(
        resolve,
        world,
        ManglingAndAbi::Legacy(LiftLowerAbi::AsyncCallback),
    );
    embed_component_metadata(&mut module, resolve, world, StringEncoding::UTF8).unwrap();
    ComponentEncoder::default()
        .module(&module)
        .unwrap()
        .validate(true)
        .encode()
        .unwrap()
}

fn validate(expected: &str, actual: &[u8]) -> Result<()> {
    RequestedWorld::resolve(&request(expected), &[], &BuildLimits::default())?.validate(
        actual,
        &BuildLimits::default(),
        Instant::now() + Duration::from_secs(10),
    )
}

#[test]
fn captured_exports_must_be_exact_not_just_trait_shaped() {
    let expected =
        "package test:contract@1.0.0; world tool { export run: func(value: u32) -> u32; }";
    validate(expected, &component(expected)).unwrap();
    for actual in [
        expected.replace("value: u32", "value: string"),
        expected.replace("-> u32", "-> s32"),
        expected.replace("export run:", "export other:"),
        expected.replace("export run:", "export extra: func(); export run:"),
    ] {
        assert!(validate(expected, &component(&actual)).is_err(), "{actual}");
    }
}

#[test]
fn generated_metadata_cannot_override_actual_binary_types() {
    let expected = "package test:contract@1.0.0; world tool { export run: func() -> u32; }";
    let mut actual = component(&expected.replace("u32", "string"));
    let requested =
        RequestedWorld::resolve(&request(expected), &[], &BuildLimits::default()).unwrap();
    embed_component_metadata(
        &mut actual,
        &requested.resolve,
        requested.world,
        StringEncoding::UTF8,
    )
    .unwrap();
    assert!(validate(expected, &actual).is_err());
}

#[test]
fn nested_types_functions_and_resource_identities_are_checked() {
    let expected = "package test:contract@1.0.0;
        interface api {
            resource first; resource second;
            record payload { value: u32 }
            run: func(handle: borrow<first>, value: payload) -> own<second>;
        }
        world tool { export api; }";
    validate(expected, &component(expected)).unwrap();
    for actual in [
        expected.replace("value: u32", "value: string"),
        expected.replace("borrow<first>", "borrow<second>"),
        expected.replace("own<second>", "own<first>"),
        expected.replace("run: func", "hidden: func(); run: func"),
    ] {
        assert!(validate(expected, &component(&actual)).is_err(), "{actual}");
    }
}

#[test]
fn imports_can_be_elided_but_not_added_or_retyped() {
    let expected = "package test:contract@1.0.0; world tool { import read: func() -> u32; export run: func(); }";
    validate(expected, &component(expected)).unwrap();
    validate(
        expected,
        &component(&expected.replace("import read: func() -> u32;", "")),
    )
    .unwrap();
    for actual in [
        expected.replace("import read:", "import hidden: func(); import read:"),
        expected.replace("-> u32", "-> string"),
    ] {
        assert!(validate(expected, &component(&actual)).is_err());
    }
}

#[test]
fn runtime_admission_checks_interface_names_functions_and_types() {
    let expected = "package test:contract@1.0.0; world tool { export run: func(); }";
    let runtime = "package wasi:cli@0.2.0; interface environment {
        get-arguments: func() -> list<string>;
    } world tool { import environment; export run: func(); }";
    validate(expected, &component(runtime)).unwrap();
    for actual in [
        runtime.replace("list<string>", "u32"),
        runtime.replace("get-arguments", "get-secrets"),
        runtime.replace("environment", "hidden"),
        runtime.replace("0.2.0", "0.3.0"),
    ] {
        assert!(validate(expected, &component(&actual)).is_err(), "{actual}");
    }
}

#[test]
fn async_streams_and_bidirectional_resource_instances_match() {
    let expected = "package test:contract@1.0.0;
        interface agent {
            resource session {
                prompt: async func() -> stream<string>;
            }
            open: async func() -> session;
        }
        world tool { import agent; export agent; }";
    validate(expected, &component(expected)).unwrap();
    assert!(
        validate(
            expected,
            &component(&expected.replace("stream<string>", "stream<u8>"))
        )
        .is_err()
    );
    assert!(
        validate(
            expected,
            &component(&expected.replace("open: async", "open:"))
        )
        .is_err()
    );
}

#[test]
fn validation_honors_output_and_deadline_limits() {
    let wit = "package test:contract@1.0.0; world tool { export run: func(); }";
    let bytes = component(wit);
    let mut requested =
        RequestedWorld::resolve(&request(wit), &[], &BuildLimits::default()).unwrap();
    let limits = BuildLimits {
        wasm_bytes: bytes.len() - 1,
        ..BuildLimits::default()
    };
    assert!(
        requested
            .validate(&bytes, &limits, Instant::now() + Duration::from_secs(10))
            .is_err()
    );
    assert!(
        requested
            .validate(&bytes, &BuildLimits::default(), Instant::now())
            .is_err()
    );
}

#[test]
fn canonical_acp_layer_matches_with_admitted_standard_runtime_imports() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../wassette-acp/wit/acp");
    let mut resolve = Resolve::default();
    resolve
        .push_dir(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../wit/component-generation"),
        )
        .unwrap();
    let (package, _) = resolve.push_dir(path).unwrap();
    let world = resolve.select_world(&[package], Some("layer")).unwrap();
    let mut expected = RequestedWorld {
        resolve: resolve.clone(),
        world,
    };
    let runtime = runtime_world().unwrap();
    let map = resolve.merge(runtime.resolve).unwrap();
    let runtime = map.worlds[runtime.world.index()].unwrap();
    resolve
        .merge_worlds(runtime, world, &mut Default::default())
        .unwrap();
    let bytes = encode_component(&resolve, world);
    expected
        .validate(
            &bytes,
            &BuildLimits::default(),
            Instant::now() + Duration::from_secs(10),
        )
        .unwrap();
}

#[test]
fn invalid_wit_exposes_message_and_location_but_not_source_snippets() {
    let request = request(
        "package test:broken@1.0.0;\nworld tool { export run: func() } // PRIVATE_SOURCE_SENTINEL",
    );
    let error = RequestedWorld::resolve(&request, &[], &BuildLimits::default())
        .err()
        .unwrap();
    let typed = error.downcast_ref::<BuildError>().unwrap();
    assert_eq!(typed.kind(), BuildErrorKind::InvalidWit);
    let diagnostic = typed.diagnostic().unwrap();
    assert!(diagnostic.contains("expected"), "{diagnostic}");
    assert!(diagnostic.contains("WIT:2:"), "{diagnostic}");
    assert!(!diagnostic.contains("PRIVATE_SOURCE_SENTINEL"));
    assert!(!format!("{error:#}").contains("PRIVATE_SOURCE_SENTINEL"));
}

#[test]
fn invalid_operator_dependency_is_not_a_requester_diagnostic() {
    let input = request("package test:contract@1.0.0; world tool { export run: func(); }");
    let error = RequestedWorld::resolve(
        &input,
        &["PRIVATE_CONFIGURATION_BODY".into()],
        &BuildLimits::default(),
    )
    .err()
    .unwrap();
    let typed = error.downcast_ref::<BuildError>().unwrap();
    assert_eq!(typed.kind(), BuildErrorKind::Unavailable);
    assert!(typed.diagnostic().is_none());
    assert!(!format!("{error:#}").contains("PRIVATE_CONFIGURATION_BODY"));
}
