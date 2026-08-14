//! Implementation of the ACP `client` interface (the methods the wasm guest
//! imports).
//!
//! Routing per stage:
//! - Topmost stage's `sink == Outbound`: events go to the bridge task via
//!   the [`OutboundEvent`] mpsc (single owner of stdio).
//! - Inner stages' `sink == Upstream(parent_idx)`: invoke the parent
//!   stage's exported `client` interface on the *same* store. The parent
//!   instance is already executing (its `agent` export is what triggered
//!   this whole call chain), so directly awaiting `bindings.call_*()`
//!   would trap with "cannot enter component instance". We dispatch via
//!   [`Accessor::spawn`] (subtask) + oneshot instead — wasmtime allows
//!   multiple concurrent subtasks per instance under the stackful
//!   concurrent component model.
//!
//! The bindgen-generated `add_to_linker` takes a `fn(&mut T) -> D::Data<'_>`
//! (no captures), so per-stage routing reads [`HostState::current_stage`]
//! from a stack pushed/popped around each `bindings.call_*()` invocation.

use agent_client_protocol::Error as AcpError;
use std::marker::PhantomData;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use wasmtime::component::{Accessor, AccessorTask, HasSelf};

use crate::state::{Bindings, ClientSink, HostState, OutboundEvent};
use crate::translate;
use crate::yosh::acp::client;
use crate::yosh::acp::errors::Error;
use crate::yosh::acp::filesystem::{
    ReadTextFileRequest, ReadTextFileResponse, WriteTextFileRequest,
};
use crate::yosh::acp::prompts::SessionUpdate;
use crate::yosh::acp::sessions::SessionId;
use crate::yosh::acp::terminals::{
    CreateTerminalRequest, CreateTerminalResponse, TerminalExitStatus, TerminalId, TerminalOutput,
};
use crate::yosh::acp::tools::{RequestPermissionRequest, RequestPermissionResponse};

const OUTBOUND_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

async fn send_and_await<T>(
    outbound: &mpsc::Sender<OutboundEvent>,
    make_event: impl FnOnce(oneshot::Sender<Result<T, AcpError>>) -> OutboundEvent,
    context: &'static str,
) -> Result<T, Error> {
    let (tx, rx) = oneshot::channel();
    outbound
        .send(make_event(tx))
        .await
        .map_err(|_| translate::internal_error(&format!("{context}: bridge task gone")))?;
    match tokio::time::timeout(OUTBOUND_REQUEST_TIMEOUT, rx).await {
        Ok(Ok(Ok(resp))) => Ok(resp),
        Ok(Ok(Err(acp_err))) => Err(translate::acp_error_to_wit(acp_err)),
        Ok(Err(_)) => Err(translate::internal_error(&format!(
            "{context}: bridge dropped reply"
        ))),
        Err(_) => Err(translate::internal_error(&format!(
            "{context}: editor did not respond within {}s",
            OUTBOUND_REQUEST_TIMEOUT.as_secs()
        ))),
    }
}

/// Snapshot of the upstream-routing context taken inside `accessor.with`.
enum Routing {
    Outbound(mpsc::Sender<OutboundEvent>),
    Upstream { idx: usize, bindings: Arc<Bindings> },
}

fn routing(accessor: &Accessor<impl Send, HasSelf<HostState>>) -> Routing {
    accessor.with(|mut a| {
        let state = a.get();
        let stage = state.current_stage();
        match &stage.sink {
            ClientSink::Outbound(tx) => Routing::Outbound(tx.clone()),
            ClientSink::Upstream(idx) => {
                let idx = *idx;
                let bindings = state.stages[idx]
                    .bindings
                    .clone()
                    .expect("upstream stage bindings filled");
                Routing::Upstream { idx, bindings }
            }
        }
    })
}

fn flatten_trap<T>(
    method: &'static str,
    res: wasmtime::Result<Result<T, Error>>,
) -> Result<T, Error> {
    match res {
        Ok(inner) => inner,
        Err(trap) => Err(translate::internal_error(&format!(
            "upstream `client.{method}` trapped: {trap:#}"
        ))),
    }
}

/// Build, spawn and await an upstream re-entrant call as a wasmtime
/// subtask. `make_task` produces the [`AccessorTask`] from a oneshot
/// sender that the task uses to report its result; this function pushes
/// the upstream stage idx, spawns, awaits the reply (with a fallback
/// trap message if the task is dropped), and pops the stage idx.
async fn spawn_upstream<R, Tk, T, F>(
    accessor: &Accessor<T, HasSelf<HostState>>,
    idx: usize,
    method: &'static str,
    make_task: F,
) -> wasmtime::Result<R>
where
    R: Send + 'static,
    T: Send + 'static,
    Tk: AccessorTask<T, HasSelf<HostState>>,
    F: FnOnce(oneshot::Sender<wasmtime::Result<R>>) -> Tk,
{
    accessor.with(|mut a| a.get().push_stage(idx));
    let (tx, rx) = oneshot::channel();
    accessor.spawn(make_task(tx));
    let res = match rx.await {
        Ok(r) => r,
        Err(_) => Err(wasmtime::Error::msg(format!(
            "upstream `client.{method}` task dropped before replying"
        ))),
    };
    accessor.with(|mut a| a.get().pop_stage());
    res
}

