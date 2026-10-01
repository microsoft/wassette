// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! End-to-end tests for the model-backed ACP providers
//! (`components/acp-ollama-provider`, `components/acp-copilot-provider`).
//!
//! Each test spawns the built `wassette acp` binary against a provider and
//! points the provider at a local `wiremock` server standing in for the model
//! API, so no network access or credentials are needed. `--allow-all` lets
//! the provider inherit the test's environment (the mock's URL) and reach
//! localhost.
//!
//! Build the artifacts first (`just test-acp` does both):
//!
//! ```sh
//! cargo build -p wassette-mcp-server
//! just build-acp-examples
//! ```
//!
//! Outside CI, missing artifacts skip these tests; in CI they fail.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;
use common::NamedFixture;

const LINE_TIMEOUT: Duration = Duration::from_secs(60);

fn wassette_binary() -> Option<PathBuf> {
    if let Some(bin) = std::env::var_os("WASSETTE_ACP_TEST_BINARY") {
        let bin = PathBuf::from(bin);
        return bin.is_file().then_some(bin);
    }
    let exe = std::env::current_exe().ok()?;
    let profile_dir = exe.parent()?.parent()?;
    let bin = profile_dir.join(if cfg!(windows) {
        "wassette.exe"
    } else {
        "wassette"
    });
    bin.is_file().then_some(bin)
}

/// `components/<dir>/target/wasm32-wasip2/release/<file>.wasm`.
fn component(dir: &str, file: &str) -> Option<NamedFixture> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../components")
        .join(dir)
        .join("target/wasm32-wasip2/release")
        .join(format!("{file}.wasm"));
    path.is_file().then(|| NamedFixture::copy(&path))
}

/// The `wassette` binary and the provider, or `None` (outside CI) with an
/// explanation of what to build.
fn artifacts(dir: &str, file: &str) -> Option<(PathBuf, NamedFixture)> {
    let found = wassette_binary().zip(component(dir, file));
    if found.is_none() {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI requires the `wassette` binary and {dir}; run \
             `cargo build -p wassette-mcp-server` and `just build-acp-examples`"
        );
        eprintln!(
            "skipping: `wassette` binary or {dir} not found; run \
             `cargo build -p wassette-mcp-server` and `just build-acp-examples`"
        );
    }
    found
}

/// A running `wassette acp` process speaking JSON-RPC on stdio.
struct Harness {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    stderr: Arc<Mutex<String>>,
    _xdg: tempfile::TempDir,
    next_id: i64,
}

