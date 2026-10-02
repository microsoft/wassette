// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use super::*;

struct Providers {
    _root: tempfile::TempDir,
    alpha: PathBuf,
    beta: PathBuf,
    secrets: PathBuf,
}

impl Providers {
    fn new(alpha_settings: &[(&str, &str)], beta_settings: &[(&str, &str)]) -> Option<Self> {
        let artifact = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/routing-provider/target")
            .join("wasm32-wasip2/release/acp_routing_provider.wasm");
        if !artifact.is_file() {
            assert!(
                std::env::var_os("CI").is_none(),
                "run just build-acp-routing-fixture"
            );
            eprintln!("skipping: run just build-acp-routing-fixture");
            return None;
        }
        let root = tempfile::tempdir().unwrap();
        let secrets = root.path().join("secrets");
        let mut paths = Vec::new();
        for (label, settings) in [("alpha", alpha_settings), ("beta", beta_settings)] {
            let mut bytes = std::fs::read(&artifact).unwrap();
            assert!(matches!(
                wassette::inspect_artifact(&bytes).unwrap().identity,
                Err(wassette::IdentityError::Missing)
            ));
            let mut names = wasm_encoder::ComponentNameSection::new();
            names.component(&format!("test:{label}"));
            bytes.push(names.id());
            names.encode(&mut bytes);
            let path = root.path().join(format!("{label}.wasm"));
            std::fs::write(&path, bytes).unwrap();
            let mut settings = settings.to_vec();
            settings.push(("ROUTING_LABEL", label));
            common::seed_secrets(&path, &secrets, &settings);
            let policy = json!({
                "version": "1.0",
                "permissions": {"environment": {"allow": settings.iter().map(|(key, _)| json!({"key": key})).collect::<Vec<_>>()}}
            });
            std::fs::write(path.with_extension("policy.yaml"), policy.to_string()).unwrap();
            paths.push(path);
        }
        Some(Self {
            alpha: paths.remove(0),
            beta: paths.remove(0),
            secrets,
            _root: root,
        })
    }

    fn start(&self, bin: &Path, extra: &[&str]) -> Harness {
        let mut args = vec!["--secrets-dir", self.secrets.to_str().unwrap()];
        args.extend_from_slice(extra);
        Harness::start_with_providers(bin, &[&self.alpha, &self.beta], &args)
    }
}

fn initialize(h: &mut Harness) -> Value {
    let id = h.request(
        "initialize",
        json!({"protocolVersion": 1, "clientCapabilities": {
            "fs": {"readTextFile": true, "writeTextFile": true}
        }}),
    );
    h.await_response(id).1
}

fn new_session(h: &mut Harness) -> Value {
    let id = h.request(
        "session/new",
        json!({"cwd": std::env::temp_dir(), "mcpServers": []}),
    );
    h.await_response(id).1
}

fn model_value(options: &Value, provider: &str, name: &str) -> String {
    options
        .as_array()
        .unwrap()
        .iter()
        .find(|option| option["id"] == "model")
        .unwrap()["options"]
        .as_array()
        .unwrap()
        .iter()
        .find(|group| group["group"] == provider)
        .unwrap()["options"]
        .as_array()
        .unwrap()
        .iter()
        .find(|option| {
            option["name"]
                .as_str()
                .is_some_and(|value| value.eq_ignore_ascii_case(name))
        })
        .unwrap_or_else(|| panic!("missing model {provider}/{name}: {options}"))["value"]
        .as_str()
        .unwrap()
        .to_string()
}

fn select(h: &mut Harness, session: &str, value: &str) -> Value {
    let id = h.request(
        "session/set_config_option",
        json!({
            "sessionId": session, "configId": "model", "value": value
        }),
    );
    h.await_response(id).1
}

fn prompt_text(h: &mut Harness, session: &str, text: &str) -> String {
    let id = h.prompt(session, text);
    let (updates, response) = h.await_response(id);
    assert_eq!(response["stopReason"], "end_turn");
    for update in updates.iter().filter(|u| u["method"] == "session/update") {
        assert_eq!(update["params"]["sessionId"], session, "{update}");
    }
    response_text(&updates)
}

fn reply(h: &mut Harness, request: &Value, result: Value) {
    let message = json!({"jsonrpc": "2.0", "id": request["id"], "result": result});
    let stdin = h.stdin.as_mut().unwrap();
    writeln!(stdin, "{message}").unwrap();
    stdin.flush().unwrap();
}

