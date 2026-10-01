// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! ACP wasm provider that forwards prompts to the GitHub Copilot chat API.
//!
//! Mirrors `ollama-provider` but targets Copilot: it resolves a GitHub token
//! from the host secrets store (or env), exchanges it for a short-lived
//! Copilot API token, and streams OpenAI-compatible chat completions back to
//! the editor as `session/update` notifications. Text only — no tool calls.

#[allow(clippy::all)]
mod bindings;
mod component_tools;
mod copilot;
mod generation;
mod storage;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

use crate::bindings::exports::wassette::acp::agent::{Guest, GuestSession, Session};
use crate::bindings::wassette::acp::client;
use crate::bindings::wassette::acp::content::{ContentBlock, TextContent};
use crate::bindings::wassette::acp::errors::{Error, ErrorCode};
use crate::bindings::wassette::acp::filesystem::{ReadTextFileRequest, WriteTextFileRequest};
use crate::bindings::wassette::acp::init::{
    AgentCapabilities, AuthenticateRequest, ImplementationInfo, InitializeRequest,
    InitializeResponse, McpCapabilities, PromptCapabilities, SessionCapabilities,
};
use crate::bindings::wassette::acp::prompts::{
    PromptResponse, SessionUpdate, StopReason, UsageCost, UsageUpdate,
};
use crate::bindings::wassette::acp::sessions::{
    ComponentSource, ListSessionsRequest, ListSessionsResponse, LoadSessionRequest,
    LoadSessionResponse, NewSessionRequest, NewSessionResponse, ResumeSessionRequest,
    ResumeSessionResponse, SessionConfigId, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigSelectOption, SessionConfigSelectOptions, SessionConfigValueId, SessionModeId,
    SessionModelId,
};
use crate::bindings::wassette::acp::terminals::CreateTerminalRequest;
use crate::bindings::wassette::acp::tools::{
    Diff, PermissionOption, PermissionOptionKind, PermissionOutcome, RequestPermissionRequest,
    ToolCallContent, ToolCallLocation, ToolCallSnapshot, ToolCallStatus, ToolKind,
};
use crate::copilot::Message;
use crate::storage::SessionState;

struct Agent;

/// Host-side representation of an `agent.session` resource. Dropping it evicts
/// the session's in-memory state.
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
        // The Copilot API exposes no "mode" concept, so this provider never
        // advertises modes and the legacy `set-mode` slot is unused (it is
        // reserved for layers). Model and thinking-level selection are driven
        // through `set-config-option` instead.
        Err(err(
            ErrorCode::InvalidParams,
            "copilot provider does not support modes",
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
        // against the option's advertised (upstream) set.
        let snapshot = SESSIONS
            .with(|s| {
                let mut sessions = s.borrow_mut();
                let Some(session) = sessions.get_mut(&session_id) else {
                    return Err(format!("unknown session id: {session_id}"));
                };
                match config_id.as_str() {
                    CONFIG_MODEL => {
                        // Accept any id the account currently advertises; if
                        // the cache is empty (e.g. the models endpoint was
                        // unreachable) accept optimistically.
                        let known = MODELS_CACHE.with(|c| {
                            let cache = c.borrow();
                            cache.is_empty() || cache.iter().any(|m| m.id == value)
                        });
                        if !known {
                            return Err(format!("unknown model: {value}"));
                        }
                        session.model = value.clone();
                        // The new model may support a different set of
                        // reasoning levels (or none). Re-resolve so we never
                        // carry a level the model rejects.
                        let models = cached_models(&session.model);
                        session.reasoning = resolve_reasoning(
                            find_model(&models, &session.model),
                            &session.reasoning,
                        );
                    }
                    CONFIG_REASONING => {
                        // Validate against the current model's upstream
                        // reasoning levels — the only source of truth.
                        let models = cached_models(&session.model);
                        let supported = find_model(&models, &session.model)
                            .map(|m| m.reasoning_efforts.contains(&value))
                            .unwrap_or(false);
                        if !supported {
                            return Err(format!(
                                "model {} does not support thinking level: {value}",
                                session.model
                            ));
                        }
                        session.reasoning = value.clone();
                    }
                    CONFIG_MODE => {
                        // Chat mode: agent / plan / autopilot.
                        match value.as_str() {
                            MODE_AGENT | MODE_PLAN | MODE_AUTOPILOT => {
                                session.mode = value.clone();
                            }
                            other => return Err(format!("unknown mode: {other}")),
                        }
                    }
                    CONFIG_ALLOW_ALL => {
                        // Auto tool approval toggle: on / off.
                        match value.as_str() {
                            "on" => session.allow_all = true,
                            "off" => session.allow_all = false,
                            other => {
                                return Err(format!("invalid allow-all value: {other} (on|off)"));
                            }
                        }
                    }
                    CONFIG_TERMINAL => {
                        // Host-only notification, never advertised by the
                        // provider. The host still enforces execution.
                        match value.as_str() {
                            "on" => session.terminal_enabled = true,
                            "off" => session.terminal_enabled = false,
                            other => {
                                return Err(format!("invalid terminal value: {other} (on|off)"));
                            }
                        }
                    }
                    CONFIG_GENERATION => {
                        // Host-only notification, never advertised by the
                        // provider. Sent only when the operator profile
                        // permits building and installing components; the
                        // host still authorizes every generation request.
                        match value.as_str() {
                            "on" => session.generation_enabled = true,
                            "off" => session.generation_enabled = false,
                            other => {
                                return Err(format!(
                                    "invalid component-generation value: {other} (on|off)"
                                ));
                            }
                        }
                    }
                    other => return Err(format!("unknown config option: {other}")),
                }
                Ok(session.clone())
            })
            .map_err(|e| err(ErrorCode::InvalidParams, &e))?;

        // The generation notice is host state, not a user choice: nothing to
        // persist and no preference to remember.
        if config_id != CONFIG_GENERATION {
            // Persistence is best-effort; a failed save shouldn't fail the switch.
            let _ = storage::save(&session_id, &snapshot);

            // Remember this choice globally so the next brand-new session starts on
            // the same model + thinking level (keeping the Thinking selector present
            // from the start for reasoning-capable models).
            let _ = storage::save_preferences(&storage::Preferences {
                model: snapshot.model.clone(),
                reasoning: snapshot.reasoning.clone(),
            });
        }

        // Rebuild the full option set from the cached model list — no network.
        let models = cached_models(&snapshot.model);
        Ok(build_config_options(
            &models,
            &snapshot.model,
            &snapshot.reasoning,
            &snapshot.mode,
            snapshot.allow_all,
        ))
    }
}

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Push a `SessionUpdate` upstream via `client.notify-session`. All session
/// updates flow through this side channel; the agent direction only carries
/// the eventual `prompt-response`.
async fn emit_update(session_id: String, update: SessionUpdate) {
    crate::bindings::wassette::acp::client::notify_session(session_id, update).await;
}

