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
//! just build-acp-examples
//! ```
//!
//! Outside CI, missing artifacts skip the tests so a plain `cargo test
//! --workspace` on a machine without the `wasm32-wasip2` target stays green.
//! In CI, missing artifacts fail the tests rather than silently reducing coverage.

use std::borrow::Cow;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use wasm_encoder::{ComponentSection, CustomSection, Encode};
use wassette::store::{
    ComponentStore, EntryRevision, InstallOwner, StoredArtifactKind, ValidationEvidence,
};

mod common;
use common::NamedFixture;

#[path = "acp_stdio/multi_provider.rs"]
mod multi_provider;

#[cfg(feature = "component-generation")]
#[path = "acp_stdio/generation.rs"]
mod generation;

/// How long to wait for any single line of output. Generous: the first
/// response includes compiling the provider component.
const LINE_TIMEOUT: Duration = Duration::from_secs(60);

/// The host holds bounded updates emitted during `session/new` until the
/// guest's session ID is known. Waiting for the gate to open keeps those
/// updates and the host's `/install` advertisement out of later assertions. A client
/// that does *not* wait is still served correctly —
/// `a_prompt_before_the_gate_flush_still_streams_first` covers that.
const GATE_FLUSH_GRACE: Duration = Duration::from_millis(500);

/// The `wassette` binary under test: a sibling of the test executable's
/// directory (`target/<profile>/deps/<test>` → `target/<profile>`).
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

/// The echo provider component, built by `just build-acp-examples`.
fn echo_provider() -> Option<NamedFixture> {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../components/acp-echo-provider/target")
        });
    let path = target_dir.join("wasm32-wasip2/release/acp_echo_provider.wasm");
    path.is_file().then(|| NamedFixture::copy(&path))
}

fn uppercase_layer() -> Option<NamedFixture> {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../components/acp-uppercase-layer/target")
        });
    let path = target_dir.join("wasm32-wasip2/release/acp_uppercase_layer.wasm");
    path.is_file().then(|| NamedFixture::copy(&path))
}

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
    eprintln!("skipping: filesystem-rs is not built; run `just build-acp-tool-fixture`");
    None
}

/// Both artifacts, or `None` with an explanation of what to build.
fn artifacts() -> Option<(PathBuf, NamedFixture)> {
    let Some(bin) = wassette_binary() else {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI requires the `wassette` binary; run `cargo build -p wassette-mcp-server`"
        );
        eprintln!(
            "skipping: `wassette` binary not found; run `cargo build -p wassette-mcp-server`"
        );
        return None;
    };
    let Some(wasm) = echo_provider() else {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI requires the ACP echo provider; run `just build-acp-examples`"
        );
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
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    /// Every line stdout has produced, in order. Used to assert the
    /// channel stayed pure JSON-RPC.
    seen: Vec<String>,
    stderr: Arc<Mutex<String>>,
    /// Kept alive for the process's lifetime: the XDG roots the host
    /// reads and writes, redirected away from the developer's real
    /// component store.
    _xdg: tempfile::TempDir,
    local_drop: Option<PathBuf>,
    next_id: i64,
    line_timeout: Duration,
}

impl Harness {
    /// Start `wassette acp --provider <wasm> [extra…]` with fresh XDG
    /// directories.
    fn start(bin: &Path, wasm: &Path, extra: &[&str]) -> Harness {
        Self::start_with_env(bin, wasm, extra, &[])
    }

    fn start_with_env(bin: &Path, wasm: &Path, extra: &[&str], env: &[(&str, &str)]) -> Harness {
        let xdg = tempfile::tempdir().expect("tempdir");
        Self::spawn(bin, wasm, extra, xdg, None, env)
    }

    fn start_with_local_tool(
        bin: &Path,
        provider: &Path,
        tool: &Path,
        mode: &str,
        expose: bool,
    ) -> Harness {
        let exposed = if expose {
            vec!["microsoft:filesystem-rs"]
        } else {
            Vec::new()
        };
        Self::start_with_local_source(bin, provider, Some(tool), mode, &exposed, &[])
    }

    fn start_with_local_source(
        bin: &Path,
        provider: &Path,
        tool: Option<&Path>,
        mode: &str,
        exposed: &[&str],
        extra_args: &[&str],
    ) -> Harness {
        let xdg = tempfile::tempdir().expect("tempdir");
        let drops = xdg.path().join("local-components");
        std::fs::create_dir_all(&drops).expect("create local component drop");
        let output = xdg.path().join("tool-output");
        std::fs::create_dir_all(&output).expect("create isolated tool output");
        if let Some(tool) = tool {
            write_local_tool(&drops, tool, &output);
        }
        let mut extra = vec![
            "--local-component-dir".to_string(),
            drops.to_str().expect("utf-8 drop directory").to_string(),
            "--local-components".to_string(),
            mode.to_string(),
        ];
        for component in exposed {
            extra.extend(["--tool".to_string(), component.to_string()]);
        }
        extra.extend(extra_args.iter().map(|arg| arg.to_string()));
        Self::spawn(bin, provider, &extra, xdg, Some(drops.clone()), &[])
    }

