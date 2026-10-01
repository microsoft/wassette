// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Offline ACP routing fixture. All instances deliberately reuse local IDs.

#[allow(clippy::all)]
#[rustfmt::skip]
#[path = "../../../../../../components/acp-echo-provider/src/bindings.rs"]
mod bindings;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use bindings::exports::wassette::acp::agent::{Guest, GuestSession, Session};
use bindings::wasmcloud::secrets::{reveal, store};
use bindings::wassette::acp::client;
use bindings::wassette::acp::content::{ContentBlock, TextContent};
use bindings::wassette::acp::errors::{Error, ErrorCode};
use bindings::wassette::acp::filesystem::{ReadTextFileRequest, WriteTextFileRequest};
use bindings::wassette::acp::init::{
    AgentCapabilities, AuthenticateRequest, ImplementationInfo, InitializeRequest,
    InitializeResponse, McpCapabilities, PromptCapabilities, SessionCapabilities,
};
use bindings::wassette::acp::prompts::{PromptResponse, SessionUpdate, StopReason};
use bindings::wassette::acp::sessions::{
    ComponentSource, ListSessionsRequest, ListSessionsResponse, LoadSessionRequest,
    LoadSessionResponse, NewSessionRequest, NewSessionResponse, ResumeSessionRequest,
    ResumeSessionResponse, SessionConfigId, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigSelectOption, SessionConfigSelectOptions, SessionConfigValueId, SessionMode,
    SessionModeId, SessionModeState, SessionModel, SessionModelId, SessionModelState,
};
use bindings::wassette::acp::tools::{
    PermissionOption, PermissionOptionKind, PermissionOutcome, RequestPermissionRequest,
    ToolCallSnapshot, ToolCallStatus, ToolKind,
};
use bindings::wassette::component_generation::builder;
use bindings::wassette::component_tools::tools as component_tools;

const SESSION_ID: &str = "collision";
const TOOL_CALL_ID: &str = "collision-tool";
const MODEL_CONFIG_ID: &str = "fixture-model";
const MODELS: [&str; 2] = ["shared", "alternate"];
const STYLES: [&str; 2] = ["plain", "loud"];

struct Agent;

struct RoutingSession {
    state: Rc<RefCell<SessionState>>,
}

thread_local! {
    static INITIALIZED: Cell<bool> = const { Cell::new(false) };
    static LAST_SESSION: RefCell<Rc<RefCell<SessionState>>> =
        RefCell::new(Rc::new(RefCell::new(SessionState::default())));
}

#[derive(Debug)]
struct Settings {
    label: String,
    no_models: bool,
    empty_models: bool,
    reject_alternate: bool,
    load_supported: bool,
    fail_new: bool,
    mcp_http: bool,
    callback_path: Option<String>,
}

impl Settings {
    fn read() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let enabled =
            |key| lookup(key).is_some_and(|value| !matches!(value.as_str(), "" | "0" | "false"));
        Self {
            label: lookup("ROUTING_LABEL").unwrap_or_else(|| "fixture".into()),
            no_models: enabled("ROUTING_NO_MODELS"),
            empty_models: enabled("ROUTING_EMPTY_MODELS"),
            reject_alternate: enabled("ROUTING_REJECT_ALTERNATE"),
            load_supported: enabled("ROUTING_LOAD_SUPPORTED"),
            fail_new: enabled("ROUTING_FAIL_NEW"),
            mcp_http: enabled("ROUTING_MCP_HTTP"),
            callback_path: enabled("ROUTING_CALLBACK_NEW").then(|| {
                lookup("ROUTING_CALLBACK_NEW")
                    .filter(|path| path.starts_with('/'))
                    .unwrap_or_else(|| "/routing/startup.txt".into())
            }),
        }
    }

    fn model_ids(&self) -> &[&str] {
        if self.empty_models {
            &[]
        } else {
            &MODELS
        }
    }
}

