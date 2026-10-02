// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Opt-in real-VM ACP tests. Build the feature-enabled binary and echo fixture,
//! set WASSETTE_ACP_GENERATION_IMAGE to a locally built builder image, then
//! optionally set WASSETTE_ACP_TEST_BINARY to an installed, Hyperlight-entitled
//! signed wassette binary on macOS (or sign target/debug/wassette before running).
//! run `cargo test -p wassette-acp --features component-generation --test acp_stdio
//! generation::real_ -- --ignored --test-threads=1`.

use wassette::generation::ComponentKind;
use wassette::store::{SourceIdentity, StoreError};

use super::*;

const SOURCE_SENTINEL: &str = "ACP_GENERATION_PRIVATE_SOURCE_SENTINEL";

fn tool_request(name: &str, value: u32) -> Value {
    json!({
        "build": {
            "component_name": name,
            "kind": ComponentKind::Tool,
            "world": "tool",
            "wit": "package test:acp-generation; world tool { export answer: func() -> u32; }",
            "source": format!(
                "// {SOURCE_SENTINEL}\nstruct Component;\n\
                 impl bindings::Guest for Component {{ fn answer() -> u32 {{ {value} }} }}\n\
                 bindings::export!(Component with_types_in bindings);\n"
            ),
        },
        "target": {"mode": "new"},
    })
}

fn stage_builder_image(image: &Path, builder: &Path) {
    let target = builder.join("rust-initrd.cpio");
    match std::fs::hard_link(image, &target) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            std::fs::copy(image, target).expect("copy builder image across devices");
        }
        Err(error) => panic!("hard-link builder image: {error}"),
    }
}

fn start_generation(bin: &Path, provider: &Path) -> Harness {
    let image = std::env::var_os("WASSETTE_ACP_GENERATION_IMAGE")
        .expect("set WASSETTE_ACP_GENERATION_IMAGE to a locally built builder image");
    let xdg = tempfile::tempdir_in(".").unwrap();
    let root = xdg.path().canonicalize().unwrap();
    let builder = root.join("data/wassette/builder");
    std::fs::create_dir_all(&builder).unwrap();
    stage_builder_image(Path::new(&image), &builder);
    let components = root.join("data/wassette/components");
    let secrets = root.join("config/wassette/secrets");
    let args = vec![
        "--component-dir".into(),
        components.into_os_string(),
        "--secrets-dir".into(),
        secrets.into_os_string(),
    ];
    let mut harness = Harness::spawn(bin, &[provider], args, xdg, None, &[], &[]);
    harness.line_timeout = Duration::from_secs(180);
    harness
}

#[test]
fn staged_builder_image_is_hard_linked_without_modifying_source() {
    let root = tempfile::tempdir_in(".").unwrap();
    let source = root.path().join("source.cpio");
    std::fs::write(&source, b"builder image").unwrap();
    let builder = root.path().join("builder");
    std::fs::create_dir(&builder).unwrap();

    stage_builder_image(&source, &builder);

    let staged = builder.join("rust-initrd.cpio");
    assert_eq!(std::fs::read(&source).unwrap(), b"builder image");
    assert_eq!(std::fs::read(&staged).unwrap(), b"builder image");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let original = source.metadata().unwrap();
        let linked = staged.metadata().unwrap();
        assert_eq!(
            (original.dev(), original.ino()),
            (linked.dev(), linked.ino())
        );
    }
}

fn await_phases(harness: &mut Harness, id: i64, decisions: &[&str]) -> (Vec<Value>, Value) {
    let mut messages = Vec::new();
    let mut phase = 0;
    loop {
        let message: Value = serde_json::from_str(&harness.next_line()).unwrap();
        if message["id"].as_i64() == Some(id) {
            assert!(message.get("error").is_none(), "{message}");
            assert_eq!(
                phase,
                decisions.len(),
                "{messages:?}\nstderr:\n{}",
                harness.stderr.lock().unwrap(),
            );
            return (messages, message["result"].clone());
        }
        if message["method"] == "session/request_permission" {
            let decision = decisions
                .get(phase)
                .unwrap_or_else(|| panic!("unexpected extra permission phase: {message}"));
            harness.respond_permission(&message, decision);
            phase += 1;
        }
        messages.push(message);
    }
}