impl Harness {
    fn start(bin: &Path, wasm: &Path, extra: &[&str], env: &[(&str, &str)]) -> Harness {
        let xdg = tempfile::tempdir().expect("tempdir");
        let mut cmd = Command::new(bin);
        cmd.arg("acp").arg("--provider").arg(wasm).args(extra);
        for sub in ["data", "config", "state"] {
            let dir = xdg.path().join(sub);
            std::fs::create_dir_all(&dir).expect("create xdg dir");
            cmd.env(format!("XDG_{}_HOME", sub.to_uppercase()), dir);
        }
        // Keep ambient credentials and endpoints from leaking into the guest,
        // which inherits this environment under `--allow-all`.
        for var in [
            "RUST_LOG",
            "COPILOT_GITHUB_TOKEN",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "COPILOT_MODEL",
            "COPILOT_BASE_URL",
            "COPILOT_TOKEN_URL",
            "OLLAMA_URL",
            "OLLAMA_MODEL",
        ] {
            cmd.env_remove(var);
        }
        cmd.envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn wassette acp");

        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        let stderr = child.stderr.take().expect("stderr");
        let captured = Arc::new(Mutex::new(String::new()));
        let sink = captured.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut sink = sink.lock().unwrap();
                sink.push_str(&line);
                sink.push('\n');
            }
        });

        Harness {
            child,
            stdin: Some(stdin),
            lines: rx,
            stderr: captured,
            _xdg: xdg,
            next_id: 0,
        }
    }

    fn request(&mut self, method: &str, params: Value) -> i64 {
        self.next_id += 1;
        let id = self.next_id;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let stdin = self.stdin.as_mut().expect("stdin is open");
        writeln!(stdin, "{msg}").expect("write request");
        stdin.flush().expect("flush request");
        id
    }

    /// Read until the response to `id`, returning the notifications seen on
    /// the way and the response's result.
    fn await_response(&mut self, id: i64) -> (Vec<Value>, Value) {
        let mut notifications = Vec::new();
        loop {
            let line = match self.lines.recv_timeout(LINE_TIMEOUT) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => panic!(
                    "timed out waiting for response {id}; stderr:\n{}",
                    self.stderr.lock().unwrap()
                ),
                Err(RecvTimeoutError::Disconnected) => panic!(
                    "the agent closed stdout; stderr:\n{}",
                    self.stderr.lock().unwrap()
                ),
            };
            let msg: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("stdout line is not JSON ({e}): {line}"));
            if msg.get("id").and_then(Value::as_i64) == Some(id) {
                assert!(
                    msg.get("error").is_none(),
                    "request {id} failed: {}\nstderr:\n{}",
                    msg["error"],
                    self.stderr.lock().unwrap()
                );
                return (notifications, msg["result"].clone());
            }
            notifications.push(msg);
        }
    }

    /// `initialize` → `session/new` → one text prompt. Returns the prompt's
    /// session updates and result.
    fn prompt_once(&mut self, text: &str) -> (Vec<Value>, Value) {
        let id = self.request(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {}}),
        );
        self.await_response(id);
        let cwd = tempfile::tempdir().expect("cwd");
        let id = self.request("session/new", json!({"cwd": cwd.path(), "mcpServers": []}));
        let (_, session) = self.await_response(id);
        let session_id = session["sessionId"].as_str().expect("sessionId").to_owned();
        let id = self.request(
            "session/prompt",
            json!({"sessionId": session_id, "prompt": [{"type": "text", "text": text}]}),
        );
        self.await_response(id)
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn session_updates<'a>(updates: &'a [Value], kind: &str) -> Vec<&'a Value> {
    updates
        .iter()
        .filter(|m| m["method"] == "session/update")
        .map(|m| &m["params"]["update"])
        .filter(|u| u["sessionUpdate"] == kind)
        .collect()
}

fn agent_text(updates: &[Value]) -> String {
    session_updates(updates, "agent_message_chunk")
        .into_iter()
        .filter_map(|u| u["content"]["text"].as_str())
        .collect()
}

fn ndjson(chunks: &[Value]) -> String {
    chunks.iter().map(|c| format!("{c}\n")).collect()
}

#[test]
fn ollama_provider_streams_a_reply_and_reports_usage() {
    let Some((bin, wasm)) = artifacts("acp-ollama-provider", "acp_ollama_provider") else {
        return;
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(async {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"models": [{"name": "llama3.2"}]})),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/show"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "capabilities": ["tools"],
                "model_info": {"llama.context_length": 8192}
            })))
            .mount(&server)
            .await;
        let body = ndjson(&[
            json!({"message": {"role": "assistant", "content": "Hello, "}, "done": false}),
            json!({"message": {"role": "assistant", "content": "world!"}, "done": false}),
            json!({"message": {"role": "assistant", "content": ""}, "done": true,
                   "prompt_eval_count": 100, "eval_count": 20}),
        ]);
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/x-ndjson"))
            .mount(&server)
            .await;
        server
    });
    let chat_url = format!("{}/api/chat", server.uri());

    let mut h = Harness::start(
        &bin,
        &wasm,
        &["--allow-all"],
        &[("OLLAMA_URL", &chat_url), ("OLLAMA_MODEL", "llama3.2")],
    );
    let (updates, result) = h.prompt_once("hi");

    assert_eq!(result["stopReason"], "end_turn", "{result}");
    assert_eq!(agent_text(&updates), "Hello, world!", "{updates:#?}");
    let usage = session_updates(&updates, "usage_update");
    let last = usage.last().expect("a usage_update");
    assert_eq!(last["used"], 120, "{last}");
    assert_eq!(last["size"], 8192, "{last}");
}