#[derive(Clone, Debug)]
struct SessionState {
    model: String,
    style: String,
    prompt_count: u64,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            model: "shared".into(),
            style: "plain".into(),
            prompt_count: 0,
        }
    }
}

impl SessionState {
    fn summary(&self, settings: &Settings) -> String {
        format!("{}:{}:{}", settings.label, self.model, self.style)
    }

    fn reply(&mut self, settings: &Settings, text: &str) -> String {
        self.prompt_count = self.prompt_count.saturating_add(1);
        format!("{}:{text}", self.summary(settings))
    }

    fn history(&self, settings: &Settings) -> String {
        format!("{}:history:{}", settings.label, self.prompt_count)
    }

    fn set_model(&mut self, model: &str, settings: &Settings) -> Result<(), Error> {
        if settings.no_models || !settings.model_ids().contains(&model) {
            return Err(error(
                ErrorCode::InvalidParams,
                format!("unknown model: {model}"),
            ));
        }
        if settings.reject_alternate && model == "alternate" {
            return Err(error(
                ErrorCode::InvalidParams,
                "alternate model selection rejected by fixture",
            ));
        }
        self.model = model.into();
        Ok(())
    }

    fn set_style(&mut self, style: &str) -> Result<(), Error> {
        if !STYLES.contains(&style) {
            return Err(error(
                ErrorCode::InvalidParams,
                format!("unknown style: {style}"),
            ));
        }
        self.style = style.into();
        Ok(())
    }

    fn set_config(
        &mut self,
        id: &str,
        value: &str,
        settings: &Settings,
    ) -> Result<Vec<SessionConfigOption>, Error> {
        match id {
            MODEL_CONFIG_ID => self.set_model(value, settings)?,
            "style" => self.set_style(value)?,
            _ => {
                return Err(error(
                    ErrorCode::InvalidParams,
                    format!("unknown config option: {id}"),
                ));
            }
        }
        Ok(self.config_options(settings))
    }

    fn config_options(&self, settings: &Settings) -> Vec<SessionConfigOption> {
        let mut options = Vec::new();
        if !settings.no_models {
            options.push(config_option(
                MODEL_CONFIG_ID,
                Some(SessionConfigOptionCategory::Model),
                &self.model,
                settings.model_ids(),
            ));
        }
        options.push(config_option("style", None, &self.style, &STYLES));
        options
    }

    fn models(&self, settings: &Settings) -> Option<SessionModelState> {
        (!settings.no_models).then(|| SessionModelState {
            current_model_id: self.model.clone(),
            available_models: settings
                .model_ids()
                .iter()
                .map(|id| SessionModel {
                    id: (*id).into(),
                    name: (*id).into(),
                    description: None,
                    provided_by: source(),
                })
                .collect(),
        })
    }

    fn modes(&self) -> SessionModeState {
        SessionModeState {
            current_mode_id: self.style.clone(),
            available_modes: STYLES
                .iter()
                .map(|id| SessionMode {
                    id: (*id).into(),
                    name: (*id).into(),
                    description: None,
                    provided_by: source(),
                })
                .collect(),
        }
    }
}

fn source() -> ComponentSource {
    ComponentSource {
        component_id: "test:routing-provider".into(),
    }
}

fn config_option(
    id: &str,
    category: Option<SessionConfigOptionCategory>,
    current: &str,
    values: &[&str],
) -> SessionConfigOption {
    SessionConfigOption {
        id: id.into(),
        name: id.into(),
        description: None,
        category,
        current_value: current.into(),
        options: SessionConfigSelectOptions::Ungrouped(
            values
                .iter()
                .map(|value| SessionConfigSelectOption {
                    value: (*value).into(),
                    name: (*value).into(),
                    description: None,
                })
                .collect(),
        ),
        provided_by: source(),
    }
}

fn error(code: ErrorCode, message: impl Into<String>) -> Error {
    Error {
        code,
        message: message.into(),
    }
}