fn response_error(h: &mut Harness, id: i64) -> Value {
    loop {
        let response: Value = serde_json::from_str(&h.next_line()).unwrap();
        if response["id"] == id {
            assert!(response["error"].is_object(), "{response}");
            return response["error"].clone();
        }
    }
}

fn reject_tool(h: &mut Harness, id: i64) -> Vec<Value> {
    let mut messages = Vec::new();
    loop {
        let message: Value = serde_json::from_str(&h.next_line()).unwrap();
        if message["id"] == id {
            assert!(message["error"].is_object(), "{message}");
            return messages;
        }
        if message["method"] == "session/request_permission" {
            h.respond_permission(&message, "reject-once");
        }
        messages.push(message);
    }
}

#[test]
fn grouped_models_select_distinct_providers_with_colliding_ids() {
    let Some(bin) = wassette_binary() else { return };
    let Some(providers) = Providers::new(&[], &[]) else {
        return;
    };
    let mut h = providers.start(&bin, &[]);
    let init = initialize(&mut h);
    assert_eq!(init["agentInfo"]["name"], "wassette-acp");
    assert_eq!(init["agentCapabilities"]["loadSession"], false);
    let first = new_session(&mut h);
    let sid = first["sessionId"].as_str().unwrap();
    assert_ne!(sid, "collision");
    let a = model_value(&first["configOptions"], "test:alpha", "Shared");
    let b = model_value(&first["configOptions"], "test:beta", "Shared");
    assert_ne!(a, b);
    assert_eq!(first["configOptions"][0]["currentValue"], a);
    assert!(prompt_text(&mut h, sid, "hello").contains("alpha:shared:plain:hello"));
    assert_eq!(prompt_text(&mut h, sid, "/history"), "alpha:history:1");
    assert!(prompt_text(&mut h, sid, "/version").starts_with("Wassette "));
    assert!(prompt_text(&mut h, sid, "/tools").contains("No tools are available"));
    assert_eq!(prompt_text(&mut h, sid, "/history"), "alpha:history:1");
    let selected = select(&mut h, sid, &b);
    assert_eq!(selected["configOptions"][0]["currentValue"], b);
    assert!(prompt_text(&mut h, sid, "hello").contains("beta:shared:plain:hello"));
    assert_eq!(prompt_text(&mut h, sid, "/history"), "beta:history:1");
    assert!(prompt_text(&mut h, sid, "/version").starts_with("Wassette "));
    assert_eq!(prompt_text(&mut h, sid, "/history"), "beta:history:1");
    let second = new_session(&mut h);
    let sid2 = second["sessionId"].as_str().unwrap();
    assert_ne!(sid, sid2);
    assert!(prompt_text(&mut h, sid2, "second").contains("alpha:shared:plain:second"));
    assert!(prompt_text(&mut h, sid, "still beta").contains("beta:shared:plain:still beta"));
    select(&mut h, sid, &a);
    assert_eq!(prompt_text(&mut h, sid, "/history"), "alpha:history:1");
    assert!(prompt_text(&mut h, sid, "again").contains("alpha:shared:plain:again"));
    let invalid = h.request(
        "session/set_config_option",
        json!({
            "sessionId": sid, "configId": "model", "value": "unknown"
        }),
    );
    response_error(&mut h, invalid);
    assert!(prompt_text(&mut h, sid, "unchanged").contains("alpha:shared:plain:unchanged"));
    h.close_stdin_and_wait();
}

#[test]
fn tools_toggle_applies_to_each_provider_in_the_editor_group() {
    let Some(bin) = wassette_binary() else { return };
    let Some(tool) = filesystem_tool() else {
        return;
    };
    let Some(providers) = Providers::new(&[], &[]) else {
        return;
    };
    let mut h = Harness::start_with_local_source_and_providers(
        &bin,
        &[&providers.alpha, &providers.beta],
        Some(&tool),
        "startup",
        &[],
        &["--secrets-dir", providers.secrets.to_str().unwrap()],
    );
    initialize(&mut h);
    let session = new_session(&mut h);
    let sid = session["sessionId"].as_str().unwrap();
    let beta = model_value(&session["configOptions"], "test:beta", "Shared");
    assert!(!prompt_text(&mut h, sid, "/tools list").contains("| disabled |"));
    assert!(
        prompt_text(&mut h, sid, "/tools disable write-file").contains("disabled for this session")
    );
    assert!(prompt_text(&mut h, sid, "/tools list").contains("| disabled |"));
    select(&mut h, sid, &beta);
    assert!(prompt_text(&mut h, sid, "/tools list").contains("| disabled |"));
    assert!(
        prompt_text(&mut h, sid, "/tools enable write-file").contains("enabled for this session")
    );
    assert!(!prompt_text(&mut h, sid, "/tools list").contains("| disabled |"));
}

