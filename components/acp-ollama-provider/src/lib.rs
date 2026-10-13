// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! ACP wasm provider that forwards prompts to a local Ollama server.

#[allow(clippy::all)]
mod bindings;
mod ollama;
mod storage;
mod tools;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::bindings::exports::wassette::acp::agent::{Guest, GuestSession, Session};
use crate::bindings::wassette::acp::content::{ContentBlock, TextContent};
use crate::bindings::wassette::acp::errors::{Error, ErrorCode};
use crate::bindings::wassette::acp::init::{
    AgentCapabilities, AuthenticateRequest, ImplementationInfo, InitializeRequest,
    InitializeResponse, McpCapabilities, PromptCapabilities, SessionCapabilities,
};
use crate::bindings::wassette::acp::prompts::{
    PromptResponse, SessionUpdate, StopReason, UsageUpdate,
};
use crate::bindings::wassette::acp::sessions::{
    ComponentSource, ListSessionsRequest, ListSessionsResponse, LoadSessionRequest,
    LoadSessionResponse, NewSessionRequest, NewSessionResponse, ResumeSessionRequest,
    ResumeSessionResponse, SessionConfigId, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigSelectOption, SessionConfigSelectOptions, SessionConfigValueId, SessionModeId,
    SessionModelId,
};

use crate::ollama::Message;
use crate::storage::SessionState;

struct Agent;

/// Host-side representation of an `agent.session` resource.
///
/// The wire identity of a session remains its string id (kept in
/// [`SessionState`-storage](crate::storage::SessionState) under that key); this
/// resource is purely a lifetime handle. Dropping it (in the host's
/// resource table) fires `Drop` here and evicts the session's
/// in-memory state.
pub struct ProviderSession {
    id: String,
}

impl Drop for ProviderSession {
    fn drop(&mut self) {
        SESSIONS.with(|s| {
            s.borrow_mut().remove(&self.id);
        });
    }
}

impl GuestSession for ProviderSession {
    async fn prompt(&self, prompt: Vec<ContentBlock>) -> Result<PromptResponse, Error> {
        prompt_impl(self.id.clone(), prompt).await
    }

    async fn set_mode(&self, _mode_id: SessionModeId) -> Result<(), Error> {
        // Provider advertises no modes; the slot is reserved for
        // layers (e.g. `plan-layer`) to manage. Anything reaching the
        // provider is therefore an unknown mode id.
        Err(err(
            ErrorCode::InvalidParams,
            "ollama provider does not advertise any modes",
        ))
    }

    async fn select_model(&self, model_id: SessionModelId) -> Result<(), Error> {
        let session_id = self.id.clone();
        SESSIONS
            .with(|s| {
                let mut sessions = s.borrow_mut();
                match sessions.get_mut(&session_id) {
                    Some(session) => {
                        session.model = model_id;
                        Ok(())
                    }
                    None => Err(format!("unknown session id: {session_id}")),
                }
            })
            .map_err(|e| err(ErrorCode::InvalidParams, &e))
    }

    async fn set_config_option(
        &self,
        config_id: SessionConfigId,
        value: SessionConfigValueId,
    ) -> Result<Vec<SessionConfigOption>, Error> {
        let session_id = self.id.clone();

        // Apply the change to the addressed option, validating the value
        // against the currently advertised model list.
        let current_model = SESSIONS
            .with(|s| {
                let mut sessions = s.borrow_mut();
                let Some(session) = sessions.get_mut(&session_id) else {
                    return Err(format!("unknown session id: {session_id}"));
                };
                match config_id.as_str() {
                    CONFIG_MODEL => {
                        // Accept any id currently installed; if the cache is
                        // empty (e.g. the models endpoint was unreachable)
                        // accept optimistically so switching stays offline.
                        let known = MODELS_CACHE.with(|c| {
                            let cache = c.borrow();
                            cache.is_empty() || cache.contains(&value)
                        });
                        if !known {
                            return Err(format!("unknown model: {value}"));
                        }
                        session.model = value.clone();
                        Ok(session.model.clone())
                    }
                    other => Err(format!("unknown config option: {other}")),
                }
            })
            .map_err(|e| err(ErrorCode::InvalidParams, &e))?;

        // Rebuild the full option set from the cached model list — no network.
        let models = cached_models(&current_model);
        Ok(build_config_options(&models, &current_model))
    }
}

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Push a `SessionUpdate` upstream via `client.notify-session`. All
/// session updates flow through this side channel; the agent direction
/// only carries the eventual `prompt-response`.
async fn emit_update(session_id: String, update: SessionUpdate) {
    crate::bindings::wassette::acp::client::notify_session(session_id, update).await;
}