fn operation_error(operation: &str, cause: impl std::fmt::Display) -> Error {
    error(ErrorCode::InternalError, format!("{operation}: {cause}"))
}

fn require_initialized() -> Result<(), Error> {
    if INITIALIZED.get() {
        Ok(())
    } else {
        Err(error(
            ErrorCode::InvalidRequest,
            "initialize must precede session creation",
        ))
    }
}

fn prompt_text(prompt: &[ContentBlock]) -> String {
    prompt
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

async fn emit(text: String) {
    client::notify_session(
        SESSION_ID.into(),
        SessionUpdate::AgentMessageChunk(ContentBlock::Text(TextContent { text })),
    )
    .await;
}

async fn read_file(path: &str) -> Result<String, Error> {
    Ok(client::read_text_file(ReadTextFileRequest {
        session_id: SESSION_ID.into(),
        path: path.into(),
        line: None,
        limit: None,
    })
    .await?
    .content)
}

async fn startup(settings: &Settings, operation: &str) -> Result<(), Error> {
    emit(format!("{}:startup:{operation}", settings.label)).await;
    if let Some(path) = &settings.callback_path {
        let content = read_file(path).await?;
        emit(format!("{}:startup:{operation}:{content}", settings.label)).await;
    }
    Ok(())
}

fn permission_request(label: &str) -> RequestPermissionRequest {
    RequestPermissionRequest {
        session_id: SESSION_ID.into(),
        tool_call: ToolCallSnapshot {
            id: TOOL_CALL_ID.into(),
            title: format!("{label}:permission"),
            kind: ToolKind::Other,
            status: ToolCallStatus::Pending,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        options: [
            ("allow", PermissionOptionKind::AllowOnce),
            ("reject", PermissionOptionKind::RejectOnce),
        ]
        .into_iter()
        .map(|(id, kind)| PermissionOption {
            id: id.into(),
            name: id.into(),
            kind,
        })
        .collect(),
    }
}

async fn request_permission(label: &str) -> Result<String, Error> {
    let response = client::request_permission(permission_request(label)).await?;
    match response.outcome {
        PermissionOutcome::Selected(id) if id == "allow" || id == "reject" => Ok(id),
        PermissionOutcome::Selected(_) => Err(error(
            ErrorCode::InvalidParams,
            "unknown permission decision",
        )),
        PermissionOutcome::Cancelled => Ok("cancelled".into()),
    }
}

fn required_argument<'a>(arguments: &'a str, usage: &str) -> Result<&'a str, Error> {
    if arguments.is_empty() {
        Err(error(ErrorCode::InvalidParams, format!("usage: {usage}")))
    } else {
        Ok(arguments)
    }
}

