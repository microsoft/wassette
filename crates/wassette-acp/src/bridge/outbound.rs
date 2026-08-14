//! Drains wasm-emitted [`OutboundEvent`]s and forwards them to the editor
//! over the ACP connection.
//!
//! Notifications go out inline. Outbound requests issue immediately via
//! `cx.send_request` and the `block_task().await` is handed to `cx.spawn`
//! so the drain loop returns to `outbound_rx.recv()` right away.
//!
//! Awaiting `block_task` inline would deadlock: it's documented as unsafe
//! to await from anything that's blocking the connection's event loop
//! (handlers, the `connect_with` body). The editor reply lands at the
//! transport actor in milliseconds but the inline await never wakes —
//! only the wasm guest's outer timeout tears the chain down. Running
//! `block_task` inside a `cx.spawn`-ed task is the supported pattern.

use agent_client_protocol::role::acp::Client;
use agent_client_protocol::{ConnectionTo, Error as AcpError, schema::v1 as schema};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, warn};

use super::gate::NotificationGate;
use crate::state::OutboundEvent;

pub(super) async fn run_outbound_drain(
    cx: ConnectionTo<Client>,
    outbound_rx: &mut mpsc::Receiver<OutboundEvent>,
    gate: &NotificationGate,
) -> Result<(), AcpError> {
    while let Some(event) = outbound_rx.recv().await {
        match event {
            OutboundEvent::SessionUpdate(notif, ack) => {
                // Hold notifications for sessions whose `session/new`
                // (or `session/load`) response hasn't been emitted to
                // the editor yet. Otherwise the editor receives the
                // notification before learning about the session id
                // and silently drops it.
                //
                // Fire the ack only AFTER the notification is either
                // forwarded (`cx.send_notification` enqueued) or held
                // in the gate \u2014 so the wasm-side caller of
                // `client.update-session` doesn't proceed until the
                // notification is committed to delivery. This
                // preserves the notification-before-response ordering
                // that callers rely on when an import is awaited just
                // before a method return.
                if let Some(notif) = gate.admit(notif)
                    && !forward_session_update(&cx, notif)
                {
                    let _ = ack.send(());
                    break;
                }
                let _ = ack.send(());
            }
            OutboundEvent::ReadTextFile(req, reply) => {
                forward_read_text_file(&cx, req, reply);
            }
            OutboundEvent::WriteTextFile(req, reply) => {
                forward_write_text_file(&cx, req, reply);
            }
            OutboundEvent::RequestPermission(req, reply) => {
                forward_request_permission(&cx, req, reply);
            }
        }
    }
    Ok(())
}

/// Forward a `session/update` notification. Returns `false` if the
/// connection has shut down and the drain loop should exit.
fn forward_session_update(cx: &ConnectionTo<Client>, notif: schema::SessionNotification) -> bool {
    if let Ok(json) = serde_json::to_string(&notif) {
        tracing::info!(payload = %json, "→ wire: session/update");
    }
    if let Err(e) = cx.send_notification(notif) {
        warn!("failed to send session/update: {e:?}");
        return false;
    }
    true
}

fn forward_read_text_file(
    cx: &ConnectionTo<Client>,
    req: schema::ReadTextFileRequest,
    reply: oneshot::Sender<Result<schema::ReadTextFileResponse, AcpError>>,
) {
    let path = req.path.display().to_string();
    let host_path = req.path.clone();
    let session = req.session_id.0.to_string();
    debug!(session = %session, path = %path, "fs/read_text_file dispatched");
    let pending = cx.send_request(req);
    let _ = cx.spawn(async move {
        let result = pending.block_task().await;
        match &result {
            Ok(_) => debug!(
                session = %session,
                path = %path,
                "fs/read_text_file responded ok"
            ),
            Err(e) => {
                debug!(
                    session = %session,
                    path = %path,
                    code = ?e.code,
                    error = %e.message,
                    "fs/read_text_file responded err"
                );
                // The editor said "no", but the file might actually
                // exist on the host's filesystem. This is the classic
                // Zed-launched-outside-a-project failure: the editor
                // restricts fs/read_text_file to its workspace tree
                // and reports a generic "Resource not found" for
                // anything outside it. Surfacing the disagreement makes
                // the failure mode obvious in the host log.
                if host_path.exists() {
                    warn!(
                        session = %session,
                        path = %path,
                        editor_code = ?e.code,
                        editor_error = %e.message,
                        "fs/read_text_file: editor refused a path that exists on the host \
                         filesystem. The editor likely doesn't consider this path part of \
                         its open workspace. Open the file's project in the editor."
                    );
                }
            }
        }
        let _ = reply.send(result);
        Ok(())
    });
}

fn forward_write_text_file(
    cx: &ConnectionTo<Client>,
    req: schema::WriteTextFileRequest,
    reply: oneshot::Sender<Result<schema::WriteTextFileResponse, AcpError>>,
) {
    let path = req.path.display().to_string();
    let session = req.session_id.0.to_string();
    debug!(session = %session, path = %path, "fs/write_text_file dispatched");
    let pending = cx.send_request(req);
    let _ = cx.spawn(async move {
        let result = pending.block_task().await;
        match &result {
            Ok(_) => debug!(
                session = %session,
                path = %path,
                "fs/write_text_file responded ok"
            ),
            Err(e) => debug!(
                session = %session,
                path = %path,
                error = %e.message,
                "fs/write_text_file responded err"
            ),
        }
        let _ = reply.send(result);
        Ok(())
    });
}

fn forward_request_permission(
    cx: &ConnectionTo<Client>,
    req: schema::RequestPermissionRequest,
    reply: oneshot::Sender<Result<schema::RequestPermissionResponse, AcpError>>,
) {
    let session = req.session_id.0.to_string();
    let tool_call = req.tool_call.tool_call_id.0.to_string();
    debug!(session = %session, tool_call = %tool_call, "session/request_permission dispatched");
    let pending = cx.send_request(req);
    let _ = cx.spawn(async move {
        let result = pending.block_task().await;
        match &result {
            Ok(_) => debug!(
                session = %session,
                tool_call = %tool_call,
                "session/request_permission responded ok"
            ),
            Err(e) => debug!(
                session = %session,
                tool_call = %tool_call,
                error = %e.message,
                "session/request_permission responded err"
            ),
        }
        let _ = reply.send(result);
        Ok(())
    });
}
