//! Wires the ACP `agent_client_protocol::Builder` over stdio and dispatches
//! incoming JSON-RPC messages to per-session actors.
//!
//! Each session is owned by a [`SessionActor`] hosted on the top-level
//! `LocalSet`. The bridge looks up the session's [`SessionHandle`] in the
//! [`SessionRegistry`] and sends commands over its channel; the actor owns
//! its [`WasmAgent`] outright. No mutex around the wasm instance.
//!
//! Stateless calls (`initialize`, `authenticate`) bypass the actor system:
//! the bridge spins up a throwaway instance via the [`SessionFactory`],
//! uses it, and drops it.
//!
//! Cancellation: the bridge calls [`SessionHandle::cancel`], which sends on
//! a `watch` channel that the actor's `tokio::select!` is racing against.
//! Cancel does not need to acquire the actor's queue and so doesn't wait
//! behind the very prompt it's interrupting.
//!
//! This module is split into:
//! - [`handlers`] — one function per ACP method, called from the builder
//!   closures below.
//! - [`outbound`] — the drain loop that forwards wasm-emitted events back
//!   out to the editor.

mod gate;
mod handlers;
mod outbound;

use std::sync::Arc;

use agent_client_protocol::role::acp::Agent as AgentRole;
use agent_client_protocol::{ByteStreams, Error as AcpError};
use anyhow::Result;
use tokio::sync::mpsc;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::state::OutboundEvent;
use crate::wasm::{SessionFactory, SessionRegistry};

pub use gate::NotificationGate;

/// Look up a session group, or return an ACP `invalid-params` error if the
/// session id is unknown.
fn require_session(
    registry: &SessionRegistry,
    id: &str,
) -> Result<crate::group::SessionGroup, AcpError> {
    registry.get(id).ok_or_else(|| {
        let mut e = AcpError::invalid_params();
        e.message = format!("unknown session id: {id}");
        e
    })
}

/// Run the ACP bridge to completion. Returns when the editor disconnects
/// (stdin EOF) or an error occurs.
pub async fn run(
    factory: Arc<SessionFactory>,
    registry: Arc<SessionRegistry>,
    mut outbound_rx: mpsc::Receiver<OutboundEvent>,
) -> Result<()> {
    let transport = ByteStreams::new(
        tokio::io::stdout().compat_write(),
        tokio::io::stdin().compat(),
    );

    let gate = Arc::new(NotificationGate::new());

    // Clone the Arcs once per handler closure. Cheap (Arc bump) and keeps
    // the closures `'static`. Each closure is a thin shim that forwards to
    // a named handler in the [`handlers`] submodule.
    let factory_init = factory.clone();
    let factory_auth = factory.clone();
    let factory_new = factory.clone();
    let factory_load = factory.clone();
    let registry_new = registry.clone();
    let registry_load = registry.clone();
    let registry_set_mode = registry.clone();
    let registry_set_config_option = registry.clone();
    let factory_prompt = factory.clone();
    let registry_prompt = registry.clone();
    let registry_cancel = registry.clone();
    let gate_new = gate.clone();
    let gate_load = gate.clone();
    let gate_drain = gate.clone();

    AgentRole
        .builder()
        .name("ollama-wasm-host")
        .on_receive_request(
            async move |req, responder, _cx| {
                handlers::handle_initialize(&factory_init, req, responder).await
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req, responder, _cx| {
                handlers::handle_authenticate(&factory_auth, req, responder).await
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req, responder, cx| {
                handlers::handle_new_session(
                    &factory_new,
                    &registry_new,
                    &gate_new,
                    req,
                    responder,
                    cx,
                )
                .await
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req, responder, cx| {
                handlers::handle_load_session(
                    &factory_load,
                    &registry_load,
                    &gate_load,
                    req,
                    responder,
                    cx,
                )
                .await
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req, responder, cx| {
                handlers::handle_set_session_mode(&registry_set_mode, req, responder, cx)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req, responder, cx| {
                handlers::handle_set_session_config_option(
                    &registry_set_config_option,
                    req,
                    responder,
                    cx,
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req, responder, cx| {
                handlers::handle_prompt(&factory_prompt, &registry_prompt, req, responder, cx)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |notif, _cx| handlers::handle_cancel(&registry_cancel, notif).await,
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(transport, async move |cx| {
            outbound::run_outbound_drain(cx, &mut outbound_rx, &gate_drain).await
        })
        .await
        .map_err(|e| anyhow::anyhow!("acp connection error: {e:?}"))?;

    Ok(())
}