    fn spawn<I, S>(
        bin: &Path,
        wasm: &Path,
        extra: I,
        xdg: tempfile::TempDir,
        local_drop: Option<PathBuf>,
        env: &[(&str, &str)],
    ) -> Harness
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
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
            .env("WASSETTE_CONFIG_FILE", config.join("config.toml"))
            .env_remove("WASSETTE_LOCAL_COMPONENT_DIR")
            .env_remove("WASSETTE_LOCAL_COMPONENTS")
            .env_remove("WASSETTE_GENERATION_CONFIG")
            // The host prefers RUST_LOG over --log-level; clear it so a
            // developer's ambient value cannot change what is logged.
            .env_remove("RUST_LOG")
            .env_remove("WASSETTE_WASM_DIRECTORY_URL")
            .envs(env.iter().copied())
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
            stdin: Some(stdin),
            lines: rx,
            seen: Vec::new(),
            stderr: stderr_output,
            _xdg: xdg,
            local_drop,
            next_id: 0,
            line_timeout: LINE_TIMEOUT,
        }
    }

    /// Send a request and return the id it was assigned.
    fn request(&mut self, method: &str, params: Value) -> i64 {
        self.next_id += 1;
        let id = self.next_id;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let stdin = self.stdin.as_mut().expect("stdin is open");
        writeln!(stdin, "{msg}").expect("write request");
        stdin.flush().expect("flush request");
        id
    }

    fn notify(&mut self, method: &str, params: Value) {
        let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let stdin = self.stdin.as_mut().expect("stdin is open");
        writeln!(stdin, "{msg}").expect("write notification");
        stdin.flush().expect("flush notification");
    }

    fn prompt(&mut self, session_id: &str, text: &str) -> i64 {
        self.request(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": text}],
            }),
        )
    }

    fn store(&self) -> ComponentStore {
        ComponentStore::open(self._xdg.path().join("data/wassette/components")).unwrap()
    }

    fn write_command(&self, content: &str) -> String {
        let path = self
            ._xdg
            .path()
            .join("tool-output")
            .canonicalize()
            .unwrap()
            .join("written.txt");
        format!(
            "/tool write-file {}",
            json!({"path": path, "content": content})
        )
    }

    fn respond_permission(&mut self, request: &Value, option_id: &str) {
        let response = json!({
            "jsonrpc": "2.0", "id": request["id"],
            "result": {"outcome": {"outcome": "selected", "optionId": option_id}},
        });
        let stdin = self.stdin.as_mut().expect("stdin is open");
        writeln!(stdin, "{response}").expect("write permission response");
        stdin.flush().expect("flush permission response");
    }

    fn await_permission(&mut self, prompt_id: i64) -> Value {
        loop {
            let message: Value = serde_json::from_str(&self.next_line()).unwrap();
            assert_ne!(
                message["id"],
                json!(prompt_id),
                "prompt ended before permission: {message}"
            );
            if message["method"] == "session/request_permission" {
                return message;
            }
        }
    }

    fn close_stdin_and_wait(&mut self) {
        drop(self.stdin.take());
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().expect("wait for wassette") {
                assert!(status.success(), "wassette exited with {status}");
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "wassette did not exit within five seconds of stdin EOF"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Read one line of stdout, recording it.
    fn next_line(&mut self) -> String {
        match self.lines.recv_timeout(self.line_timeout) {
            Ok(line) => {
                self.seen.push(line.clone());
                line
            }
            Err(RecvTimeoutError::Timeout) => panic!(
                "timed out after {:?} waiting for output; saw so far:\n{}\nstderr:\n{}",
                self.line_timeout,
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

    fn await_response_with_permission(&mut self, id: i64, option_id: &str) -> (Vec<Value>, Value) {
        let mut messages = Vec::new();
        loop {
            let line = self.next_line();
            let msg: Value = serde_json::from_str(&line)
                .unwrap_or_else(|error| panic!("stdout line is not JSON ({error}): {line}"));
            if msg.get("id").and_then(Value::as_i64) == Some(id) {
                assert!(
                    msg.get("error").is_none(),
                    "request {id} failed: {}",
                    msg["error"]
                );
                return (messages, msg["result"].clone());
            }
            if msg["method"] == "session/request_permission" {
                self.respond_permission(&msg, option_id);
            }
            messages.push(msg);
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
    /// wait out the gate flush so `session/new` updates don't land in
    /// the middle of what a later assertion is reading.
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

fn write_local_tool(drop: &Path, tool: &Path, output: &Path) {
    let uri = format!("fs://{}", output.canonicalize().unwrap().display());
    let policy = json!({
        "version": "1.0",
        "permissions": {"storage": {"allow": [{"uri": uri, "access": ["read", "write"]}]}},
    });
    std::fs::write(
        drop.join("unrelated-name.policy.yaml"),
        serde_json::to_vec(&policy).unwrap(),
    )
    .unwrap();
    std::fs::copy(tool, drop.join("unrelated-name.wasm")).unwrap();
}

fn permission_count(messages: &[Value]) -> usize {
    messages
        .iter()
        .filter(|message| message["method"] == "session/request_permission")
        .count()
}

fn response_text(messages: &[Value]) -> String {
    messages
        .iter()
        .filter_map(agent_message_chunk_text)
        .collect()
}

fn wait_for_revision(store: &ComponentStore, before: &EntryRevision) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let current = store
            .read("microsoft:filesystem-rs")
            .expect("read managed tool");
        if &current.receipt.revision != before {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "local replacement was not committed"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn replace_with_equivalent_component(path: &Path) {
    let mut wasm = std::fs::read(path).expect("read local tool");
    let section = CustomSection {
        name: Cow::Borrowed("wassette-test-revision"),
        data: Cow::Borrowed(b"replacement"),
    };
    wasm.push(section.id());
    section.encode(&mut wasm);
    std::fs::write(path, wasm).expect("replace local tool");
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
fn exits_on_idle_stdin_eof() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let mut h = Harness::start(&bin, &wasm, &[]);
    h.open_session();
    h.close_stdin_and_wait();
}

#[test]
fn exits_on_stdin_eof_during_prompt() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let mut h = Harness::start(&bin, &wasm, &[]);
    let sid = h.open_session();
    let prompt = "hello ".repeat(5000);
    h.request(
        "session/prompt",
        json!({"sessionId": sid, "prompt": [{"type": "text", "text": prompt}]}),
    );
    loop {
        let msg: Value = serde_json::from_str(&h.next_line()).expect("JSON-RPC output");
        if agent_message_chunk_text(&msg).is_some() {
            break;
        }
    }
    h.close_stdin_and_wait();
}

#[test]
fn cancelled_prompts_flush_all_chunks_before_the_response() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let layer =
        uppercase_layer().expect("build ACP uppercase layer with `just build-acp-examples`");
    for extra in [&[][..], &["--layer", layer.to_str().unwrap()][..]] {
        let mut h = Harness::start(&bin, &wasm, extra);
        let sid = h.open_session();
        let prompt = "hello ".repeat(5000);
        for turn in 0..20 {
            let id = h.request(
                "session/prompt",
                json!({"sessionId": sid, "prompt": [{"type": "text", "text": prompt}]}),
            );
            loop {
                let msg: Value = serde_json::from_str(&h.next_line()).expect("JSON-RPC output");
                if agent_message_chunk_text(&msg).is_some() {
                    break;
                }
                assert_ne!(msg["id"], id, "turn {turn} ended before its first chunk");
            }
            h.notify("session/cancel", json!({"sessionId": sid}));
            let (_, result) = h.await_response(id);
            assert_eq!(result["stopReason"], "cancelled", "turn {turn}: {result}");
            let late = h.drain_pending();
            assert!(
                late.iter().all(|m| agent_message_chunk_text(m).is_none()),
                "turn {turn} delivered chunks after cancellation: {late:?}"
            );
        }
    }
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
fn echo_does_not_advertise_an_unselectable_default_mode() {
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
    assert!(result.get("modes").is_none(), "{result}");
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
fn load_session_is_rejected_when_not_advertised() {
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
        "session/load",
        json!({"sessionId": "echo-load", "cwd": ".", "mcpServers": []}),
    );
    let response: Value = serde_json::from_str(&h.next_line()).expect("JSON-RPC output");
    assert_eq!(response["id"], id);
    assert_eq!(response["error"]["code"], -32602, "{response}");
    assert!(
        response["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("loadSession was not advertised")),
        "{response}"
    );
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
    assert_eq!(session["configOptions"][0]["name"], "Terminal", "{session}");
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

#[test]
fn two_layered_sessions_keep_independent_shout_state() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let layer =
        uppercase_layer().expect("build ACP uppercase layer with `just build-acp-examples`");
    let mut h = Harness::start(&bin, &wasm, &["--layer", layer.to_str().unwrap()]);
    let a = h.open_session();
    let id = h.request(
        "session/prompt",
        json!({"sessionId": a, "prompt": [{"type": "text", "text": "/shout"}]}),
    );
    h.await_response(id);
    let id = h.request(
        "session/new",
        json!({"cwd": std::env::temp_dir(), "mcpServers": []}),
    );
    let (_, session) = h.await_response(id);
    let b = session["sessionId"].as_str().expect("second session id");
    assert_ne!(a, b, "guest sessions must have unique IDs");
    let b = b.to_string();

    for (sid, expected) in [(&a, "HELLO FROM A"), (&b, "hello from b")] {
        let text = if sid == &a {
            "hello from a"
        } else {
            "hello from b"
        };
        let id = h.request(
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type": "text", "text": text}]}),
        );
        let (updates, result) = h.await_response(id);
        assert_eq!(result["stopReason"], "end_turn");
        let echoed: String = updates
            .iter()
            .filter_map(agent_message_chunk_text)
            .collect();
        assert_eq!(echoed, expected, "session {sid} lost its independent state");
    }
}

#[test]
fn unadvertised_load_does_not_replace_the_active_session() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let mut h = Harness::start(&bin, &wasm, &[]);
    let sid = h.open_session();
    let id = h.request(
        "session/load",
        json!({"sessionId": sid, "cwd": std::env::temp_dir(), "mcpServers": []}),
    );
    let line: Value = serde_json::from_str(&h.next_line()).expect("JSON-RPC output");
    assert_eq!(line["id"], id);
    assert_eq!(line["error"]["code"], -32602, "{line}");
    assert!(
        line["error"]["message"]
            .as_str()
            .is_some_and(|msg| msg.contains("loadSession was not advertised")),
        "{line}"
    );
    let id = h.request(
        "session/prompt",
        json!({"sessionId": sid, "prompt": [{"type": "text", "text": "still here"}]}),
    );
    let (updates, result) = h.await_response(id);
    assert_eq!(result["stopReason"], "end_turn");
    assert_eq!(
        updates
            .iter()
            .filter_map(agent_message_chunk_text)
            .collect::<String>(),
        "still here"
    );
}

#[test]
fn install_local_path_reports_receipt_backed_installation() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let mut h = Harness::start(&bin, &wasm, &[]);
    let sid = h.open_session();
    let id = h.request(
        "session/prompt",
        json!({"sessionId": sid, "prompt": [{"type": "text", "text": format!("/install {}", wasm.display())}]}),
    );
    let (updates, response) = h.await_response(id);
    assert_eq!(response["stopReason"], "end_turn");
    let finish = updates.iter().find(|m| {
        m["params"]["update"]["sessionUpdate"] == "tool_call_update"
            && m["params"]["update"]["status"] == "completed"
    });
    let finish = finish.expect("completed install tool-call update");
    let text = finish["params"]["update"]["content"][0]["content"]["text"]
        .as_str()
        .expect("install result text");
    assert!(text.contains("Ready to use"), "{text}");
    assert!(text.contains("acp_echo_provider.wasm"), "{text}");
    assert!(
        h._xdg
            .path()
            .join("data/wassette/components/acp_echo_provider.wasm")
            .exists(),
        "local input was not transactionally installed"
    );
}

#[test]
fn generation_import_is_disabled_without_an_operator_profile() {
    let Some((bin, provider)) = artifacts() else {
        return;
    };
    let mut h = Harness::start(&bin, &provider, &[]);
    let sid = h.open_session();
    let id = h.prompt(&sid, "/generate {}");
    let (messages, response) = h.await_response_with_permission(id, "allow-once");
    assert_eq!(response["stopReason"], "end_turn");
    assert_eq!(permission_count(&messages), 0);
    assert!(
        response_text(&messages).contains("Disabled"),
        "{messages:?}"
    );
}

#[test]
fn local_tool_invocation_routes_permission_and_status_over_stdio() {
    let Some((bin, provider)) = artifacts() else {
        return;
    };
    let Some(tool) = filesystem_tool() else {
        return;
    };
    let mut h = Harness::start_with_local_tool(&bin, &provider, &tool, "startup", true);
    let sid = h.open_session();

    let output = h._xdg.path().join("tool-output/written.txt");
    let command = h.write_command("allowed through ACP");
    let id = h.prompt(&sid, &command);
    let (rejected, response) = h.await_response_with_permission(id, "reject-once");
    assert_eq!(response["stopReason"], "end_turn");
    assert!(
        rejected
            .iter()
            .any(|message| message["method"] == "session/request_permission"),
        "permission request was not routed over ACP: {rejected:?}"
    );
    assert!(
        rejected.iter().any(|message| {
            message["params"]["update"]["sessionUpdate"] == "tool_call_update"
                && message["params"]["update"]["status"] == "failed"
        }),
        "rejected tool call did not emit failed status: {rejected:?}"
    );
    assert!(
        rejected
            .iter()
            .filter_map(agent_message_chunk_text)
            .any(|text| text.contains("PermissionDenied")),
        "provider did not receive the rejection: {rejected:?}"
    );
    assert!(!output.exists(), "rejected invocation wrote a file");

    let id = h.prompt(&sid, &command);
    let (allowed, response) = h.await_response_with_permission(id, "allow-once");
    assert_eq!(response["stopReason"], "end_turn");
    assert!(
        allowed.iter().any(|message| {
            message["params"]["update"]["sessionUpdate"] == "tool_call_update"
                && message["params"]["update"]["status"] == "in_progress"
        }),
        "allowed tool call did not become in-progress: {allowed:?}"
    );
    assert!(
        allowed.iter().any(|message| {
            message["params"]["update"]["sessionUpdate"] == "tool_call_update"
                && message["params"]["update"]["status"] == "completed"
        }),
        "allowed tool call did not complete: {allowed:?}"
    );
    assert!(
        allowed
            .iter()
            .filter_map(agent_message_chunk_text)
            .any(|text| text.contains("Successfully wrote")),
        "provider did not receive a successful tool result: {allowed:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&output).unwrap(),
        "allowed through ACP"
    );
    assert_eq!(permission_count(&allowed), 1);
    let calls: Vec<_> = allowed
        .iter()
        .filter(|message| {
            matches!(
                message["params"]["update"]["sessionUpdate"].as_str(),
                Some("tool_call" | "tool_call_update")
            )
        })
        .collect();
    assert_eq!(calls.len(), 3, "{calls:?}");
    let call_id = calls[0]["params"]["update"]["toolCallId"].as_str().unwrap();
    for message in &calls {
        assert_eq!(message["params"]["sessionId"], sid);
        assert_eq!(message["params"]["update"]["toolCallId"], call_id);
    }
    let request = allowed
        .iter()
        .find(|message| message["method"] == "session/request_permission")
        .unwrap();
    assert_eq!(request["params"]["sessionId"], sid);
    assert_eq!(request["params"]["toolCall"]["toolCallId"], call_id);

    let id = h.prompt(&sid, r#"/tool write-file {"path":42}"#);
    let (invalid, _) = h.await_response_with_permission(id, "allow-once");
    assert!(
        response_text(&invalid).contains("InvalidArguments"),
        "{invalid:?}"
    );
    assert_eq!(permission_count(&invalid), 0);
}

#[test]
fn startup_discovery_installs_but_does_not_expose_local_tools() {
    let Some((bin, provider)) = artifacts() else {
        return;
    };
    let Some(tool) = filesystem_tool() else {
        return;
    };
    let mut h = Harness::start_with_local_tool(&bin, &provider, &tool, "startup", false);
    let sid = h.open_session();
    let id = h.request(
        "session/prompt",
        json!({
            "sessionId": sid,
            "prompt": [{"type": "text", "text": "/remember-tool file-exists"}],
        }),
    );
    let (messages, response) = h.await_response(id);
    assert_eq!(response["stopReason"], "end_turn");
    assert!(
        messages
            .iter()
            .filter_map(agent_message_chunk_text)
            .any(|text| text == "tool not found: file-exists"),
        "local tool was unexpectedly exposed: {messages:?}"
    );
    let receipt = h.store().read("microsoft:filesystem-rs").unwrap().receipt;
    assert_eq!(receipt.kind, StoredArtifactKind::Tool);
    assert!(matches!(receipt.owner, InstallOwner::ManagedLocalSource(_)));
    assert_ne!(receipt.storage_key.as_str(), receipt.component_id.as_str());
}

#[test]
fn watch_add_replace_remove_preserves_revision_bound_permissions() {
    let Some((bin, provider)) = artifacts() else {
        return;
    };
    let Some(tool) = filesystem_tool() else {
        return;
    };
    let mut h = Harness::start_with_local_source(
        &bin,
        &provider,
        None,
        "watch",
        &["microsoft:filesystem-rs"],
        &[],
    );
    let sid = h.open_session();
    let id = h.prompt(&sid, "/remember-tool write-file");
    let (messages, _) = h.await_response(id);
    assert_eq!(response_text(&messages), "tool not found: write-file");
    let id = h.prompt(&sid, "/wait-tools");
    let drop = h.local_drop.as_ref().expect("watch drop directory").clone();
    let output_dir = h._xdg.path().join("tool-output");
    write_local_tool(&drop, &tool, &output_dir);
    let (messages, _) = h.await_response(id);
    assert_eq!(response_text(&messages), "catalog changed");
    let id = h.prompt(&sid, "/remember-tool write-file");
    let (messages, _) = h.await_response(id);
    assert_eq!(response_text(&messages), "remembered write-file");
    let store = h.store();
    let before = store
        .read("microsoft:filesystem-rs")
        .unwrap()
        .receipt
        .revision;

    for (content, expected_requests) in [("first call", 1), ("remembered call", 0)] {
        let command = h.write_command(content);
        let id = h.prompt(&sid, &command);
        let (messages, _) = h.await_response_with_permission(id, "allow-always");
        assert_eq!(permission_count(&messages), expected_requests);
        assert_eq!(
            std::fs::read_to_string(output_dir.join("written.txt")).unwrap(),
            content
        );
    }
    let wasm = drop.join("unrelated-name.wasm");
    replace_with_equivalent_component(&wasm);
    wait_for_revision(&store, &before);
    let id = h.prompt(&sid, r#"/call-saved {"path":".","content":"stale"}"#);
    let (stale, _) = h.await_response_with_permission(id, "allow-once");
    assert!(response_text(&stale).contains("Stale"), "{stale:?}");
    assert_eq!(permission_count(&stale), 0);
    assert_eq!(
        std::fs::read_to_string(output_dir.join("written.txt")).unwrap(),
        "remembered call"
    );

    let command = h.write_command("new revision");
    let id = h.prompt(&sid, &command);
    let (messages, _) = h.await_response_with_permission(id, "allow-always");
    assert_eq!(
        permission_count(&messages),
        1,
        "permission leaked across a revision"
    );
    assert_eq!(
        std::fs::read_to_string(output_dir.join("written.txt")).unwrap(),
        "new revision"
    );
    let new_id = h.request(
        "session/new",
        json!({"cwd": std::env::temp_dir(), "mcpServers": []}),
    );
    let (_, session) = h.await_response(new_id);
    let second = session["sessionId"].as_str().unwrap();
    let id = h.prompt(second, &command);
    let (messages, _) = h.await_response_with_permission(id, "reject-once");
    assert_eq!(
        permission_count(&messages),
        1,
        "permission leaked across a session"
    );
    assert!(response_text(&messages).contains("PermissionDenied"));

    let id = h.prompt(&sid, "/remember-tool write-file");
    let (messages, _) = h.await_response(id);
    assert_eq!(response_text(&messages), "remembered write-file");
    let id = h.prompt(&sid, "/wait-tools");
    std::fs::remove_file(&wasm).expect("remove watched local tool");
    std::fs::remove_file(wasm.with_extension("policy.yaml")).unwrap();
    let (messages, _) = h.await_response(id);
    assert_eq!(response_text(&messages), "catalog changed");
    let id = h.prompt(&sid, r#"/call-saved {"path":".","content":"removed"}"#);
    let (messages, _) = h.await_response_with_permission(id, "allow-once");
    assert_eq!(permission_count(&messages), 0);
    assert!(response_text(&messages).contains("Stale"), "{messages:?}");
}

#[test]
fn replacement_while_permission_is_pending_never_executes() {
    let Some((bin, provider)) = artifacts() else {
        return;
    };
    let Some(tool) = filesystem_tool() else {
        return;
    };
    let mut h = Harness::start_with_local_tool(&bin, &provider, &tool, "watch", true);
    let sid = h.open_session();
    let store = h.store();
    let before = store
        .read("microsoft:filesystem-rs")
        .unwrap()
        .receipt
        .revision;
    let command = h.write_command("must not run");
    let id = h.prompt(&sid, &command);
    let permission = h.await_permission(id);
    replace_with_equivalent_component(&h.local_drop.as_ref().unwrap().join("unrelated-name.wasm"));
    wait_for_revision(&store, &before);
    h.respond_permission(&permission, "allow-once");
    let (messages, _) = h.await_response(id);
    assert!(response_text(&messages).contains("Stale"), "{messages:?}");
    assert!(!h._xdg.path().join("tool-output/written.txt").exists());
}

#[test]
fn cancellation_while_permission_is_pending_reports_no_execution() {
    let Some((bin, provider)) = artifacts() else {
        return;
    };
    let Some(tool) = filesystem_tool() else {
        return;
    };
    let mut h = Harness::start_with_local_tool(&bin, &provider, &tool, "startup", true);
    let sid = h.open_session();
    let command = h.write_command("must not run");
    let id = h.prompt(&sid, &command);
    let permission = h.await_permission(id);
    h.notify("session/cancel", json!({"sessionId": sid}));
    let (messages, result) = h.await_response(id);
    assert_eq!(result["stopReason"], "cancelled");
    assert!(!h._xdg.path().join("tool-output/written.txt").exists());
    assert!(
        messages.iter().any(|message| {
            message["params"]["update"]["sessionUpdate"] == "tool_call_update"
                && message["params"]["update"]["status"] == "failed"
                && message["params"]["update"]["rawOutput"]
                    .as_str()
                    .is_some_and(|text| text.contains("may still be finishing"))
        }),
        "missing honest cancellation update: {messages:?}"
    );
    h.respond_permission(&permission, "allow-always");
    let id = h.prompt(&sid, "after cancellation");
    let (messages, _) = h.await_response(id);
    assert_eq!(response_text(&messages), "after cancellation");
    assert!(!h._xdg.path().join("tool-output/written.txt").exists());
    let id = h.prompt(&sid, &command);
    let (messages, _) = h.await_response_with_permission(id, "reject-once");
    assert_eq!(
        permission_count(&messages),
        1,
        "late permission was remembered after cancellation"
    );
    h.close_stdin_and_wait();
}

#[test]
fn layered_tool_permissions_require_opt_in_and_reach_the_editor() {
    let Some((bin, provider)) = artifacts() else {
        return;
    };
    let Some(tool) = filesystem_tool() else {
        return;
    };
    let Some(layer) = uppercase_layer() else {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI requires the uppercase layer"
        );
        eprintln!("skipping: uppercase layer not built");
        return;
    };
    let layer = layer.to_str().unwrap();
    let mut denied = Harness::start_with_local_source(
        &bin,
        &provider,
        Some(&tool),
        "startup",
        &["microsoft:filesystem-rs"],
        &["--layer", layer],
    );
    drop(denied.stdin.take());
    assert!(
        !denied.child.wait().unwrap().success(),
        "layered tools ran without opt-in"
    );
    let mut h = Harness::start_with_local_source(
        &bin,
        &provider,
        Some(&tool),
        "startup",
        &["microsoft:filesystem-rs"],
        &["--layer", layer, "--allow-shared-grants"],
    );
    let sid = h.open_session();
    let command = h.write_command("layered");
    let id = h.prompt(&sid, &command);
    let (messages, _) = h.await_response_with_permission(id, "allow-once");
    assert_eq!(permission_count(&messages), 1);
    let permission = messages
        .iter()
        .find(|message| message["method"] == "session/request_permission")
        .unwrap();
    assert_eq!(permission["params"]["sessionId"], sid);
    assert_eq!(
        std::fs::read_to_string(h._xdg.path().join("tool-output/written.txt")).unwrap(),
        "layered"
    );
}

#[test]
fn local_acp_drops_are_export_checked_and_never_activated() {
    let Some((bin, provider)) = artifacts() else {
        return;
    };
    let mut h = Harness::start_with_local_source(
        &bin,
        &provider,
        None,
        "watch",
        &["discovered-agent"],
        &[],
    );
    let sid = h.open_session();
    let drops = h.local_drop.as_ref().unwrap();
    let candidate = |name: &str, version: &str| {
        wat::parse_str(format!(
            r#"(component ${name}
            (instance $exports)
            (export "wassette:acp/agent@{version}" (instance $exports)))"#
        ))
        .unwrap()
    };
    std::fs::write(
        drops.join("candidate.wasm"),
        candidate("discovered-agent", "7.0.0"),
    )
    .unwrap();
    std::fs::write(
        drops.join("bad-version.wasm"),
        candidate("incompatible-agent", "99.0.0"),
    )
    .unwrap();
    let store = h.store();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let receipt = loop {
        match store.read("discovered-agent") {
            Ok(snapshot) => break snapshot.receipt,
            Err(wassette::store::StoreError::NotFound(_)) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "ACP drop not committed"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => panic!("{error}"),
        }
    };
    assert_eq!(receipt.kind, StoredArtifactKind::AcpProvider);
    assert!(!receipt.requests_tool_exposure());
    assert!(matches!(
        receipt.validation,
        ValidationEvidence::AcpCompiledAndExportChecked { .. }
    ));
    assert!(matches!(
        store.read("incompatible-agent"),
        Err(wassette::store::StoreError::NotFound(_))
    ));
    let id = h.prompt(&sid, "original provider remains active");
    let (messages, _) = h.await_response(id);
    assert_eq!(response_text(&messages), "original provider remains active");
    h.close_stdin_and_wait();
}

#[test]
fn install_resolves_wit_selectors_through_wasm_directory() {
    use wassette::wasm_directory::fixtures::{FixturePackage, WasmDirectoryFixture};

    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let fixture = runtime
        .block_on(WasmDirectoryFixture::start(vec![
            FixturePackage::new("owner/first", Some("demo:agent"), []),
            FixturePackage::new("owner/second", Some("demo:agent"), []),
        ]))
        .unwrap();
    let api_url = fixture.api_url.to_string();
    let mut h = Harness::start_with_env(
        &bin,
        &wasm,
        &[],
        &[("WASSETTE_WASM_DIRECTORY_URL", &api_url)],
    );
    let sid = h.open_session();
    for (selector, expected) in [
        (
            "demo:absent",
            "No wasm.directory component package has WIT identity demo:absent".to_owned(),
        ),
        (
            "demo:agent@1.0.0",
            format!(
                "matches multiple wasm.directory packages; select one by its \
                 registry/repository identity: {}, {}",
                fixture.package_id("owner/first"),
                fixture.package_id("owner/second")
            ),
        ),
    ] {
        let id = h.request(
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type": "text", "text": format!("/install {selector}")}]}),
        );
        let (updates, response) = h.await_response(id);
        assert_eq!(response["stopReason"], "end_turn");
        let failed = updates
            .iter()
            .find(|m| {
                m["params"]["update"]["sessionUpdate"] == "tool_call_update"
                    && m["params"]["update"]["status"] == "failed"
            })
            .expect("failed install tool-call update");
        let text = failed["params"]["update"]["content"][0]["content"]["text"]
            .as_str()
            .expect("install result text");
        assert!(text.contains(&expected), "{text}");
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
fn install_command_rejects_unknown_session_without_installing() {
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
        "session/prompt",
        json!({"sessionId": "not-a-session", "prompt": [
            {"type": "text", "text": "/install"}
        ]}),
    );
    let response: Value = serde_json::from_str(&h.next_line()).unwrap();
    assert_eq!(response["id"], id, "{response}");
    assert_eq!(response["error"]["code"], -32602, "{response}");
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown session id"),
        "{response}"
    );
    assert!(
        h.drain_pending().is_empty(),
        "invalid session emitted updates"
    );
}

#[test]
fn invalid_session_selectors_do_not_advertise_install() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let mut h = Harness::start(&bin, &wasm, &[]);
    let id = h.request(
        "initialize",
        json!({"protocolVersion": 1, "clientCapabilities": {}}),
    );
    h.await_response(id);
    for (method, params) in [
        (
            "session/set_mode",
            json!({"sessionId": "unknown", "modeId": "default"}),
        ),
        (
            "session/set_config_option",
            json!({"sessionId": "unknown", "configId": "terminal", "type": "boolean", "value": false}),
        ),
    ] {
        let id = h.request(method, params);
        let response: Value = serde_json::from_str(&h.next_line()).unwrap();
        assert_eq!(response["id"], id, "{response}");
        assert_eq!(response["error"]["code"], -32602, "{response}");
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("unknown session id"),
            "{response}"
        );
        assert!(h.drain_pending().is_empty(), "{method} emitted updates");
    }
}

#[test]
fn duplicate_provider_selection_fails_with_a_clear_cli_error() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let components = tempfile::tempdir().unwrap();
    let output = Command::new(bin)
        .arg("acp")
        .arg("--component-dir")
        .arg(components.path())
        .arg("--provider")
        .arg(&wasm)
        .arg("--provider")
        .arg(&wasm)
        .output()
        .expect("run CLI");
    assert!(
        !output.status.success(),
        "duplicate providers were accepted"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("was selected more than once"),
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
    let session_id = h.open_session_without_grace();
    std::thread::sleep(GATE_FLUSH_GRACE);
    let updates = h.drain_pending();
    assert!(
        updates.iter().any(|update| {
            update["method"] == "session/update"
                && update["params"]["sessionId"] == session_id
                && update["params"]["update"]["sessionUpdate"] == "available_commands_update"
                && update["params"]["update"]["availableCommands"][0]["name"] == "shout"
        }),
        "layer did not advertise /shout after session/new: {updates:#?}"
    );
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
}

#[test]
fn policy_free_layer_chain_runs_with_shared_grants_flag() {
    let Some((bin, wasm)) = artifacts() else {
        return;
    };
    let layer = uppercase_layer().expect("build the uppercase layer with just build-acp-examples");
    let mut h = Harness::start(
        &bin,
        &wasm,
        &["--layer", layer.to_str().unwrap(), "--allow-shared-grants"],
    );
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
    common::seed_secrets(&layer, secrets.path(), &[("TOKEN", "hidden")]);
    let components = tempfile::tempdir().unwrap();
    let output = Command::new(bin)
        .arg("acp")
        .arg("--provider")
        .arg(&wasm)
        .arg("--layer")
        .arg(&layer)
        .arg("--secrets-dir")
        .arg(secrets.path())
        .arg("--component-dir")
        .arg(components.path())
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
    common::seed_secrets(&wasm, secrets.path(), &[("TOKEN", "hidden")]);
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
    let components = tempfile::tempdir().unwrap();
    let output = Command::new(bin)
        .arg("acp")
        .arg("--provider")
        .arg(&wasm)
        .arg("--layer")
        .arg(&layer)
        .arg("--allow-all")
        .arg("--component-dir")
        .arg(components.path())
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
