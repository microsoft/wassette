// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! End-to-end tests for `wassette acp` over its real stdio transport.
//!
//! These spawn the built `wassette` binary, speak newline-delimited
//! JSON-RPC to it exactly as an editor would, and drive the in-tree echo
//! provider through a full session. They exercise the wiring no unit test
//! can: the subcommand, the component load, the wasm chain, the bridge,
//! and the promise that **stdout carries protocol and nothing else**.
//!
//! Both artifacts are built out-of-band (`just test-acp` does it):
//!
//! ```sh
//! cargo build -p wassette-mcp-server
//! (cd components/acp-echo-provider && cargo build --release --target wasm32-wasip2)
//! ```
//!
//! When either is missing the tests print why and pass, so a plain
//! `cargo test --workspace` on a machine without the `wasm32-wasip2`
//! target stays green.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

/// How long to wait for any single line of output. Generous: the first
/// response includes compiling the provider component.
const LINE_TIMEOUT: Duration = Duration::from_secs(60);

/// The host holds `session/update` notifications emitted during
/// `session/new` until just after the `session/new` response goes out,
/// then flushes them (see `bridge/gate.rs`). Waiting the flush out keeps
/// `session/new`'s own updates from landing in the middle of a later
/// assertion. A client that does *not* wait is still served correctly —
/// `a_prompt_before_the_gate_flush_still_streams_first` covers that.
const GATE_FLUSH_GRACE: Duration = Duration::from_millis(500);

/// The `wassette` binary under test: a sibling of the test executable's
/// directory (`target/<profile>/deps/<test>` → `target/<profile>`).
fn wassette_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let profile_dir = exe.parent()?.parent()?;
    let bin = profile_dir.join(if cfg!(windows) {
        "wassette.exe"
    } else {
        "wassette"
    });
    bin.is_file().then_some(bin)
}

/// The echo provider component, built by `just build-acp-examples`.
fn echo_provider() -> Option<PathBuf> {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../components/acp-echo-provider/target")
        });
    let path = target_dir.join("wasm32-wasip2/release/acp_echo_provider.wasm");
    path.is_file().then_some(path)
}

fn uppercase_layer() -> Option<PathBuf> {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../components/acp-uppercase-layer/target")
        });
    let path = target_dir.join("wasm32-wasip2/release/acp_uppercase_layer.wasm");
    path.is_file().then_some(path)
}

/// Both artifacts, or `None` with an explanation of what to build.
fn artifacts() -> Option<(PathBuf, PathBuf)> {
    let Some(bin) = wassette_binary() else {
        eprintln!(
            "skipping: `wassette` binary not found; run `cargo build -p wassette-mcp-server`"
        );
        return None;
    };
    let Some(wasm) = echo_provider() else {
        eprintln!(
            "skipping: ACP echo provider not found in its component target directory \
             (or CARGO_TARGET_DIR); run `just build-acp-examples`"
        );
        return None;
    };
    Some((bin, wasm))
}

/// A running `wassette acp` process plus its stdout line stream.
struct Harness {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    /// Every line stdout has produced, in order. Used to assert the
    /// channel stayed pure JSON-RPC.
    seen: Vec<String>,
    stderr: Arc<Mutex<String>>,
    /// Kept alive for the process's lifetime: the XDG roots the host
    /// reads and writes, redirected away from the developer's real
    /// component store.
    _xdg: tempfile::TempDir,
    next_id: i64,
}