#[test]
fn concurrent_callbacks_use_host_ids_and_reply_to_their_own_session() {
    let Some(bin) = wassette_binary() else { return };
    let Some(providers) = Providers::new(&[], &[]) else {
        return;
    };
    let mut h = providers.start(&bin, &[]);
    initialize(&mut h);
    let first = new_session(&mut h);
    let second = new_session(&mut h);
    let a = first["sessionId"].as_str().unwrap();
    let b = second["sessionId"].as_str().unwrap();
    let beta = model_value(&first["configOptions"], "test:beta", "Shared");
    select(&mut h, b, &beta);
    let pa = h.prompt(a, "/permission");
    let permission_a = h.await_permission(pa);
    let busy_version = h.prompt(a, "/version");
    let busy_error = response_error(&mut h, busy_version);
    assert!(
        busy_error["message"].as_str().unwrap().contains("busy"),
        "{busy_error}"
    );
    let busy_tools = h.prompt(a, "/tools list");
    let tools_error = response_error(&mut h, busy_tools);
    assert!(
        tools_error["message"].as_str().unwrap().contains("busy"),
        "{tools_error}"
    );
    let pb = h.prompt(b, "/permission");
    let permission_b = h.await_permission(pb);
    assert_eq!(permission_a["params"]["sessionId"], a);
    assert_eq!(permission_b["params"]["sessionId"], b);
    assert_ne!(permission_a["id"], permission_b["id"]);
    assert_ne!(
        permission_a["params"]["toolCall"]["toolCallId"],
        permission_b["params"]["toolCall"]["toolCallId"]
    );
    h.respond_permission(&permission_b, "allow");
    let (updates_b, _) = h.await_response(pb);
    assert!(response_text(&updates_b).contains("beta"), "{updates_b:?}");
    h.respond_permission(&permission_a, "reject");
    let (updates_a, _) = h.await_response(pa);
    assert!(response_text(&updates_a).contains("alpha"), "{updates_a:?}");
    for (sid, updates) in [(a, updates_a), (b, updates_b)] {
        for update in updates.iter().filter(|u| u["method"] == "session/update") {
            assert_eq!(update["params"]["sessionId"], sid, "{update}");
        }
    }
    let read = h.prompt(b, "/read /project/read.txt");
    loop {
        let request: Value = serde_json::from_str(&h.next_line()).unwrap();
        assert_ne!(request["id"], read, "{request}");
        if request["method"] == "fs/read_text_file" {
            assert_eq!(request["params"]["sessionId"], b);
            reply(&mut h, &request, json!({"content": "editor-sentinel"}));
            break;
        }
    }
    let (updates, _) = h.await_response(read);
    assert!(response_text(&updates).contains("editor-sentinel"));
    let write = h.prompt(b, "/write /project/write.txt written");
    loop {
        let request: Value = serde_json::from_str(&h.next_line()).unwrap();
        assert_ne!(request["id"], write, "{request}");
        if request["method"] == "fs/write_text_file" {
            assert_eq!(request["params"]["sessionId"], b);
            assert_eq!(request["params"]["content"], "written");
            reply(&mut h, &request, json!({}));
            break;
        }
    }
    h.await_response(write);
}