// Wasm components are single-threaded; using thread-local + RefCell avoids
// any synchronization cost while keeping interior mutability.
thread_local! {
    static SESSIONS: RefCell<HashMap<String, SessionState>> = RefCell::new(HashMap::new());
}

thread_local! {
    /// The locally installed Ollama model list, cached per host process the
    /// first time a session lifecycle response is built. Lets
    /// `set-config-option` rebuild the full option set without re-hitting the
    /// `/api/tags` endpoint on every switch.
    static MODELS_CACHE: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

thread_local! {
    /// Per-model context-window length (tokens), cached per host process the
    /// first time a prompt turn reports usage. Avoids an `/api/show` round
    /// trip on every turn just to compute the "context %" denominator.
    static CONTEXT_LENGTHS: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
}

/// One-time system message inserted at the start of every fresh session.
///
/// Small tool-capable Ollama models will happily call any tool we
/// advertise on the slimmest pretext, including bare greetings. The
/// guidance below tells them to be conservative; it doesn't have to be
/// perfectly obeyed (we also defensively validate args server-side), but
/// it dramatically reduces useless tool-call storms.
const SYSTEM_PROMPT: &str = "You are a coding assistant connected to the user's editor. You have access to a `read_file` tool that can read source files in the user's project. Only call tools when the user explicitly asks you to read a file, or when reading a specific file is strictly necessary to answer the user's request. For greetings, small talk, or general questions, respond in plain text without calling any tools. Never call `read_file` with an empty path, `/`, `.`, or any directory.";

/// Component id stamped on every config option this provider contributes.
const COMPONENT_ID: &str = "local:ollama-provider";

/// Config option id. Stable identifier the editor echoes back on
/// `set-config-option`.
const CONFIG_MODEL: &str = "model";

fn component_source() -> ComponentSource {
    ComponentSource {
        component_id: COMPONENT_ID.to_string(),
    }
}

/// Build the session's config-option selectors from the installed model list.
///
/// Exposes a single `model` selector (category [`Model`]) whose options are
/// the locally installed Ollama models, mirroring how `copilot-provider`
/// advertises model selection through ACP's unified `config_options`
/// mechanism (which supersedes the removed `session/models` capability).
///
/// [`Model`]: SessionConfigOptionCategory::Model
fn build_config_options(models: &[String], current_model: &str) -> Vec<SessionConfigOption> {
    let model_options = models
        .iter()
        .map(|name| SessionConfigSelectOption {
            value: name.clone(),
            name: name.clone(),
            description: None,
        })
        .collect();
    vec![SessionConfigOption {
        id: CONFIG_MODEL.to_string(),
        name: "Model".to_string(),
        description: Some("Which locally installed Ollama model backs this session.".to_string()),
        category: Some(SessionConfigOptionCategory::Model),
        current_value: current_model.to_string(),
        options: SessionConfigSelectOptions::Ungrouped(model_options),
        provided_by: component_source(),
    }]
}

/// Pick the current model: `preferred` if installed, else
/// [`ollama::default_model`] if installed, else the first model listed.
fn pick_current_model(listed: &[String], preferred: Option<&str>) -> String {
    let default = ollama::default_model();
    let has = |want: &str| listed.iter().any(|m| m == want);
    preferred
        .filter(|m| has(m))
        .map(|s| s.to_string())
        .or_else(|| if has(&default) { Some(default.clone()) } else { None })
        .unwrap_or_else(|| listed[0].clone())
}

/// Read the cached model list, falling back to a single entry for
/// `current_model` when the cache is empty (e.g. `/api/tags` was
/// unreachable). Keeps `set-config-option` offline.
fn cached_models(current_model: &str) -> Vec<String> {
    let cached = MODELS_CACHE.with(|c| c.borrow().clone());
    if cached.is_empty() {
        vec![current_model.to_string()]
    } else {
        cached
    }
}

/// Resolve the context-window length (tokens) for `model`, consulting
/// [`CONTEXT_LENGTHS`] first and querying `/api/show` on a miss. Returns
/// `None` when the size is unknown, so callers skip usage reporting rather
/// than emit a meaningless denominator.
async fn context_length_for(model: &str) -> Option<u64> {
    if let Some(n) = CONTEXT_LENGTHS.with(|c| c.borrow().get(model).copied()) {
        return Some(n);
    }
    match ollama::model_context_length(model).await {
        Ok(Some(n)) => {
            CONTEXT_LENGTHS.with(|c| c.borrow_mut().insert(model.to_string(), n));
            Some(n)
        }
        Ok(None) => None,
        Err(e) => {
            eprintln!("failed to fetch context length for {model} ({e})");
            None
        }
    }
}

/// List the locally installed Ollama models (populating [`MODELS_CACHE`]) and
/// build the session's config-option selectors plus the resolved current
/// model. The current model defaults to `preferred_model` when installed,
/// otherwise `default_model()`, otherwise the first model returned.
///
/// If Ollama is unreachable or returns no models, falls back to a single
/// entry for `default_model()` so session creation still succeeds; the
/// failure is logged to stderr.
async fn build_session_config(
    preferred_model: Option<&str>,
) -> (Vec<SessionConfigOption>, String) {
    let default = ollama::default_model();
    let listed = match ollama::list_models().await {
        Ok(models) if !models.is_empty() => models,
        Ok(_) => {
            eprintln!("ollama returned no models; using default");
            vec![default.clone()]
        }
        Err(e) => {
            eprintln!("failed to list ollama models ({e}); using default");
            vec![default.clone()]
        }
    };
    MODELS_CACHE.with(|c| *c.borrow_mut() = listed.clone());
    let current = pick_current_model(&listed, preferred_model);
    let options = build_config_options(&listed, &current);
    (options, current)
}

fn next_session_id() -> String {
    // Mix wall-clock seconds with a per-process counter so ids are unique
    // across host restarts. The editor's chat panel often retains the last
    // session id and replays it via `session/load` after a host restart;
    // returning the *same* id from a fresh `next_session_id()` call would
    // make distinct sessions collide on disk.
    let n = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("ollama-session-{secs:x}-{n}")
}

fn err(code: ErrorCode, message: &str) -> Error {
    Error {
        code,
        message: message.to_string(),
    }
}

/// Look up the absolute working directory associated with a session.
/// Returns the empty string when the session is unknown (which can happen
/// in tests or if `prompt` is reached without a prior `new-session`).
pub fn session_cwd(session_id: &str) -> String {
    SESSIONS.with(|s| {
        s.borrow()
            .get(session_id)
            .map(|sess| sess.cwd.clone())
            .unwrap_or_default()
    })
}

/// Pull all `text` content blocks from a prompt, joined with blank lines.
fn extract_user_text(prompt: &[ContentBlock]) -> String {
    let mut out = String::new();
    for block in prompt {
        if let ContentBlock::Text(TextContent { text }) = block {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(text);
        }
    }
    out
}

impl Guest for Agent {
    type Session = ProviderSession;

    async fn initialize(_req: InitializeRequest) -> Result<InitializeResponse, Error> {
        Ok(InitializeResponse {
            protocol_version: 1,
            agent_capabilities: AgentCapabilities {
                load_session: true,
                prompt_capabilities: PromptCapabilities {
                    image: false,
                    audio: false,
                    embedded_context: false,
                },
                mcp_capabilities: McpCapabilities {
                    http: false,
                    sse: false,
                },
                session_capabilities: SessionCapabilities {
                    list: false,
                    resume: true,
                    close: false,
                },
            },
            agent_info: Some(ImplementationInfo {
                name: "ollama-wasm-agent".to_string(),
                title: Some("Ollama (wasm)".to_string()),
                version: env!("CARGO_PKG_VERSION").to_string(),
            }),
            auth_methods: Vec::new(),
        })
    }

    async fn authenticate(_req: AuthenticateRequest) -> Result<(), Error> {
        Err(err(
            ErrorCode::MethodNotFound,
            "authentication not required",
        ))
    }

    async fn new_session(req: NewSessionRequest) -> Result<(Session, NewSessionResponse), Error> {
        let id = next_session_id();
        let (config_options, current_model) = build_session_config(None).await;
        SESSIONS.with(|s| {
            s.borrow_mut().insert(
                id.clone(),
                crate::storage::SessionState {
                    history: Vec::new(),
                    model: current_model,
                    cwd: req.cwd,
                },
            )
        });
        let resource = Session::new(ProviderSession { id: id.clone() });
        Ok((
            resource,
            NewSessionResponse {
                session_id: id,
                modes: None,
                models: None,
                config_options: Some(config_options),
            },
        ))
    }

    async fn load_session(
        req: LoadSessionRequest,
    ) -> Result<(Session, LoadSessionResponse), Error> {
        let session_id = req.session_id.clone();
        // Load the persisted session if present, then replay history to
        // the client as `update-session` notifications (per the ACP spec
        // for `session/load`). Missing file = fresh session.
        let default = ollama::default_model();
        let stored = match storage::load(&session_id, &default) {
            Ok(s) => s,
            Err(e) => return Err(err(ErrorCode::InternalError, &format!("load: {e}"))),
        };
        let history = stored
            .as_ref()
            .map(|s| s.history.clone())
            .unwrap_or_default();
        let preferred = stored.as_ref().map(|s| s.model.clone());
        let (config_options, current_model) = build_session_config(preferred.as_deref()).await;
        for msg in &history {
            let block = ContentBlock::Text(TextContent {
                text: msg.content.clone(),
            });
            let update = match msg.role.as_str() {
                "user" => SessionUpdate::UserMessageChunk(block),
                "assistant" => SessionUpdate::AgentMessageChunk(block),
                _ => continue,
            };
            emit_update(session_id.clone(), update).await;
        }
        SESSIONS.with(|s| {
            s.borrow_mut().insert(
                session_id.clone(),
                crate::storage::SessionState {
                    history,
                    model: current_model,
                    cwd: req.cwd,
                },
            );
        });
        let resource = Session::new(ProviderSession { id: session_id });
        Ok((
            resource,
            LoadSessionResponse {
                modes: None,
                models: None,
                config_options: Some(config_options),
            },
        ))
    }

    async fn list_sessions(_req: ListSessionsRequest) -> Result<ListSessionsResponse, Error> {
        Err(err(
            ErrorCode::MethodNotFound,
            "list-sessions not supported",
        ))
    }

    async fn resume_session(
        req: ResumeSessionRequest,
    ) -> Result<(Session, ResumeSessionResponse), Error> {
        let session_id = req.session_id.clone();
        // Like `load_session`, but the spec forbids replaying history
        // via `update-session`. Just rehydrate the in-memory map.
        let default = ollama::default_model();
        let stored = match storage::load(&session_id, &default) {
            Ok(s) => s,
            Err(e) => return Err(err(ErrorCode::InternalError, &format!("resume: {e}"))),
        };
        let history = stored
            .as_ref()
            .map(|s| s.history.clone())
            .unwrap_or_default();
        let preferred = stored.as_ref().map(|s| s.model.clone());
        let (config_options, current_model) = build_session_config(preferred.as_deref()).await;
        SESSIONS.with(|s| {
            s.borrow_mut().insert(
                session_id.clone(),
                crate::storage::SessionState {
                    history,
                    model: current_model,
                    cwd: req.cwd,
                },
            );
        });
        let resource = Session::new(ProviderSession { id: session_id });
        Ok((
            resource,
            ResumeSessionResponse {
                modes: None,
                models: None,
                config_options: Some(config_options),
            },
        ))
    }
}

async fn prompt_impl(
    session_id: String,
    prompt: Vec<ContentBlock>,
) -> Result<PromptResponse, Error> {
    let user_text = extract_user_text(&prompt);
    if user_text.is_empty() {
        return Err(err(
            ErrorCode::InvalidParams,
            "prompt contained no text content",
        ));
    }

    // Append the user turn to this session's history and grab a copy
    // (plus the active model) to send to Ollama. New sessions can land
    // here without going through `new-session` (e.g. tests); fall back
    // to the default model in that case.
    //
    // We also prepend a one-time `system` message on the first prompt
    // of a session, instructing the model to be conservative about
    // tool calls. Without this, small tool-capable models tend to call
    // `read_file` on greetings like "hello" out of pure enthusiasm.
    let (mut history, model) = SESSIONS.with(|s| {
        let mut sessions = s.borrow_mut();
        let entry = sessions
            .entry(session_id.clone())
            .or_insert_with(|| SessionState {
                history: Vec::new(),
                model: ollama::default_model(),
                cwd: String::new(),
            });
        if entry.history.is_empty() {
            let mut prompt = SYSTEM_PROMPT.to_string();
            if !entry.cwd.is_empty() {
                prompt.push_str("\n\nThe user's current project directory is `");
                prompt.push_str(&entry.cwd);
                prompt.push_str(
                    "`. When the user refers to files without an absolute path, \
                         resolve them relative to this directory. Never call `read_file` \
                         with `/`, `.`, `..`, the project directory itself, or any other \
                         directory - only call it with a path to a specific file.",
                );
            }
            entry.history.push(Message {
                role: "system".to_string(),
                content: prompt,
                tool_calls: Vec::new(),
            });
        }
        entry.history.push(Message::user(user_text.clone()));
        (entry.history.clone(), entry.model.clone())
    });

    // Probe whether the active model supports tool-calling. We only
    // advertise tools when it does; otherwise the request is plain
    // chat. The probe is best-effort: if `/api/show` fails we just
    // assume no tool support and degrade to chat. We also surface a
    // one-time thought chunk so users understand why their model
    // isn't using tools.
    let session_id = session_id.clone();
    let model_clone = model.clone();
    let tools_supported = ollama::supports_tools(&model_clone).await.unwrap_or(false);
    if !tools_supported {
        emit_update(
            session_id.clone(),
            SessionUpdate::AgentThoughtChunk(ContentBlock::Text(TextContent {
                text: format!(
                    "(model `{model}` does not advertise tool support; running in chat-only mode)"
                ),
            })),
        )
        .await;
    }
    let advertised_tools = if tools_supported {
        tools::ollama_tools()
    } else {
        Vec::new()
    };

    // Tool-call loop. The model can answer in one turn (no tool calls
    // → done) or it can request tools, in which case we dispatch each,
    // append a `role: "tool"` message per result, and loop. We cap
    // iterations to avoid runaway models.
    const MAX_TURNS: usize = 3;
    let mut tool_call_seq: u64 = 0;
    let mut stop = StopReason::EndTurn;
    let mut turns_remaining = MAX_TURNS;
    // Token counts from the most recent `/api/chat` turn. The final turn's
    // counts describe the full context (history grows monotonically), which
    // we forward as a `usage-update` once the loop settles.
    let mut last_prompt_eval: Option<u64> = None;
    let mut last_eval: Option<u64> = None;
    loop {
        if turns_remaining == 0 {
            stop = StopReason::MaxTurnRequests;
            break;
        }
        turns_remaining -= 1;

        let session_id_chunk = session_id.clone();
        let turn = ollama::chat(
            model.clone(),
            history.clone(),
            advertised_tools.clone(),
            |chunk| {
                let sid = session_id_chunk.clone();
                async move {
                    emit_update(
                        sid,
                        SessionUpdate::AgentMessageChunk(ContentBlock::Text(TextContent {
                            text: chunk,
                        })),
                    )
                    .await;
                }
            },
        )
        .await
        .map_err(|e| err(ErrorCode::InternalError, &format!("ollama: {e}")))?;

        // Remember this turn's token accounting for the usage-update.
        if turn.prompt_eval_count.is_some() {
            last_prompt_eval = turn.prompt_eval_count;
        }
        if turn.eval_count.is_some() {
            last_eval = turn.eval_count;
        }

        // Persist the assistant turn (text + any tool-call requests)
        // back into history so the next ollama call sees it.
        history.push(Message::assistant(
            turn.content.clone(),
            turn.tool_calls.clone(),
        ));

        if turn.tool_calls.is_empty() {
            break;
        }

        // Dispatch each tool call. For each one, send the editor a
        // `tool_call` notification (status: in_progress), run the
        // tool, send a `tool_call_update` (status: completed/failed
        // with content), and feed the result back as a `role: "tool"`
        // message for the next iteration.
        //
        // TODO(streams phase 4): replace these inline `SessionUpdate::ToolCall`
        // / `SessionUpdate::ToolCallUpdate` emissions with the new
        // [`tools.tool-call`] resource (constructor + `update(patch)`).
        // For phase 1 the tool-call lifecycle simply isn't surfaced to
        // the editor — the model still calls tools and consumes their
        // results, the user just won't see progress cards yet.
        for call in &turn.tool_calls {
            tool_call_seq += 1;
            let _tc_id = format!("tc-{tool_call_seq}");
            let _tool = tools::lookup(&call.function.name);
            let _title = tools::render_title(&call.function.name, &call.function.arguments);
            let _raw_input = serde_json::to_string(&call.function.arguments).ok();

            let outcome =
                tools::dispatch(&call.function.name, &session_id, &call.function.arguments).await;

            // Feed the result back to the model as a `role: "tool"`
            // message. For failures, prefix with a clear marker so
            // small models don't mistake the error text for data
            // (Ollama's chat API doesn't carry an `is_error` flag).
            let tool_msg = if outcome.failed {
                format!("Error: {}", outcome.content)
            } else {
                outcome.content
            };
            history.push(Message::tool(tool_msg));
        }
    }

    // Emit context-window usage so the editor can render a "context %"
    // indicator (context % = used / size). `used` is the final turn's
    // prompt + generated tokens (the tokens now occupying the context);
    // `size` is the model's context-window length. Skip silently when
    // Ollama didn't report counts or the window size is unknown.
    let used = last_prompt_eval.unwrap_or(0) + last_eval.unwrap_or(0);
    if used > 0
        && let Some(size) = context_length_for(&model).await {
            emit_update(
                session_id.clone(),
                SessionUpdate::UsageUpdate(UsageUpdate {
                    used,
                    size,
                    cost: None,
                }),
            )
            .await;
        }

    // Replace the session's history with our updated copy and persist.
    let snapshot = SESSIONS.with(|s| {
        let mut sessions = s.borrow_mut();
        sessions.get_mut(&session_id).map(|entry| {
            entry.history = history;
            entry.clone()
        })
    });
    if let Some(session) = snapshot
        && let Err(e) = storage::save(&session_id, &session) {
            // Persistence is best-effort: a failed save shouldn't fail
            // the prompt turn (the user already saw the reply). Log
            // via a thought chunk so it's visible.
            emit_update(
                session_id.clone(),
                SessionUpdate::AgentThoughtChunk(ContentBlock::Text(TextContent {
                    text: format!("(failed to persist session: {e})"),
                })),
            )
            .await;
        }

    Ok(PromptResponse { stop_reason: stop })
}

bindings::export!(Agent with_types_in bindings);
