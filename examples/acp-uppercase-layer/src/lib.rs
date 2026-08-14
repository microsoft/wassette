//! ACP layer that uppercases all agent-emitted text flowing toward the
//! editor. Every other call is forwarded verbatim to the next stage.
//!
//! Agent direction (downstream): forward as-is via the imported `agent`
//! interface.
//!
//! Client direction (upstream): forward via the imported `client`
//! interface, but rewrite `update-session` payloads so any text inside
//! agent message / thought chunks is uppercased before it bubbles up to
//! the host.

#![allow(clippy::too_many_arguments)]

use std::sync::atomic::{AtomicBool, Ordering};

use acp_wasm_sys::layer::exports::yosh::acp::agent::{Guest as AgentGuest, GuestSession, Session};
use acp_wasm_sys::layer::exports::yosh::acp::client::{
    Guest as ClientGuest, GuestTerminal,
};
use acp_wasm_sys::layer::yosh::acp::content::{ContentBlock, TextContent};
use acp_wasm_sys::layer::yosh::acp::errors::Error;
use acp_wasm_sys::layer::yosh::acp::filesystem::{
    ReadTextFileRequest, ReadTextFileResponse, WriteTextFileRequest,
};
use acp_wasm_sys::layer::yosh::acp::init::{
    AuthenticateRequest, InitializeRequest, InitializeResponse,
};
use acp_wasm_sys::layer::yosh::acp::prompts::{
    AvailableCommand, PromptResponse, SessionUpdate, StopReason,
};
#[allow(unused_imports)]
use wit_bindgen::rt::async_support::StreamReader;
use acp_wasm_sys::layer::yosh::acp::sessions::{
    ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse,
    NewSessionRequest, NewSessionResponse, ResumeSessionRequest, ResumeSessionResponse,
    SessionConfigId, SessionConfigOption, SessionConfigValueId, SessionId, SessionModeId,
    SessionModelId,
};
use acp_wasm_sys::layer::yosh::acp::tools::{RequestPermissionRequest, RequestPermissionResponse};
use acp_wasm_sys::layer::yosh::acp::{agent, client};
struct Layer;

/// Layer-side session resource. Wraps the downstream stage's owned
/// session handle so that dropping the upstream resource cascades the
/// close downstream.
pub struct LayerSession {
    /// The wire-level session id, used when emitting client-direction
    /// notifications from within session methods.
    session_id: String,
    /// Owned import-side resource handle for the downstream session.
    /// Used by [`GuestSession`] methods to forward to the next stage.
    downstream: agent::Session,
}

impl GuestSession for LayerSession {
    async fn prompt(&self, prompt: Vec<ContentBlock>) -> Result<PromptResponse, Error> {
        // `/shout` intercept ends the turn locally. The ack is
        // emitted via `notify-session`; the response is just an
        // immediate `EndTurn`.
        if is_shout_command(&prompt) {
            let now_on = !SHOUT_ENABLED.fetch_xor(true, Ordering::Relaxed);
            let msg = if now_on { "CAPS LOCK ENGAGED!" } else { "no more capsie lock :)" };
            client::notify_session(
                self.session_id.clone(),
                SessionUpdate::AgentMessageChunk(ContentBlock::Text(TextContent {
                    text: msg.to_string(),
                })),
            )
            .await;
            return Ok(PromptResponse { stop_reason: StopReason::EndTurn });
        }
        self.downstream.prompt(prompt).await
    }

    async fn set_mode(&self, mode_id: SessionModeId) -> Result<(), Error> {
        self.downstream.set_mode(mode_id).await
    }

    async fn select_model(&self, model_id: SessionModelId) -> Result<(), Error> {
        self.downstream.select_model(model_id).await
    }

    async fn set_config_option(
        &self,
        config_id: SessionConfigId,
        value: SessionConfigValueId,
    ) -> Result<Vec<SessionConfigOption>, Error> {
        self.downstream.set_config_option(config_id, value).await
    }
}

/// Whether agent-emitted text should be shouted (uppercased). Toggled
/// in-process via the `/shout` slash command; not persisted across
/// component restarts.
static SHOUT_ENABLED: AtomicBool = AtomicBool::new(false);

/// Push the layer's `available-commands-update` upstream as a
/// non-turn session notification. Phase 3: this uses
/// `client.notify-session` (a new one-way method that replaces the
/// dropped legacy `update-session`); per-turn updates flow on the
/// prompt-turn stream body instead.
async fn advertise_commands(session_id: &SessionId) {
    let cmds = vec![AvailableCommand {
        name: "shout".to_string(),
        description: "Toggle uppercase rewriting of agent output for this session.".to_string(),
        input: None,
    }];
    client::notify_session(
        session_id.clone(),
        SessionUpdate::AvailableCommandsUpdate(cmds),
    )
    .await;
}