async fn run_command(command: &str, settings: &Settings) -> Result<Option<String>, Error> {
    let (name, arguments) = command.split_once(' ').unwrap_or((command, ""));
    let output = match name {
        "/read" => {
            let path = required_argument(arguments, "/read <path>")?;
            format!("read:{}", read_file(path).await?)
        }
        "/write" => {
            let (path, content) = arguments
                .split_once(' ')
                .ok_or_else(|| error(ErrorCode::InvalidParams, "usage: /write <path> <content>"))?;
            required_argument(path, "/write <path> <content>")?;
            client::write_text_file(WriteTextFileRequest {
                session_id: SESSION_ID.into(),
                path: path.into(),
                content: content.into(),
            })
            .await?;
            "write:ok".into()
        }
        "/permission" => format!("permission:{}", request_permission(&settings.label).await?),
        "/secret" => {
            let key = required_argument(arguments, "/secret <key>")?;
            let secret = store::get(key.into())
                .await
                .map_err(|cause| operation_error("secret lookup", cause))?;
            let value = match reveal::reveal(&secret).await {
                store::SecretValue::String(value) => value,
                store::SecretValue::Bytes(value) => String::from_utf8(value)
                    .map_err(|cause| operation_error("secret UTF-8", cause))?,
            };
            format!("secret:{value}")
        }
        "/data-write" => {
            let value = required_argument(arguments, "/data-write <value>")?;
            std::fs::write("/data/value", value)
                .map_err(|cause| operation_error("data write", cause))?;
            "data-write:ok".into()
        }
        "/data-read" => {
            let value = std::fs::read_to_string("/data/value")
                .map_err(|cause| operation_error("data read", cause))?;
            format!("data-read:{value}")
        }
        "/env" => {
            let key = required_argument(arguments, "/env <key>")?;
            let value = match std::env::var(key) {
                Ok(value) => value,
                Err(std::env::VarError::NotPresent) => "<unset>".into(),
                Err(cause) => return Err(operation_error("environment", cause)),
            };
            format!("env:{value}")
        }
        "/tool" => {
            let (name, arguments) = arguments.split_once(' ').ok_or_else(|| {
                error(
                    ErrorCode::InvalidParams,
                    "usage: /tool <name> <arguments-json>",
                )
            })?;
            required_argument(name, "/tool <name> <arguments-json>")?;
            match component_tools::call_tool_by_name(name.into(), arguments.into()).await {
                Ok(result) => format!("tool:{}", result.text),
                Err(component_tools::ToolError::Cancelled) => return Ok(None),
                Err(cause) => return Err(operation_error("tool", cause)),
            }
        }
        "/generate" => {
            let request = required_argument(arguments, "/generate <request-json>")?;
            let report =
                builder::generate(request).map_err(|cause| operation_error("generation", cause))?;
            format!(
                "generation:{:?}:{}:{:?}",
                report.disposition, report.report_json, report.tool_handles
            )
        }
        _ => {
            return Err(error(
                ErrorCode::InvalidParams,
                format!("unknown command: {name}"),
            ));
        }
    };
    Ok(Some(format!("{}:{output}", settings.label)))
}

impl GuestSession for RoutingSession {
    async fn prompt(&self, prompt: Vec<ContentBlock>) -> Result<PromptResponse, Error> {
        let text = prompt_text(&prompt);
        if !text.is_empty() {
            let settings = Settings::read();
            let output = if text == "/state" {
                Some(self.state.borrow().summary(&settings))
            } else if text == "/history" {
                Some(self.state.borrow().history(&settings))
            } else if text.starts_with('/') {
                run_command(&text, &settings).await?
            } else {
                Some(self.state.borrow_mut().reply(&settings, &text))
            };
            match output {
                Some(output) => emit(output).await,
                None => {
                    return Ok(PromptResponse {
                        stop_reason: StopReason::Cancelled,
                    });
                }
            }
        }
        Ok(PromptResponse {
            stop_reason: StopReason::EndTurn,
        })
    }

    async fn set_mode(&self, mode_id: SessionModeId) -> Result<(), Error> {
        self.state.borrow_mut().set_style(&mode_id)
    }

    async fn select_model(&self, model_id: SessionModelId) -> Result<(), Error> {
        self.state
            .borrow_mut()
            .set_model(&model_id, &Settings::read())
    }

    async fn set_config_option(
        &self,
        config_id: SessionConfigId,
        value: SessionConfigValueId,
    ) -> Result<Vec<SessionConfigOption>, Error> {
        self.state
            .borrow_mut()
            .set_config(&config_id, &value, &Settings::read())
    }
}

impl Guest for Agent {
    type Session = RoutingSession;