#[test]
fn cancellation_and_busy_selection_do_not_cross_provider_boundaries() {
    let Some(bin) = wassette_binary() else { return };
    let Some(providers) = Providers::new(&[], &[]) else {
        return;
    };
    let mut h = providers.start(&bin, &[]);
    initialize(&mut h);
    let session = new_session(&mut h);
    let sid = session["sessionId"].as_str().unwrap();
    let beta = model_value(&session["configOptions"], "test:beta", "Shared");
    let prompt = h.prompt(sid, "/permission");
    let permission = h.await_permission(prompt);
    let change = h.request(
        "session/set_config_option",
        json!({
            "sessionId": sid, "configId": "model", "value": beta
        }),
    );
    let error = response_error(&mut h, change);
    assert!(error["message"].as_str().unwrap().contains("busy"));
    h.notify("session/cancel", json!({"sessionId": sid}));
    let (_, response) = h.await_response(prompt);
    assert_eq!(response["stopReason"], "cancelled");
    select(&mut h, sid, &beta);
    h.respond_permission(&permission, "allow");
    assert!(
        prompt_text(&mut h, sid, "after cancellation")
            .contains("beta:shared:plain:after cancellation")
    );
    let prompt = h.prompt(sid, "/permission");
    let next = h.await_permission(prompt);
    assert_ne!(permission["id"], next["id"]);
    h.respond_permission(&next, "reject");
    h.await_response(prompt);
    h.close_stdin_and_wait();
}

#[test]
fn provider_data_environment_and_secrets_stay_separate() {
    let Some(bin) = wassette_binary() else { return };
    let Some(providers) = Providers::new(
        &[("TOKEN", "alpha-secret"), ("ONLY_ALPHA", "private")],
        &[("TOKEN", "beta-secret")],
    ) else {
        return;
    };
    let mut h = providers.start(&bin, &[]);
    initialize(&mut h);
    let session = new_session(&mut h);
    let sid = session["sessionId"].as_str().unwrap();
    let alpha = model_value(&session["configOptions"], "test:alpha", "Shared");
    let beta = model_value(&session["configOptions"], "test:beta", "Shared");
    assert!(prompt_text(&mut h, sid, "/secret TOKEN").contains("alpha-secret"));
    prompt_text(&mut h, sid, "/data-write alpha-data");
    select(&mut h, sid, &beta);
    assert!(prompt_text(&mut h, sid, "/secret TOKEN").contains("beta-secret"));
    assert!(!prompt_text(&mut h, sid, "/env ONLY_ALPHA").contains("private"));
    let id = h.prompt(sid, "/data-read");
    assert!(
        response_error(&mut h, id)["message"]
            .as_str()
            .unwrap()
            .contains("data read")
    );
    prompt_text(&mut h, sid, "/data-write beta-data");
    select(&mut h, sid, &alpha);
    assert!(prompt_text(&mut h, sid, "/data-read").contains("alpha-data"));
}

#[test]
fn initialization_intersects_capabilities_and_never_advertises_composite_load() {
    let Some(bin) = wassette_binary() else { return };
    let Some(providers) = Providers::new(
        &[("ROUTING_LOAD_SUPPORTED", "1"), ("ROUTING_MCP_HTTP", "1")],
        &[("ROUTING_LOAD_SUPPORTED", "1")],
    ) else {
        return;
    };
    let mut h = providers.start(&bin, &[]);
    let init = initialize(&mut h);
    assert_eq!(init["agentCapabilities"]["loadSession"], false);
    assert_eq!(init["agentCapabilities"]["mcpCapabilities"]["http"], false);
    let id = h.request(
        "session/load",
        json!({"sessionId": "collision", "cwd": ".", "mcpServers": []}),
    );
    assert!(
        response_error(&mut h, id)["message"]
            .as_str()
            .unwrap()
            .contains("loadSession was not advertised")
    );
    let mut single = Harness::start(
        &bin,
        &providers.alpha,
        &["--secrets-dir", providers.secrets.to_str().unwrap()],
    );
    let init = initialize(&mut single);
    assert_eq!(init["agentInfo"]["name"], "acp-routing-provider");
    assert_eq!(init["agentCapabilities"]["loadSession"], true);
    assert_eq!(init["agentCapabilities"]["mcpCapabilities"]["http"], true);
    let id = single.request(
        "session/load",
        json!({"sessionId": "collision", "cwd": std::env::temp_dir(), "mcpServers": []}),
    );
    let (_, loaded) = single.await_response(id);
    assert_eq!(loaded["configOptions"][0]["id"], "fixture-model");
    assert!(prompt_text(&mut single, "collision", "loaded").contains("alpha:shared:plain:loaded"));
}