// -----------------------------------------------------------------------------
// AccessorTask per upstream client method.
//
// Each one runs `b.yosh_acp_client().call_X(accessor, req).await` and
// forwards the result through the oneshot. The accessor here is the
// *same* one feeding the upstream's host imports, so nested calls (e.g.
// the upstream layer's `client.update-session` impl re-entering an even
// further upstream stage) work transparently.
// -----------------------------------------------------------------------------

// NOTE(streams phase 1): `UpdateSessionTask` and the five terminal
// tasks (`CreateTerminalTask`, `GetTerminalOutputTask`,
// `WaitForTerminalExitTask`, `KillTerminalTask`, `ReleaseTerminalTask`)
// were deleted with the corresponding WIT funcs. `update-session`
// becomes the prompt-turn body stream (phase 3); terminal funcs
// collapse into a `client.terminal` resource (phase 2).

struct NotifySessionTask<T> {
    bindings: Arc<Bindings>,
    session_id: SessionId,
    update: SessionUpdate,
    reply: oneshot::Sender<wasmtime::Result<()>>,
    _t: PhantomData<fn() -> T>,
}

impl<T: Send + 'static> AccessorTask<T, HasSelf<HostState>> for NotifySessionTask<T> {
    async fn run(self, accessor: &Accessor<T, HasSelf<HostState>>) -> wasmtime::Result<()> {
        let res = match &*self.bindings {
            Bindings::Layer(b) => {
                b.yosh_acp_client()
                    .call_notify_session(accessor, self.session_id, self.update)
                    .await
            }
            Bindings::Provider(_) => Err(wasmtime::Error::msg(
                "host bug: provider stage routed as upstream client.notify-session",
            )),
        };
        let _ = self.reply.send(res);
        Ok(())
    }
}

struct RequestPermissionTask<T> {
    bindings: Arc<Bindings>,
    req: RequestPermissionRequest,
    reply: oneshot::Sender<wasmtime::Result<Result<RequestPermissionResponse, Error>>>,
    _t: PhantomData<fn() -> T>,
}

impl<T: Send + 'static> AccessorTask<T, HasSelf<HostState>> for RequestPermissionTask<T> {
    async fn run(self, accessor: &Accessor<T, HasSelf<HostState>>) -> wasmtime::Result<()> {
        let res = match &*self.bindings {
            Bindings::Layer(b) => {
                b.yosh_acp_client()
                    .call_request_permission(accessor, self.req)
                    .await
            }
            Bindings::Provider(_) => Err(wasmtime::Error::msg(
                "host bug: provider stage routed as upstream client.request-permission",
            )),
        };
        let _ = self.reply.send(res);
        Ok(())
    }
}

struct ReadTextFileTask<T> {
    bindings: Arc<Bindings>,
    req: ReadTextFileRequest,
    reply: oneshot::Sender<wasmtime::Result<Result<ReadTextFileResponse, Error>>>,
    _t: PhantomData<fn() -> T>,
}

impl<T: Send + 'static> AccessorTask<T, HasSelf<HostState>> for ReadTextFileTask<T> {
    async fn run(self, accessor: &Accessor<T, HasSelf<HostState>>) -> wasmtime::Result<()> {
        let res = match &*self.bindings {
            Bindings::Layer(b) => {
                b.yosh_acp_client()
                    .call_read_text_file(accessor, self.req)
                    .await
            }
            Bindings::Provider(_) => Err(wasmtime::Error::msg(
                "host bug: provider stage routed as upstream client.read-text-file",
            )),
        };
        let _ = self.reply.send(res);
        Ok(())
    }
}

struct WriteTextFileTask<T> {
    bindings: Arc<Bindings>,
    req: WriteTextFileRequest,
    reply: oneshot::Sender<wasmtime::Result<Result<(), Error>>>,
    _t: PhantomData<fn() -> T>,
}

impl<T: Send + 'static> AccessorTask<T, HasSelf<HostState>> for WriteTextFileTask<T> {
    async fn run(self, accessor: &Accessor<T, HasSelf<HostState>>) -> wasmtime::Result<()> {
        let res = match &*self.bindings {
            Bindings::Layer(b) => {
                b.yosh_acp_client()
                    .call_write_text_file(accessor, self.req)
                    .await
            }
            Bindings::Provider(_) => Err(wasmtime::Error::msg(
                "host bug: provider stage routed as upstream client.write-text-file",
            )),
        };
        let _ = self.reply.send(res);
        Ok(())
    }
}

impl client::Host for HostState {}