#[test]
fn copilot_provider_uses_the_stored_secret_and_reports_cost() {
    let Some((bin, wasm)) = artifacts("acp-copilot-provider", "acp_copilot_provider") else {
        return;
    };
    const TOKEN: &str = "gho_e2e_stored_secret";
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(async {
        let server = MockServer::start().await;
        // The editor token exchange is unavailable for this token type, so
        // the provider falls back to sending the GitHub token directly.
        Mock::given(method("GET"))
            .and(path("/copilot_internal/v2/token"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{
                    "id": "gpt-e2e",
                    "name": "gpt-e2e",
                    "model_picker_category": "powerful",
                    "capabilities": {
                        "type": "chat",
                        "supports": {"reasoning_effort": ["low", "high"]},
                        "limits": {"max_context_window_tokens": 128000}
                    }
                }]
            })))
            .mount(&server)
            .await;
        let body = format!(
            "data: {}\n\ndata: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            json!({"choices": [{"delta": {"role": "assistant", "content": "Hi there!"},
                                "finish_reason": null}]}),
            json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
            json!({"choices": [],
                   "usage": {"prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120},
                   "copilot_usage": {"total_nano_aiu": 39_000_000}}),
        );
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
            .mount(&server)
            .await;
        server
    });

    let secrets = tempfile::tempdir().unwrap();
    common::seed_secrets(&wasm, secrets.path(), &[("github_token", TOKEN)]);
    let base_url = server.uri();
    let token_url = format!("{base_url}/copilot_internal/v2/token");
    let mut h = Harness::start(
        &bin,
        &wasm,
        &[
            "--allow-all",
            "--secrets-dir",
            secrets.path().to_str().unwrap(),
        ],
        &[
            ("COPILOT_BASE_URL", &base_url),
            ("COPILOT_TOKEN_URL", &token_url),
            ("COPILOT_MODEL", "gpt-e2e"),
        ],
    );
    let (updates, result) = h.prompt_once("hi");

    assert_eq!(result["stopReason"], "end_turn", "{result}");
    assert_eq!(agent_text(&updates), "Hi there!", "{updates:#?}");
    let usage = session_updates(&updates, "usage_update");
    let last = usage.last().expect("a usage_update");
    assert_eq!(last["used"], 120, "{last}");
    assert_eq!(last["size"], 128000, "{last}");
    assert_eq!(last["cost"]["currency"], "AIU", "{last}");
    let amount = last["cost"]["amount"].as_f64().expect("cost amount");
    assert!((amount - 0.039).abs() < 1e-9, "{last}");
}