#[test]
fn creation_callbacks_are_bound_before_guests_return_colliding_ids() {
    let Some(bin) = wassette_binary() else { return };
    let Some(providers) = Providers::new(
        &[("ROUTING_CALLBACK_NEW", "1")],
        &[("ROUTING_CALLBACK_NEW", "1")],
    ) else {
        return;
    };
    let mut h = providers.start(&bin, &[]);
    initialize(&mut h);
    let mut previous = String::new();
    for _ in 0..2 {
        let id = h.request(
            "session/new",
            json!({"cwd": std::env::temp_dir(), "mcpServers": []}),
        );
        let mut callbacks = Vec::new();
        let session = loop {
            let message: Value = serde_json::from_str(&h.next_line()).unwrap();
            if message["id"] == id {
                assert!(message.get("error").is_none(), "{message}");
                break message["result"].clone();
            }
            assert_ne!(
                message["method"], "session/update",
                "creation update preceded response"
            );
            assert_eq!(message["method"], "fs/read_text_file", "{message}");
            callbacks.push(message["params"]["sessionId"].as_str().unwrap().to_string());
            reply(&mut h, &message, json!({"content": "creation"}));
        };
        let sid = session["sessionId"].as_str().unwrap();
        assert_eq!(callbacks, [sid, sid]);
        assert_ne!(sid, previous);
        previous = sid.to_string();
        std::thread::sleep(GATE_FLUSH_GRACE);
        let notifications = h.drain_pending();
        assert!(
            response_text(&notifications).contains("alpha"),
            "{notifications:?}"
        );
        assert!(
            !response_text(&notifications).contains("beta"),
            "{notifications:?}"
        );
        for notification in notifications {
            assert_eq!(notification["params"]["sessionId"], sid);
        }
    }
}

#[test]
fn failed_group_creation_does_not_publish_or_keep_partial_sessions() {
    let Some(bin) = wassette_binary() else { return };
    let Some(providers) = Providers::new(&[], &[("ROUTING_FAIL_NEW", "1")]) else {
        return;
    };
    let mut h = providers.start(&bin, &[]);
    initialize(&mut h);
    for sequence in 1..=3 {
        let id = h.request(
            "session/new",
            json!({"cwd": std::env::temp_dir(), "mcpServers": []}),
        );
        response_error(&mut h, id);
        assert!(
            h.drain_pending().is_empty(),
            "failed creation leaked notifications"
        );
        let prompt = h.prompt(&format!("wassette-{sequence}"), "not registered");
        assert!(
            response_error(&mut h, prompt)["message"]
                .as_str()
                .unwrap()
                .contains("unknown session")
        );
    }
    h.close_stdin_and_wait();
}

#[test]
fn model_and_other_config_changes_are_provider_local() {
    let Some(bin) = wassette_binary() else { return };
    let Some(providers) = Providers::new(&[], &[]) else {
        return;
    };
    let mut h = providers.start(&bin, &[]);
    initialize(&mut h);
    let session = new_session(&mut h);
    let sid = session["sessionId"].as_str().unwrap();
    let alpha = model_value(&session["configOptions"], "test:alpha", "Shared");
    let beta = model_value(&session["configOptions"], "test:beta", "Alternate");
    select(&mut h, sid, &beta);
    let id = h.request(
        "session/set_config_option",
        json!({
            "sessionId": sid, "configId": "style", "value": "loud"
        }),
    );
    h.await_response(id);
    assert!(prompt_text(&mut h, sid, "changed").contains("beta:alternate:loud:changed"));
    select(&mut h, sid, &alpha);
    assert!(prompt_text(&mut h, sid, "unchanged").contains("alpha:shared:plain:unchanged"));
    let id = h.request(
        "session/set_mode",
        json!({"sessionId": sid, "modeId": "loud"}),
    );
    h.await_response(id);
    assert!(prompt_text(&mut h, sid, "mode").contains("alpha:shared:loud:mode"));
    select(&mut h, sid, &beta);
    assert!(prompt_text(&mut h, sid, "retained").contains("beta:alternate:loud:retained"));
}

