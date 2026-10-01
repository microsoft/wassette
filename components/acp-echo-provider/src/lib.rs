// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! A self-contained ACP provider used to demo and test the host without a
//! model backend.
//!
//! It answers a prompt by streaming the user's own text back as agent message
//! chunks, one word at a time, then ends the turn. No network, no `wstd`, no
//! secrets — just `wit-bindgen` — so it builds anywhere the `wasm32-wasip2`
//! target does and makes an end-to-end ACP session reproducible offline.

#[allow(clippy::all)]
mod bindings;

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::bindings::exports::wassette::acp::agent::{Guest, GuestSession, Session};
use crate::bindings::wassette::acp::content::{ContentBlock, TextContent};
use crate::bindings::wassette::acp::errors::{Error, ErrorCode};
use crate::bindings::wassette::acp::init::{
    AgentCapabilities, AuthenticateRequest, ImplementationInfo, InitializeRequest,
    InitializeResponse, McpCapabilities, PromptCapabilities, SessionCapabilities,
};
use crate::bindings::wassette::acp::prompts::{PromptResponse, SessionUpdate, StopReason};
use crate::bindings::wassette::acp::sessions::{
    ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse,
    NewSessionRequest, NewSessionResponse, ResumeSessionRequest, ResumeSessionResponse,
    SessionConfigId, SessionConfigOption, SessionConfigValueId, SessionModeId, SessionModelId,
};
use crate::bindings::wassette::component_tools::tools as component_tools;

struct Agent;

/// Per-session state. The wire identity is the string id; the resource is a
/// lifetime handle whose `Drop` evicts the entry.
struct EchoSession {
    id: String,
}

impl Drop for EchoSession {
    fn drop(&mut self) {
        SESSIONS.with(|s| {
            s.borrow_mut().remove(&self.id);
        });
    }
}

/// What the session remembers between turns.
#[derive(Default)]
struct SessionState {
    /// Every prompt seen so far, so `load-session` has history to replay.
    history: Vec<String>,
    saved_tool: Option<String>,
    catalog_generation: u64,
}

// Wasm components are single-threaded, so thread-local + RefCell gives
// interior mutability with no synchronization.
thread_local! {
    static SESSIONS: RefCell<HashMap<String, SessionState>> = RefCell::new(HashMap::new());
    static INITIALIZED: RefCell<bool> = const { RefCell::new(false) };
}

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