impl Harness {
    /// Start `wassette acp --provider <wasm> [extra…]` with fresh XDG
    /// directories.
    fn start(bin: &Path, wasm: &Path, extra: &[&str]) -> Harness {
        let xdg = tempfile::tempdir().expect("tempdir");
        let data = xdg.path().join("data");
        let config = xdg.path().join("config");
        let state = xdg.path().join("state");
        for dir in [&data, &config, &state] {
            std::fs::create_dir_all(dir).expect("create xdg dir");
        }

        let mut cmd = Command::new(bin);
        cmd.arg("acp")
            .arg("--provider")
            .arg(wasm)
            .args(extra)
            .env("XDG_DATA_HOME", &data)
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_STATE_HOME", &state)
            // The host prefers RUST_LOG over --log-level; clear it so a
            // developer's ambient value cannot change what is logged.
            .env_remove("RUST_LOG")
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
        // Drain stderr so the host never blocks on a full pipe. The
        // content is logging; these tests only care that it is not
        // stdout.
        let stderr = child.stderr.take().expect("stderr");
        let stderr_output = Arc::new(Mutex::new(String::new()));
        let captured = stderr_output.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                captured.lock().unwrap().push_str(&line);
                captured.lock().unwrap().push('\n');
            }
        });

        Harness {
            child,
            stdin,
            lines: rx,
            seen: Vec::new(),
            stderr: stderr_output,
            _xdg: xdg,
            next_id: 0,
        }
    }

    /// Send a request and return the id it was assigned.
    fn request(&mut self, method: &str, params: Value) -> i64 {
        self.next_id += 1;
        let id = self.next_id;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{msg}").expect("write request");
        self.stdin.flush().expect("flush request");
        id
    }

    /// Read one line of stdout, recording it.
    fn next_line(&mut self) -> String {
        match self.lines.recv_timeout(LINE_TIMEOUT) {
            Ok(line) => {
                self.seen.push(line.clone());
                line
            }
            Err(RecvTimeoutError::Timeout) => panic!(
                "timed out after {LINE_TIMEOUT:?} waiting for output; saw so far:\n{}\nstderr:\n{}",
                self.seen.join("\n"),
                self.stderr.lock().unwrap()
            ),
            Err(RecvTimeoutError::Disconnected) => panic!(
                "the agent closed stdout; saw so far:\n{}",
                self.seen.join("\n")
            ),
        }
    }

    /// Read until the response to `id` arrives, returning the
    /// notifications seen on the way plus the response's result.
    fn await_response(&mut self, id: i64) -> (Vec<Value>, Value) {
        let mut notifications = Vec::new();
        loop {
            let line = self.next_line();
            let msg: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("stdout line is not JSON ({e}): {line}"));
            if msg.get("id").and_then(Value::as_i64) == Some(id) {
                assert!(
                    msg.get("error").is_none(),
                    "request {id} failed: {}",
                    msg["error"]
                );
                return (notifications, msg["result"].clone());
            }
            notifications.push(msg);
        }
    }

    /// Collect whatever arrives in the next few hundred milliseconds
    /// without requiring anything to.
    fn drain_pending(&mut self) -> Vec<Value> {
        let mut out = Vec::new();
        while let Ok(line) = self.lines.recv_timeout(Duration::from_millis(200)) {
            self.seen.push(line.clone());
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                out.push(v);
            }
        }
        out
    }

    /// `initialize` → `session/new`, returning the new session id, then
    /// wait out the gate flush so `session/new`'s own updates don't land
    /// in the middle of what a later assertion is reading.
    fn open_session(&mut self) -> String {
        let session_id = self.open_session_without_grace();
        std::thread::sleep(GATE_FLUSH_GRACE);
        self.drain_pending();
        session_id
    }

    /// `initialize` → `session/new`, returning as soon as the response
    /// lands — the way an editor that prompts immediately behaves.
    fn open_session_without_grace(&mut self) -> String {
        let id = self.request(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {}}),
        );
        let (_, init) = self.await_response(id);
        assert!(
            init.get("protocolVersion").is_some(),
            "initialize result has no protocolVersion: {init}"
        );

        let id = self.request(
            "session/new",
            json!({"cwd": std::env::temp_dir(), "mcpServers": []}),
        );
        let (_, new_session) = self.await_response(id);
        new_session["sessionId"]
            .as_str()
            .unwrap_or_else(|| panic!("session/new result has no sessionId: {new_session}"))
            .to_string()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Text carried by an `agent_message_chunk` session update, if that is
/// what `msg` is.
fn agent_message_chunk_text(msg: &Value) -> Option<&str> {
    if msg.get("method")? != "session/update" {
        return None;
    }
    let update = msg.get("params")?.get("update")?;
    if update.get("sessionUpdate")? != "agent_message_chunk" {
        return None;
    }
    update.get("content")?.get("text")?.as_str()
}