// Wasm components are single-threaded; thread-local + RefCell avoids any
// synchronization cost while keeping interior mutability.
/// Component id stamped on every config option this provider contributes.
const COMPONENT_ID: &str = "local:copilot-provider";

// Config option ids. Stable identifiers the editor echoes back on
// `set-config-option`.
const CONFIG_MODEL: &str = "model";
const CONFIG_REASONING: &str = "reasoning-effort";
const CONFIG_MODE: &str = "mode";
const CONFIG_ALLOW_ALL: &str = "allow-all";
const CONFIG_TERMINAL: &str = "terminal";
const CONFIG_GENERATION: &str = "component-generation";

// Chat mode ids, mirroring the GitHub Copilot CLI's mode selector.
const MODE_AGENT: &str = "agent";
const MODE_PLAN: &str = "plan";
const MODE_AUTOPILOT: &str = "autopilot";
/// Default chat mode for a fresh session (safe, conversational).
const DEFAULT_MODE: &str = MODE_AGENT;

/// Preferred default thinking level, used when a model supports reasoning
/// but the session has no (valid) selection yet. Falls back to the model's
/// first advertised level when `medium` isn't offered.
const PREFERRED_REASONING: &str = "medium";

thread_local! {
    /// The account's chat-capable model list (with per-model capabilities),
    /// cached per host process the first time a session lifecycle response
    /// is built. Lets `set-config-option` rebuild the full option set —
    /// including each model's upstream reasoning-effort levels — without
    /// re-hitting the models endpoint on every switch.
    static MODELS_CACHE: RefCell<Vec<copilot::CopilotModel>> = const { RefCell::new(Vec::new()) };
}

thread_local! {
    static SESSIONS: RefCell<HashMap<String, SessionState>> = RefCell::new(HashMap::new());
}

/// Per-session "always allow / always reject" memory for tool permissions. Not
/// persisted — it resets when the host process restarts.
#[derive(Default)]
struct PermState {
    always_allow: HashSet<String>,
    always_reject: HashSet<String>,
}

thread_local! {
    static PERMS: RefCell<HashMap<String, PermState>> = RefCell::new(HashMap::new());
}

/// One-time system message inserted at the start of every fresh session.
const SYSTEM_PROMPT: &str = "You are GitHub Copilot, an AI coding assistant connected to the user's editor. Answer concisely and helpfully. When you include code, use fenced code blocks tagged with the language.";

/// Extra instruction injected (per turn, not persisted) when the session is in
/// `plan` mode: steer the model toward proposing a plan instead of editing.
const PLAN_DIRECTIVE: &str = "You are in Plan mode. Focus on understanding the task and proposing a clear, step-by-step plan. Do not modify files or run mutating tools; only read and inspect. Present the plan and wait for the user to switch modes before implementing.";

fn component_source() -> ComponentSource {
    ComponentSource {
        component_id: COMPONENT_ID.to_string(),
    }
}

/// Human-readable label for a reasoning-effort id (e.g. `xhigh` -> `XHigh`).
fn reasoning_label(id: &str) -> String {
    match id {
        "none" => "None".to_string(),
        "low" => "Low".to_string(),
        "medium" => "Medium".to_string(),
        "high" => "High".to_string(),
        "xhigh" => "XHigh".to_string(),
        "max" => "Max".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                None => other.to_string(),
            }
        }
    }
}

/// Locate a model by id in a listing.
fn find_model<'a>(
    models: &'a [copilot::CopilotModel],
    id: &str,
) -> Option<&'a copilot::CopilotModel> {
    models.iter().find(|m| m.id == id)
}

/// Resolve the effective reasoning level for a model, given the session's
/// stored preference. Returns an empty string when the model advertises no
/// reasoning support (so no thinking selector is shown, and no
/// `reasoning_effort` is sent). Otherwise keeps the stored preference if the
/// model still supports it, else prefers [`PREFERRED_REASONING`], else the
/// model's first advertised level.
fn resolve_reasoning(model: Option<&copilot::CopilotModel>, stored: &str) -> String {
    let Some(model) = model else {
        return String::new();
    };
    let efforts = &model.reasoning_efforts;
    if efforts.is_empty() {
        return String::new();
    }
    if efforts.iter().any(|e| e == stored) {
        return stored.to_string();
    }
    if efforts.iter().any(|e| e == PREFERRED_REASONING) {
        return PREFERRED_REASONING.to_string();
    }
    efforts[0].clone()
}

/// Resolve a stored chat-mode id to a known mode, defaulting to
/// [`DEFAULT_MODE`] for empty (legacy sessions) or unrecognized values.
fn resolve_mode(stored: &str) -> String {
    match stored {
        MODE_AGENT | MODE_PLAN | MODE_AUTOPILOT => stored.to_string(),
        _ => DEFAULT_MODE.to_string(),
    }
}

/// Whether a mode/allow-all combination should approve tool calls without
/// prompting: an explicit "allow all", or autopilot (which implies it).
fn auto_approves(mode: &str, allow_all: bool) -> bool {
    allow_all || mode == MODE_AUTOPILOT
}

