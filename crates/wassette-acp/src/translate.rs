//! Type translation between the wasmtime-generated WIT types
//! (`yosh::acp` interfaces) and the `agent_client_protocol::schema` types.
//!
//! Only covers the variants the MVP exercises (text content, end-turn,
//! agent-message-chunk, etc.). Anything we can't translate yields an error
//! that surfaces back to the editor as a JSON-RPC error.

use std::path::PathBuf;

use agent_client_protocol::schema::v1 as schema;
use agent_client_protocol::{Error as AcpError, ErrorCode as AcpErrorCode};
use tracing::debug;

use crate::yosh::acp::content::{ContentBlock, TextContent};
use crate::yosh::acp::errors::{Error, ErrorCode};
use crate::yosh::acp::filesystem::{
    ReadTextFileRequest, ReadTextFileResponse, WriteTextFileRequest,
};
use crate::yosh::acp::init::{
    AuthenticateRequest, ClientCapabilities, FsCapabilities, ImplementationInfo, InitializeRequest,
    InitializeResponse,
};
use crate::yosh::acp::prompts::{PromptResponse, SessionUpdate, StopReason};
use crate::yosh::acp::sessions::{
    ComponentSource, EnvVar, HttpHeader, LoadSessionRequest, LoadSessionResponse, McpServer,
    McpServerHttp, McpServerSse, McpServerStdio, NewSessionRequest, NewSessionResponse,
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectGroup,
    SessionConfigSelectOption, SessionConfigSelectOptions, SessionId, SessionMode, SessionModeId,
    SessionModeState,
};
use crate::yosh::acp::tools::{
    PermissionOption, PermissionOptionKind, PermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, ToolCallContent, ToolCallSnapshot, ToolCallStatus, ToolKind,
};

// -----------------------------------------------------------------------------
// JSON synthesis helper
// -----------------------------------------------------------------------------

/// Some `agent_client_protocol::schema` response types are `non_exhaustive`.
/// We construct them via JSON to avoid depending on private fields. If the
/// schema shape ever drifts, surface the failure as an ACP `internal-error`
/// rather than panicking.
fn synth<T: serde::de::DeserializeOwned>(
    context: &'static str,
    json: serde_json::Value,
) -> Result<T, AcpError> {
    serde_json::from_value(json).map_err(|e| {
        let mut out = AcpError::internal_error();
        out.message = format!("{context}: schema drift: {e}");
        out
    })
}

// -----------------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------------

/// Convert a WIT-side ACP `Error` (returned by the wasm guest) into the
/// JSON-RPC `Error` shape the `agent_client_protocol` crate expects.
pub fn wit_error_to_acp(e: Error) -> AcpError {
    let mut out = match e.code {
        ErrorCode::ParseError => AcpError::parse_error(),
        ErrorCode::InvalidRequest => AcpError::invalid_request(),
        ErrorCode::MethodNotFound => AcpError::method_not_found(),
        ErrorCode::InvalidParams => AcpError::invalid_params(),
        ErrorCode::InternalError => AcpError::internal_error(),
        ErrorCode::AuthRequired => AcpError::auth_required(),
        ErrorCode::ResourceNotFound => AcpError::resource_not_found(None),
        ErrorCode::Other(n) => {
            let mut e = AcpError::internal_error();
            e.code = AcpErrorCode::Other(n);
            e
        }
    };
    out.message = e.message;
    out
}

/// Wrap a wasmtime trap as an internal JSON-RPC error.
pub fn trap_to_acp(context: &str, e: wasmtime::Error) -> AcpError {
    let mut out = AcpError::internal_error();
    out.message = format!("{context}: {e:#}");
    out
}

/// Wrap an anyhow error (e.g. from instantiation) as an internal JSON-RPC
/// error.
pub fn anyhow_to_acp(context: &str, e: anyhow::Error) -> AcpError {
    let mut out = AcpError::internal_error();
    out.message = format!("{context}: {e:#}");
    out
}

/// Build a WIT `Error` with `internal-error` code and the given message.
/// Used for host-side transport failures bubbling back to the wasm guest.
pub fn internal_error(message: &str) -> Error {
    Error {
        code: ErrorCode::InternalError,
        message: message.to_string(),
    }
}

// -----------------------------------------------------------------------------
// Protocol version
// -----------------------------------------------------------------------------

pub fn pv_to_u32(pv: &agent_client_protocol::schema::ProtocolVersion) -> u32 {
    pv.to_string().parse::<u32>().unwrap_or(0)
}

pub fn pv_from_u32(n: u32) -> agent_client_protocol::schema::ProtocolVersion {
    agent_client_protocol::schema::ProtocolVersion::from(n as u16)
}

// -----------------------------------------------------------------------------
// Initialize
// -----------------------------------------------------------------------------

pub fn init_request_schema_to_wit(req: schema::InitializeRequest) -> InitializeRequest {
    let cc = &req.client_capabilities;
    InitializeRequest {
        protocol_version: pv_to_u32(&req.protocol_version),
        client_capabilities: ClientCapabilities {
            fs: FsCapabilities {
                read_text_file: cc.fs.read_text_file,
                write_text_file: cc.fs.write_text_file,
            },
            terminal: cc.terminal,
        },
        client_info: req.client_info.map(|i| ImplementationInfo {
            name: i.name,
            title: i.title,
            version: i.version,
        }),
    }
}

pub fn init_response_wit_to_schema(resp: InitializeResponse) -> schema::InitializeResponse {
    let agent_caps = schema::AgentCapabilities::new()
        .load_session(resp.agent_capabilities.load_session)
        .prompt_capabilities(
            schema::PromptCapabilities::new()
                .image(resp.agent_capabilities.prompt_capabilities.image)
                .audio(resp.agent_capabilities.prompt_capabilities.audio)
                .embedded_context(resp.agent_capabilities.prompt_capabilities.embedded_context),
        )
        .mcp_capabilities(
            schema::McpCapabilities::new()
                .http(resp.agent_capabilities.mcp_capabilities.http)
                .sse(resp.agent_capabilities.mcp_capabilities.sse),
        );

    let mut out = schema::InitializeResponse::new(pv_from_u32(resp.protocol_version))
        .agent_capabilities(agent_caps);
    if let Some(info) = resp.agent_info {
        let impl_ = schema::Implementation::new(info.name, info.version).title(info.title);
        out = out.agent_info(impl_);
    }
    out
}