#[test]
fn initialize_advertises_the_echo_provider() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let mut h = Harness::start(&bin, &wasm, &[]);

    let id = h.request(
        "initialize",
        json!({"protocolVersion": 1, "clientCapabilities": {}}),
    );
    let (_, result) = h.await_response(id);

    assert!(
        result.get("protocolVersion").is_some(),
        "no protocolVersion in {result}"
    );
    assert_eq!(
        result["agentInfo"]["name"], "acp-echo-provider",
        "agentInfo should name the wasm component that answered: {result}"
    );
    assert_eq!(
        result["agentCapabilities"]["loadSession"], false,
        "{result}"
    );
}

#[test]
fn fresh_provider_instance_receives_initialize_before_new_session() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let mut h = Harness::start(&bin, &wasm, &[]);
    let id = h.request(
        "initialize",
        json!({"protocolVersion": 1, "clientCapabilities": {}}),
    );
    h.await_response(id);
    let id = h.request(
        "session/new",
        json!({"cwd": std::env::temp_dir(), "mcpServers": []}),
    );
    let (_, result) = h.await_response(id);
    assert!(result["sessionId"].is_string(), "{result}");
}

#[test]
fn echo_provider_advertises_host_terminal_option_to_boolean_clients() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let mut h = Harness::start(&bin, &wasm, &[]);
    let id = h.request(
        "initialize",
        json!({"protocolVersion": 1, "clientCapabilities": {
            "session": {"configOptions": {"boolean": {}}}
        }}),
    );
    h.await_response(id);
    let id = h.request(
        "session/new",
        json!({"cwd": std::env::temp_dir(), "mcpServers": []}),
    );
    let (_, session) = h.await_response(id);
    assert_eq!(session["configOptions"][0]["id"], "terminal", "{session}");
    assert_eq!(session["configOptions"][0]["type"], "boolean", "{session}");
    assert_eq!(
        session["configOptions"][0]["currentValue"], false,
        "{session}"
    );
}

#[test]
fn a_prompt_streams_chunks_and_ends_the_turn() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let mut h = Harness::start(&bin, &wasm, &[]);
    let session_id = h.open_session();
    assert!(!session_id.is_empty(), "empty sessionId");

    let prompt = "sandboxed hello";
    let id = h.request(
        "session/prompt",
        json!({
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": prompt}],
        }),
    );
    let (mut updates, result) = h.await_response(id);
    // The gate can still be draining when the response lands, so keep
    // reading rather than assuming every update preceded it.
    updates.extend(h.drain_pending());

    assert_eq!(
        result["stopReason"], "end_turn",
        "unexpected stop reason in {result}"
    );

    let chunks: Vec<&str> = updates
        .iter()
        .filter_map(agent_message_chunk_text)
        .collect();
    assert!(
        !chunks.is_empty(),
        "no agent_message_chunk updates; saw: {updates:#?}"
    );
    let echoed: String = chunks.concat();
    assert!(
        echoed.contains(prompt),
        "the echoed text `{echoed}` does not contain the prompt `{prompt}`"
    );
    for update in updates.iter().filter(|u| u["method"] == "session/update") {
        assert_eq!(
            update["params"]["sessionId"],
            session_id.as_str(),
            "update carried the wrong session id: {update}"
        );
    }
}

/// Prompting the instant `session/new` returns — inside the gate's flush
/// delay — must still stream the answer *before* the turn's response.
///
/// The gate holds `session/update`s until it believes the editor has
/// registered the session, and it used to reach that belief only on a
/// timer. A client that prompted first therefore had its whole turn
/// buffered and replayed after `end_turn`: the editor saw a completed
/// turn with no text in it, and one that stops reading at `end_turn` saw
/// nothing at all. The inbound `session/prompt` is itself proof the
/// editor knows the session id, so it now opens the gate on arrival.
#[test]
fn a_prompt_before_the_gate_flush_still_streams_first() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let mut h = Harness::start(&bin, &wasm, &[]);
    // Deliberately no GATE_FLUSH_GRACE: this is the race.
    let session_id = h.open_session_without_grace();

    let prompt = "no grace period";
    let id = h.request(
        "session/prompt",
        json!({
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": prompt}],
        }),
    );
    // Everything in `updates` arrived strictly before the response.
    let (updates, result) = h.await_response(id);

    assert_eq!(
        result["stopReason"], "end_turn",
        "unexpected stop reason in {result}"
    );
    let echoed: String = updates
        .iter()
        .filter_map(agent_message_chunk_text)
        .collect();
    assert!(
        echoed.contains(prompt),
        "the turn's text should arrive before its response, but only {updates:#?} did"
    );
}

