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