#[test]
fn installed_layers_are_not_hot_swapped_into_running_providers() {
    let Some(bin) = wassette_binary() else { return };
    let Some(providers) = Providers::new(&[], &[]) else {
        return;
    };
    let layer = uppercase_layer().expect("run just build-acp-examples");
    let mut h = providers.start(&bin, &[]);
    initialize(&mut h);
    let session = new_session(&mut h);
    let sid = session["sessionId"].as_str().unwrap();
    let id = h.prompt(sid, &format!("/install {}", layer.display()));
    let (messages, _) = h.await_response(id);
    assert!(
        messages.iter().any(|message| {
            message
                .pointer("/params/update/content/0/content/text")
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains("Installed ACP component"))
        }),
        "{messages:?}"
    );
    let id = h.prompt(sid, "/shout");
    assert!(
        response_error(&mut h, id)["message"]
            .as_str()
            .unwrap()
            .contains("unknown command")
    );
    h.close_stdin_and_wait();
}

#[test]
fn ordinary_tool_approvals_are_not_shared_between_providers_or_sessions() {
    let Some(bin) = wassette_binary() else { return };
    let Some(providers) = Providers::new(&[], &[]) else {
        return;
    };
    let Some(tool) = filesystem_tool() else {
        return;
    };
    let mut h = Harness::start_with_local_source_and_providers(
        &bin,
        &[&providers.alpha, &providers.beta],
        Some(&tool),
        "startup",
        &["microsoft:filesystem-rs".to_owned()],
        &["--secrets-dir", providers.secrets.to_str().unwrap()],
    );
    initialize(&mut h);
    let session = new_session(&mut h);
    let sid = session["sessionId"].as_str().unwrap();
    let alpha = model_value(&session["configOptions"], "test:alpha", "Shared");
    let beta = model_value(&session["configOptions"], "test:beta", "Shared");
    assert!(
        prompt_text(&mut h, sid, "/tools enable write-file").contains("enabled for this session")
    );
    let command = h.write_command("alpha-authorized");
    let id = h.prompt(sid, &command);
    let (messages, _) = h.await_response_with_permission(id, "allow-always");
    assert_eq!(permission_count(&messages), 1);
    select(&mut h, sid, &beta);
    let command = h.write_command("beta-not-authorized");
    let id = h.prompt(sid, &command);
    let messages = reject_tool(&mut h, id);
    assert_eq!(
        permission_count(&messages),
        1,
        "alpha's approval leaked to beta"
    );
    let output = h._xdg.path().join("tool-output/written.txt");
    assert_eq!(
        std::fs::read_to_string(&output).unwrap(),
        "alpha-authorized"
    );
    select(&mut h, sid, &alpha);
    let id = h.prompt(sid, &h.write_command("alpha-still-authorized"));
    let (messages, _) = h.await_response_with_permission(id, "reject-once");
    assert_eq!(permission_count(&messages), 0);
    let second = new_session(&mut h);
    let sid2 = second["sessionId"].as_str().unwrap();
    std::thread::sleep(GATE_FLUSH_GRACE);
    h.drain_pending();
    assert!(prompt_text(&mut h, sid2, "/tools list").contains("microsoft:filesystem-rs/"));
    assert!(
        prompt_text(&mut h, sid2, "/tools enable write-file").contains("enabled for this session")
    );
    let id = h.prompt(sid2, &h.write_command("other-session"));
    let request = h.await_permission(id);
    assert_eq!(request["params"]["sessionId"], sid2);
    h.notify("session/cancel", json!({"sessionId": sid2}));
    let (messages, response) = h.await_response(id);
    assert_eq!(response["stopReason"], "cancelled");
    assert!(
        messages.iter().any(|m| {
            m["params"]["update"]["toolCallId"] == request["params"]["toolCall"]["toolCallId"]
                && m["params"]["update"]["status"] == "failed"
        }),
        "cancellation tool ID did not match permission: {messages:?}"
    );
    h.respond_permission(&request, "allow-always");
    let id = h.prompt(sid2, &h.write_command("late-approval"));
    let messages = reject_tool(&mut h, id);
    assert_eq!(permission_count(&messages), 1, "late approval was retained");
    assert_eq!(
        std::fs::read_to_string(output).unwrap(),
        "alpha-still-authorized"
    );
}