fn next_session_id() -> String {
    let nonce = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    format!(
        "echo-{nonce:016x}-{}",
        SESSION_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn err(code: ErrorCode, message: &str) -> Error {
    Error {
        code,
        message: message.to_string(),
    }
}

/// Push a `session-update` upstream. The agent direction only carries the
/// eventual `prompt-response`; everything streamed goes this way.
async fn emit(session_id: &str, update: SessionUpdate) {
    crate::bindings::wassette::acp::client::notify_session(session_id.to_string(), update).await;
}

fn text_chunk(text: impl Into<String>) -> ContentBlock {
    ContentBlock::Text(TextContent { text: text.into() })
}

/// Flatten a prompt into plain text, keeping only the text blocks. The
/// provider advertises no image/audio capability, so nothing else is expected.
fn prompt_text(prompt: &[ContentBlock]) -> String {
    prompt
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

async fn call_tool(session_id: &str, command: &str) -> Result<PromptResponse, Error> {
    let mut parts = command.splitn(3, ' ');
    let _ = parts.next();
    let name = parts.next().unwrap_or_default();
    let arguments = parts.next().unwrap_or("{}");
    if name.is_empty() {
        emit(
            session_id,
            SessionUpdate::AgentMessageChunk(text_chunk(
                "usage: /tool <tool-name> <arguments-json>",
            )),
        )
        .await;
        return Ok(PromptResponse {
            stop_reason: StopReason::EndTurn,
        });
    }
    let text =
        match component_tools::call_tool_by_name(name.to_string(), arguments.to_string()).await {
            Ok(result) => result.text,
            Err(component_tools::ToolError::Cancelled) => {
                return Ok(PromptResponse {
                    stop_reason: StopReason::Cancelled,
                })
            }
            Err(error) => format!("tool error: {error}"),
        };
    emit(
        session_id,
        SessionUpdate::AgentMessageChunk(text_chunk(text)),
    )
    .await;
    Ok(PromptResponse {
        stop_reason: StopReason::EndTurn,
    })
}

async fn remember_tool(session_id: &str, command: &str) -> Result<PromptResponse, Error> {
    let name = command.split_once(' ').map(|(_, name)| name).unwrap_or("");
    let message = match component_tools::list_tools(None).await {
        Ok(component_tools::CatalogResult::Changed(catalog)) => {
            let mut matches = catalog.tools.into_iter().filter(|tool| tool.name == name);
            let first = matches.next();
            let ambiguous = matches.next().is_some();
            let handle = first.filter(|_| !ambiguous).map(|tool| tool.handle);
            SESSIONS.with(|sessions| {
                let mut sessions = sessions.borrow_mut();
                let state = sessions.get_mut(session_id).expect("live session");
                state.catalog_generation = catalog.generation;
                state.saved_tool = handle.clone();
            });
            if ambiguous {
                format!("ambiguous tool: {name}")
            } else if handle.is_some() {
                format!("remembered {name}")
            } else {
                format!("tool not found: {name}")
            }
        }
        Ok(component_tools::CatalogResult::Unchanged(_)) => {
            "tool error: expected a full catalog".to_string()
        }
        Err(error) => format!("tool error: {error}"),
    };
    emit(
        session_id,
        SessionUpdate::AgentMessageChunk(text_chunk(message)),
    )
    .await;
    Ok(PromptResponse {
        stop_reason: StopReason::EndTurn,
    })
}

async fn call_saved_tool(session_id: &str, arguments: &str) -> Result<PromptResponse, Error> {
    let handle = SESSIONS.with(|sessions| {
        sessions
            .borrow()
            .get(session_id)
            .expect("live session")
            .saved_tool
            .clone()
    });
    let message = match handle {
        Some(handle) => match component_tools::call_tool(handle, arguments.to_string()).await {
            Ok(result) => result.text,
            Err(component_tools::ToolError::Cancelled) => {
                return Ok(PromptResponse {
                    stop_reason: StopReason::Cancelled,
                })
            }
            Err(error) => format!("tool error: {error}"),
        },
        None => "no remembered tool".to_string(),
    };
    emit(
        session_id,
        SessionUpdate::AgentMessageChunk(text_chunk(message)),
    )
    .await;
    Ok(PromptResponse {
        stop_reason: StopReason::EndTurn,
    })
}

async fn wait_for_tools(session_id: &str) -> Result<PromptResponse, Error> {
    let after = SESSIONS.with(|sessions| {
        sessions
            .borrow()
            .get(session_id)
            .expect("live session")
            .catalog_generation
    });
    let text = match component_tools::wait_for_change(after).await {
        Ok(_) => "catalog changed".to_string(),
        Err(error) => format!("tool error: {error}"),
    };
    emit(
        session_id,
        SessionUpdate::AgentMessageChunk(text_chunk(text)),
    )
    .await;
    Ok(PromptResponse {
        stop_reason: StopReason::EndTurn,
    })
}

impl GuestSession for EchoSession {
    async fn prompt(&self, prompt: Vec<ContentBlock>) -> Result<PromptResponse, Error> {
        let said = prompt_text(&prompt);

        SESSIONS.with(|s| {
            if let Some(state) = s.borrow_mut().get_mut(&self.id) {
                state.history.push(said.clone());
            }
        });

        // An empty prompt is how the host drives history replay on
        // `load-session`; there is nothing to echo.
        if said.is_empty() {
            return Ok(PromptResponse {
                stop_reason: StopReason::EndTurn,
            });
        }
        if let Some(request) = said.strip_prefix("/generate ") {
            use crate::bindings::wassette::component_generation::builder;
            let text = match builder::generate(request) {
                Ok(report) => format!(
                    "generation {:?}: {} handles {:?}",
                    report.disposition, report.report_json, report.tool_handles,
                ),
                Err(error) => format!("generation error: {error}"),
            };
            emit(&self.id, SessionUpdate::AgentMessageChunk(text_chunk(text))).await;
            return Ok(PromptResponse {
                stop_reason: StopReason::EndTurn,
            });
        }
        if said == "/tool" || said.starts_with("/tool ") {
            return call_tool(&self.id, &said).await;
        }
        if said == "/remember-tool" || said.starts_with("/remember-tool ") {
            return remember_tool(&self.id, &said).await;
        }
        if let Some(arguments) = said.strip_prefix("/call-saved ") {
            return call_saved_tool(&self.id, arguments).await;
        }
        if said == "/wait-tools" {
            return wait_for_tools(&self.id).await;
        }

        emit(
            &self.id,
            SessionUpdate::AgentThoughtChunk(text_chunk("echoing the prompt back")),
        )
        .await;

        // Stream word by word so the client exercises the same incremental
        // rendering path a real model would drive.
        for (i, word) in said.split_whitespace().enumerate() {
            let chunk = if i == 0 {
                word.to_string()
            } else {
                format!(" {word}")
            };
            emit(
                &self.id,
                SessionUpdate::AgentMessageChunk(text_chunk(chunk)),
            )
            .await;
        }

        Ok(PromptResponse {
            stop_reason: StopReason::EndTurn,
        })
    }

    async fn set_mode(&self, _mode_id: SessionModeId) -> Result<(), Error> {
        Err(err(
            ErrorCode::InvalidParams,
            "echo provider does not advertise any modes",
        ))
    }

    async fn select_model(&self, _model_id: SessionModelId) -> Result<(), Error> {
        Err(err(
            ErrorCode::InvalidParams,
            "echo provider does not advertise any models",
        ))
    }

    async fn set_config_option(
        &self,
        config_id: SessionConfigId,
        _value: SessionConfigValueId,
    ) -> Result<Vec<SessionConfigOption>, Error> {
        Err(err(
            ErrorCode::InvalidParams,
            &format!("unknown config option: {config_id}"),
        ))
    }
}

impl Guest for Agent {
    type Session = EchoSession;

    async fn initialize(_req: InitializeRequest) -> Result<InitializeResponse, Error> {
        INITIALIZED.with(|ready| *ready.borrow_mut() = true);
        Ok(InitializeResponse {
            protocol_version: 1,
            agent_capabilities: AgentCapabilities {
                load_session: false,
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
                    list: true,
                    resume: true,
                    close: false,
                },
            },
            agent_info: Some(ImplementationInfo {
                name: "acp-echo-provider".to_string(),
                title: Some("Echo (wasm)".to_string()),
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

    async fn new_session(_req: NewSessionRequest) -> Result<(Session, NewSessionResponse), Error> {
        if !INITIALIZED.with(|ready| *ready.borrow()) {
            return Err(err(
                ErrorCode::InvalidRequest,
                "initialize must precede session/new",
            ));
        }
        let id = next_session_id();
        SESSIONS.with(|s| {
            s.borrow_mut().insert(
                id.clone(),
                SessionState {
                    history: Vec::new(),
                    ..Default::default()
                },
            )
        });
        Ok((
            Session::new(EchoSession { id: id.clone() }),
            NewSessionResponse {
                session_id: id,
                modes: None,
                models: None,
                config_options: None,
            },
        ))
    }

    async fn load_session(
        req: LoadSessionRequest,
    ) -> Result<(Session, LoadSessionResponse), Error> {
        if !INITIALIZED.with(|ready| *ready.borrow()) {
            return Err(err(
                ErrorCode::InvalidRequest,
                "initialize must precede session/load",
            ));
        }
        if !std::path::Path::new(&req.cwd).is_absolute() {
            return Err(err(
                ErrorCode::InvalidParams,
                "session/load cwd must be absolute",
            ));
        }
        let id = req.session_id.clone();

        // State lives only in this instance's memory, so a reload after a
        // restart legitimately finds nothing; start fresh rather than failing.
        let history = SESSIONS.with(|s| {
            s.borrow()
                .get(&id)
                .map(|state| state.history.clone())
                .unwrap_or_default()
        });

        // `session/load` replays history to the client before returning.
        for entry in &history {
            emit(
                &id,
                SessionUpdate::UserMessageChunk(text_chunk(entry.clone())),
            )
            .await;
            emit(
                &id,
                SessionUpdate::AgentMessageChunk(text_chunk(entry.clone())),
            )
            .await;
        }

        SESSIONS.with(|s| {
            s.borrow_mut()
                .entry(id.clone())
                .or_insert_with(|| SessionState {
                    history,
                    ..Default::default()
                });
        });

        Ok((
            Session::new(EchoSession { id }),
            LoadSessionResponse {
                modes: None,
                models: None,
                config_options: None,
            },
        ))
    }

    async fn list_sessions(_req: ListSessionsRequest) -> Result<ListSessionsResponse, Error> {
        Ok(ListSessionsResponse {
            sessions: Vec::new(),
            next_cursor: None,
        })
    }

    async fn resume_session(
        req: ResumeSessionRequest,
    ) -> Result<(Session, ResumeSessionResponse), Error> {
        // Unlike `load-session`, resume MUST NOT stream history.
        let id = req.session_id.clone();
        SESSIONS.with(|s| {
            s.borrow_mut()
                .entry(id.clone())
                .or_insert_with(|| SessionState {
                    history: Vec::new(),
                    ..Default::default()
                });
        });
        Ok((
            Session::new(EchoSession { id }),
            ResumeSessionResponse {
                modes: None,
                models: None,
                config_options: None,
            },
        ))
    }
}

bindings::export!(Agent with_types_in bindings);