/// Build the config-option selectors advertised on every session lifecycle
/// response and returned from `set-config-option`: a **Mode** selector
/// (agent / plan / autopilot), the **Model** selector (always present), a
/// **Thinking** selector (only when the current model advertises
/// `reasoning_effort` upstream), and an **Auto-approve** tool-approval
/// toggle. Mirrors the selectors the GitHub Copilot CLI advertises over ACP.
fn build_config_options(
    models: &[copilot::CopilotModel],
    current_model: &str,
    current_reasoning: &str,
    current_mode: &str,
    allow_all: bool,
) -> Vec<SessionConfigOption> {
    // Mode selector — how Copilot operates for this session.
    let mut options = vec![SessionConfigOption {
        id: CONFIG_MODE.to_string(),
        name: "Mode".to_string(),
        description: Some(
            "Controls how Copilot responds: a conversational agent, planning \
             multi-step work, or autonomous autopilot."
                .to_string(),
        ),
        category: Some(SessionConfigOptionCategory::Mode),
        current_value: resolve_mode(current_mode),
        options: SessionConfigSelectOptions::Ungrouped(vec![
            SessionConfigSelectOption {
                value: MODE_AGENT.to_string(),
                name: "Agent".to_string(),
                description: Some(
                    "Default agent mode for conversational interactions.".to_string(),
                ),
            },
            SessionConfigSelectOption {
                value: MODE_PLAN.to_string(),
                name: "Plan".to_string(),
                description: Some(
                    "Plan mode: propose a step-by-step plan without making changes.".to_string(),
                ),
            },
            SessionConfigSelectOption {
                value: MODE_AUTOPILOT.to_string(),
                name: "Autopilot".to_string(),
                description: Some(
                    "Autonomous mode that enables allow-all and runs until the task is \
                     complete without prompting."
                        .to_string(),
                ),
            },
        ]),
        provided_by: component_source(),
    }];

    let model_options = models
        .iter()
        .map(|m| SessionConfigSelectOption {
            value: m.id.clone(),
            name: m.name.clone(),
            description: None,
        })
        .collect();
    options.push(SessionConfigOption {
        id: CONFIG_MODEL.to_string(),
        name: "Model".to_string(),
        description: Some("Which Copilot model backs this session.".to_string()),
        category: Some(SessionConfigOptionCategory::Model),
        current_value: current_model.to_string(),
        options: SessionConfigSelectOptions::Ungrouped(model_options),
        provided_by: component_source(),
    });

    // Thinking levels come straight from the current model's upstream
    // capabilities; omit the selector entirely for models without reasoning.
    if let Some(model) = find_model(models, current_model)
        && !model.reasoning_efforts.is_empty()
    {
        let reasoning_options = model
            .reasoning_efforts
            .iter()
            .map(|id| SessionConfigSelectOption {
                value: id.clone(),
                name: reasoning_label(id),
                description: None,
            })
            .collect();
        options.push(SessionConfigOption {
            id: CONFIG_REASONING.to_string(),
            name: "Thinking".to_string(),
            description: Some(
                "How much reasoning effort the model applies (from the model's \
                     capabilities)."
                    .to_string(),
            ),
            category: Some(SessionConfigOptionCategory::ThoughtLevel),
            current_value: current_reasoning.to_string(),
            options: SessionConfigSelectOptions::Ungrouped(reasoning_options),
            provided_by: component_source(),
        });
    }

    // Auto tool approval ("allow all"). A plain on/off toggle in the
    // `permissions` category. Autopilot implies "on".
    let effective_allow_all = auto_approves(current_mode, allow_all);
    options.push(SessionConfigOption {
        id: CONFIG_ALLOW_ALL.to_string(),
        name: "Auto-approve".to_string(),
        description: Some(
            "Controls whether Copilot prompts for approval before using tools. When on, \
             tool calls are approved automatically."
                .to_string(),
        ),
        category: Some(SessionConfigOptionCategory::Other(
            "permissions".to_string(),
        )),
        current_value: if effective_allow_all { "on" } else { "off" }.to_string(),
        options: SessionConfigSelectOptions::Ungrouped(vec![
            SessionConfigSelectOption {
                value: "on".to_string(),
                name: "On".to_string(),
                description: Some("Automatically approve all tool requests.".to_string()),
            },
            SessionConfigSelectOption {
                value: "off".to_string(),
                name: "Off".to_string(),
                description: Some("Require approval for tool requests.".to_string()),
            },
        ]),
        provided_by: component_source(),
    });

    options
}

/// Pick the current model: `preferred` if advertised, else
/// [`copilot::default_model`] if advertised, else the first model listed.
fn pick_current_model(models: &[copilot::CopilotModel], preferred: Option<&str>) -> String {
    let default = copilot::default_model();
    let has = |want: &str| models.iter().any(|m| m.id == want);
    preferred
        .filter(|m| has(m))
        .map(|s| s.to_string())
        .or_else(|| {
            if has(&default) {
                Some(default.clone())
            } else {
                None
            }
        })
        .unwrap_or_else(|| models[0].id.clone())
}

/// Read the cached model list, falling back to a single (reasoning-less)
/// entry for `current_model` when the cache is empty (e.g. the models
/// endpoint was unreachable). Keeps `set-config-option` offline.
fn cached_models(current_model: &str) -> Vec<copilot::CopilotModel> {
    let cached = MODELS_CACHE.with(|c| c.borrow().clone());
    if cached.is_empty() {
        vec![copilot::CopilotModel::fallback(current_model)]
    } else {
        cached
    }
}

/// List chat-capable Copilot models (populating [`MODELS_CACHE`]) and build the
/// session's config-option selectors, the resolved current model, the resolved
/// thinking level for that model, and the resolved chat mode. Falls back to a
/// single entry for [`copilot::default_model`] if the API is unreachable or
/// returns nothing, so session creation still succeeds.
async fn build_session_config(
    preferred_model: Option<&str>,
    stored_reasoning: &str,
    stored_mode: &str,
    allow_all: bool,
) -> (Vec<SessionConfigOption>, String, String, String) {
    let default = copilot::default_model();
    let listed = match copilot::list_models().await {
        Ok(models) if !models.is_empty() => models,
        Ok(_) => {
            eprintln!("copilot returned no models; using default");
            vec![copilot::CopilotModel::fallback(default.clone())]
        }
        Err(e) => {
            eprintln!("failed to list copilot models ({e}); using default");
            vec![copilot::CopilotModel::fallback(default.clone())]
        }
    };
    MODELS_CACHE.with(|c| *c.borrow_mut() = listed.clone());
    let current = pick_current_model(&listed, preferred_model);
    let reasoning = resolve_reasoning(find_model(&listed, &current), stored_reasoning);
    let mode = resolve_mode(stored_mode);
    let options = build_config_options(&listed, &current, &reasoning, &mode, allow_all);
    (options, current, reasoning, mode)
}

fn next_session_id() -> String {
    // Mix wall-clock seconds with a per-process counter so ids are unique
    // across host restarts.
    let n = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("copilot-session-{secs:x}-{n}")
}