// -----------------------------------------------------------------------------
// Authenticate
// -----------------------------------------------------------------------------

pub fn authenticate_request_schema_to_wit(req: schema::AuthenticateRequest) -> AuthenticateRequest {
    AuthenticateRequest {
        method_id: req.method_id.to_string(),
    }
}

// -----------------------------------------------------------------------------
// New session
// -----------------------------------------------------------------------------

pub fn new_session_request_schema_to_wit(req: schema::NewSessionRequest) -> NewSessionRequest {
    NewSessionRequest {
        cwd: path_to_string(&req.cwd),
        mcp_servers: req.mcp_servers.into_iter().map(mcp_server_to_wit).collect(),
    }
}

pub fn new_session_response_wit_to_schema(
    resp: NewSessionResponse,
    component_id: &str,
    terminal: Option<bool>,
) -> Result<schema::NewSessionResponse, AcpError> {
    // schema::NewSessionResponse is `non_exhaustive`. Roundtrip via JSON to
    // construct it without depending on the (unstable) field set.
    let mut json = serde_json::json!({ "sessionId": resp.session_id });
    // `config_options` (the unified selector mechanism) is XOR with the legacy
    // `modes` field client-side: when present it fully replaces modes, so skip
    // the host-injected default mode to avoid advertising a phantom selector.
    if let Some(config_options) = resp.config_options {
        json["configOptions"] = config_options_json(config_options, terminal);
    } else if let Some(modes) = ensure_host_default_mode(resp.modes) {
        json["modes"] = session_mode_state_to_json(modes, component_id);
    }
    synth("new-session response", json)
}

/// Build a `session/new` response from a host-merged config-option set
/// (the multi-provider path). The merged options already carry the
/// cross-provider model selector, so no legacy `modes` fallback applies.
pub fn new_session_response_with_config_options(
    session_id: &str,
    config_options: Vec<SessionConfigOption>,
    terminal: Option<bool>,
) -> Result<schema::NewSessionResponse, AcpError> {
    let json = serde_json::json!({
        "sessionId": session_id,
        "configOptions": config_options_json(config_options, terminal),
    });
    synth("new-session response", json)
}

/// Build a `session/load` response from a host-merged config-option set
/// (the multi-provider path).
pub fn load_session_response_with_config_options(
    config_options: Vec<SessionConfigOption>,
    terminal: Option<bool>,
) -> Result<schema::LoadSessionResponse, AcpError> {
    let json = serde_json::json!({
        "configOptions": config_options_json(config_options, terminal),
    });
    synth("load-session response", json)
}

// -----------------------------------------------------------------------------
// Load session
// -----------------------------------------------------------------------------

pub fn load_session_request_schema_to_wit(req: schema::LoadSessionRequest) -> LoadSessionRequest {
    LoadSessionRequest {
        session_id: req.session_id.0.to_string(),
        cwd: path_to_string(&req.cwd),
        mcp_servers: req.mcp_servers.into_iter().map(mcp_server_to_wit).collect(),
    }
}

/// Convert a WIT `LoadSessionResponse` into the schema shape, propagating
/// any `modes` the agent advertised so the editor can render its picker
/// for resumed sessions.
pub fn load_session_response_wit_to_schema(
    resp: LoadSessionResponse,
    component_id: &str,
    terminal: Option<bool>,
) -> Result<schema::LoadSessionResponse, AcpError> {
    let mut json = serde_json::json!({});
    if let Some(config_options) = resp.config_options {
        json["configOptions"] = config_options_json(config_options, terminal);
    } else if let Some(modes) = ensure_host_default_mode(resp.modes) {
        json["modes"] = session_mode_state_to_json(modes, component_id);
    }
    synth("load-session response", json)
}

// -----------------------------------------------------------------------------
// Session modes
// -----------------------------------------------------------------------------

/// Empty `SetSessionModeResponse`. Constructed via JSON because the schema
/// type is `non_exhaustive`.
pub fn empty_set_session_mode_response() -> Result<schema::SetSessionModeResponse, AcpError> {
    synth("set-session-mode response", serde_json::json!({}))
}

// -----------------------------------------------------------------------------
// Session modes
// -----------------------------------------------------------------------------

fn session_mode_state_to_json(state: SessionModeState, component_id: &str) -> serde_json::Value {
    let SessionModeState {
        current_mode_id,
        available_modes,
    } = state;
    serde_json::json!({
        "currentModeId": current_mode_id,
        "availableModes": available_modes
            .into_iter()
            .map(|m| session_mode_to_json(m, component_id))
            .collect::<Vec<_>>(),
    })
}

/// Stable id for the host-injected "default" mode. Always present in
/// every session so the user has something to switch back to when a
/// layer-injected mode (e.g. `plan`) needs to be toggled off.
pub const HOST_DEFAULT_MODE_ID: &str = "default";

