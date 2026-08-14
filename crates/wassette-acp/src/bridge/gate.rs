//! Notification gate for held-back `session/update` events.
//!
//! `available_commands_update` (and other notifications) emitted by a
//! layer **during** `session/new` arrive at the editor before the
//! `session/new` response, so the editor doesn't yet know the session
//! id and silently drops them. The gate buffers updates per session
//! until the bridge handler calls [`open_session`] after responding.
//!
//! Once a session is opened, future notifications bypass the gate and
//! are forwarded immediately.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use agent_client_protocol::schema::v1 as schema;

#[derive(Default)]
pub struct NotificationGate {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Sessions whose `new-session` (or `load-session`) response has
    /// already been sent to the editor. Notifications for these flow
    /// straight through.
    opened: HashSet<String>,
    /// Notifications received before the session was opened.
    held: HashMap<String, Vec<schema::SessionNotification>>,
}

impl NotificationGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `Some(notif)` to forward immediately, or `None` if the
    /// notification was held for later replay.
    pub fn admit(&self, notif: schema::SessionNotification) -> Option<schema::SessionNotification> {
        let session_id = notif.session_id.0.to_string();
        let mut g = self.inner.lock().unwrap();
        if g.opened.contains(&session_id) {
            tracing::info!(session = %session_id, "gate: forwarding notification (session opened)");
            return Some(notif);
        }
        tracing::info!(session = %session_id, "gate: holding notification until session opens");
        g.held.entry(session_id).or_default().push(notif);
        None
    }

    /// Mark a session as opened and return any notifications that were
    /// held for it. Called by the bridge handler **after** the
    /// `session/new` (or `session/load`) response has been sent.
    pub fn open_session(&self, session_id: &str) -> Vec<schema::SessionNotification> {
        let mut g = self.inner.lock().unwrap();
        g.opened.insert(session_id.to_string());
        let held = g.held.remove(session_id).unwrap_or_default();
        tracing::info!(session = %session_id, held = held.len(), "gate: opening session, flushing held notifications");
        held
    }
}