fn generated_report(messages: &[Value]) -> Value {
    let text = response_text(messages);
    let start = text
        .find('{')
        .unwrap_or_else(|| panic!("no generated receipt: {text}"));
    serde_json::Deserializer::from_str(&text[start..])
        .into_iter::<Value>()
        .next()
        .unwrap()
        .unwrap()
}

fn assert_absent(harness: &Harness, component: &str) {
    assert!(matches!(
        harness.store().read(component),
        Err(StoreError::NotFound(_))
    ));
}

#[test]
#[ignore = "requires a real builder image via WASSETTE_ACP_GENERATION_IMAGE"]
fn real_generation_permissions_store_and_session_scope() {
    let (bin, provider) = artifacts().expect("build the feature-enabled CLI and ACP echo fixture");
    let mut h = start_generation(&bin, &provider);
    let session = h.open_session();
    let rejected_name = "test:acp-generation/rejected";
    let request = tool_request(rejected_name, 40);
    let id = h.prompt(&session, &format!("/generate {request}"));
    let (messages, _) = await_phases(&mut h, id, &["reject-once"]);
    assert!(response_text(&messages).contains("PermissionDenied"));
    assert_absent(&h, rejected_name);

    let id = h.prompt(&session, &format!("/generate {request}"));
    let (messages, _) = await_phases(&mut h, id, &["allow-once", "reject-once"]);
    assert!(response_text(&messages).contains("PermissionDenied"));
    assert_absent(&h, rejected_name);
    let preview = &messages
        .iter()
        .filter(|m| m["method"] == "session/request_permission")
        .nth(1)
        .unwrap()["params"]["toolCall"]["rawInput"]["operation"];
    assert_eq!(preview["wasm_sha256"].as_str().unwrap().len(), 64);

    let exposed_name = "test:acp-generation/exposed";
    let request = tool_request(exposed_name, 42);
    let id = h.prompt(&session, &format!("/generate {request}"));
    let (messages, _) = await_phases(&mut h, id, &["allow-once", "allow-once"]);
    assert!(
        response_text(&messages).starts_with("generation Disposition::SessionTools:"),
        "{messages:?}"
    );
    assert_eq!(generated_report(&messages)["component_id"], exposed_name);
    let receipt = h.store().read(exposed_name).unwrap().receipt;
    assert!(matches!(receipt.source, SourceIdentity::Generated { .. }));
    let report = generated_report(&messages);
    assert_eq!(report["revision"], receipt.revision.to_string());
    assert!(receipt.origin.generation.is_some());
    let permissions: Vec<_> = messages
        .iter()
        .filter(|m| m["method"] == "session/request_permission")
        .collect();
    let install = &permissions[1]["params"]["toolCall"]["rawInput"]["operation"];
    assert_eq!(install["wasm_sha256"], receipt.artifact_sha256);
    for permission in permissions {
        assert!(!permission.to_string().contains(SOURCE_SENTINEL));
    }
    let id = h.prompt(&session, "/tool answer {}");
    let (messages, _) = await_phases(&mut h, id, &["allow-once"]);
    assert_eq!(response_text(&messages).trim(), "42");

    let id = h.request(
        "session/new",
        json!({"cwd": h._xdg.path(), "mcpServers": []}),
    );
    let (_, result) = h.await_response(id);
    let other = result["sessionId"].as_str().unwrap().to_owned();
    std::thread::sleep(GATE_FLUSH_GRACE);
    h.drain_pending();
    let id = h.prompt(&other, "/tool answer {}");
    let (messages, _) = await_phases(&mut h, id, &[]);
    assert!(response_text(&messages).contains("NotFound"));
    let id = h.prompt(&session, "/tool answer {}");
    let (messages, _) = await_phases(&mut h, id, &["allow-once"]);
    assert_eq!(response_text(&messages).trim(), "42");

    let cancelled_name = "test:acp-generation/cancelled";
    let request = tool_request(cancelled_name, 43);
    let id = h.prompt(&session, &format!("/generate {request}"));
    let permission = h.await_permission(id);
    h.notify("session/cancel", json!({"sessionId": session}));
    h.respond_permission(&permission, "allow-once");
    let (_, result) = h.await_response(id);
    assert_eq!(result["stopReason"], "cancelled");
    assert_absent(&h, cancelled_name);
    h.close_stdin_and_wait();
    assert!(!h.stderr.lock().unwrap().contains(SOURCE_SENTINEL));
}