/// Ensure the outbound mode state contains a host-owned `default`
/// mode and uses it as the current mode if the chain didn't pick
/// one. Layers (e.g. `plan-layer`) deliberately don't synthesize a
/// "not-me" mode; this is where it comes from instead.
fn ensure_host_default_mode(modes: Option<SessionModeState>) -> Option<SessionModeState> {
    let mut state = modes.unwrap_or(SessionModeState {
        current_mode_id: HOST_DEFAULT_MODE_ID.to_string(),
        available_modes: Vec::new(),
    });
    let has_default = state
        .available_modes
        .iter()
        .any(|m| m.id == HOST_DEFAULT_MODE_ID);
    if !has_default {
        state.available_modes.insert(
            0,
            SessionMode {
                id: HOST_DEFAULT_MODE_ID.to_string(),
                name: "Default".to_string(),
                description: Some(
                    "Normal execution. Selectable to disengage any layer-injected mode such as \
                     plan."
                        .to_string(),
                ),
                provided_by: ComponentSource {
                    component_id: "local:host".to_string(),
                },
            },
        );
    }
    // If the chain picked a current mode that nothing advertises,
    // fall back to the host default. This happens e.g. when the
    // plan-layer was the only contributor and used the plan id as
    // its placeholder current mode.
    let current_exists = state
        .available_modes
        .iter()
        .any(|m| m.id == state.current_mode_id);
    if !current_exists {
        state.current_mode_id = HOST_DEFAULT_MODE_ID.to_string();
    }
    Some(state)
}

fn session_mode_to_json(mode: SessionMode, _component_id: &str) -> serde_json::Value {
    let SessionMode {
        id,
        name,
        description,
        provided_by,
    } = mode;
    // Upstream ACP's `SessionMode` doesn't carry provenance, so bake
    // the contributing component's id into the display name \u2014 except
    // for the host's own synthetic modes, which would otherwise read
    // as e.g. "Default (local:host)" and just look like noise.
    let display = if provided_by.component_id == "local:host" {
        name
    } else {
        format!("{name} ({})", provided_by.component_id)
    };
    let mut entry = serde_json::json!({
        "id": id,
        "name": display,
    });
    if let Some(d) = description {
        entry["description"] = serde_json::Value::String(d);
    }
    entry
}

/// Suppress the unused-import lint for `SessionModeId` — it's part of the
/// public WIT surface but not directly referenced in this module.
const _: fn(SessionModeId) = |_| {};

// -----------------------------------------------------------------------------
// Session config options (the unified selector mechanism: model / mode /
// thought-level, rendered client-side from a single `configOptions` list)
// -----------------------------------------------------------------------------

/// Build the `{ configOptions: [...] }` response to `session/set_config_option`.
pub fn set_config_option_response(
    options: Vec<SessionConfigOption>,
    terminal: Option<bool>,
) -> Result<schema::SetSessionConfigOptionResponse, AcpError> {
    let json = serde_json::json!({ "configOptions": config_options_json(options, terminal) });
    synth("set-config-option response", json)
}

fn config_options_json(
    options: Vec<SessionConfigOption>,
    terminal: Option<bool>,
) -> serde_json::Value {
    let mut arr: Vec<serde_json::Value> = options
        .into_iter()
        .map(session_config_option_to_json)
        .collect();
    // Append the host-owned `terminal` boolean toggle when the client
    // opted into boolean config options (`terminal` is `Some`). Per the
    // boolean-config-option RFD, agents must not emit `type: "boolean"`
    // options to clients that didn't advertise support.
    if let Some(current) = terminal {
        arr.push(terminal_config_option_json(current));
    }
    serde_json::Value::Array(arr)
}

/// The host-owned `terminal` boolean config option, serialized per the
/// ACP boolean-config-option RFD (`type: "boolean"` with a `currentValue`
/// bool, no `options` list or `category`).
fn terminal_config_option_json(current: bool) -> serde_json::Value {
    serde_json::json!({
        "id": crate::group::TERMINAL_CONFIG_ID,
        "name": "Terminal tools",
        "description": "Allow the agent to run terminal (CLI) commands on this machine.",
        "type": "boolean",
        "currentValue": current,
    })
}

fn session_config_option_to_json(option: SessionConfigOption) -> serde_json::Value {
    let SessionConfigOption {
        id,
        name,
        description,
        category,
        current_value,
        options,
        // Upstream ACP's `SessionConfigOption` carries no provenance field;
        // these are top-level category selectors, so provenance would just be
        // display noise. Drop it, mirroring how the host handles its own modes.
        provided_by: _,
    } = option;
    // The schema flattens `kind` via a `type` discriminator; we only emit
    // `select` options. `SessionConfigSelectOptions` is untagged: an
    // ungrouped list serializes as a flat array of option objects, a
    // grouped list as an array of `{group, name, options}` objects.
    let mut entry = serde_json::json!({
        "id": id,
        "name": name,
        "type": "select",
        "currentValue": current_value,
        "options": session_config_select_options_to_json(options),
    });
    if let Some(d) = description {
        entry["description"] = serde_json::Value::String(d);
    }
    if let Some(cat) = category {
        entry["category"] = session_config_category_to_json(cat);
    }
    entry
}

fn session_config_category_to_json(category: SessionConfigOptionCategory) -> serde_json::Value {
    match category {
        SessionConfigOptionCategory::Mode => serde_json::Value::String("mode".to_string()),
        SessionConfigOptionCategory::Model => serde_json::Value::String("model".to_string()),
        SessionConfigOptionCategory::ThoughtLevel => {
            serde_json::Value::String("thought_level".to_string())
        }
        // Free-form categories serialize as their raw string (schema's
        // `#[serde(untagged)] Other(String)`).
        SessionConfigOptionCategory::Other(s) => serde_json::Value::String(s),
    }
}

fn session_config_select_option_to_json(option: SessionConfigSelectOption) -> serde_json::Value {
    let SessionConfigSelectOption {
        value,
        name,
        description,
    } = option;
    let mut entry = serde_json::json!({
        "value": value,
        "name": name,
    });
    if let Some(d) = description {
        entry["description"] = serde_json::Value::String(d);
    }
    entry
}

/// Serialize the (untagged) select-options variant: ungrouped → a flat
/// array of option objects; grouped → an array of group objects.
fn session_config_select_options_to_json(
    options: SessionConfigSelectOptions,
) -> serde_json::Value {
    match options {
        SessionConfigSelectOptions::Ungrouped(list) => serde_json::Value::Array(
            list.into_iter()
                .map(session_config_select_option_to_json)
                .collect(),
        ),
        SessionConfigSelectOptions::Grouped(groups) => serde_json::Value::Array(
            groups
                .into_iter()
                .map(session_config_select_group_to_json)
                .collect(),
        ),
    }
}