/// Uppercase the `text` field of any `ContentBlock::Text`. Other content
/// variants (image, audio, resource) pass through unchanged — they don't
/// carry user-visible prose to transform.
fn uppercase_block(block: ContentBlock) -> ContentBlock {
    match block {
        ContentBlock::Text(TextContent { text }) => ContentBlock::Text(TextContent {
            text: text.to_uppercase(),
        }),
        other => other,
    }
}

/// Rewrite an outbound `session/update` so any agent-authored text is
/// uppercased. User-message replays and tool-call payloads are left
/// alone: the layer's purpose is to mangle what the *agent* says, not
/// what the user wrote or what tools report.
fn uppercase_update(update: SessionUpdate) -> SessionUpdate {
    match update {
        SessionUpdate::AgentMessageChunk(b) => SessionUpdate::AgentMessageChunk(uppercase_block(b)),
        SessionUpdate::AgentThoughtChunk(b) => SessionUpdate::AgentThoughtChunk(uppercase_block(b)),
        other => other,
    }
}

/// Returns true when the prompt's concatenated text content (across
/// any text blocks, ignoring non-text blocks like resource links) is
/// exactly `/shout`.
fn is_shout_command(blocks: &[ContentBlock]) -> bool {
    let mut text = String::new();
    for block in blocks {
        if let ContentBlock::Text(TextContent { text: t }) = block {
            text.push_str(t);
        }
    }
    text.trim() == "/shout"
}

// -----------------------------------------------------------------------------
// agent direction: forward downstream verbatim
// -----------------------------------------------------------------------------

impl AgentGuest for Layer {
    type Session = LayerSession;

    async fn initialize(req: InitializeRequest) -> Result<InitializeResponse, Error> {
        agent::initialize(req).await
    }

    async fn authenticate(req: AuthenticateRequest) -> Result<(), Error> {
        agent::authenticate(req).await
    }

    async fn new_session(req: NewSessionRequest) -> Result<(Session, NewSessionResponse), Error> {
        let (ds_session, resp) = agent::new_session(req).await?;
        advertise_commands(&resp.session_id).await;
        let session = Session::new(LayerSession {
            session_id: resp.session_id.clone(),
            downstream: ds_session,
        });
        Ok((session, resp))
    }

    async fn load_session(
        req: LoadSessionRequest,
    ) -> Result<(Session, LoadSessionResponse), Error> {
        let sid = req.session_id.clone();
        let (ds_session, resp) = agent::load_session(req).await?;
        advertise_commands(&sid).await;
        let session = Session::new(LayerSession {
            session_id: sid,
            downstream: ds_session,
        });
        Ok((session, resp))
    }

    async fn list_sessions(req: ListSessionsRequest) -> Result<ListSessionsResponse, Error> {
        agent::list_sessions(req).await
    }

    async fn resume_session(
        req: ResumeSessionRequest,
    ) -> Result<(Session, ResumeSessionResponse), Error> {
        let sid = req.session_id.clone();
        let (ds_session, resp) = agent::resume_session(req).await?;
        advertise_commands(&sid).await;
        let session = Session::new(LayerSession {
            session_id: sid,
            downstream: ds_session,
        });
        Ok((session, resp))
    }
}

// -----------------------------------------------------------------------------
// client direction: forward to next stage. The legacy `update_session`
// path is gone — update-stream rewriting now happens in `LayerPromptTurn`
// (phase 3). Terminal lifecycle moved onto a `terminal` resource which
// this phase 1 stub does not yet implement.
// -----------------------------------------------------------------------------

pub struct LayerTerminal;

impl GuestTerminal for LayerTerminal {
    fn new(_req: acp_wasm_sys::layer::yosh::acp::terminals::CreateTerminalRequest) -> Self {
        unimplemented!("phase 2: LayerTerminal::new")
    }

    async fn output(&self) -> StreamReader<u8> {
        // Phase 2 wires this through to the downstream/host terminal.
        let (_w, r) = acp_wasm_sys::layer::wit_stream::new::<u8>();
        r
    }

    async fn wait_for_exit(
        &self,
    ) -> Result<acp_wasm_sys::layer::yosh::acp::terminals::TerminalExitStatus, Error> {
        unimplemented!("phase 2: LayerTerminal::wait_for_exit")
    }
}

impl ClientGuest for Layer {
    type Terminal = LayerTerminal;

    async fn notify_session(session_id: SessionId, update: SessionUpdate) {
        let rewritten = if SHOUT_ENABLED.load(Ordering::Relaxed) {
            uppercase_update(update)
        } else {
            update
        };
        client::notify_session(session_id, rewritten).await;
    }

    async fn request_permission(
        req: RequestPermissionRequest,
    ) -> Result<RequestPermissionResponse, Error> {
        client::request_permission(req).await
    }

    async fn read_text_file(req: ReadTextFileRequest) -> Result<ReadTextFileResponse, Error> {
        client::read_text_file(req).await
    }

    async fn write_text_file(req: WriteTextFileRequest) -> Result<(), Error> {
        client::write_text_file(req).await
    }
}

acp_wasm_sys::layer::export!(Layer with_types_in acp_wasm_sys::layer);
