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
    /// Local component drop directory and tool output directory, kept alive
    /// for the harness's lifetime.
    _scratch: Vec<tempfile::TempDir>,
    next_id: i64,
}

impl Harness {
    fn start(bin: &Path, wasm: &Path, extra: &[&str], env: &[(&str, &str)]) -> Harness {
        let xdg = tempfile::tempdir().expect("tempdir");
        Harness::spawn(bin, wasm, extra, env, xdg, Vec::new())
    }

    /// Like [`Harness::start`], but also drops `tool` into a local component
    /// directory the host installs at startup. The tool is installed and
    /// *not* exposed: only `/tools enable` admits it to the session.
    ///
    /// Returns the harness and the directory the tool is granted write access
    /// to, so a test can check what a routed call actually did.
    fn start_with_local_tool(
        bin: &Path,
        wasm: &Path,
        tool: &Path,
        env: &[(&str, &str)],
    ) -> (Harness, PathBuf) {
        let drops = tempfile::tempdir().expect("drop dir");
        let output = tempfile::tempdir().expect("tool output dir");
        let output_path = output.path().canonicalize().expect("canonical output dir");
        // Grant the tool nothing but this throwaway directory, so a routed
        // call can be observed without touching the rest of the filesystem.
        let policy = json!({
            "version": "1.0",
            "permissions": {"storage": {"allow": [
                {"uri": format!("fs://{}", output_path.display()), "access": ["read", "write"]}
            ]}},
        });
        std::fs::write(
            drops.path().join("broker-tool.policy.yaml"),
            serde_json::to_vec(&policy).expect("encode policy"),
        )
        .expect("write policy");
        std::fs::copy(tool, drops.path().join("broker-tool.wasm")).expect("copy tool");

        let xdg = tempfile::tempdir().expect("tempdir");
        let drop_dir = drops.path().to_str().expect("utf-8 drop dir").to_owned();
        let extra = [
            "--allow-all",
            "--local-component-dir",
            &drop_dir,
            "--local-components",
            "startup",
        ];
        let harness = Harness::spawn(bin, wasm, &extra, env, xdg, vec![drops, output]);
        (harness, output_path)
    }