fn session_config_select_group_to_json(group: SessionConfigSelectGroup) -> serde_json::Value {
    let SessionConfigSelectGroup {
        group,
        name,
        options,
    } = group;
    serde_json::json!({
        "group": group,
        "name": name,
        "options": options
            .into_iter()
            .map(session_config_select_option_to_json)
            .collect::<Vec<_>>(),
    })
}

// -----------------------------------------------------------------------------
// Prompt
// -----------------------------------------------------------------------------

pub fn prompt_response_wit_to_schema(
    resp: PromptResponse,
) -> Result<schema::PromptResponse, AcpError> {
    let stop_reason = match resp.stop_reason {
        StopReason::EndTurn => "end_turn",
        StopReason::MaxTokens => "max_tokens",
        StopReason::MaxTurnRequests => "max_turn_requests",
        StopReason::Refusal => "refusal",
        StopReason::Cancelled => "cancelled",
    };
    synth(
        "prompt response",
        serde_json::json!({ "stopReason": stop_reason }),
    )
}

/// Build a `PromptResponse` with `stop_reason: cancelled`. The protocol
/// requires the agent to return this when a `session/cancel` notification
/// arrives during a prompt turn.
pub fn synthesised_cancelled_response() -> Result<schema::PromptResponse, AcpError> {
    prompt_response_wit_to_schema(PromptResponse {
        stop_reason: StopReason::Cancelled,
    })
}

/// Empty `AuthenticateResponse`. Constructed via JSON because the schema
/// type is `non_exhaustive`.
pub fn empty_authenticate_response() -> Result<schema::AuthenticateResponse, AcpError> {
    synth("authenticate response", serde_json::json!({}))
}

/// JSON shape advertised for the host-side `/install` slash command.
/// Centralised so both [`session_update_wit_to_schema`] (injection into
/// chain-emitted updates) and [`synthetic_install_command_update`]
/// (host-emitted update for chains that never advertise commands)
/// agree on description and hint.
fn install_command_json() -> serde_json::Value {
    serde_json::json!({
        "name": "install",
        "description": "Install a wasm component plugin by WIT name (e.g. `wasi:clocks@0.2.0`).",
        "input": { "hint": "<namespace>:<package>[@version]" },
    })
}

/// Synthetic `available_commands_update` notification advertising
/// just the host-side `/install` command. Sent on every `session/new`
/// and `session/load` so `/install` shows up even when no layer ever
/// emits an `available-commands-update`.
pub fn synthetic_install_command_update(session_id: &str) -> Option<schema::SessionNotification> {
    let upd: schema::SessionUpdate = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "available_commands_update",
        "availableCommands": [install_command_json()],
    }))
    .ok()?;
    let sid = schema::SessionId::from(session_id.to_string());
    Some(schema::SessionNotification::new(sid, upd))
}

/// `PromptResponse` returned by the host-side `/install` command:
/// always `end_turn` regardless of success (the outcome is reported as
/// a streamed agent chunk).
pub fn install_command_response() -> Result<schema::PromptResponse, AcpError> {
    prompt_response_wit_to_schema(PromptResponse {
        stop_reason: StopReason::EndTurn,
    })
}

// -----------------------------------------------------------------------------
// `/install` progress as an ACP tool call
// -----------------------------------------------------------------------------
//
// Modeled as a single tool call (kind=fetch) with status transitions
// and content updates so editors render it as a progress card with a
// status pill, instead of appending plain lines to the chat transcript.

/// Initial `tool_call` notification: status=in-progress, kind=fetch.
/// `text` becomes the first content block (subsequent
/// [`install_tool_call_update`] calls *replace* the content list).
pub fn install_tool_call_start(
    session_id: &str,
    tool_call_id: &str,
    title: &str,
    text: &str,
) -> Option<schema::SessionNotification> {
    let json = serde_json::json!({
        "sessionUpdate": "tool_call",
        "toolCallId": tool_call_id,
        "title": title,
        "kind": "fetch",
        "status": "in_progress",
        "content": [text_content_item(text)],
    });
    build_session_notification(session_id, json)
}

/// `tool_call_update` notification. `status` is `"in_progress"`,
/// `"completed"`, or `"failed"`. `text`, when set, replaces the
/// content list with a single text block.
pub fn install_tool_call_update(
    session_id: &str,
    tool_call_id: &str,
    status: &str,
    text: Option<&str>,
) -> Option<schema::SessionNotification> {
    let mut json = serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": tool_call_id,
        "status": status,
    });
    if let Some(t) = text {
        json["content"] = serde_json::json!([text_content_item(t)]);
    }
    build_session_notification(session_id, json)
}

fn text_content_item(text: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "content",
        "content": { "type": "text", "text": text },
    })
}

fn build_session_notification(
    session_id: &str,
    update_json: serde_json::Value,
) -> Option<schema::SessionNotification> {
    let upd: schema::SessionUpdate = serde_json::from_value(update_json).ok()?;
    Some(schema::SessionNotification::new(
        schema::SessionId::from(session_id.to_string()),
        upd,
    ))
}

// -----------------------------------------------------------------------------
// Session updates (wasm → editor)
// -----------------------------------------------------------------------------