    async fn initialize(_req: InitializeRequest) -> Result<InitializeResponse, Error> {
        let settings = Settings::read();
        INITIALIZED.set(true);
        Ok(InitializeResponse {
            protocol_version: 1,
            agent_capabilities: AgentCapabilities {
                load_session: settings.load_supported,
                prompt_capabilities: PromptCapabilities {
                    image: false,
                    audio: false,
                    embedded_context: false,
                },
                mcp_capabilities: McpCapabilities {
                    http: settings.mcp_http,
                    sse: false,
                },
                session_capabilities: SessionCapabilities {
                    list: false,
                    resume: false,
                    close: false,
                },
            },
            agent_info: Some(ImplementationInfo {
                name: "acp-routing-provider".into(),
                title: Some(settings.label),
                version: env!("CARGO_PKG_VERSION").into(),
            }),
            auth_methods: Vec::new(),
        })
    }

    async fn authenticate(_req: AuthenticateRequest) -> Result<(), Error> {
        Err(error(
            ErrorCode::MethodNotFound,
            "authentication not required",
        ))
    }

    async fn new_session(_req: NewSessionRequest) -> Result<(Session, NewSessionResponse), Error> {
        require_initialized()?;
        let settings = Settings::read();
        if settings.fail_new {
            return Err(error(
                ErrorCode::InternalError,
                format!("{}:new-session failed", settings.label),
            ));
        }
        let state = SessionState::default();
        let response = NewSessionResponse {
            session_id: SESSION_ID.into(),
            modes: Some(state.modes()),
            models: state.models(&settings),
            config_options: Some(state.config_options(&settings)),
        };
        startup(&settings, "new").await?;
        let state = Rc::new(RefCell::new(state));
        LAST_SESSION.with(|last| *last.borrow_mut() = Rc::clone(&state));
        Ok((Session::new(RoutingSession { state }), response))
    }

    async fn load_session(
        req: LoadSessionRequest,
    ) -> Result<(Session, LoadSessionResponse), Error> {
        require_initialized()?;
        let settings = Settings::read();
        if !settings.load_supported {
            return Err(error(ErrorCode::MethodNotFound, "session/load is disabled"));
        }
        if req.session_id != SESSION_ID {
            return Err(error(
                ErrorCode::InvalidParams,
                "session/load expects provider-local id collision",
            ));
        }
        let state = LAST_SESSION.with(|last| Rc::clone(&last.borrow()));
        let response = {
            let state = state.borrow();
            LoadSessionResponse {
                modes: Some(state.modes()),
                models: state.models(&settings),
                config_options: Some(state.config_options(&settings)),
            }
        };
        startup(&settings, "load").await?;
        Ok((Session::new(RoutingSession { state }), response))
    }

    async fn list_sessions(_req: ListSessionsRequest) -> Result<ListSessionsResponse, Error> {
        Err(error(
            ErrorCode::MethodNotFound,
            "session/list is not supported",
        ))
    }

    async fn resume_session(
        _req: ResumeSessionRequest,
    ) -> Result<(Session, ResumeSessionResponse), Error> {
        Err(error(
            ErrorCode::MethodNotFound,
            "session/resume is not supported",
        ))
    }
}

