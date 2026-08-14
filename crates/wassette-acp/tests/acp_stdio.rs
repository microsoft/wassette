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
//! (cd examples/acp-echo-provider && cargo build --release --target wasm32-wasip2)
//! ```
//!
//! When either is missing the tests print why and pass, so a plain
//! `cargo test --workspace` on a machine without the `wasm32-wasip2`
//! target stays green.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

use serde_json::{Value, json};

/// How long to wait for any single line of output. Generous: the first
/// response includes compiling the provider component.
const LINE_TIMEOUT: Duration = Duration::from_secs(60);

/// The host holds `session/update` notifications emitted during
/// `session/new` until just after the `session/new` response goes out,
/// then flushes them (see `bridge/gate.rs`). Prompting before the flush
/// makes the prompt's own updates queue behind it, so wait out the flush
/// before sending the prompt. That ordering is the host's deliberate
/// behaviour, not a race to fix here.
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
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "../../examples/acp-echo-provider/target/wasm32-wasip2/release/acp_echo_provider.wasm",
    );
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
            "skipping: examples/acp-echo-provider/target/wasm32-wasip2/release/\
             acp_echo_provider.wasm not found; run `just build-acp-examples`"
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
        std::thread::spawn(
            move || {
                for _ in BufReader::new(stderr).lines().map_while(Result::ok) {}
            },
        );

        Harness {
            child,
            stdin,
            lines: rx,
            seen: Vec::new(),
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
                "timed out after {LINE_TIMEOUT:?} waiting for output; saw so far:\n{}",
                self.seen.join("\n")
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

    /// `initialize` → `session/new`, returning the new session id.
    fn open_session(&mut self) -> String {
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
        let session_id = new_session["sessionId"]
            .as_str()
            .unwrap_or_else(|| panic!("session/new result has no sessionId: {new_session}"))
            .to_string();

        // Let the notification gate flush before prompting.
        std::thread::sleep(GATE_FLUSH_GRACE);
        self.drain_pending();
        session_id
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
