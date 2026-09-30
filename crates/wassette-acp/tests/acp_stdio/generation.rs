// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Opt-in real-VM ACP tests. Build the feature-enabled binary and echo fixture,
//! set WASSETTE_ACP_GENERATION_CONFIG to a trusted local operator profile, then
//! run `cargo test -p wassette-acp --features component-generation --test acp_stdio
//! generation::real_ -- --ignored --test-threads=1`.

use wassette::generation::{ComponentKind, GenerationConfig};
use wassette::store::{InstallIntent, SourceIdentity, StoreError};

use super::*;

const SOURCE_SENTINEL: &str = "ACP_GENERATION_PRIVATE_SOURCE_SENTINEL";

fn tool_request(name: &str, value: u32, intent: InstallIntent) -> Value {
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
        "intent": intent,
    })
}

fn wit_package(directory: &Path) -> String {
    let mut paths: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "wit"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty());
    paths
        .into_iter()
        .enumerate()
        .map(|(index, path)| {
            let text = std::fs::read_to_string(path).unwrap();
            if index == 0 {
                text
            } else {
                text.split_once(';').unwrap().1.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn start_generation(
    bin: &Path,
    provider: &Path,
    acp_dependencies: bool,
    selected_layer: Option<&Path>,
) -> Harness {
    let selected = std::env::var_os("WASSETTE_ACP_GENERATION_CONFIG")
        .expect("set WASSETTE_ACP_GENERATION_CONFIG to a trusted, signed-helper operator profile");
    let mut config = GenerationConfig::read(Path::new(&selected)).unwrap();
    assert!(config.allow_build && config.allow_install && config.allow_expose);
    let xdg = tempfile::tempdir_in(".").unwrap();
    let root = xdg.path().canonicalize().unwrap();
    config.builder.staging_root = root.join("builder-staging");
    std::fs::create_dir(&config.builder.staging_root).unwrap();
    if acp_dependencies {
        let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("wit/acp");
        config.builder.wit_dependencies = vec![
            wit_package(&wit.join("deps/wasmcloud-secrets")),
            wit_package(&wit.join("deps/wassette-component-tools")),
            wit_package(
                &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../wit/component-generation"),
            ),
            wit_package(&wit),
        ];
    }
    let profile = root.join("operator.json");
    std::fs::write(&profile, serde_json::to_vec(&config).unwrap()).unwrap();
    let components = root.join("data/wassette/components");
    let secrets = root.join("config/wassette/secrets");
    let mut args = vec![
        std::ffi::OsString::from("--generation-config"),
        profile.into_os_string(),
        "--component-dir".into(),
        components.into_os_string(),
        "--secrets-dir".into(),
        secrets.into_os_string(),
    ];
    if let Some(layer) = selected_layer {
        args.extend([
            "--layer".into(),
            layer.as_os_str().to_owned(),
            "--allow-shared-grants".into(),
        ]);
    }
    let mut harness = Harness::spawn(bin, provider, args, xdg, None);
    harness.line_timeout = Duration::from_secs(180);
    harness
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
#[ignore = "requires an explicitly selected real Hyperlight helper/initrd profile"]
fn real_generation_permissions_store_and_session_scope() {
    let (bin, provider) = artifacts().expect("build the feature-enabled CLI and ACP echo fixture");
    let mut h = start_generation(&bin, &provider, false, None);
    let session = h.open_session();
    let rejected_name = "test:acp-generation/rejected";
    let request = tool_request(rejected_name, 40, InstallIntent::ExposeTools);
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

    let hidden_name = "test:acp-generation/install-only";
    let request = tool_request(hidden_name, 41, InstallIntent::InstallOnly);
    let id = h.prompt(&session, &format!("/generate {request}"));
    let (messages, _) = await_phases(&mut h, id, &["allow-once", "allow-once"]);
    assert!(
        response_text(&messages).starts_with("generation Disposition::Installed:"),
        "{messages:?}"
    );
    assert_eq!(generated_report(&messages)["component_id"], hidden_name);
    let receipt = h.store().read(hidden_name).unwrap().receipt;
    assert_eq!(receipt.intent, InstallIntent::InstallOnly);
    assert!(matches!(receipt.source, SourceIdentity::Generated { .. }));
    let id = h.prompt(&session, "/tool answer {}");
    let (messages, _) = await_phases(&mut h, id, &[]);
    assert!(response_text(&messages).contains("NotFound"));

    let exposed_name = "test:acp-generation/exposed";
    let request = tool_request(exposed_name, 42, InstallIntent::ExposeTools);
    let id = h.prompt(&session, &format!("/generate {request}"));
    let (messages, _) = await_phases(&mut h, id, &["allow-once", "allow-once", "allow-once"]);
    assert!(
        response_text(&messages).starts_with("generation Disposition::SessionTools:"),
        "{messages:?}"
    );
    let report = generated_report(&messages);
    let receipt = h.store().read(exposed_name).unwrap().receipt;
    assert_eq!(report["revision"], receipt.revision.to_string());
    assert_eq!(receipt.intent, InstallIntent::ExposeTools);
    assert!(receipt.origin.generation.is_some());
    let permissions: Vec<_> = messages
        .iter()
        .filter(|m| m["method"] == "session/request_permission")
        .collect();
    let install = &permissions[1]["params"]["toolCall"]["rawInput"]["operation"];
    let expose = &permissions[2]["params"]["toolCall"]["rawInput"]["operation"];
    assert_eq!(install["wasm_sha256"], expose["preview"]["wasm_sha256"]);
    assert_eq!(install["wasm_sha256"], receipt.artifact_sha256);
    assert!(expose["scope"].as_str().unwrap().contains("shared store"));
    assert!(
        expose["scope"]
            .as_str()
            .unwrap()
            .contains("only this ACP session")
    );
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
    let request = tool_request(cancelled_name, 43, InstallIntent::InstallOnly);
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
#[ignore = "requires an explicitly selected real Hyperlight helper/initrd profile"]
fn real_generated_layer_requires_explicit_selection() {
    let (bin, provider) = artifacts().expect("build the feature-enabled CLI and ACP echo fixture");
    let mut h = start_generation(&bin, &provider, true, None);
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
        "intent": InstallIntent::InstallOnly,
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
    assert_eq!(receipt.intent, InstallIntent::InstallOnly);
    assert!(matches!(
        receipt.validation,
        ValidationEvidence::AcpCompiledAndExportChecked { .. }
    ));
    let id = h.prompt(&session, "/shout");
    let (messages, _) = h.await_response(id);
    assert_eq!(response_text(&messages).trim(), "/shout");
    let components = h._xdg.path().join("data/wassette/components");
    let secrets = h._xdg.path().join("config/wassette/secrets");
    h.close_stdin_and_wait();

    let mut selected = Harness::start(
        &bin,
        &provider,
        &[
            "--component-dir",
            components.to_str().unwrap(),
            "--secrets-dir",
            secrets.to_str().unwrap(),
            "--layer",
            name,
            "--allow-shared-grants",
        ],
    );
    let session = selected.open_session();
    let id = selected.prompt(&session, "/shout");
    let (messages, _) = selected.await_response(id);
    assert!(response_text(&messages).contains("CAPS LOCK ENGAGED!"));
    let id = selected.prompt(&session, "explicit layer");
    let (messages, _) = selected.await_response(id);
    assert_eq!(response_text(&messages).trim(), "EXPLICIT LAYER");
    selected.close_stdin_and_wait();
}

#[test]
#[ignore = "requires an explicitly selected real Hyperlight helper/initrd profile"]
fn real_generation_uses_bound_editor_with_an_explicit_layer() {
    let (bin, provider) = artifacts().expect("build the feature-enabled CLI and ACP echo fixture");
    let layer = uppercase_layer().expect("build the ACP uppercase layer fixture");
    let mut h = start_generation(&bin, &provider, false, Some(&layer));
    let session = h.open_session();
    let name = "test:acp-generation/through-layer";
    let request = tool_request(name, 42, InstallIntent::ExposeTools);
    let id = h.prompt(&session, &format!("/generate {request}"));
    let (messages, _) = await_phases(&mut h, id, &["allow-once", "allow-once", "allow-once"]);
    assert!(response_text(&messages).starts_with("generation Disposition::SessionTools:"));
    assert_eq!(generated_report(&messages)["component_id"], name);
    let id = h.prompt(&session, "/tool answer {}");
    let (messages, _) = await_phases(&mut h, id, &["allow-once"]);
    assert_eq!(response_text(&messages).trim(), "42");
    h.close_stdin_and_wait();
}

#[test]
#[ignore = "requires an explicitly selected real Hyperlight helper/initrd profile"]
fn real_generation_disconnect_waits_for_private_job_cleanup() {
    let (bin, provider) = artifacts().expect("build the feature-enabled CLI and ACP echo fixture");
    let mut h = start_generation(&bin, &provider, false, None);
    let session = h.open_session();
    let name = "test:acp-generation/disconnected";
    let request = tool_request(name, 42, InstallIntent::InstallOnly);
    let id = h.prompt(&session, &format!("/generate {request}"));
    let permission = h.await_permission(id);
    h.respond_permission(&permission, "allow-once");
    let staging = h._xdg.path().join("builder-staging");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::fs::read_dir(&staging).unwrap().next().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "native builder did not create its private job"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    h.close_stdin_and_wait();
    assert!(
        std::fs::read_dir(staging).unwrap().next().is_none(),
        "transport returned before the native job finished cleanup"
    );
    assert_absent(&h, name);
}
