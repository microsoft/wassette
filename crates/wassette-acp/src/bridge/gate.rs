// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Notification gate for held-back `session/update` events.
//!
//! `available_commands_update` (and other notifications) emitted by a
//! layer **during** `session/new` arrive at the editor before the
//! `session/new` response, so the editor doesn't yet know the session
//! id and silently drops them. The gate buffers updates per session
//! until the bridge handler calls [`NotificationGate::open_session`]
//! after responding.
//!
//! Once a session is opened, future notifications bypass the gate and
//! are forwarded immediately. Opening happens on a short timer *or* on
//! the first inbound request naming the session, whichever comes first —
//! see `handlers::open_gate_now` for why the timer alone isn't enough.

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
    ///
    /// Returns `None` if the session was already open. Two callers race
    /// to open it — the delayed flush task and any inbound request that
    /// names the session — and only the winner may replay the held
    /// notifications and re-advertise `/install`; the loser must do
    /// nothing.
    pub fn open_session(&self, session_id: &str) -> Option<Vec<schema::SessionNotification>> {
        let mut g = self.inner.lock().unwrap();
        if !g.opened.insert(session_id.to_string()) {
            return None;
        }
        let held = g.held.remove(session_id).unwrap_or_default();
        tracing::info!(session = %session_id, held = held.len(), "gate: opening session, flushing held notifications");
        Some(held)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(session: &str) -> schema::SessionNotification {
        let chunk = schema::ContentChunk::new(schema::ContentBlock::Text(
            schema::TextContent::new("hi".to_string()),
        ));
        schema::SessionNotification::new(
            schema::SessionId::from(session.to_string()),
            schema::SessionUpdate::AgentMessageChunk(chunk),
        )
    }

    #[test]
    fn updates_before_the_open_are_held_and_replayed_once() {
        let gate = NotificationGate::new();
        assert!(gate.admit(notification("s")).is_none(), "should be held");

        let held = gate.open_session("s").expect("first open owns the flush");
        assert_eq!(held.len(), 1, "the held update should come back");

        assert!(
            gate.admit(notification("s")).is_some(),
            "after opening, updates flow straight through"
        );
    }

    #[test]
    fn only_the_first_open_flushes() {
        let gate = NotificationGate::new();
        gate.admit(notification("s"));

        assert!(gate.open_session("s").is_some(), "first open");
        assert!(
            gate.open_session("s").is_none(),
            "a second open must not replay the notifications a third party \
             already sent, nor re-advertise /install"
        );
    }

    #[test]
    fn sessions_do_not_share_a_gate() {
        let gate = NotificationGate::new();
        gate.admit(notification("a"));
        gate.admit(notification("b"));

        assert_eq!(gate.open_session("a").expect("open a").len(), 1);
        assert_eq!(
            gate.open_session("b").expect("open b").len(),
            1,
            "opening one session must not flush another's"
        );
    }
}