    fn spawn(
        bin: &Path,
        wasm: &Path,
        extra: &[&str],
        env: &[(&str, &str)],
        xdg: tempfile::TempDir,
        scratch: Vec<tempfile::TempDir>,
    ) -> Harness {
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
            _scratch: scratch,
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

    /// Read until the response to `id`, answering any permission request on
    /// the way with `option_id`.
    fn await_response_with_permission(&mut self, id: i64, option_id: &str) -> (Vec<Value>, Value) {
        let mut notifications = Vec::new();
        loop {
            let line = match self.lines.recv_timeout(LINE_TIMEOUT) {
                Ok(line) => line,
                Err(error) => panic!(
                    "waiting for response {id}: {error}; stderr:\n{}",
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
            if msg["method"] == "session/request_permission" {
                let response = json!({
                    "jsonrpc": "2.0", "id": msg["id"],
                    "result": {"outcome": {"outcome": "selected", "optionId": option_id}},
                });
                let stdin = self.stdin.as_mut().expect("stdin is open");
                writeln!(stdin, "{response}").expect("write permission response");
                stdin.flush().expect("flush permission response");
            }
            notifications.push(msg);
        }
    }

    /// `initialize` (declaring boolean config options) → `session/new`,
    /// returning the new session id.
    fn open_session(&mut self) -> String {
        let id = self.request(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {
                "session": {"configOptions": {"boolean": {}}}
            }}),
        );
        self.await_response(id);
        let cwd = tempfile::tempdir().expect("cwd");
        let id = self.request("session/new", json!({"cwd": cwd.path(), "mcpServers": []}));
        let (_, session) = self.await_response(id);
        let session_id = session["sessionId"].as_str().expect("sessionId").to_owned();
        // `cwd` only has to exist while the session is created.
        drop(cwd);
        session_id
    }

    /// Send one prompt turn and return its session updates and result.
    fn prompt(&mut self, session_id: &str, text: &str) -> (Vec<Value>, Value) {
        let id = self.request(
            "session/prompt",
            json!({"sessionId": session_id, "prompt": [{"type": "text", "text": text}]}),
        );
        self.await_response(id)
    }

    /// Run a host-side slash command (`/tools …`), which never reaches the
    /// model, and return the host's reply text.
    fn slash(&mut self, session_id: &str, command: &str) -> String {
        let (updates, response) = self.prompt(session_id, command);
        assert_eq!(response["stopReason"], "end_turn", "{command}: {response}");
        agent_text(&updates)
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
            json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "/tools list"}]}),
        );
        let (updates, _) = h.await_response(id);
        let table = updates
            .iter()
            .filter_map(|update| update["params"]["update"]["content"]["text"].as_str())
            .collect::<String>();
        assert!(
            table.contains(&format!(
                "| `terminal` | `host` | {} |",
                if enabled { "enabled" } else { "disabled" }
            )),
            "{updates:?}"
        );
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
/// builder platform.
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

    fn start_generation(bin: &Path, wasm: &Path, image: bool, env: &[(&str, &str)]) -> Harness {
        let xdg = tempfile::tempdir_in(".").unwrap();
        if image {
            let builder = xdg.path().join("data/wassette/builder");
            std::fs::create_dir_all(&builder).unwrap();
            std::fs::write(builder.join("rust-initrd.cpio"), b"not an image").unwrap();
        }
        Harness::spawn(bin, wasm, &["--allow-all"], env, xdg, Vec::new())
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

    /// The Copilot provider advertises `build_component` only when the builder
    /// image exists, and a model call reaches editor approval rather than being
    /// rejected as disabled. The placeholder image makes the build itself fail.
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

        // Without the image, the model-facing build tool is absent.
        let mut h = start_generation(&bin, &wasm, false, &env);
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
        let mut h = start_generation(&bin, &wasm, true, &env);
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

// -----------------------------------------------------------------------------
// Component-broker exposure, end to end through the mock `/chat/completions`.
// -----------------------------------------------------------------------------

/// A scripted sequence of `/chat/completions` bodies. Each request consumes
/// the next entry; the last one repeats, so a test only scripts the rounds it
/// actually cares about.
struct ChatScript(Mutex<std::collections::VecDeque<String>>);

impl ChatScript {
    fn new(bodies: impl IntoIterator<Item = String>) -> Self {
        ChatScript(Mutex::new(bodies.into_iter().collect()))
    }
}

impl wiremock::Respond for ChatScript {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        let mut bodies = self.0.lock().unwrap();
        let body = if bodies.len() > 1 {
            bodies.pop_front().expect("a scripted body")
        } else {
            bodies.front().expect("a scripted body").clone()
        };
        ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
    }
}

fn sse(events: &[Value]) -> String {
    let mut body: String = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect();
    body.push_str("data: [DONE]\n\n");
    body
}

/// An SSE round that just answers with text.
fn text_round(text: &str) -> String {
    sse(&[json!({"choices": [{"delta": {"content": text}, "finish_reason": "stop"}]})])
}

/// An SSE round that asks for one tool call.
fn tool_call_round(id: &str, name: &str, arguments: Value) -> String {
    sse(&[json!({"choices": [{"delta": {"tool_calls": [{
        "index": 0,
        "id": id,
        "type": "function",
        "function": {"name": name, "arguments": arguments.to_string()},
    }]}, "finish_reason": "tool_calls"}]})])
}

/// Mock the Copilot endpoints a provider needs before it can chat.
async fn copilot_mocks(server: &MockServer, chat: ChatScript) {
    // No editor token exchange: the provider falls back to the GitHub token.
    Mock::given(method("GET"))
        .and(path("/copilot_internal/v2/token"))
        .respond_with(ResponseTemplate::new(404))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": "gpt-e2e", "capabilities": {"type": "chat"}}]
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(chat)
        .mount(server)
        .await;
}

/// Every `tools` entry the provider sent on its most recent chat request.
fn last_chat_tools(rt: &tokio::runtime::Runtime, server: &MockServer) -> Vec<Value> {
    let requests = rt.block_on(server.received_requests()).unwrap();
    let body = &requests
        .iter()
        .rfind(|r| r.url.path() == "/chat/completions")
        .expect("a chat request")
        .body;
    let chat: Value = serde_json::from_slice(body).expect("chat request is JSON");
    chat["tools"].as_array().cloned().unwrap_or_default()
}

fn tool_names(tools: &[Value]) -> Vec<&str> {
    tools
        .iter()
        .map(|tool| tool["function"]["name"].as_str().expect("a function name"))
        .collect()
}