impl<T: Send> client::HostWithStore<T> for HasSelf<HostState> {
    fn notify_session(
        accessor: &Accessor<T, Self>,
        session_id: SessionId,
        update: SessionUpdate,
    ) -> impl ::core::future::Future<Output = ()> + Send {
        let route = routing(accessor);
        async move {
            match route {
                Routing::Outbound(outbound) => {
                    // Multi-provider groups stamp a single editor-facing id on
                    // every chain's updates; fall back to the guest id (the
                    // single-provider passthrough) when unset.
                    let session_id = accessor
                        .with(|mut a| a.get().editor_session_id.clone())
                        .unwrap_or(session_id);
                    let Some(notif) = translate::session_update_wit_to_schema(session_id, update)
                    else {
                        return;
                    };
                    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
                    if outbound
                        .send(OutboundEvent::SessionUpdate(notif, ack_tx))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    let _ = ack_rx.await;
                }
                Routing::Upstream { idx, bindings } => {
                    let res = spawn_upstream(accessor, idx, "notify-session", |reply| {
                        NotifySessionTask {
                            bindings,
                            session_id,
                            update,
                            reply,
                            _t: PhantomData,
                        }
                    })
                    .await;
                    if let Err(trap) = res {
                        tracing::warn!(error = %trap, "upstream `client.notify-session` trapped");
                    }
                }
            }
        }
    }

    fn request_permission(
        accessor: &Accessor<T, Self>,
        req: RequestPermissionRequest,
    ) -> impl ::core::future::Future<Output = Result<RequestPermissionResponse, Error>> + Send {
        let route = routing(accessor);
        async move {
            match route {
                Routing::Outbound(outbound) => {
                    let Some(schema_req) =
                        translate::request_permission_request_wit_to_schema(req)
                    else {
                        return Err(translate::internal_error(
                            "request-permission: could not translate request",
                        ));
                    };
                    let resp = send_and_await(
                        &outbound,
                        |tx| OutboundEvent::RequestPermission(schema_req, tx),
                        "session/request_permission",
                    )
                    .await?;
                    Ok(translate::request_permission_response_schema_to_wit(resp))
                }
                Routing::Upstream { idx, bindings } => {
                    let res = spawn_upstream(accessor, idx, "request-permission", |reply| {
                        RequestPermissionTask {
                            bindings,
                            req,
                            reply,
                            _t: PhantomData,
                        }
                    })
                    .await;
                    match res {
                        Ok(Ok(r)) => Ok(r),
                        Ok(Err(wit_err)) => Err(wit_err),
                        Err(trap) => Err(translate::internal_error(&format!(
                            "upstream `client.request-permission` trapped: {trap:#}"
                        ))),
                    }
                }
            }
        }
    }

    fn read_text_file(
        accessor: &Accessor<T, Self>,
        req: ReadTextFileRequest,
    ) -> impl ::core::future::Future<Output = Result<ReadTextFileResponse, Error>> + Send {
        let route = routing(accessor);
        async move {
            match route {
                Routing::Outbound(outbound) => {
                    let schema_req = translate::read_text_file_request_wit_to_schema(req);
                    let resp = send_and_await(
                        &outbound,
                        |tx| OutboundEvent::ReadTextFile(schema_req, tx),
                        "fs/read",
                    )
                    .await?;
                    Ok(translate::read_text_file_response_schema_to_wit(resp))
                }
                Routing::Upstream { idx, bindings } => {
                    let res =
                        spawn_upstream(accessor, idx, "read-text-file", |reply| ReadTextFileTask {
                            bindings,
                            req,
                            reply,
                            _t: PhantomData,
                        })
                        .await;
                    flatten_trap("read-text-file", res)
                }
            }
        }
    }

    fn write_text_file(
        accessor: &Accessor<T, Self>,
        req: WriteTextFileRequest,
    ) -> impl ::core::future::Future<Output = Result<(), Error>> + Send {
        let route = routing(accessor);
        async move {
            match route {
                Routing::Outbound(outbound) => {
                    let schema_req = translate::write_text_file_request_wit_to_schema(req);
                    send_and_await(
                        &outbound,
                        |tx| OutboundEvent::WriteTextFile(schema_req, tx),
                        "fs/write",
                    )
                    .await?;
                    Ok(())
                }
                Routing::Upstream { idx, bindings } => {
                    let res = spawn_upstream(accessor, idx, "write-text-file", |reply| {
                        WriteTextFileTask {
                            bindings,
                            req,
                            reply,
                            _t: PhantomData,
                        }
                    })
                    .await;
                    flatten_trap("write-text-file", res)
                }
            }
        }
    }

    // Terminal lifecycle and `update_session` moved to the streams WIT:
    // `update_session` is replaced by the prompt-turn body stream
    // ([`HostPromptTurnWithStore::response`] + its updates), and the
    // five terminal funcs collapsed into a `client.terminal` resource
    // whose host impl lives in [`crate::wasm`] for now (phase 2 stub).
}