#[test]
#[ignore = "requires a real builder image via WASSETTE_ACP_GENERATION_IMAGE"]
fn real_generated_layer_is_not_hot_swapped() {
    let (bin, provider) = artifacts().expect("build the feature-enabled CLI and ACP echo fixture");
    let mut h = start_generation(&bin, &provider);
    let session = h.open_session();
    let source = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../components/acp-uppercase-layer/src/lib.rs"),
    )
    .unwrap();
    let declaration = "#[allow(clippy::all)]\nmod bindings;";
    assert_eq!(source.matches(declaration).count(), 1);
    let name = "test:acp-generation/layer";
    let request = json!({
        "build": {
            "component_name": name,
            "kind": ComponentKind::AcpLayer,
            "world": "generated",
            "wit": "package test:acp-layer-generation; world generated { include wassette:acp/layer@7.0.0; }",
            "source": source.replace(declaration, ""),
        },
        "target": {"mode": "new"},
    });
    let id = h.prompt(&session, &format!("/generate {request}"));
    let (messages, _) = await_phases(&mut h, id, &["allow-once", "allow-once"]);
    let text = response_text(&messages);
    assert!(
        text.starts_with("generation Disposition::LaterSelectionRequired:"),
        "{text}"
    );
    assert!(text.ends_with("handles []"));
    let receipt = h.store().read(name).unwrap().receipt;
    assert_eq!(receipt.kind, StoredArtifactKind::AcpLayer);
    assert!(matches!(
        receipt.validation,
        ValidationEvidence::AcpCompiledAndExportChecked { .. }
    ));
    let id = h.prompt(&session, "/shout");
    let (messages, _) = h.await_response(id);
    assert_eq!(response_text(&messages).trim(), "/shout");
    h.close_stdin_and_wait();
}

#[test]
#[ignore = "requires a real builder image via WASSETTE_ACP_GENERATION_IMAGE"]
fn real_generation_uses_bound_editor_without_layers() {
    let (bin, provider) = artifacts().expect("build the feature-enabled CLI and ACP echo fixture");
    let mut h = start_generation(&bin, &provider);
    let session = h.open_session();
    let name = "test:acp-generation/through-layer";
    let request = tool_request(name, 42);
    let id = h.prompt(&session, &format!("/generate {request}"));
    let (messages, _) = await_phases(&mut h, id, &["allow-once", "allow-once"]);
    assert!(response_text(&messages).starts_with("generation Disposition::SessionTools:"));
    assert_eq!(generated_report(&messages)["component_id"], name);
    let id = h.prompt(&session, "/tool answer {}");
    let (messages, _) = await_phases(&mut h, id, &["allow-once"]);
    assert_eq!(response_text(&messages).trim(), "42");
    h.close_stdin_and_wait();
}

#[test]
#[ignore = "requires a real builder image via WASSETTE_ACP_GENERATION_IMAGE"]
fn real_generation_disconnect_waits_for_private_job_cleanup() {
    let (bin, provider) = artifacts().expect("build the feature-enabled CLI and ACP echo fixture");
    let mut h = start_generation(&bin, &provider);
    let session = h.open_session();
    let name = "test:acp-generation/disconnected";
    let request = tool_request(name, 42);
    let id = h.prompt(&session, &format!("/generate {request}"));
    let permission = h.await_permission(id);
    h.respond_permission(&permission, "allow-once");
    let staging = h._xdg.path().join("data/wassette/builder/staging");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::fs::read_dir(&staging).unwrap().next().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "builder did not create its private job"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    h.close_stdin_and_wait();
    assert!(
        std::fs::read_dir(staging).unwrap().next().is_none(),
        "transport returned before the builder job finished cleanup"
    );
    assert_absent(&h, name);
}
