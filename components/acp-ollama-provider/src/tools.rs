// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Tools the language model can call during a prompt turn.
//!
//! Each tool has:
//!   - a name (must match what we advertise to Ollama),
//!   - a `ToolKind` for the editor's UI hint,
//!   - a description (the model uses this to decide when to call it),
//!   - a JSON-schema for its arguments,
//!   - a runner that dispatches the call into the host's `client`
//!     interface (or, in future, into peer sessions).
//!
//! For now there's a single tool, `read_file`, which forwards to ACP's
//! `fs/read_text_file`. Adding more tools is "add another `Tool` to
//! [`all`]" — no boilerplate beyond the schema.

use crate::bindings::wassette::acp::client;
use crate::bindings::wassette::acp::filesystem::ReadTextFileRequest;
use crate::bindings::wassette::acp::tools::ToolKind;
use serde_json::{Value, json};

use crate::ollama::{OllamaTool, OllamaToolCall};

/// Outcome of running a tool. Either a string result to feed back into
/// the model, or an error message that surfaces to the user as a failed
/// tool-call status.
///
/// `locations` are file paths the call ended up acting on (resolved to
/// absolute form). The bridge forwards these to the editor on the
/// `tool_call_update`, so the editor can highlight or anchor the activity.
pub struct ToolOutcome {
    pub content: String,
    pub failed: bool,
    pub locations: Vec<String>,
}

impl ToolOutcome {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            failed: false,
            locations: Vec::new(),
        }
    }

    pub fn fail(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            failed: true,
            locations: Vec::new(),
        }
    }

    pub fn with_location(mut self, path: impl Into<String>) -> Self {
        self.locations.push(path.into());
        self
    }
}

/// One tool the model can call.
pub struct Tool {
    pub name: &'static str,
    // Editor UI hint; not yet surfaced on tool-call updates.
    #[allow(dead_code)]
    pub kind: ToolKind,
    pub description: &'static str,
    /// JSON-schema for the function arguments.
    pub parameters: fn() -> Value,
}

impl Tool {
    /// Build the per-call `ToolCall` payload (id, title, status pending)
    /// for the editor. The id namespaces tool-call updates within a
    /// session; we use a simple incrementing counter passed in.
    pub fn ollama_tool(&self) -> OllamaTool {
        OllamaTool::function(self.name, self.description, (self.parameters)())
    }
}

/// All tools enabled for this provider. Order is informational; the model
/// addresses tools by name.
pub fn all() -> &'static [Tool] {
    static TOOLS: &[Tool] = &[Tool {
        name: "read_file",
        kind: ToolKind::Read,
        description: "Read a UTF-8 text file from the user's project. \
                      Only call this tool when the user explicitly asks \
                      you to look at a file, or when reading is strictly \
                      necessary to answer their question. Do NOT call \
                      this for greetings or general chat. \
                      \
                      The `path` argument is a path to a specific file: \
                      either absolute, or relative to the project root \
                      (e.g. `src/main.rs`, `Cargo.toml`). It MUST NOT be \
                      a directory, `/`, or empty.",
        parameters: read_file_schema,
    }];
    TOOLS
}

/// Look up a tool by the name the model used in its `tool_calls` entry.
pub fn lookup(name: &str) -> Option<&'static Tool> {
    all().iter().find(|t| t.name == name)
}

fn read_file_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "Path to the file. Absolute, or relative to the project root."
            }
        },
        "required": ["path"],
        "additionalProperties": false
    })
}

/// Dispatch a tool by name. Unknown names produce a `ToolOutcome::fail`
/// the caller can feed back to the model.
pub async fn dispatch(name: &str, session_id: &str, args: &Value) -> ToolOutcome {
    match name {
        "read_file" => read_file_run(session_id, args).await,
        _ => ToolOutcome::fail(format!("unknown tool `{name}`")),
    }
}

async fn read_file_run(session_id: &str, args: &Value) -> ToolOutcome {
    // Models inconsistently send arguments as a JSON object or as a
    // JSON-encoded string. Accept both.
    let args = match args {
        Value::Object(_) => args.clone(),
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(v) => v,
            Err(e) => return ToolOutcome::fail(format!("invalid arguments JSON: {e}")),
        },
        _ => return ToolOutcome::fail("expected an object of arguments".to_string()),
    };
    let Some(path) = args.get("path").and_then(Value::as_str) else {
        return ToolOutcome::fail("missing required argument `path`".to_string());
    };
    // Reject obviously-bad paths up front. Models occasionally call
    // `read_file` with `""`, `/`, or a bare directory name when they're
    // confused; forwarding those to the editor wedges its fs handler.
    let trimmed = path.trim();
    if trimmed.is_empty()
        || trimmed == "/"
        || trimmed == "."
        || trimmed == ".."
        || trimmed.ends_with('/')
    {
        return ToolOutcome::fail(format!(
            "`{path}` is not a file path. Provide a path to a specific file (e.g. `src/main.rs`)."
        ));
    }

    // ACP `fs/read_text_file` requires absolute paths. Models reliably
    // hand us paths relative to the project root (e.g. `src/main.rs`),
    // so we resolve them against the session's `cwd` here. Absolute
    // paths pass through unchanged.
    let absolute = if std::path::Path::new(trimmed).is_absolute() {
        trimmed.to_string()
    } else {
        let cwd = crate::session_cwd(session_id);
        if cwd.is_empty() {
            return ToolOutcome::fail(format!(
                "cannot resolve relative path `{path}`: session has no working directory.                  Pass an absolute path."
            ));
        }
        // Plain string concatenation is enough; we only handle POSIX
        // paths (the editor's cwd is always absolute, the model's path
        // is a project-relative segment).
        let trimmed_path = trimmed.trim_start_matches("./");
        if cwd.ends_with('/') {
            format!("{cwd}{trimmed_path}")
        } else {
            format!("{cwd}/{trimmed_path}")
        }
    };

    let req = ReadTextFileRequest {
        session_id: session_id.to_string(),
        path: absolute.clone(),
        line: None,
        limit: None,
    };
    let absolute_for_loc = absolute.clone();
    match client::read_text_file(req).await {
        Ok(resp) => ToolOutcome::ok(resp.content).with_location(absolute_for_loc),
        Err(e) => ToolOutcome::fail(format!("read_text_file({absolute}): {}", e.message))
            .with_location(absolute_for_loc),
    }
}

/// Render the tool-call args (whatever the model gave us) into a short
/// human-readable title for the ACP `ToolCall.title` field.
pub fn render_title(name: &str, args: &Value) -> String {
    match name {
        "read_file" => {
            let path = args
                .get("path")
                .and_then(Value::as_str)
                .or_else(|| args.as_str())
                .unwrap_or("?");
            format!("Reading {path}")
        }
        _ => name.to_string(),
    }
}

/// Convenience: build the `tools[]` array we send to Ollama.
pub fn ollama_tools() -> Vec<OllamaTool> {
    all().iter().map(|t| t.ollama_tool()).collect()
}

/// Marker so `OllamaToolCall` is referenced in this module — keeps the
/// `use` import alive for future tool dispatch helpers.
#[allow(dead_code)]
fn _unused_marker(_: &OllamaToolCall) {}