/// The provider's own tools, always present regardless of the broker.
const BUILT_IN_TOOLS: [&str; 2] = ["read_text_file", "write_text_file"];

/// `/tools enable` admits a Wassette component to the session, and the
/// Copilot provider advertises it to the model on the *next* turn as an
/// OpenAI-compatible function built from the component's JSON Schema.
/// `/tools disable` withdraws it again. Nothing is advertised by default.
#[test]
fn copilot_provider_advertises_broker_tools_only_while_exposed() {
    let Some((bin, wasm)) = artifacts("acp-copilot-provider", "acp_copilot_provider") else {
        return;
    };
    let Some(tool) = filesystem_tool() else {
        return;
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(async {
        let server = MockServer::start().await;
        copilot_mocks(&server, ChatScript::new([text_round("ok")])).await;
        server
    });
    let base_url = server.uri();
    let token_url = format!("{base_url}/copilot_internal/v2/token");
    let (mut h, _output) = Harness::start_with_local_tool(
        &bin,
        &wasm,
        &tool,
        &[
            ("COPILOT_GITHUB_TOKEN", "gho_e2e_broker_tools"),
            ("COPILOT_BASE_URL", &base_url),
            ("COPILOT_TOKEN_URL", &token_url),
            ("COPILOT_MODEL", "gpt-e2e"),
        ],
    );
    let sid = h.open_session();

    // The component is installed but not exposed, so the host offers the
    // provider nothing and the model sees only the built-in tools.
    let listed = h.slash(&sid, "/tools list");
    assert!(listed.contains("| disabled |"), "{listed}");
    h.prompt(&sid, "hi");
    assert_eq!(
        tool_names(&last_chat_tools(&rt, &server)),
        BUILT_IN_TOOLS.to_vec(),
        "a tool that is not exposed must never reach the model"
    );

    // Admit one export. The host says it takes effect on the next turn.
    let name = exposed_export_name(&listed);
    let enabled = h.slash(&sid, &format!("/tools enable {name}"));
    assert!(enabled.contains("enabled for this session"), "{enabled}");

    h.prompt(&sid, "hi again");
    let tools = last_chat_tools(&rt, &server);
    let names = tool_names(&tools);
    assert_eq!(
        names.len(),
        BUILT_IN_TOOLS.len() + 1,
        "exactly one broker tool should be added: {names:?}"
    );
    assert_eq!(&names[..BUILT_IN_TOOLS.len()], &BUILT_IN_TOOLS[..]);
    let broker_tool = tools.last().expect("the broker tool");
    assert_eq!(broker_tool["type"], "function", "{broker_tool}");

    // It is advertised as a real OpenAI function: an API-safe name, a
    // description, and an object schema with explicit properties.
    let advertised = broker_tool["function"]["name"].as_str().unwrap();
    assert!(
        advertised
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        "{advertised} is not an API-safe function name"
    );
    assert!(advertised.contains("write-file"), "{advertised}");
    assert!(
        broker_tool["function"]["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "{broker_tool}"
    );
    let parameters = &broker_tool["function"]["parameters"];
    assert_eq!(parameters["type"], "object", "{broker_tool}");
    assert!(parameters["properties"].is_object(), "{broker_tool}");
    assert!(
        parameters["properties"]
            .as_object()
            .unwrap()
            .contains_key("path"),
        "the component's own schema should be forwarded verbatim: {parameters}"
    );

    // Withdrawing it takes the function away again on the next turn.
    let disabled = h.slash(&sid, &format!("/tools disable {name}"));
    assert!(disabled.contains("disabled for this session"), "{disabled}");
    h.prompt(&sid, "and again");
    assert_eq!(
        tool_names(&last_chat_tools(&rt, &server)),
        BUILT_IN_TOOLS.to_vec(),
        "a withdrawn tool must stop being advertised"
    );
}

/// A tool call the model makes against an exposed component is routed back
/// through the broker: the host prompts for permission, runs the component,
/// and the result is fed to the model as a `tool` message.
#[test]
fn copilot_provider_routes_broker_tool_calls_through_the_host() {
    let Some((bin, wasm)) = artifacts("acp-copilot-provider", "acp_copilot_provider") else {
        return;
    };
    let Some(tool) = filesystem_tool() else {
        return;
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(async {
        let server = MockServer::start().await;
        copilot_mocks(&server, ChatScript::new([text_round("ok")])).await;
        server
    });
    let base_url = server.uri();
    let token_url = format!("{base_url}/copilot_internal/v2/token");
    let (mut h, output) = Harness::start_with_local_tool(
        &bin,
        &wasm,
        &tool,
        &[
            ("COPILOT_GITHUB_TOKEN", "gho_e2e_broker_call"),
            ("COPILOT_BASE_URL", &base_url),
            ("COPILOT_TOKEN_URL", &token_url),
            ("COPILOT_MODEL", "gpt-e2e"),
        ],
    );
    let sid = h.open_session();
    let listed = h.slash(&sid, "/tools list");
    let name = exposed_export_name(&listed);
    assert!(
        h.slash(&sid, &format!("/tools enable {name}"))
            .contains("enabled for this session")
    );

    // Learn the exact function name the provider advertises, then script the
    // model to call it.
    h.prompt(&sid, "hi");
    let tools = last_chat_tools(&rt, &server);
    let advertised = tools.last().expect("the broker tool")["function"]["name"]
        .as_str()
        .expect("a function name")
        .to_owned();

    let written = output.join("broker-routed.txt");
    rt.block_on(async {
        server.reset().await;
        copilot_mocks(
            &server,
            ChatScript::new([
                tool_call_round(
                    "call-1",
                    &advertised,
                    json!({"path": written.to_str().unwrap(), "content": "routed"}),
                ),
                text_round("done"),
            ]),
        )
        .await;
    });

    let id = h.request(
        "session/prompt",
        json!({"sessionId": sid, "prompt": [{"type": "text", "text": "write it"}]}),
    );
    let (updates, response) = h.await_response_with_permission(id, "allow-once");
    assert_eq!(response["stopReason"], "end_turn", "{response}");

    // The host — not the provider — asked for permission and reported the
    // call, so the component's grants stay on its side of the boundary.
    assert!(
        updates
            .iter()
            .any(|m| m["method"] == "session/request_permission"),
        "the host should have prompted for permission: {updates:#?}"
    );
    assert!(
        !session_updates(&updates, "tool_call").is_empty(),
        "the host should have reported the tool call: {updates:#?}"
    );

    // The component really ran, inside the only directory it was granted.
    assert_eq!(
        std::fs::read_to_string(&written).expect("the tool wrote its file"),
        "routed"
    );

    // The provider fed the broker's result back as a `tool` message keyed to
    // the model's own call id, and the model's follow-up answer reached the
    // editor.
    let requests = rt.block_on(server.received_requests()).unwrap();
    let follow_up: Value = serde_json::from_slice(
        &requests
            .iter()
            .rfind(|r| r.url.path() == "/chat/completions")
            .expect("a follow-up chat request")
            .body,
    )
    .unwrap();
    let messages = follow_up["messages"].as_array().expect("messages");
    let result = messages
        .iter()
        .find(|m| m["role"] == "tool")
        .unwrap_or_else(|| panic!("no tool result was fed back: {follow_up}"));
    assert_eq!(result["tool_call_id"], "call-1", "{result}");
    let content = result["content"].as_str().unwrap_or_default();
    assert!(
        !content.is_empty() && !content.starts_with("Error:"),
        "the broker result should be a success: {result}"
    );
    assert_eq!(agent_text(&updates), "done", "{updates:#?}");
}

/// Resolve the fully-qualified `write-file` export from a `/tools list` table.
fn exposed_export_name(listed: &str) -> String {
    listed
        .lines()
        .find(|line| line.contains("write-file"))
        .unwrap_or_else(|| panic!("no write-file row in:\n{listed}"))
        .split('`')
        .nth(1)
        .expect("a quoted tool name")
        .to_owned()
}

/// The ordinary tool component the broker tests expose.
fn filesystem_tool() -> Option<NamedFixture> {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/filesystem-rs/target")
        });
    let path = target_dir.join("wasm32-wasip2/release/filesystem.wasm");
    if path.is_file() {
        return Some(NamedFixture::copy(&path));
    }
    assert!(
        std::env::var_os("CI").is_none(),
        "CI requires filesystem-rs; run `just build-acp-tool-fixture`"
    );
    eprintln!("skipping: filesystem-rs not found; run `just build-acp-tool-fixture`");
    None
}