/// Convert a WIT-side `update-session` call into a schema
/// `SessionNotification` to forward to the editor. Returns `None` for variants
/// we don't yet support.
pub fn session_update_wit_to_schema(
    session_id: SessionId,
    update: SessionUpdate,
) -> Option<schema::SessionNotification> {
    let block = match update {
        SessionUpdate::AgentMessageChunk(b) => Some(("agent", b)),
        SessionUpdate::AgentThoughtChunk(b) => Some(("thought", b)),
        SessionUpdate::UserMessageChunk(b) => Some(("user", b)),
        SessionUpdate::ToolCall(call) => {
            let upd = tool_call_to_schema_update(&session_id, call, false)?;
            return Some(schema::SessionNotification::new(
                schema::SessionId::from(session_id),
                upd,
            ));
        }
        SessionUpdate::ToolCallUpdate(snapshot) => {
            // Both `ToolCall` and `ToolCallUpdate` variants carry a full
            // [`ToolCallSnapshot`]; the only wire difference is the
            // `sessionUpdate` discriminator, so reuse the same translation.
            let upd = tool_call_to_schema_update(&session_id, snapshot, true)?;
            return Some(schema::SessionNotification::new(
                schema::SessionId::from(session_id),
                upd,
            ));
        }
        SessionUpdate::Plan(_) => {
            debug!(session = %session_id, "dropped session update: plan (not yet wired)");
            None
        }
        SessionUpdate::CurrentModeUpdate(mode_id) => {
            // The guest reports a mode switch (e.g. user picked a
            // different model). Forward as a real `current-mode-update`
            // notification so the editor's picker can reflect it.
            let upd: schema::SessionUpdate = serde_json::from_value(serde_json::json!({
                "sessionUpdate": "current_mode_update",
                "currentModeId": mode_id,
            }))
            .ok()?;
            return Some(schema::SessionNotification::new(
                schema::SessionId::from(session_id),
                upd,
            ));
        }
        SessionUpdate::SessionInfoUpdate(_) => {
            debug!(session = %session_id, "dropped session update: session-info-update (not yet wired)");
            None
        }
        SessionUpdate::UsageUpdate(usage) => {
            // Context-window usage: forward as ACP's stable `usage_update`
            // so the editor can render a "context %" indicator
            // (context % = used / size).
            let cost = usage
                .cost
                .map(|c| schema::Cost::new(c.amount, c.currency));
            let upd = schema::SessionUpdate::UsageUpdate(
                schema::UsageUpdate::new(usage.used, usage.size).cost(cost),
            );
            return Some(schema::SessionNotification::new(
                schema::SessionId::from(session_id),
                upd,
            ));
        }
        SessionUpdate::AvailableCommandsUpdate(cmds) => {
            let mut cmds_json: Vec<serde_json::Value> = cmds
                .into_iter()
                .map(|c| {
                    let mut v = serde_json::json!({
                        "name": c.name,
                        "description": c.description,
                    });
                    if let Some(input) = c.input {
                        v["input"] = serde_json::json!({ "hint": input.hint });
                    }
                    v
                })
                .collect();
            // Inject the host-side `/install` command if the chain
            // didn't already advertise one. The host intercepts
            // `/install <wit-name>` in `handle_prompt` before
            // forwarding into the chain.
            if !cmds_json
                .iter()
                .any(|c| c.get("name").and_then(|n| n.as_str()) == Some("install"))
            {
                cmds_json.push(install_command_json());
            }
            let json = serde_json::json!({
                "sessionUpdate": "available_commands_update",
                "availableCommands": cmds_json,
            });
            tracing::info!(session = %session_id, payload = %json, "available-commands-update outbound");
            let upd: schema::SessionUpdate = match serde_json::from_value(json) {
                Ok(u) => u,
                Err(e) => {
                    tracing::warn!(session = %session_id, error = %e, "failed to deserialize available-commands-update");
                    return None;
                }
            };
            return Some(schema::SessionNotification::new(
                schema::SessionId::from(session_id),
                upd,
            ));
        }
    }?;
    let (kind, b) = block;
    let schema_block = content_block_wit_to_schema(&session_id, b)?;
    let chunk = schema::ContentChunk::new(schema_block);
    let upd = match kind {
        "agent" => schema::SessionUpdate::AgentMessageChunk(chunk),
        "thought" => schema::SessionUpdate::AgentThoughtChunk(chunk),
        "user" => schema::SessionUpdate::UserMessageChunk(chunk),
        _ => unreachable!(),
    };
    Some(schema::SessionNotification::new(
        schema::SessionId::from(session_id),
        upd,
    ))
}

// -----------------------------------------------------------------------------
// Content blocks (text-only for MVP; non-text variants are dropped)
// -----------------------------------------------------------------------------

pub fn content_block_schema_to_wit(block: schema::ContentBlock) -> Option<ContentBlock> {
    Some(match block {
        schema::ContentBlock::Text(t) => ContentBlock::Text(TextContent { text: t.text }),
        // Non-text variants ignored for MVP; the wasm guest only handles text.
        other => {
            debug!(
                variant = ?std::mem::discriminant(&other),
                "dropped inbound content block: non-text variant not yet supported"
            );
            return None;
        }
    })
}

fn content_block_wit_to_schema(
    session_id: &str,
    block: ContentBlock,
) -> Option<schema::ContentBlock> {
    Some(match block {
        ContentBlock::Text(t) => schema::ContentBlock::Text(schema::TextContent::new(t.text)),
        _ => {
            debug!(
                session = %session_id,
                "dropped outbound content block: non-text variant not yet supported"
            );
            return None;
        }
    })
}

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

fn path_to_string(p: &std::path::Path) -> String {
    p.to_string_lossy().into_owned()
}

fn mcp_server_to_wit(s: schema::McpServer) -> McpServer {
    match s {
        schema::McpServer::Stdio(server) => McpServer::Stdio(McpServerStdio {
            name: server.name,
            command: path_to_string(&PathBuf::from(server.command)),
            args: server.args,
            env: server
                .env
                .into_iter()
                .map(|e| EnvVar {
                    name: e.name,
                    value: e.value,
                })
                .collect(),
        }),
        schema::McpServer::Http(server) => McpServer::Http(McpServerHttp {
            name: server.name,
            url: server.url.to_string(),
            headers: server
                .headers
                .into_iter()
                .map(|h| HttpHeader {
                    name: h.name,
                    value: h.value,
                })
                .collect(),
        }),
        schema::McpServer::Sse(server) => McpServer::Sse(McpServerSse {
            name: server.name,
            url: server.url.to_string(),
            headers: server
                .headers
                .into_iter()
                .map(|h| HttpHeader {
                    name: h.name,
                    value: h.value,
                })
                .collect(),
        }),
        // Schema enum is `non_exhaustive`; future variants are dropped to a
        // stub stdio entry so we don't crash on protocol additions.
        other => {
            debug!(
                variant = ?std::mem::discriminant(&other),
                "unknown McpServer variant: substituting empty stdio stub"
            );
            McpServer::Stdio(McpServerStdio {
                name: String::from("<unknown>"),
                command: String::new(),
                args: Vec::new(),
                env: Vec::new(),
            })
        }
    }
}