#[test]
fn stdout_carries_only_jsonrpc() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    // Trace logging is the worst case for stdout pollution: if a log
    // line could ever leak into the protocol channel, it happens here.
    let mut h = Harness::start(&bin, &wasm, &["--log-level", "trace"]);
    let session_id = h.open_session();

    let id = h.request(
        "session/prompt",
        json!({
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "logging noise check"}],
        }),
    );
    h.await_response(id);
    h.drain_pending();

    assert!(!h.seen.is_empty(), "no output at all");
    for line in &h.seen {
        let msg: Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("non-JSON line on stdout ({e}): {line}"));
        assert_eq!(
            msg["jsonrpc"], "2.0",
            "stdout line is JSON but not JSON-RPC: {line}"
        );
        assert!(
            msg.get("method").is_some() || msg.get("id").is_some(),
            "stdout line is neither a request/notification nor a response: {line}"
        );
    }
}

#[test]
fn default_logs_omit_request_and_notification_contents() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let logs = tempfile::tempdir().expect("log dir");
    let log_path = logs.path().join("host.log");
    let mut h = Harness::start(&bin, &wasm, &["--log-file", log_path.to_str().unwrap()]);

    let id = h.request(
        "initialize",
        json!({"protocolVersion": 1, "clientCapabilities": {}}),
    );
    h.await_response(id);
    let id = h.request(
        "session/new",
        json!({
            "cwd": std::env::temp_dir(),
            "mcpServers": [
                {"name": "private-stdio", "command": "echo",
                 "env": [{"name": "TOKEN", "value": "sensitive-env-770"}]},
                {"name": "private-http", "url": "https://example.com",
                 "headers": [{"name": "Authorization", "value": "sensitive-header-770"}]}
            ]
        }),
    );
    let (_, session) = h.await_response(id);
    let id = h.request(
        "session/prompt",
        json!({"sessionId": session["sessionId"], "prompt": [
            {"type": "text", "text": "sensitive-prompt-770"}
        ]}),
    );
    h.await_response(id);
    h.drain_pending();

    let stderr = h.stderr.lock().unwrap().clone();
    let file = std::fs::read_to_string(
        std::fs::read_dir(logs.path())
            .unwrap()
            .next()
            .expect("log file")
            .unwrap()
            .path(),
    )
    .unwrap();
    for output in [&stderr, &file] {
        assert!(output.contains("session/prompt"), "no host logs: {output}");
        for secret in [
            "sensitive-env-770",
            "sensitive-header-770",
            "sensitive-prompt-770",
        ] {
            assert!(!output.contains(secret), "secret in logs: {output}");
        }
    }
}

#[test]
fn unsupported_prompt_content_is_rejected_without_running_the_turn() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let mut h = Harness::start(&bin, &wasm, &[]);
    let session_id = h.open_session();
    let id = h.request(
        "session/prompt",
        json!({"sessionId": session_id, "prompt": [
            {"type": "text", "text": "do not echo"},
            {"type": "resource_link", "uri": "file:///tmp/example", "name": "example"}
        ]}),
    );
    loop {
        let message: Value = serde_json::from_str(&h.next_line()).unwrap();
        if message["id"] == id {
            assert_eq!(message["error"]["code"], -32602, "{message}");
            break;
        }
        assert!(
            agent_message_chunk_text(&message).is_none(),
            "rejected prompt emitted agent text: {message}"
        );
    }
}