bindings::export!(Agent with_types_in bindings);

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(values: &[(&str, &str)]) -> Settings {
        Settings::from_lookup(|key| {
            values
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).into())
        })
    }

    #[test]
    fn defaults_are_deterministic() {
        let settings = settings(&[]);
        let state = SessionState::default();
        assert_eq!(state.summary(&settings), "fixture:shared:plain");
        assert!(!settings.no_models);
        assert!(!settings.empty_models);
        assert!(!settings.reject_alternate);
        assert!(!settings.load_supported);
        assert!(!settings.fail_new);
        assert!(!settings.mcp_http);
        assert!(settings.callback_path.is_none());
        let models = state.models(&settings).unwrap();
        assert_eq!(models.current_model_id, "shared");
        assert_eq!(
            models
                .available_models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            MODELS
        );
    }

    #[test]
    fn flags_and_callback_path_are_explicit() {
        let settings = settings(&[
            ("ROUTING_LABEL", "a"),
            ("ROUTING_NO_MODELS", "1"),
            ("ROUTING_LOAD_SUPPORTED", "true"),
            ("ROUTING_FAIL_NEW", "0"),
            ("ROUTING_MCP_HTTP", "false"),
            ("ROUTING_CALLBACK_NEW", "/editor/startup"),
        ]);
        assert_eq!(settings.label, "a");
        assert!(settings.no_models);
        assert!(settings.load_supported);
        assert!(!settings.fail_new);
        assert!(!settings.mcp_http);
        assert_eq!(settings.callback_path.as_deref(), Some("/editor/startup"));
        let enabled = Settings::from_lookup(|_| Some("1".into()));
        assert!(enabled.empty_models);
        assert!(enabled.reject_alternate);
        assert!(enabled.fail_new);
        assert!(enabled.mcp_http);
        assert_eq!(
            enabled.callback_path.as_deref(),
            Some("/routing/startup.txt")
        );
    }

    #[test]
    fn set_config_returns_full_latest_options() {
        let settings = settings(&[("ROUTING_LABEL", "a")]);
        let mut state = SessionState::default();
        let options = state
            .set_config(MODEL_CONFIG_ID, "alternate", &settings)
            .unwrap();
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].id, MODEL_CONFIG_ID);
        assert!(matches!(
            options[0].category,
            Some(SessionConfigOptionCategory::Model)
        ));
        assert_eq!(options[0].current_value, "alternate");
        assert_eq!(options[1].id, "style");
        assert!(options[1].category.is_none());
        assert_eq!(options[1].current_value, "plain");

        let options = state.set_config("style", "loud", &settings).unwrap();
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].current_value, "alternate");
        assert_eq!(options[1].current_value, "loud");
        assert_eq!(state.summary(&settings), "a:alternate:loud");
        assert_eq!(state.modes().current_mode_id, "loud");
        assert_eq!(
            state.models(&settings).unwrap().current_model_id,
            "alternate"
        );
    }

    #[test]
    fn prompt_counts_are_independent_and_survive_selection_changes() {
        let alpha_settings = settings(&[("ROUTING_LABEL", "alpha")]);
        let beta_settings = settings(&[("ROUTING_LABEL", "beta")]);
        let alpha = Rc::new(RefCell::new(SessionState::default()));
        let mut beta = SessionState::default();
        assert_eq!(alpha.borrow().history(&alpha_settings), "alpha:history:0");
        assert_eq!(beta.history(&beta_settings), "beta:history:0");

        assert_eq!(
            alpha.borrow_mut().reply(&alpha_settings, "hello"),
            "alpha:shared:plain:hello"
        );
        assert_eq!(
            beta.reply(&beta_settings, "other"),
            "beta:shared:plain:other"
        );
        alpha
            .borrow_mut()
            .set_config(MODEL_CONFIG_ID, "alternate", &alpha_settings)
            .unwrap();
        alpha
            .borrow_mut()
            .set_config("style", "loud", &alpha_settings)
            .unwrap();
        assert_eq!(
            alpha.borrow_mut().reply(&alpha_settings, "again"),
            "alpha:alternate:loud:again"
        );

        let loaded = Rc::clone(&alpha);
        assert_eq!(loaded.borrow().history(&alpha_settings), "alpha:history:2");
        assert_eq!(alpha.borrow().history(&alpha_settings), "alpha:history:2");
        assert_eq!(beta.history(&beta_settings), "beta:history:1");
        assert_eq!(
            SessionState::default().history(&alpha_settings),
            "alpha:history:0"
        );
    }

    #[test]
    fn invalid_selection_does_not_mutate_state() {
        let settings = settings(&[]);
        let mut state = SessionState::default();
        for (id, value) in [
            (MODEL_CONFIG_ID, "missing"),
            ("style", "missing"),
            ("missing", "loud"),
        ] {
            let error = state.set_config(id, value, &settings).unwrap_err();
            assert!(matches!(error.code, ErrorCode::InvalidParams));
            assert_eq!(state.summary(&settings), "fixture:shared:plain");
        }
    }

    #[test]
    fn no_models_keeps_non_model_options() {
        let settings = settings(&[("ROUTING_NO_MODELS", "1")]);
        let mut state = SessionState::default();
        assert!(state.models(&settings).is_none());
        assert!(state.set_model("shared", &settings).is_err());
        assert!(state
            .set_config(MODEL_CONFIG_ID, "alternate", &settings)
            .is_err());
        let options = state.set_config("style", "loud", &settings).unwrap();
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].id, "style");
        assert_eq!(options[0].current_value, "loud");
    }

    #[test]
    fn empty_models_advertises_empty_selectors() {
        let settings = settings(&[("ROUTING_EMPTY_MODELS", "1")]);
        let mut state = SessionState::default();
        let models = state.models(&settings).unwrap();
        assert!(models.available_models.is_empty());
        assert_eq!(models.current_model_id, "shared");
        assert!(state.set_model("shared", &settings).is_err());
        assert!(state
            .set_config(MODEL_CONFIG_ID, "alternate", &settings)
            .is_err());
        let options = state.set_config("style", "loud", &settings).unwrap();
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].id, MODEL_CONFIG_ID);
        assert!(matches!(
            &options[0].options,
            SessionConfigSelectOptions::Ungrouped(values) if values.is_empty()
        ));
        assert_eq!(options[1].id, "style");
        assert_eq!(options[1].current_value, "loud");
    }

    #[test]
    fn advertised_alternate_can_fail_without_mutating_state() {
        let settings = settings(&[("ROUTING_REJECT_ALTERNATE", "1")]);
        let mut state = SessionState::default();
        assert!(state
            .models(&settings)
            .unwrap()
            .available_models
            .iter()
            .any(|model| model.id == "alternate"));
        let options = state.config_options(&settings);
        assert!(matches!(
            &options[0].options,
            SessionConfigSelectOptions::Ungrouped(values)
                if values.iter().any(|value| value.value == "alternate")
        ));
        state.set_style("loud").unwrap();
        state.reply(&settings, "before");
        state.set_model("shared", &settings).unwrap();
        for error in [
            state.set_model("alternate", &settings).unwrap_err(),
            state
                .set_config(MODEL_CONFIG_ID, "alternate", &settings)
                .unwrap_err(),
        ] {
            assert!(matches!(error.code, ErrorCode::InvalidParams));
            assert_eq!(
                error.message,
                "alternate model selection rejected by fixture"
            );
        }
        assert_eq!(state.summary(&settings), "fixture:shared:loud");
        assert_eq!(state.history(&settings), "fixture:history:1");
    }

    #[test]
    fn local_permission_ids_collide_across_labels() {
        let a = permission_request("a");
        let b = permission_request("b");
        assert_eq!(a.session_id, SESSION_ID);
        assert_eq!(b.session_id, SESSION_ID);
        assert_eq!(a.tool_call.id, TOOL_CALL_ID);
        assert_eq!(b.tool_call.id, TOOL_CALL_ID);
        assert_ne!(a.tool_call.title, b.tool_call.title);
        assert_eq!(a.options[0].id, "allow");
        assert_eq!(a.options[1].id, "reject");
    }

    #[test]
    fn initialization_is_required() {
        INITIALIZED.set(false);
        assert!(matches!(
            require_initialized().unwrap_err().code,
            ErrorCode::InvalidRequest
        ));
        INITIALIZED.set(true);
        assert!(require_initialized().is_ok());
        INITIALIZED.set(false);
    }

    #[test]
    fn prompt_blocks_preserve_text_order() {
        let text = |value: &str| ContentBlock::Text(TextContent { text: value.into() });
        assert_eq!(prompt_text(&[text("hello"), text("world")]), "hello world");
        assert_eq!(prompt_text(&[]), "");
        assert!(required_argument("", "/read <path>").is_err());
    }
}