#[test]
fn model_less_providers_are_omitted_and_the_first_eligible_provider_is_active() {
    let Some(bin) = wassette_binary() else { return };
    let Some(providers) = Providers::new(&[("ROUTING_NO_MODELS", "1")], &[]) else {
        return;
    };
    let mut h = providers.start(&bin, &[]);
    initialize(&mut h);
    let session = new_session(&mut h);
    let sid = session["sessionId"].as_str().unwrap();
    let models = &session["configOptions"][0];
    assert_eq!(models["options"].as_array().unwrap().len(), 1);
    assert_eq!(models["options"][0]["group"], "test:beta");
    let beta = model_value(&session["configOptions"], "test:beta", "Shared");
    assert_eq!(models["currentValue"], beta);
    let text = prompt_text(&mut h, sid, "selected");
    assert!(text.contains("beta:shared:plain:selected"), "{text}");
    assert!(
        !text.contains("alpha"),
        "omitted provider emitted updates: {text}"
    );
    select(&mut h, sid, &beta);
}

#[test]
fn no_eligible_models_is_an_error_but_single_model_less_provider_still_works() {
    let Some(bin) = wassette_binary() else { return };
    let Some(providers) =
        Providers::new(&[("ROUTING_NO_MODELS", "1")], &[("ROUTING_NO_MODELS", "1")])
    else {
        return;
    };
    let mut h = providers.start(&bin, &[]);
    initialize(&mut h);
    let id = h.request(
        "session/new",
        json!({"cwd": std::env::temp_dir(), "mcpServers": []}),
    );
    assert!(
        response_error(&mut h, id)["message"]
            .as_str()
            .unwrap()
            .contains("no selectable ACP providers")
    );
    assert!(h.drain_pending().is_empty());
    let mut single = Harness::start(
        &bin,
        &providers.alpha,
        &["--secrets-dir", providers.secrets.to_str().unwrap()],
    );
    initialize(&mut single);
    let session = new_session(&mut single);
    assert_eq!(session["sessionId"], "collision");
    assert!(prompt_text(&mut single, "collision", "single").contains("alpha:shared:plain:single"));
}

#[test]
fn a_rejected_advertised_model_does_not_switch_the_active_provider() {
    let Some(bin) = wassette_binary() else { return };
    let Some(providers) = Providers::new(&[], &[("ROUTING_REJECT_ALTERNATE", "1")]) else {
        return;
    };
    let mut h = providers.start(&bin, &[]);
    initialize(&mut h);
    let session = new_session(&mut h);
    let sid = session["sessionId"].as_str().unwrap();
    let beta = model_value(&session["configOptions"], "test:beta", "Alternate");
    let id = h.request(
        "session/set_config_option",
        json!({
            "sessionId": sid, "configId": "model", "value": beta
        }),
    );
    response_error(&mut h, id);
    assert!(prompt_text(&mut h, sid, "retained").contains("alpha:shared:plain:retained"));
}

#[test]
fn empty_model_choices_are_omitted_like_absent_choices() {
    let Some(bin) = wassette_binary() else { return };
    let Some(providers) = Providers::new(&[], &[("ROUTING_EMPTY_MODELS", "1")]) else {
        return;
    };
    let mut h = providers.start(&bin, &[]);
    initialize(&mut h);
    let session = new_session(&mut h);
    let sid = session["sessionId"].as_str().unwrap();
    assert_eq!(
        session["configOptions"][0]["options"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        session["configOptions"][0]["options"][0]["group"],
        "test:alpha"
    );
    assert!(prompt_text(&mut h, sid, "selected").contains("alpha:shared:plain:selected"));
}

#[test]
fn cancelling_creation_discards_its_routes_and_late_callbacks() {
    let Some(bin) = wassette_binary() else { return };
    let Some(providers) = Providers::new(&[("ROUTING_CALLBACK_NEW", "1")], &[]) else {
        return;
    };
    let mut h = providers.start(&bin, &[]);
    initialize(&mut h);
    let id = h.request(
        "session/new",
        json!({"cwd": std::env::temp_dir(), "mcpServers": []}),
    );
    let callback: Value = serde_json::from_str(&h.next_line()).unwrap();
    assert_eq!(callback["method"], "fs/read_text_file");
    let sid = callback["params"]["sessionId"].as_str().unwrap();
    h.notify("session/cancel", json!({"sessionId": sid}));
    assert!(
        response_error(&mut h, id)["message"]
            .as_str()
            .unwrap()
            .contains("creation cancelled")
    );
    reply(&mut h, &callback, json!({"content": "late"}));
    let id = h.prompt(sid, "not registered");
    assert!(
        response_error(&mut h, id)["message"]
            .as_str()
            .unwrap()
            .contains("unknown session")
    );
    assert!(h.drain_pending().is_empty());
    h.close_stdin_and_wait();
}