// -----------------------------------------------------------------------------
// File system (wasm → editor)
// -----------------------------------------------------------------------------

pub fn read_text_file_request_wit_to_schema(
    req: ReadTextFileRequest,
) -> schema::ReadTextFileRequest {
    let mut out = schema::ReadTextFileRequest::new(
        schema::SessionId::from(req.session_id),
        PathBuf::from(req.path),
    );
    out.line = req.line;
    out.limit = req.limit;
    out
}

pub fn read_text_file_response_schema_to_wit(
    resp: schema::ReadTextFileResponse,
) -> ReadTextFileResponse {
    ReadTextFileResponse {
        content: resp.content,
    }
}

pub fn write_text_file_request_wit_to_schema(
    req: WriteTextFileRequest,
) -> schema::WriteTextFileRequest {
    schema::WriteTextFileRequest::new(
        schema::SessionId::from(req.session_id),
        PathBuf::from(req.path),
        req.content,
    )
}

/// Convert an ACP JSON-RPC error into a WIT error to return to the wasm
/// guest. Inverse of [`wit_error_to_acp`].
pub fn acp_error_to_wit(e: AcpError) -> Error {
    let code = match e.code {
        schema::ErrorCode::ParseError => ErrorCode::ParseError,
        schema::ErrorCode::InvalidRequest => ErrorCode::InvalidRequest,
        schema::ErrorCode::MethodNotFound => ErrorCode::MethodNotFound,
        schema::ErrorCode::InvalidParams => ErrorCode::InvalidParams,
        schema::ErrorCode::InternalError => ErrorCode::InternalError,
        schema::ErrorCode::AuthRequired => ErrorCode::AuthRequired,
        schema::ErrorCode::ResourceNotFound => ErrorCode::ResourceNotFound,
        AcpErrorCode::Other(n) => ErrorCode::Other(n),
        // schema::ErrorCode is `non_exhaustive`. Anything new gets reported
        // as InternalError to keep our errors well-typed.
        _ => ErrorCode::InternalError,
    };
    Error {
        code,
        message: e.message,
    }
}

// -----------------------------------------------------------------------------
// Tool calls (wasm → editor)
// -----------------------------------------------------------------------------

/// Map our WIT `ToolKind` to the ACP wire string.
fn tool_kind_str(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Delete => "delete",
        ToolKind::Move => "move",
        ToolKind::Search => "search",
        ToolKind::Execute => "execute",
        ToolKind::Think => "think",
        ToolKind::Fetch => "fetch",
        ToolKind::Other => "other",
    }
}

/// Map our WIT `ToolCallStatus` to the ACP wire string.
fn tool_status_str(status: ToolCallStatus) -> &'static str {
    match status {
        ToolCallStatus::Pending => "pending",
        ToolCallStatus::InProgress => "in_progress",
        ToolCallStatus::Completed => "completed",
        ToolCallStatus::Failed => "failed",
    }
}

/// Render a `ToolCallContent` element into the JSON object the wire
/// expects. Currently only the `content` (standard content block) variant
/// is forwarded — diff and terminal variants are logged-and-dropped until
/// they're needed.
fn tool_call_content_to_json(
    session_id: &str,
    content: ToolCallContent,
) -> Option<serde_json::Value> {
    match content {
        ToolCallContent::Content(block) => {
            let schema_block = content_block_wit_to_schema(session_id, block)?;
            let inner = serde_json::to_value(schema_block).ok()?;
            Some(serde_json::json!({ "type": "content", "content": inner }))
        }
        ToolCallContent::Diff(diff) => {
            let mut d = serde_json::json!({
                "type": "diff",
                "path": diff.path,
                "newText": diff.new_text,
            });
            if let Some(old) = diff.old_text {
                d["oldText"] = serde_json::Value::String(old);
            }
            Some(d)
        }
        ToolCallContent::Terminal(_) => {
            debug!(session = %session_id, "dropped tool-call terminal content (not yet wired)");
            None
        }
    }
}

/// Build the shared `{ toolCallId, title, kind, status, content, rawInput,
/// rawOutput }` JSON object used by both `tool_call` / `tool_call_update`
/// session updates and the `toolCall` field of a permission request.
fn tool_call_snapshot_to_json(session_id: &str, call: ToolCallSnapshot) -> serde_json::Value {
    let content: Vec<serde_json::Value> = call
        .content
        .into_iter()
        .filter_map(|c| tool_call_content_to_json(session_id, c))
        .collect();
    let raw_input = call
        .raw_input
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
    let raw_output = call
        .raw_output
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .or_else(|| call.raw_output.map(serde_json::Value::String));
    let mut json = serde_json::json!({
        "toolCallId": call.id,
        "title": call.title,
        "kind": tool_kind_str(call.kind),
        "status": tool_status_str(call.status),
        "content": content,
    });
    if let Some(v) = raw_input {
        json["rawInput"] = v;
    }
    if let Some(v) = raw_output {
        json["rawOutput"] = v;
    }
    json
}

fn tool_call_to_schema_update(
    session_id: &str,
    call: ToolCallSnapshot,
    is_update: bool,
) -> Option<schema::SessionUpdate> {
    // The initial announcement is a `tool_call`; every later state change is a
    // `tool_call_update`. Both carry the same top-level field layout.
    let discriminator = if is_update { "tool_call_update" } else { "tool_call" };
    let mut json = tool_call_snapshot_to_json(session_id, call);
    json["sessionUpdate"] = serde_json::Value::String(discriminator.to_string());
    serde_json::from_value(json).ok()
}