fn err(code: ErrorCode, message: &str) -> Error {
    Error {
        code,
        message: message.to_string(),
    }
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
                name: "copilot-wasm-agent".to_string(),
                title: Some("GitHub Copilot (wasm)".to_string()),
                version: env!("CARGO_PKG_VERSION").to_string(),
            }),
            auth_methods: Vec::new(),
        })
    }

    async fn authenticate(_req: AuthenticateRequest) -> Result<(), Error> {
        // Auth is handled out-of-band via the host secrets store / env; there
        // is no interactive auth method to run here.
        Err(err(
            ErrorCode::MethodNotFound,
            "authentication not required; configure a GitHub token via the host secrets store",
        ))
    }

    async fn new_session(req: NewSessionRequest) -> Result<(Session, NewSessionResponse), Error> {
        let id = next_session_id();
        // Default a brand-new session to the model + thinking level the user
        // last selected (persisted globally), falling back to the configured
        // default model. This ensures the Thinking selector is populated from
        // the start whenever the last-used model supports reasoning, instead of
        // only appearing after the user switches away from a non-reasoning
        // default (e.g. gpt-4o).
        let prefs = storage::load_preferences();
        let preferred = prefs.as_ref().map(|p| p.model.as_str());
        let stored_reasoning = prefs
            .as_ref()
            .map(|p| p.reasoning.as_str())
            .unwrap_or(PREFERRED_REASONING);
        let (config_options, current_model, reasoning, mode) =
            build_session_config(preferred, stored_reasoning, DEFAULT_MODE, false).await;
        SESSIONS.with(|s| {
            s.borrow_mut().insert(
                id.clone(),
                SessionState {
                    history: Vec::new(),
                    model: current_model,
                    reasoning,
                    mode,
                    allow_all: false,
                    terminal_enabled: false,
                    generation_enabled: false,
                    cwd: req.cwd,
                    cost_aiu: 0.0,
                    used_tokens: 0,
                    report_cost: false,
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
        // Load the persisted session if present, then replay history to the
        // client as `update-session` notifications (per the ACP spec for
        // `session/load`). Missing file = fresh session.
        let default = copilot::default_model();
        let stored = match storage::load(&session_id, &default) {
            Ok(s) => s,
            Err(e) => return Err(err(ErrorCode::InternalError, &format!("load: {e}"))),
        };
        let history = stored
            .as_ref()
            .map(|s| s.history.clone())
            .unwrap_or_default();
        let preferred = stored.as_ref().map(|s| s.model.clone());
        let stored_reasoning = stored
            .as_ref()
            .map(|s| s.reasoning.clone())
            .unwrap_or_default();
        let stored_cost = stored.as_ref().map(|s| s.cost_aiu).unwrap_or(0.0);
        let stored_mode = stored.as_ref().map(|s| s.mode.clone()).unwrap_or_default();
        let stored_allow_all = stored.as_ref().map(|s| s.allow_all).unwrap_or(false);
        let stored_used = stored.as_ref().map(|s| s.used_tokens).unwrap_or(0);
        let stored_report_cost = stored.as_ref().map(|s| s.report_cost).unwrap_or(false);
        let (config_options, current_model, reasoning, mode) = build_session_config(
            preferred.as_deref(),
            &stored_reasoning,
            &stored_mode,
            stored_allow_all,
        )
        .await;
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
                SessionState {
                    history,
                    model: current_model,
                    reasoning,
                    mode,
                    allow_all: stored_allow_all,
                    terminal_enabled: false,
                    generation_enabled: false,
                    cwd: req.cwd,
                    cost_aiu: stored_cost,
                    used_tokens: stored_used,
                    report_cost: stored_report_cost,
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
        // Like `load_session`, but the spec forbids replaying history via
        // `update-session`. Just rehydrate the in-memory map.
        let default = copilot::default_model();
        let stored = match storage::load(&session_id, &default) {
            Ok(s) => s,
            Err(e) => return Err(err(ErrorCode::InternalError, &format!("resume: {e}"))),
        };
        let history = stored
            .as_ref()
            .map(|s| s.history.clone())
            .unwrap_or_default();
        let preferred = stored.as_ref().map(|s| s.model.clone());
        let stored_reasoning = stored
            .as_ref()
            .map(|s| s.reasoning.clone())
            .unwrap_or_default();
        let stored_cost = stored.as_ref().map(|s| s.cost_aiu).unwrap_or(0.0);
        let stored_mode = stored.as_ref().map(|s| s.mode.clone()).unwrap_or_default();
        let stored_allow_all = stored.as_ref().map(|s| s.allow_all).unwrap_or(false);
        let stored_used = stored.as_ref().map(|s| s.used_tokens).unwrap_or(0);
        let stored_report_cost = stored.as_ref().map(|s| s.report_cost).unwrap_or(false);
        let (config_options, current_model, reasoning, mode) = build_session_config(
            preferred.as_deref(),
            &stored_reasoning,
            &stored_mode,
            stored_allow_all,
        )
        .await;
        SESSIONS.with(|s| {
            s.borrow_mut().insert(
                session_id.clone(),
                SessionState {
                    history,
                    model: current_model,
                    reasoning,
                    mode,
                    allow_all: stored_allow_all,
                    terminal_enabled: false,
                    generation_enabled: false,
                    cwd: req.cwd,
                    cost_aiu: stored_cost,
                    used_tokens: stored_used,
                    report_cost: stored_report_cost,
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

    // Append the user turn to this session's history and grab a copy (plus the
    // active model and thinking level) to send to Copilot. New sessions can
    // land here without going through `new-session` (e.g. tests); fall back to
    // defaults. A one-time `system` message is prepended on the first prompt.
    let (
        mut working,
        model,
        reasoning,
        cwd,
        mode,
        terminal_enabled,
        generation_enabled,
        prev_used,
        prev_cost,
        prev_report_cost,
    ) = SESSIONS.with(|s| {
        let mut sessions = s.borrow_mut();
        let entry = sessions
            .entry(session_id.clone())
            .or_insert_with(|| SessionState {
                history: Vec::new(),
                model: copilot::default_model(),
                reasoning: String::new(),
                mode: DEFAULT_MODE.to_string(),
                allow_all: false,
                terminal_enabled: false,
                generation_enabled: false,
                cwd: String::new(),
                cost_aiu: 0.0,
                used_tokens: 0,
                report_cost: false,
            });
        if entry.history.is_empty() {
            let mut prompt = SYSTEM_PROMPT.to_string();
            if !entry.cwd.is_empty() {
                prompt.push_str("\n\nThe user's current project directory is `");
                prompt.push_str(&entry.cwd);
                prompt.push_str("`. Resolve relative file references against it.");
            }
            entry.history.push(Message::system(prompt));
        }
        entry.history.push(Message::user(user_text.clone()));
        (
            entry.history.clone(),
            entry.model.clone(),
            entry.reasoning.clone(),
            entry.cwd.clone(),
            entry.mode.clone(),
            entry.terminal_enabled,
            entry.generation_enabled,
            entry.used_tokens,
            entry.cost_aiu,
            entry.report_cost,
        )
    });

    // Resolve the model's context-window size up front so we can render the
    // context-usage meter from the *start* of the turn — before any tokens
    // stream — instead of only after the turn completes. Emitting it here keeps
    // the editor's UI stable: the meter is present the instant prompting
    // begins (at its last-known value, or `0` for a fresh session) and simply
    // updates in place when the turn finishes. `size` is the only value we
    // can't fabricate, so we skip the meter entirely when it's unknown.
    let context_window = find_model(&cached_models(&model), &model).and_then(|m| m.context_window);
    if let Some(size) = context_window {
        // Mirror the end-of-turn cost logic: only attach cost once this session
        // has ever seen usage-based billing, so the meter's shape stays stable
        // across the turn (and unlimited/usage-based plans still see `0`).
        let cost = prev_report_cost.then(|| UsageCost {
            amount: prev_cost,
            currency: "AIU".to_string(),
        });
        emit_update(
            session_id.clone(),
            SessionUpdate::UsageUpdate(UsageUpdate {
                used: prev_used,
                size,
                cost,
            }),
        )
        .await;
    }

    // Plan mode: steer this turn toward proposing a plan. Injected into the
    // per-turn copy only (right after the base system prompt) so it reflects
    // the *current* mode each turn without polluting persisted history.
    if mode == MODE_PLAN {
        let at = working.iter().take_while(|m| m.role == "system").count();
        working.insert(at, Message::system(PLAN_DIRECTIVE.to_string()));
    }

    // Apply the thinking level as the model's native `reasoning_effort`
    // parameter — but only when the selected model actually advertises that
    // level upstream, so we never send it to a model that would reject it
    // (e.g. gpt-4o returns `invalid_reasoning_effort`).
    let effort = if reasoning.is_empty() {
        None
    } else {
        let models = cached_models(&model);
        let supported = find_model(&models, &model)
            .map(|m| m.reasoning_efforts.contains(&reasoning))
            .unwrap_or(false);
        supported.then(|| reasoning.clone())
    };

    // The ACP host doesn't plumb the editor's fs capabilities through to
    // the session (`initialize` runs on a throwaway instance), so the file
    // tools remain advertised and the editor may reject individual calls.
    //
    // Wassette's component tools come from the host's broker instead: the
    // catalog is read at the top of every round (below), so the model only
    // ever sees what the host currently exposes — nothing when the host
    // exposes nothing.
    let mut broker = component_tools::Broker::default();
    let mut tools = None;

    // Agentic loop: stream a round; if the model asked for tools, surface each
    // one to the client, get permission, run it through the client fs, feed the
    // results back, and go again — until the model answers with no tool calls
    // (or we hit the round cap).
    const MAX_ROUNDS: usize = 8;
    let mut stop_reason = StopReason::EndTurn;
    let mut last_usage: Option<copilot::Usage> = None;
    let mut turn_nano_aiu: u64 = 0;
    let mut saw_copilot_usage = false;
    for round in 0..MAX_ROUNDS {
        // Re-read the broker catalog each round: the host may expose or retire
        // tools between rounds, and it never pushes that to us.
        let (changed, broker_error) = broker.refresh(RESERVED_TOOL_NAMES).await;
        if changed {
            tools = tool_defs(terminal_enabled, generation_enabled, &broker);
        }
        if let Some(message) = broker_error {
            emit_update(
                session_id.clone(),
                SessionUpdate::AgentThoughtChunk(ContentBlock::Text(TextContent {
                    text: format!("(wassette tools: {message})"),
                })),
            )
            .await;
        }

        let sid = session_id.clone();
        let outcome = copilot::chat_round(
            &model,
            effort.as_deref(),
            tools.as_ref(),
            &working,
            move |chunk| {
                let sid = sid.clone();
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
        .map_err(|e| err(ErrorCode::InternalError, &format!("copilot: {e}")))?;

        // Keep the most recent round's token accounting: it reflects the
        // tokens now occupying the model's context window.
        if outcome.usage.is_some() {
            last_usage = outcome.usage;
        }

        // Usage-based (AIU) billing accrues per round; sum it across every
        // round of the turn. `saw_copilot_usage` records that the endpoint
        // reported billing at all, so we can distinguish "billed 0 AIU" (e.g.
        // an included model or an unlimited plan) from "no billing signal".
        if let Some(cu) = outcome.copilot_usage {
            turn_nano_aiu += cu.total_nano_aiu;
            saw_copilot_usage = true;
        }

        if outcome.tool_calls.is_empty() {
            working.push(Message::assistant(outcome.text));
            // A `length` finish means the model was cut off by the token limit.
            stop_reason = if outcome.finish_reason.as_deref() == Some("length") {
                StopReason::MaxTokens
            } else {
                StopReason::EndTurn
            };
            break;
        }

        // Record the assistant's tool-call turn, then run each requested call.
        working.push(Message::assistant_tool_calls(
            outcome.text,
            outcome.tool_calls.clone(),
        ));
        let mut cancelled = false;
        for call in &outcome.tool_calls {
            match execute_tool_call(&session_id, &cwd, &mut broker, call).await {
                ToolExec::Result(text) => working.push(Message::tool_result(&call.id, text)),
                ToolExec::Cancelled => {
                    cancelled = true;
                    break;
                }
            }
        }
        if cancelled {
            stop_reason = StopReason::Cancelled;
            break;
        }
        if round + 1 == MAX_ROUNDS {
            stop_reason = StopReason::MaxTurnRequests;
        }
    }

    // Cost is sourced from the streamed usage-based (AIU) billing accrued
    // above, not from the model entry: GitHub deprecated premium-request
    // multipliers in favor of usage-based billing measured in AI Units (AIU).
    // `context_window` was resolved before the loop for the start-of-turn meter.
    let turn_aiu = turn_nano_aiu as f64 / 1e9;
    let turn_used = last_usage.map(|u| u.total_tokens).unwrap_or(0);

    // Persist the full working history — including tool-call and tool-result
    // messages — so a resumed session keeps the model's tool context. Also
    // fold this turn's AIU cost into the running session total, and record the
    // tokens now occupying the context plus whether we've seen usage-based
    // billing, so the *next* turn (and a resumed session) can render the meter
    // from its first instant instead of flashing `0`.
    let snapshot = SESSIONS.with(|s| {
        let mut sessions = s.borrow_mut();
        sessions.get_mut(&session_id).map(|entry| {
            entry.history = working;
            entry.cost_aiu += turn_aiu;
            if turn_used > 0 {
                entry.used_tokens = turn_used;
            }
            if saw_copilot_usage {
                entry.report_cost = true;
            }
            entry.clone()
        })
    });

    // Refresh the context-usage meter with this turn's real values. We already
    // emitted it at the start of the turn (see above); this updates it in
    // place. `used` is the last round's total tokens (the tokens now occupying
    // the context); `size` is the model's context window; cost is the
    // cumulative AI Units (AIU) drawn. Skip fields we can't source upstream.
    if let Some(session) = &snapshot
        && turn_used > 0
        && let Some(size) = context_window
    {
        // Report cost whenever this session has ever seen usage-based
        // billing — even `0`, so unlimited/usage-based plans still see
        // the meter — matching the start-of-turn emit's shape.
        let cost = session.report_cost.then(|| UsageCost {
            amount: session.cost_aiu,
            currency: "AIU".to_string(),
        });
        emit_update(
            session_id.clone(),
            SessionUpdate::UsageUpdate(UsageUpdate {
                used: turn_used,
                size,
                cost,
            }),
        )
        .await;
    }

    if let Some(session) = snapshot
        && let Err(e) = storage::save(&session_id, &session)
    {
        // Persistence is best-effort: a failed save shouldn't fail the
        // prompt turn. Surface it as a thought chunk.
        emit_update(
            session_id.clone(),
            SessionUpdate::AgentThoughtChunk(ContentBlock::Text(TextContent {
                text: format!("(failed to persist session: {e})"),
            })),
        )
        .await;
    }

    Ok(PromptResponse { stop_reason })
}

// -----------------------------------------------------------------------------
// Tool calling (read/write files), surfaced to the client with permission.
// -----------------------------------------------------------------------------

const TOOL_READ: &str = "read_text_file";
const TOOL_WRITE: &str = "write_text_file";
const TOOL_TERMINAL: &str = "run_terminal_command";

/// Tool names this provider owns. A broker tool may never shadow one of them,
/// so the host's terminal and `build_component` semantics stay intact.
const RESERVED_TOOL_NAMES: &[&str] = &[
    TOOL_READ,
    TOOL_WRITE,
    TOOL_TERMINAL,
    generation::TOOL_BUILD_COMPONENT,
];

/// Max bytes of terminal output retained. The host truncates from the start
/// once this is exceeded, keeping the tail — where errors and summaries
/// usually live.
const TERMINAL_OUTPUT_LIMIT: u64 = 32 * 1024;

/// Build the OpenAI-compatible tool array advertised to the model: a
/// `read_text_file` and a `write_text_file` function routed through the
/// ACP client's filesystem, plus a `run_terminal_command` function routed
/// through the host's terminal when enabled, and a `build_component`
/// function routed through host component generation when the operator
/// permits it.
///
/// Every tool the host's component broker currently exposes to this session is
/// appended as well, so the Wassette components the operator selected become
/// ordinary functions for the model.
///
/// The host notifies this provider when its terminal toggle changes and when
/// generation is available; it remains the final authority on whether a
/// command may run, a component may be built, or a broker tool may run.
fn tool_defs(
    terminal_enabled: bool,
    generation_enabled: bool,
    broker: &component_tools::Broker,
) -> Option<Value> {
    let mut tools = json!([
        {
            "type": "function",
            "function": {
                "name": TOOL_READ,
                "description": "Read a UTF-8 text file from the user's workspace, \
                    including any unsaved editor changes. Use this to inspect files \
                    before answering questions about them.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Absolute path, or a path relative to the project directory." },
                        "line": { "type": "integer", "description": "Optional 1-based line to start reading from." },
                        "limit": { "type": "integer", "description": "Optional maximum number of lines to read." }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": TOOL_WRITE,
                "description": "Create or overwrite a UTF-8 text file in the user's \
                    workspace with the given contents.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Absolute path, or a path relative to the project directory." },
                        "content": { "type": "string", "description": "The full new contents of the file." }
                    },
                    "required": ["path", "content"]
                }
            }
        }
    ]);
    if terminal_enabled {
        tools.as_array_mut().unwrap().push(json!({
            "type": "function",
            "function": {
                "name": TOOL_TERMINAL,
                "description": "Run a shell command in the user's workspace and return \
                    its combined stdout and stderr plus the exit status. The command runs \
                    via `sh -c`, so shell features like pipes, globs, quoting, and `&&` \
                    work. Use this to build, run tests, inspect the environment, or \
                    otherwise operate on the project from the command line.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "The shell command line to execute, e.g. `cargo test` or `ls -la src`." },
                        "cwd": { "type": "string", "description": "Optional working directory (absolute, or relative to the project directory). Defaults to the project directory." }
                    },
                    "required": ["command"]
                }
            }
        }));
    }
    if generation_enabled {
        tools.as_array_mut().unwrap().push(generation::tool_def());
    }
    tools.as_array_mut().unwrap().extend(broker.defs().cloned());
    Some(tools)
}

/// Resolve a possibly-relative path against the session cwd. ACP requires
/// absolute paths; the model often produces relative ones.
fn resolve_path(cwd: &str, path: &str) -> String {
    if path.starts_with('/') || cwd.is_empty() {
        path.to_string()
    } else {
        format!("{}/{}", cwd.trim_end_matches('/'), path)
    }
}

/// A short, char-boundary-safe preview of a tool's textual output for the UI.
fn preview(s: &str) -> String {
    const MAX: usize = 2000;
    if s.len() <= MAX {
        return s.to_string();
    }
    let mut end = MAX;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… ({} more bytes)", &s[..end], s.len() - end)
}

/// The user's decision on a tool-permission prompt.
enum Decision {
    Allow,
    Reject,
    Cancel,
}

/// Outcome of running a single tool call.
enum ToolExec {
    /// Feed this text back to the model as the `tool` result message.
    Result(String),
    /// The user cancelled the permission prompt; abort the whole turn.
    Cancelled,
}

/// Tracks a tool call's display identity so we can emit consistent
/// `tool_call` / `tool_call_update` session updates as it progresses.
struct ToolUi {
    session_id: String,
    id: String,
    title: String,
    kind: ToolKind,
    locations: Vec<ToolCallLocation>,
    raw_input: String,
}

impl ToolUi {
    fn snapshot(
        &self,
        status: ToolCallStatus,
        content: Vec<ToolCallContent>,
        raw_output: Option<String>,
    ) -> ToolCallSnapshot {
        ToolCallSnapshot {
            id: self.id.clone(),
            title: self.title.clone(),
            kind: self.kind,
            status,
            content,
            locations: self.locations.clone(),
            raw_input: Some(self.raw_input.clone()),
            raw_output,
        }
    }

    /// Announce the call to the client (status `pending`).
    async fn announce(&self) {
        emit_update(
            self.session_id.clone(),
            SessionUpdate::ToolCall(self.snapshot(ToolCallStatus::Pending, Vec::new(), None)),
        )
        .await;
    }

    /// Emit a `tool_call_update` carrying the call's new state.
    async fn update(
        &self,
        status: ToolCallStatus,
        content: Vec<ToolCallContent>,
        raw_output: Option<String>,
    ) {
        emit_update(
            self.session_id.clone(),
            SessionUpdate::ToolCallUpdate(self.snapshot(status, content, raw_output)),
        )
        .await;
    }
}

fn remember_permission(session_id: &str, tool_name: &str, allow: bool) {
    PERMS.with(|p| {
        let mut map = p.borrow_mut();
        let st = map.entry(session_id.to_string()).or_default();
        if allow {
            st.always_allow.insert(tool_name.to_string());
        } else {
            st.always_reject.insert(tool_name.to_string());
        }
    });
}

/// Ask the client to authorize a tool call, honoring per-session
/// always-allow / always-reject memory so we don't re-prompt.
async fn request_tool_permission(session_id: &str, tool_name: &str, ui: &ToolUi) -> Decision {
    // Auto tool approval: skip the prompt entirely when the session has
    // "allow all" on, or is in autopilot mode (which implies it).
    let auto = SESSIONS.with(|s| {
        s.borrow()
            .get(session_id)
            .map(|st| auto_approves(&st.mode, st.allow_all))
            .unwrap_or(false)
    });
    if auto {
        return Decision::Allow;
    }

    let remembered = PERMS.with(|p| {
        p.borrow().get(session_id).and_then(|st| {
            if st.always_reject.contains(tool_name) {
                Some(false)
            } else if st.always_allow.contains(tool_name) {
                Some(true)
            } else {
                None
            }
        })
    });
    match remembered {
        Some(true) => return Decision::Allow,
        Some(false) => return Decision::Reject,
        None => {}
    }

    let req = RequestPermissionRequest {
        session_id: session_id.to_string(),
        tool_call: ui.snapshot(ToolCallStatus::Pending, Vec::new(), None),
        options: vec![
            PermissionOption {
                id: "allow-once".to_string(),
                name: "Allow".to_string(),
                kind: PermissionOptionKind::AllowOnce,
            },
            PermissionOption {
                id: "allow-always".to_string(),
                name: "Allow for this session".to_string(),
                kind: PermissionOptionKind::AllowAlways,
            },
            PermissionOption {
                id: "reject-once".to_string(),
                name: "Reject".to_string(),
                kind: PermissionOptionKind::RejectOnce,
            },
            PermissionOption {
                id: "reject-always".to_string(),
                name: "Reject for this session".to_string(),
                kind: PermissionOptionKind::RejectAlways,
            },
        ],
    };

    match client::request_permission(req).await {
        Ok(resp) => match resp.outcome {
            PermissionOutcome::Selected(id) => match id.as_str() {
                "allow-once" => Decision::Allow,
                "allow-always" => {
                    remember_permission(session_id, tool_name, true);
                    Decision::Allow
                }
                "reject-always" => {
                    remember_permission(session_id, tool_name, false);
                    Decision::Reject
                }
                _ => Decision::Reject,
            },
            PermissionOutcome::Cancelled => Decision::Cancel,
        },
        // No permission UI (or the client errored): fail safe by rejecting.
        Err(_) => Decision::Reject,
    }
}

/// Surface, authorize, and run one tool call, returning the text to feed back
/// to the model (or [`ToolExec::Cancelled`] to abort the turn).
async fn execute_tool_call(
    session_id: &str,
    cwd: &str,
    broker: &mut component_tools::Broker,
    call: &copilot::ToolCall,
) -> ToolExec {
    let name = call.function.name.as_str();

    // Wassette component tools are routed straight back through the broker:
    // the host prompts the user, emits this call's `tool_call` updates, and
    // enforces the component's grants, so adding our own prompt or UI here
    // would duplicate both.
    if let Some(tool) = broker.find(name).cloned() {
        let text = broker.call(&tool, &call.function.arguments).await;
        return ToolExec::Result(text);
    }

    let args: Value = serde_json::from_str(&call.function.arguments).unwrap_or(Value::Null);
    let path = resolve_path(cwd, args.get("path").and_then(Value::as_str).unwrap_or(""));
    let command = args
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let (title, kind) = match name {
        TOOL_READ => (format!("Read {path}"), ToolKind::Read),
        TOOL_WRITE => (format!("Write {path}"), ToolKind::Edit),
        TOOL_TERMINAL => (format!("Run `{command}`"), ToolKind::Execute),
        generation::TOOL_BUILD_COMPONENT => (
            format!(
                "Build component `{}`",
                args.get("component_name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
            ),
            ToolKind::Execute,
        ),
        other => (format!("Run {other}"), ToolKind::Other),
    };
    let line = args.get("line").and_then(Value::as_u64).map(|n| n as u32);
    let locations = if path.is_empty() {
        Vec::new()
    } else {
        vec![ToolCallLocation {
            path: path.clone(),
            line,
        }]
    };
    let ui = ToolUi {
        session_id: session_id.to_string(),
        id: call.id.clone(),
        title,
        kind,
        locations,
        raw_input: call.function.arguments.clone(),
    };

    ui.announce().await;

    // The host prompts the editor for every generation phase itself.
    if name == generation::TOOL_BUILD_COMPONENT {
        return build_component(&ui, &args).await;
    }

    match request_tool_permission(session_id, name, &ui).await {
        Decision::Allow => {}
        Decision::Reject => {
            ui.update(
                ToolCallStatus::Failed,
                Vec::new(),
                Some("permission denied".to_string()),
            )
            .await;
            return ToolExec::Result("The user denied permission to run this tool.".to_string());
        }
        Decision::Cancel => {
            ui.update(
                ToolCallStatus::Failed,
                Vec::new(),
                Some("cancelled".to_string()),
            )
            .await;
            return ToolExec::Cancelled;
        }
    }

    ui.update(ToolCallStatus::InProgress, Vec::new(), None)
        .await;

    match name {
        TOOL_READ => {
            if path.is_empty() {
                ui.update(
                    ToolCallStatus::Failed,
                    Vec::new(),
                    Some("missing 'path'".to_string()),
                )
                .await;
                return ToolExec::Result("Error: the 'path' argument is required.".to_string());
            }
            let req = ReadTextFileRequest {
                session_id: session_id.to_string(),
                path: path.clone(),
                line,
                limit: args.get("limit").and_then(Value::as_u64).map(|n| n as u32),
            };
            match client::read_text_file(req).await {
                Ok(resp) => {
                    let block = ContentBlock::Text(TextContent {
                        text: preview(&resp.content),
                    });
                    ui.update(
                        ToolCallStatus::Completed,
                        vec![ToolCallContent::Content(block)],
                        None,
                    )
                    .await;
                    ToolExec::Result(resp.content)
                }
                Err(e) => {
                    let msg = format!("Error reading {path}: {}", e.message);
                    ui.update(ToolCallStatus::Failed, Vec::new(), Some(msg.clone()))
                        .await;
                    ToolExec::Result(msg)
                }
            }
        }
        TOOL_WRITE => {
            if path.is_empty() {
                ui.update(
                    ToolCallStatus::Failed,
                    Vec::new(),
                    Some("missing 'path'".to_string()),
                )
                .await;
                return ToolExec::Result("Error: the 'path' argument is required.".to_string());
            }
            let content = args
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let req = WriteTextFileRequest {
                session_id: session_id.to_string(),
                path: path.clone(),
                content: content.clone(),
            };
            match client::write_text_file(req).await {
                Ok(()) => {
                    let diff = Diff {
                        path: path.clone(),
                        old_text: None,
                        new_text: content.clone(),
                    };
                    ui.update(
                        ToolCallStatus::Completed,
                        vec![ToolCallContent::Diff(diff)],
                        Some(format!("wrote {} bytes", content.len())),
                    )
                    .await;
                    ToolExec::Result(format!("Wrote {} bytes to {path}.", content.len()))
                }
                Err(e) => {
                    let msg = format!("Error writing {path}: {}", e.message);
                    ui.update(ToolCallStatus::Failed, Vec::new(), Some(msg.clone()))
                        .await;
                    ToolExec::Result(msg)
                }
            }
        }
        TOOL_TERMINAL => {
            if command.is_empty() {
                ui.update(
                    ToolCallStatus::Failed,
                    Vec::new(),
                    Some("missing 'command'".to_string()),
                )
                .await;
                return ToolExec::Result("Error: the 'command' argument is required.".to_string());
            }
            // Optional cwd override; default to the session cwd. The host does
            // a direct exec, so route the command through `sh -c` to give the
            // model normal shell semantics (pipes, globs, quoting, `&&`, …).
            let run_cwd = match args.get("cwd").and_then(Value::as_str) {
                Some(c) if !c.is_empty() => resolve_path(cwd, c),
                _ => cwd.to_string(),
            };
            let req = CreateTerminalRequest {
                session_id: session_id.to_string(),
                command: "sh".to_string(),
                args: vec!["-c".to_string(), command.clone()],
                env: Vec::new(),
                cwd: if run_cwd.is_empty() {
                    None
                } else {
                    Some(run_cwd)
                },
                output_byte_limit: Some(TERMINAL_OUTPUT_LIMIT),
            };
            let terminal = client::Terminal::new(&req);
            // Draining `output()` awaits process exit and the final flush;
            // `wait_for_exit()` then resolves immediately (or reports that the
            // host-owned `terminal` config option is disabled).
            let bytes = terminal.output().await.collect().await;
            let output = String::from_utf8_lossy(&bytes).into_owned();
            match terminal.wait_for_exit().await {
                Ok(status) => {
                    let exit_desc = match (status.exit_code, &status.signal) {
                        (Some(code), _) => format!("exit code {code}"),
                        (None, Some(sig)) => format!("terminated by signal {sig}"),
                        (None, None) => "exited".to_string(),
                    };
                    let result = if output.is_empty() {
                        format!("Command completed ({exit_desc}) with no output.")
                    } else {
                        format!("{output}\n[{exit_desc}]")
                    };
                    let block = ContentBlock::Text(TextContent {
                        text: preview(&result),
                    });
                    ui.update(
                        ToolCallStatus::Completed,
                        vec![ToolCallContent::Content(block)],
                        Some(exit_desc),
                    )
                    .await;
                    ToolExec::Result(result)
                }
                Err(e) => {
                    // The host refuses to spawn when the `terminal` option is
                    // off; relay its message so the model knows terminal tools
                    // must be enabled rather than treating this as a hard error.
                    let msg = format!("Terminal command failed: {}", e.message);
                    ui.update(ToolCallStatus::Failed, Vec::new(), Some(msg.clone()))
                        .await;
                    ToolExec::Result(msg)
                }
            }
        }
        other => {
            let msg = format!("Error: unknown tool '{other}'.");
            ui.update(ToolCallStatus::Failed, Vec::new(), Some(msg.clone()))
                .await;
            ToolExec::Result(msg)
        }
    }
}

/// Run `build_component` through the host's generation import. The host
/// authorizes the request (operator profile plus per-phase editor approval)
/// and errors when generation is disabled.
async fn build_component(ui: &ToolUi, args: &Value) -> ToolExec {
    let request = match generation::request_json(args) {
        Ok(request) => request,
        Err(message) => {
            let message = format!("Error: {message}.");
            ui.update(ToolCallStatus::Failed, Vec::new(), Some(message.clone()))
                .await;
            return ToolExec::Result(message);
        }
    };
    ui.update(ToolCallStatus::InProgress, Vec::new(), None)
        .await;
    let result = crate::bindings::wassette::component_generation::builder::generate(&request);
    let text = generation::describe(&result);
    let status = if result.is_ok() {
        ToolCallStatus::Completed
    } else {
        ToolCallStatus::Failed
    };
    let block = ContentBlock::Text(TextContent {
        text: preview(&text),
    });
    ui.update(status, vec![ToolCallContent::Content(block)], None)
        .await;
    ToolExec::Result(text)
}

bindings::export!(Agent with_types_in bindings);

#[cfg(test)]
mod tests {
    use super::*;

    fn names(tools: &Value) -> Vec<String> {
        tools
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap().to_string())
            .collect()
    }

    fn weather() -> crate::bindings::wassette::component_tools::tools::ToolDescriptor {
        crate::bindings::wassette::component_tools::tools::ToolDescriptor {
            handle: "h1".to_string(),
            name: "get-weather".to_string(),
            component_id: "local:weather".to_string(),
            export_name: "get-weather".to_string(),
            description: Some("forecast".to_string()),
            input_schema: r#"{"type":"object","properties":{}}"#.to_string(),
            output_schema: None,
        }
    }

    #[test]
    fn broker_tools_are_appended_without_disturbing_built_ins() {
        let empty = component_tools::Broker::default();
        assert_eq!(
            names(&tool_defs(false, false, &empty).unwrap()),
            vec![TOOL_READ, TOOL_WRITE]
        );

        let broker = component_tools::Broker::from_tools(
            component_tools::build(vec![weather()], RESERVED_TOOL_NAMES).unwrap(),
        );
        assert_eq!(
            names(&tool_defs(true, true, &broker).unwrap()),
            vec![
                TOOL_READ,
                TOOL_WRITE,
                TOOL_TERMINAL,
                generation::TOOL_BUILD_COMPONENT,
                "get-weather",
            ]
        );
    }

    #[test]
    fn reserved_names_cover_every_built_in_tool() {
        let all = tool_defs(true, true, &component_tools::Broker::default()).unwrap();
        for name in names(&all) {
            assert!(
                RESERVED_TOOL_NAMES.contains(&name.as_str()),
                "{name} is advertised but not reserved"
            );
        }
    }
}