#[test]
fn copilot_provider_round_trips_boolean_and_legacy_approval_options() {
    let Some((bin, wasm)) = artifacts("acp-copilot-provider", "acp_copilot_provider") else {
        return;
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(async {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/copilot_internal/v2/token"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{
                    "id": "gpt-e2e",
                    "name": "gpt-e2e",
                    "capabilities": {"type": "chat"}
                }]
            })))
            .mount(&server)
            .await;
        server
    });
    let base_url = server.uri();
    let token_url = format!("{base_url}/copilot_internal/v2/token");

    for boolean_supported in [true, false] {
        let mut h = Harness::start(
            &bin,
            &wasm,
            &["--allow-all"],
            &[
                ("COPILOT_GITHUB_TOKEN", "gho_e2e_config_options"),
                ("COPILOT_BASE_URL", &base_url),
                ("COPILOT_TOKEN_URL", &token_url),
                ("COPILOT_MODEL", "gpt-e2e"),
            ],
        );
        let capabilities = if boolean_supported {
            json!({"session": {"configOptions": {"boolean": {}}}})
        } else {
            json!({})
        };
        let id = h.request(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": capabilities}),
        );
        h.await_response(id);
        let cwd = tempfile::tempdir().unwrap();
        let id = h.request("session/new", json!({"cwd": cwd.path(), "mcpServers": []}));
        let (_, session) = h.await_response(id);
        let session_id = session["sessionId"].as_str().expect("sessionId");

        let check = |response: &Value, auto_approve, terminal_enabled| {
            let options = response["configOptions"].as_array().expect("configOptions");
            let approval = options.iter().find(|o| o["id"] == "allow-all").unwrap();
            assert_eq!(approval["name"], "Auto-approve", "{response}");
            if boolean_supported {
                assert_eq!(approval["type"], "boolean", "{response}");
                assert_eq!(approval["currentValue"], auto_approve, "{response}");
                assert!(approval.get("options").is_none(), "{response}");
                assert!(approval.get("category").is_none(), "{response}");
                let terminal = options.iter().find(|o| o["id"] == "terminal").unwrap();
                assert_eq!(terminal["name"], "Terminal", "{response}");
                assert_eq!(terminal["type"], "boolean", "{response}");
                assert_eq!(terminal["currentValue"], terminal_enabled, "{response}");
            } else {
                assert_eq!(approval["type"], "select", "{response}");
                assert_eq!(
                    approval["currentValue"],
                    if auto_approve { "on" } else { "off" },
                    "{response}"
                );
                assert_eq!(approval["options"].as_array().unwrap().len(), 2);
                assert!(!options.iter().any(|o| o["id"] == "terminal"));
            }

            for id in ["model", "mode"] {
                let option = options.iter().find(|o| o["id"] == id).unwrap();
                assert_eq!(option["type"], "select", "{response}");
            }
        };
        check(&session, false, false);
        let approval_value = |enabled| {
            if boolean_supported {
                json!(enabled)
            } else {
                json!(if enabled { "on" } else { "off" })
            }
        };
        let approval_type = if boolean_supported {
            "boolean"
        } else {
            "select"
        };
        for enabled in [true, false] {
            let id = h.request(
                "session/set_config_option",
                json!({
                    "sessionId": session_id, "configId": "allow-all",
                    "type": approval_type, "value": approval_value(enabled),
                }),
            );
            let (_, response) = h.await_response(id);
            check(&response, enabled, false);
        }
        if boolean_supported {
            for enabled in [true, false] {
                let id = h.request(
                    "session/set_config_option",
                    json!({
                        "sessionId": session_id, "configId": "terminal",
                        "type": "boolean", "value": enabled,
                    }),
                );
                let (_, response) = h.await_response(id);
                check(&response, false, enabled);
            }
        }

        for (mode, auto_approve) in [("autopilot", true), ("agent", false)] {
            let id = h.request(
                "session/set_config_option",
                json!({"sessionId": session_id, "configId": "mode", "value": mode}),
            );
            let (_, response) = h.await_response(id);
            check(&response, auto_approve, false);
            if auto_approve {
                let id = h.request(
                    "session/set_config_option",
                    json!({
                        "sessionId": session_id, "configId": "allow-all",
                        "type": approval_type, "value": approval_value(false),
                    }),
                );
                let (_, response) = h.await_response(id);
                check(&response, true, false);
            }
        }
    }
}

#[test]
fn copilot_provider_only_advertises_terminal_when_enabled() {
    let Some((bin, wasm)) = artifacts("acp-copilot-provider", "acp_copilot_provider") else {
        return;
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(async {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/copilot_internal/v2/token"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": "gpt-e2e", "capabilities": {"type": "chat"}}]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(
                    "data: {}\n\ndata: [DONE]\n\n",
                    json!({"choices": [{"delta": {"content": "ok"}, "finish_reason": "stop"}]})
                ),
                "text/event-stream",
            ))
            .mount(&server)
            .await;
        server
    });
    let base_url = server.uri();
    let token_url = format!("{base_url}/copilot_internal/v2/token");
    let mut h = Harness::start(
        &bin,
        &wasm,
        &["--allow-all"],
        &[
            ("COPILOT_GITHUB_TOKEN", "gho_e2e_terminal_tools"),
            ("COPILOT_BASE_URL", &base_url),
            ("COPILOT_TOKEN_URL", &token_url),
            ("COPILOT_MODEL", "gpt-e2e"),
        ],
    );
    let id = h.request(
        "initialize",
        json!({"protocolVersion": 1, "clientCapabilities": {
            "session": {"configOptions": {"boolean": {}}}
        }}),
    );
    h.await_response(id);
    let cwd = tempfile::tempdir().unwrap();
    let id = h.request("session/new", json!({"cwd": cwd.path(), "mcpServers": []}));
    let (_, session) = h.await_response(id);
    let session_id = session["sessionId"].as_str().unwrap();
    for (turn, enabled) in [false, true, false].into_iter().enumerate() {
        if turn > 0 {
            let id = h.request(
                "session/set_config_option",
                json!({"sessionId": session_id, "configId": "terminal",
                    "type": "boolean", "value": enabled}),
            );
            h.await_response(id);
        }
        let id = h.request(
            "session/prompt",
            json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "hi"}]}),
        );
        let (_, response) = h.await_response(id);
        assert_eq!(response["stopReason"], "end_turn", "{response}");
        let requests = rt.block_on(server.received_requests()).unwrap();
        let chat: Value = serde_json::from_slice(
            &requests
                .iter()
                .rfind(|r| r.url.path() == "/chat/completions")
                .unwrap()
                .body,
        )
        .unwrap();
        let names: Vec<&str> = chat["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            if enabled {
                vec!["read_text_file", "write_text_file", "run_terminal_command"]
            } else {
                vec!["read_text_file", "write_text_file"]
            },
            "{chat}"
        );
    }
}