/// Map our WIT `PermissionOptionKind` to the ACP wire string.
fn permission_option_kind_str(kind: PermissionOptionKind) -> &'static str {
    match kind {
        PermissionOptionKind::AllowOnce => "allow_once",
        PermissionOptionKind::AllowAlways => "allow_always",
        PermissionOptionKind::RejectOnce => "reject_once",
        PermissionOptionKind::RejectAlways => "reject_always",
    }
}

/// Translate a guest `request-permission` call into the schema request the
/// editor expects. Returns `None` if the assembled JSON doesn't round-trip.
pub fn request_permission_request_wit_to_schema(
    req: RequestPermissionRequest,
) -> Option<schema::RequestPermissionRequest> {
    let session = req.session_id;
    let tool_call = tool_call_snapshot_to_json(&session, req.tool_call);
    let options: Vec<serde_json::Value> = req
        .options
        .into_iter()
        .map(|o: PermissionOption| {
            serde_json::json!({
                "optionId": o.id,
                "name": o.name,
                "kind": permission_option_kind_str(o.kind),
            })
        })
        .collect();
    let json = serde_json::json!({
        "sessionId": session,
        "toolCall": tool_call,
        "options": options,
    });
    serde_json::from_value(json).ok()
}

/// Translate the editor's permission response back into the WIT shape.
pub fn request_permission_response_schema_to_wit(
    resp: schema::RequestPermissionResponse,
) -> RequestPermissionResponse {
    let outcome = match resp.outcome {
        schema::RequestPermissionOutcome::Selected(sel) => {
            PermissionOutcome::Selected(sel.option_id.0.to_string())
        }
        // `Cancelled` — and any future non-exhaustive variant — abort the call.
        _ => PermissionOutcome::Cancelled,
    };
    RequestPermissionResponse { outcome }
}