#[test]
fn multiple_providers_fail_with_a_clear_cli_error() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let output = Command::new(bin)
        .arg("acp")
        .arg("--provider")
        .arg(&wasm)
        .arg("--provider")
        .arg(&wasm)
        .output()
        .expect("run CLI");
    assert!(!output.status.success(), "multiple providers were accepted");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("exactly one --provider"),
        "unexpected error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn policy_free_layer_chain_runs_without_shared_grants_flag() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let layer = uppercase_layer().expect("build the uppercase layer with just build-acp-examples");
    let mut h = Harness::start(&bin, &wasm, &["--layer", layer.to_str().unwrap()]);
    let session_id = h.open_session();
    let id = h.request(
        "session/prompt",
        json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "layered demo"}]}),
    );
    let (updates, result) = h.await_response(id);
    assert_eq!(result["stopReason"], "end_turn");
    let echoed: String = updates
        .iter()
        .filter_map(agent_message_chunk_text)
        .collect();
    assert!(echoed.contains("layered demo"), "{updates:#?}");

    let id = h.request(
        "session/prompt",
        json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "/shout"}]}),
    );
    let (_, result) = h.await_response(id);
    assert_eq!(result["stopReason"], "end_turn");
    let id = h.request(
        "session/prompt",
        json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "layered demo"}]}),
    );
    let (updates, result) = h.await_response(id);
    assert_eq!(result["stopReason"], "end_turn");
    let shouted: String = updates
        .iter()
        .filter_map(agent_message_chunk_text)
        .collect();
    assert!(shouted.contains("LAYERED DEMO"), "{updates:#?}");
}

#[test]
fn stored_secrets_in_a_policy_free_layer_chain_require_opt_in() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let layer = uppercase_layer().expect("build the uppercase layer with just build-acp-examples");
    let secrets = tempfile::tempdir().unwrap();
    let component_id = layer.file_stem().unwrap().to_str().unwrap();
    std::fs::write(
        secrets.path().join(format!("{component_id}.yaml")),
        "TOKEN: hidden\n",
    )
    .unwrap();
    let output = Command::new(bin)
        .arg("acp")
        .arg("--provider")
        .arg(wasm)
        .arg("--layer")
        .arg(layer)
        .arg("--secrets-dir")
        .arg(secrets.path())
        .output()
        .expect("run CLI");
    assert!(
        !output.status.success(),
        "layer secrets were accepted without opt-in"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("concurrent callbacks may be attributed to the wrong stage"),
        "{stderr}"
    );
    assert!(stderr.contains("--allow-shared-grants"), "{stderr}");
    assert!(
        !stderr.contains("hidden"),
        "secret value in error: {stderr}"
    );
}

#[test]
fn stored_secrets_in_a_layer_chain_run_with_opt_in() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let layer = uppercase_layer().expect("build the uppercase layer with just build-acp-examples");
    let secrets = tempfile::tempdir().unwrap();
    let component_id = wasm.file_stem().unwrap().to_str().unwrap();
    std::fs::write(
        secrets.path().join(format!("{component_id}.yaml")),
        "TOKEN: hidden\n",
    )
    .unwrap();
    let mut h = Harness::start(
        &bin,
        &wasm,
        &[
            "--layer",
            layer.to_str().unwrap(),
            "--secrets-dir",
            secrets.path().to_str().unwrap(),
            "--allow-shared-grants",
        ],
    );
    let session_id = h.open_session();
    let id = h.request(
        "session/prompt",
        json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "opted in"}]}),
    );
    let (updates, result) = h.await_response(id);
    assert_eq!(result["stopReason"], "end_turn");
    let echoed: String = updates
        .iter()
        .filter_map(agent_message_chunk_text)
        .collect();
    assert!(echoed.contains("opted in"), "{updates:#?}");
}

#[test]
fn privileged_layer_chain_requires_shared_grants_flag() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let layer = uppercase_layer().expect("build the uppercase layer with just build-acp-examples");
    let output = Command::new(bin)
        .arg("acp")
        .arg("--provider")
        .arg(wasm)
        .arg("--layer")
        .arg(layer)
        .arg("--allow-all")
        .output()
        .expect("run CLI");
    assert!(
        !output.status.success(),
        "privileged layer chain was accepted"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--allow-shared-grants"),
        "unexpected error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