/// Generation needs the feature-enabled binary (`cargo build -p
/// wassette-mcp-server --features component-generation`) and a supported
/// builder platform; the profile below never starts a VM.
#[cfg(all(
    feature = "component-generation",
    any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        )
    )
))]
mod generation {
    use super::*;

    /// Writes a syntactically valid operator profile whose helper digest can never
    /// match, so the host accepts the profile at startup but never executes the
    /// placeholder helper.
    fn placeholder_generation_profile(dir: &Path, allow_install: bool) -> PathBuf {
        std::fs::write(dir.join("helper"), b"not a builder").unwrap();
        std::fs::write(dir.join("initrd"), b"not an image").unwrap();
        std::fs::create_dir_all(dir.join("staging")).unwrap();
        let profile = dir.join("operator.json");
        std::fs::write(
            &profile,
            json!({
                "builder": {
                    "helper_path": "helper",
                    "helper_sha256": "0".repeat(64),
                    "initrd_path": "initrd",
                    "initrd_sha256": "0".repeat(64),
                    "staging_root": "staging",
                    "wit_dependencies": []
                },
                "allow_build": true,
                "allow_install": allow_install,
            })
            .to_string(),
        )
        .unwrap();
        profile
    }

    /// Mock Copilot API whose first chat round calls `build_component` with
    /// `arguments` and whose later rounds end the turn.
    async fn build_component_mock(arguments: &Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/copilot_internal/v2/token"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": "gpt-e2e", "capabilities": {"type": "chat"}}]
            })))
            .mount(&server)
            .await;
        let tool_call = json!({"choices": [{"delta": {"tool_calls": [{
            "index": 0, "id": "call_build", "type": "function",
            "function": {"name": "build_component", "arguments": arguments.to_string()}
        }]}, "finish_reason": "tool_calls"}]});
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!("data: {tool_call}\n\ndata: [DONE]\n\n"),
                "text/event-stream",
            ))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(
                    "data: {}\n\ndata: [DONE]\n\n",
                    json!({"choices": [{"delta": {"content": "done"}, "finish_reason": "stop"}]})
                ),
                "text/event-stream",
            ))
            .mount(&server)
            .await;
        server
    }

    /// The Copilot provider advertises `build_component` only when the host's
    /// operator profile permits build and install, and a model call reaches the
    /// host's `builder.generate` (and its editor approval) rather than being
    /// rejected as disabled. The placeholder helper makes the build itself fail.
    #[test]
    fn copilot_provider_build_component_reaches_host_generation() {
        let Some((bin, wasm)) = artifacts("acp-copilot-provider", "acp_copilot_provider") else {
            return;
        };
        let arguments = json!({
            "component_name": "local:answer",
            "world": "tool",
            "wit": "package local:answer; world tool { export answer: func() -> u32; }",
            "source": "struct Component;\nimpl bindings::Guest for Component { fn answer() -> u32 { 42 } }\nbindings::export!(Component with_types_in bindings);\n",
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        let server = rt.block_on(build_component_mock(&arguments));
        let base_url = server.uri();
        let token_url = format!("{base_url}/copilot_internal/v2/token");
        let env = [
            ("COPILOT_GITHUB_TOKEN", "gho_e2e_generation"),
            ("COPILOT_BASE_URL", base_url.as_str()),
            ("COPILOT_TOKEN_URL", token_url.as_str()),
            ("COPILOT_MODEL", "gpt-e2e"),
        ];
        let chat_requests = |server: &MockServer| -> Vec<Value> {
            rt.block_on(server.received_requests())
                .unwrap()
                .iter()
                .filter(|r| r.url.path() == "/chat/completions")
                .map(|r| serde_json::from_slice(&r.body).unwrap())
                .collect()
        };
        let tool_names = |chat: &Value| -> Vec<String> {
            chat["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["function"]["name"].as_str().unwrap().to_owned())
                .collect()
        };

        // A profile that does not permit install keeps the tool hidden, and the
        // model's call never reaches a mock that would request it.
        let denied = tempfile::tempdir().unwrap();
        let profile = placeholder_generation_profile(denied.path(), false);
        let mut h = Harness::start(
            &bin,
            &wasm,
            &[
                "--allow-all",
                "--generation-config",
                profile.to_str().unwrap(),
            ],
            &env,
        );
        let (_, result) = h.prompt_once("hi");
        drop(h);
        // The first (tool-call) mock answered; the provider reports the unknown
        // tool, then the fallback mock ends the turn.
        assert_eq!(result["stopReason"], "end_turn", "{result}");
        let chats = chat_requests(&server);
        assert!(
            !tool_names(&chats[0]).contains(&"build_component".to_owned()),
            "{}",
            chats[0]
        );
        drop(server);

        let server = rt.block_on(build_component_mock(&arguments));
        let base_url = server.uri();
        let token_url = format!("{base_url}/copilot_internal/v2/token");
        let env = [
            ("COPILOT_GITHUB_TOKEN", "gho_e2e_generation"),
            ("COPILOT_BASE_URL", base_url.as_str()),
            ("COPILOT_TOKEN_URL", token_url.as_str()),
            ("COPILOT_MODEL", "gpt-e2e"),
        ];
        let allowed = tempfile::tempdir().unwrap();
        let profile = placeholder_generation_profile(allowed.path(), true);
        let mut h = Harness::start(
            &bin,
            &wasm,
            &[
                "--allow-all",
                "--generation-config",
                profile.to_str().unwrap(),
            ],
            &env,
        );
        let id = h.request(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {}}),
        );
        h.await_response(id);
        let cwd = tempfile::tempdir().unwrap();
        let id = h.request("session/new", json!({"cwd": cwd.path(), "mcpServers": []}));
        let (_, session) = h.await_response(id);
        let session_id = session["sessionId"].as_str().unwrap().to_owned();
        let prompt = h.request(
            "session/prompt",
            json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "build it"}]}),
        );
        let mut permissions = Vec::new();
        let result = loop {
            let line = h.lines.recv_timeout(LINE_TIMEOUT).unwrap_or_else(|e| {
                panic!(
                    "waiting for the prompt ({e}); stderr:\n{}",
                    h.stderr.lock().unwrap()
                )
            });
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == json!(prompt) && message.get("method").is_none() {
                assert!(message.get("error").is_none(), "{message}");
                break message["result"].clone();
            }
            if message["method"] == "session/request_permission" {
                let response = json!({
                    "jsonrpc": "2.0", "id": message["id"],
                    "result": {"outcome": {"outcome": "selected", "optionId": "allow-once"}},
                });
                let stdin = h.stdin.as_mut().unwrap();
                writeln!(stdin, "{response}").unwrap();
                stdin.flush().unwrap();
                permissions.push(message);
            }
        };
        assert_eq!(result["stopReason"], "end_turn", "{result}");
        assert!(
            permissions
                .iter()
                .any(|p| p["params"]["toolCall"]["title"] == "Build component in isolated VM"),
            "the host did not request build approval: {permissions:#?}"
        );
        let chats = chat_requests(&server);
        assert_eq!(chats.len(), 2, "{chats:#?}");
        assert!(
            tool_names(&chats[0]).contains(&"build_component".to_owned()),
            "{}",
            chats[0]
        );
        let tool_result = chats[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == "tool")
            .and_then(|m| m["content"].as_str())
            .unwrap_or_else(|| panic!("no tool result: {}", chats[1]))
            .to_owned();
        assert!(
            !tool_result.contains("disabled") && !tool_result.contains("unknown tool"),
            "{tool_result}"
        );
        assert!(
            tool_result.contains("unavailable") || tool_result.contains("Build failed"),
            "{tool_result}"
        );
    }
}