fn _dead_tool_call_update_to_schema_update(
    _session_id: &str,
    _update: (),
) -> Option<schema::SessionUpdate> {
    // Removed in streams phase 1: tool-call updates now ship a full
    // [`ToolCallSnapshot`] which is handled by [`tool_call_to_schema_update`].
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yosh::acp::content::ImageContent;
    use crate::yosh::acp::init::{
        AgentCapabilities, McpCapabilities, PromptCapabilities, SessionCapabilities,
    };

    #[test]
    fn error_code_roundtrip() {
        let cases = [
            (ErrorCode::ParseError, schema::ErrorCode::ParseError),
            (ErrorCode::InvalidRequest, schema::ErrorCode::InvalidRequest),
            (ErrorCode::MethodNotFound, schema::ErrorCode::MethodNotFound),
            (ErrorCode::InvalidParams, schema::ErrorCode::InvalidParams),
            (ErrorCode::InternalError, schema::ErrorCode::InternalError),
            (ErrorCode::AuthRequired, schema::ErrorCode::AuthRequired),
            (
                ErrorCode::ResourceNotFound,
                schema::ErrorCode::ResourceNotFound,
            ),
        ];
        for (wit_code, schema_code) in cases {
            let acp = wit_error_to_acp(Error {
                code: wit_code,
                message: "msg".into(),
            });
            assert_eq!(acp.code, schema_code, "code mismatch for {:?}", wit_code);
            assert_eq!(acp.message, "msg");
        }
    }

    #[test]
    fn error_code_other_passthrough() {
        let acp = wit_error_to_acp(Error {
            code: ErrorCode::Other(-12345),
            message: "boom".into(),
        });
        assert_eq!(acp.code, AcpErrorCode::Other(-12345));
        assert_eq!(acp.message, "boom");
    }

    #[test]
    fn protocol_version_roundtrip() {
        let n = pv_to_u32(&pv_from_u32(1));
        assert_eq!(n, 1);
    }

    #[test]
    fn usage_update_with_cost_serializes_to_wire() {
        use crate::yosh::acp::prompts::{UsageCost, UsageUpdate};
        let note = session_update_wit_to_schema(
            "sess-1".to_string(),
            SessionUpdate::UsageUpdate(UsageUpdate {
                used: 120,
                size: 8192,
                cost: Some(UsageCost {
                    amount: 1.5,
                    currency: "AIU".to_string(),
                }),
            }),
        )
        .expect("usage update translates to a notification");
        let json = serde_json::to_value(&note).expect("serialize notification");
        assert_eq!(json.pointer("/sessionId"), Some(&serde_json::json!("sess-1")));
        assert_eq!(
            json.pointer("/update/sessionUpdate"),
            Some(&serde_json::json!("usage_update")),
            "full wire: {json}"
        );
        assert_eq!(json.pointer("/update/used"), Some(&serde_json::json!(120)));
        assert_eq!(json.pointer("/update/size"), Some(&serde_json::json!(8192)));
        assert_eq!(
            json.pointer("/update/cost/amount"),
            Some(&serde_json::json!(1.5)),
            "cost.amount missing/renamed on the wire: {json}"
        );
        assert_eq!(
            json.pointer("/update/cost/currency"),
            Some(&serde_json::json!("AIU")),
            "cost.currency missing/renamed on the wire: {json}"
        );
    }

    #[test]
    fn usage_update_without_cost_omits_cost() {
        use crate::yosh::acp::prompts::UsageUpdate;
        let note = session_update_wit_to_schema(
            "sess-2".to_string(),
            SessionUpdate::UsageUpdate(UsageUpdate {
                used: 42,
                size: 4096,
                cost: None,
            }),
        )
        .expect("usage update translates");
        let json = serde_json::to_value(&note).expect("serialize notification");
        assert_eq!(json.pointer("/update/used"), Some(&serde_json::json!(42)));
        assert_eq!(
            json.pointer("/update/cost"),
            None,
            "cost should be absent when None: {json}"
        );
    }

    #[test]
    fn initialize_request_translation() {
        let req = schema::InitializeRequest::new(agent_client_protocol::schema::ProtocolVersion::V1)
            .client_info(schema::Implementation::new("editor", "1.0").title(Some("Ed".into())));
        let wit_req = init_request_schema_to_wit(req);
        assert_eq!(wit_req.protocol_version, 1);
        let info = wit_req.client_info.unwrap();
        assert_eq!(info.name, "editor");
        assert_eq!(info.version, "1.0");
        assert_eq!(info.title.as_deref(), Some("Ed"));
    }

    #[test]
    fn initialize_response_translation() {
        let resp = InitializeResponse {
            protocol_version: 1,
            agent_capabilities: AgentCapabilities {
                load_session: true,
                prompt_capabilities: PromptCapabilities {
                    image: true,
                    audio: false,
                    embedded_context: false,
                },
                mcp_capabilities: McpCapabilities {
                    http: true,
                    sse: false,
                },
                session_capabilities: SessionCapabilities {
                    list: false,
                    resume: false,
                    close: false,
                },
            },
            agent_info: Some(ImplementationInfo {
                name: "ag".into(),
                title: None,
                version: "0.1".into(),
            }),
            auth_methods: vec![],
        };
        let schema_resp = init_response_wit_to_schema(resp);
        assert_eq!(pv_to_u32(&schema_resp.protocol_version), 1);
        assert!(schema_resp.agent_capabilities.load_session);
        assert!(schema_resp.agent_capabilities.prompt_capabilities.image);
        assert!(schema_resp.agent_capabilities.mcp_capabilities.http);
        let info = schema_resp.agent_info.unwrap();
        assert_eq!(info.name, "ag");
        assert_eq!(info.version, "0.1");
    }

    #[test]
    fn prompt_request_text_only() {
        // Phase 1 streams: `prompt_request_schema_to_wit` was deleted
        // along with the top-level `PromptRequest` record. Prompts now
        // arrive as `list<content-block>` directly on `session.prompt`.
        // Keep the test name + comment so the missing coverage is
        // visible; phase 3 will reintroduce an equivalent translator
        // (schema content-block -> wit content-block) and replace this
        // body.
    }

    #[test]
    fn prompt_response_stop_reasons() {
        for (wit_sr, json_sr) in [
            (StopReason::EndTurn, "end_turn"),
            (StopReason::MaxTokens, "max_tokens"),
            (StopReason::Refusal, "refusal"),
            (StopReason::Cancelled, "cancelled"),
        ] {
            let resp = prompt_response_wit_to_schema(PromptResponse {
                stop_reason: wit_sr,
            })
            .expect("synth ok");
            let json = serde_json::to_value(&resp).unwrap();
            assert_eq!(json["stopReason"], json_sr);
        }
    }

    #[test]
    fn session_update_agent_chunk() {
        let notif = session_update_wit_to_schema(
            "s1".into(),
            SessionUpdate::AgentMessageChunk(ContentBlock::Text(TextContent { text: "hi".into() })),
        )
        .unwrap();
        let json = serde_json::to_value(&notif).unwrap();
        assert_eq!(json["sessionId"], "s1");
        assert_eq!(json["update"]["sessionUpdate"], "agent_message_chunk");
        assert_eq!(json["update"]["content"]["text"], "hi");
    }

    #[test]
    fn session_update_unsupported_drops() {
        // Non-text content blocks aren't translated yet, so the update is
        // dropped rather than panicking or sending malformed data.
        let dropped = session_update_wit_to_schema(
            "s".into(),
            SessionUpdate::AgentMessageChunk(ContentBlock::Image(ImageContent {
                data: "x".into(),
                mime_type: "image/png".into(),
                uri: None,
            })),
        );
        assert!(dropped.is_none());
    }

    #[test]
    fn tool_call_announce_vs_update_discriminator() {
        use crate::yosh::acp::tools::ToolCallSnapshot;
        let snap = |status| ToolCallSnapshot {
            id: "tc1".into(),
            title: "Read /tmp/x".into(),
            kind: ToolKind::Read,
            status,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: Some("{\"path\":\"/tmp/x\"}".into()),
            raw_output: None,
        };

        // The initial announcement is `tool_call`.
        let announce = session_update_wit_to_schema(
            "s".into(),
            SessionUpdate::ToolCall(snap(ToolCallStatus::Pending)),
        )
        .unwrap();
        let json = serde_json::to_value(&announce).unwrap();
        assert_eq!(json["update"]["sessionUpdate"], "tool_call");
        assert_eq!(json["update"]["toolCallId"], "tc1");
        assert_eq!(json["update"]["kind"], "read");
        assert_eq!(json["update"]["rawInput"]["path"], "/tmp/x");

        // Every later state change is a `tool_call_update`.
        let update = session_update_wit_to_schema(
            "s".into(),
            SessionUpdate::ToolCallUpdate(snap(ToolCallStatus::Completed)),
        )
        .unwrap();
        let json = serde_json::to_value(&update).unwrap();
        assert_eq!(json["update"]["sessionUpdate"], "tool_call_update");
        assert_eq!(json["update"]["toolCallId"], "tc1");
        assert_eq!(json["update"]["status"], "completed");
    }

    #[test]
    fn tool_call_diff_content_is_wired() {
        use crate::yosh::acp::tools::{Diff, ToolCallSnapshot};
        let snap = ToolCallSnapshot {
            id: "tc2".into(),
            title: "Write /tmp/y".into(),
            kind: ToolKind::Edit,
            status: ToolCallStatus::Completed,
            content: vec![ToolCallContent::Diff(Diff {
                path: "/tmp/y".into(),
                old_text: None,
                new_text: "hello\n".into(),
            })],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        };
        let notif =
            session_update_wit_to_schema("s".into(), SessionUpdate::ToolCallUpdate(snap)).unwrap();
        let json = serde_json::to_value(&notif).unwrap();
        let diff = &json["update"]["content"][0];
        assert_eq!(diff["type"], "diff");
        assert_eq!(diff["path"], "/tmp/y");
        assert_eq!(diff["newText"], "hello\n");
    }
}
