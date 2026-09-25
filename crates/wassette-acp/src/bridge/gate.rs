// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Notification gate for held-back `session/update` events.
//!
//! Updates for registered pending session IDs are held until the bridge
//! sends the response. During `session/new`, the guest chooses its ID;
//! bounded updates for as-yet-unknown IDs are held only while a new session
//! is in flight, then only those matching its returned ID are kept.
//!
//! Once a session is opened, future notifications bypass the gate and
//! are forwarded immediately. Opening happens on a short timer *or* on
//! the first inbound request naming the session, whichever comes first —
//! see `handlers::open_gate_now` for why the timer alone isn't enough.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use agent_client_protocol::schema::v1 as schema;

const MAX_HELD_PER_SESSION: usize = 64;
const MAX_HELD_TOTAL: usize = 512;

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
    pending: HashSet<String>,
    creating: usize,
    unmatched: HashMap<String, Vec<schema::SessionNotification>>,
    /// Notifications received before the session was opened.
    held: HashMap<String, Vec<schema::SessionNotification>>,
    held_total: usize,
    dropped: usize,
}

impl NotificationGate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin_new_session(self: &std::sync::Arc<Self>) -> NewSessionRegistration {
        self.inner.lock().unwrap().creating += 1;
        NewSessionRegistration { gate: self.clone() }
    }

    fn finish_new_session(&self) {
        let mut g = self.inner.lock().unwrap();
        g.creating -= 1;
        if g.creating == 0 {
            g.held_total -= g.unmatched.values().map(Vec::len).sum::<usize>();
            g.unmatched.clear();
        }
    }

    /// Only known session IDs may hold notifications before their response.
    pub fn register_pending(self: &std::sync::Arc<Self>, session_id: &str) -> PendingRegistration {
        let mut g = self.inner.lock().unwrap();
        g.pending.insert(session_id.to_string());
        if let Some(early) = g.unmatched.remove(session_id) {
            g.held
                .entry(session_id.to_string())
                .or_default()
                .extend(early);
        }
        PendingRegistration {
            gate: self.clone(),
            session_id: session_id.to_string(),
            keep: false,
        }
    }

    fn abandon(&self, session_id: &str) {
        let mut g = self.inner.lock().unwrap();
        g.pending.remove(session_id);
        if let Some(held) = g.held.remove(session_id) {
            g.held_total -= held.len();
        }
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
        let pending = g.pending.contains(&session_id);
        if (!pending && g.creating == 0)
            || (if pending { &g.held } else { &g.unmatched })
                .get(&session_id)
                .is_some_and(|v| v.len() >= MAX_HELD_PER_SESSION)
            || g.held_total >= MAX_HELD_TOTAL
        {
            g.dropped = g.dropped.saturating_add(1);
            if g.dropped == 1 || g.dropped.is_multiple_of(64) {
                tracing::warn!(session = %session_id, dropped = g.dropped, "gate: dropping unknown or excess notification");
            }
            return None;
        }
        tracing::info!(session = %session_id, "gate: holding notification until session opens");
        if pending {
            g.held.entry(session_id).or_default().push(notif);
        } else {
            g.unmatched.entry(session_id).or_default().push(notif);
        }
        g.held_total += 1;
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
        if !g.pending.remove(session_id) && !g.opened.contains(session_id) {
            return None;
        }
        if !g.opened.insert(session_id.to_string()) {
            return None;
        }
        let held = g.held.remove(session_id).unwrap_or_default();
        g.held_total -= held.len();
        tracing::info!(session = %session_id, held = held.len(), "gate: opening session, flushing held notifications");
        Some(held)
    }
}

pub struct NewSessionRegistration {
    gate: std::sync::Arc<NotificationGate>,
}

impl Drop for NewSessionRegistration {
    fn drop(&mut self) {
        self.gate.finish_new_session();
    }
}

pub struct PendingRegistration {
    gate: std::sync::Arc<NotificationGate>,
    session_id: String,
    keep: bool,
}

impl PendingRegistration {
    /// Preserve this registration until the post-response gate opens it.
    pub fn keep(mut self) {
        self.keep = true;
    }
}

impl Drop for PendingRegistration {
    fn drop(&mut self) {
        if !self.keep {
            self.gate.abandon(&self.session_id);
        }
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
        let gate = std::sync::Arc::new(NotificationGate::new());
        let pending = gate.register_pending("s");
        assert!(gate.admit(notification("s")).is_none(), "should be held");

        pending.keep();
        let held = gate.open_session("s").expect("first open owns the flush");
        assert_eq!(held.len(), 1, "the held update should come back");

        assert!(
            gate.admit(notification("s")).is_some(),
            "after opening, updates flow straight through"
        );
    }

    #[test]
    fn only_the_first_open_flushes() {
        let gate = std::sync::Arc::new(NotificationGate::new());
        gate.register_pending("s").keep();
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
        let gate = std::sync::Arc::new(NotificationGate::new());
        gate.register_pending("a").keep();
        gate.register_pending("b").keep();
        gate.admit(notification("a"));
        gate.admit(notification("b"));

        assert_eq!(gate.open_session("a").expect("open a").len(), 1);
        assert_eq!(
            gate.open_session("b").expect("open b").len(),
            1,
            "opening one session must not flush another's"
        );
    }

    #[test]
    fn unknown_sessions_are_dropped_without_allocating_a_queue() {
        let gate = std::sync::Arc::new(NotificationGate::new());
        for index in 0..1024 {
            assert!(
                gate.admit(notification(&format!("unknown-{index}")))
                    .is_none()
            );
        }
        let g = gate.inner.lock().unwrap();
        assert!(g.held.is_empty());
        assert_eq!(g.dropped, 1024);
        drop(g);
        assert!(gate.open_session("unknown-0").is_none());
    }

    #[test]
    fn pending_notifications_have_per_session_and_global_limits() {
        let gate = std::sync::Arc::new(NotificationGate::new());
        for id in 0..10 {
            gate.register_pending(&format!("session-{id}")).keep();
        }
        for id in 0..10 {
            for _ in 0..(MAX_HELD_PER_SESSION + 10) {
                gate.admit(notification(&format!("session-{id}")));
            }
        }
        let g = gate.inner.lock().unwrap();
        assert_eq!(g.held_total, MAX_HELD_TOTAL);
        assert_eq!(g.dropped, 10 * (MAX_HELD_PER_SESSION + 10) - MAX_HELD_TOTAL);
        drop(g);
        assert_eq!(
            gate.open_session("session-0").unwrap().len(),
            MAX_HELD_PER_SESSION
        );
        assert_eq!(
            gate.inner.lock().unwrap().held_total,
            MAX_HELD_TOTAL - MAX_HELD_PER_SESSION
        );
    }

    #[test]
    fn failed_pending_session_discards_held_notifications() {
        let gate = std::sync::Arc::new(NotificationGate::new());
        let pending = gate.register_pending("failed");
        gate.admit(notification("failed"));
        drop(pending);
        assert_eq!(gate.inner.lock().unwrap().held_total, 0);
        assert!(gate.open_session("failed").is_none());
    }

    #[test]
    fn new_session_replays_only_its_early_updates_after_registration() {
        let gate = std::sync::Arc::new(NotificationGate::new());
        let creating = gate.begin_new_session();
        gate.admit(notification("new"));
        gate.admit(notification("unrelated"));
        let pending = gate.register_pending("new");
        drop(creating);
        assert!(gate.inner.lock().unwrap().unmatched.is_empty());
        assert_eq!(gate.inner.lock().unwrap().held_total, 1);
        pending.keep();
        assert_eq!(gate.open_session("new").unwrap().len(), 1);
        assert!(gate.open_session("unrelated").is_none());
    }

    #[test]
    fn failed_new_session_discards_unknown_updates_with_a_global_limit() {
        let gate = std::sync::Arc::new(NotificationGate::new());
        let creating = gate.begin_new_session();
        for index in 0..(MAX_HELD_TOTAL + 10) {
            gate.admit(notification(&format!("unknown-{index}")));
        }
        assert_eq!(gate.inner.lock().unwrap().held_total, MAX_HELD_TOTAL);
        drop(creating);
        let g = gate.inner.lock().unwrap();
        assert!(g.unmatched.is_empty());
        assert_eq!(g.held_total, 0);
        assert_eq!(g.dropped, 10);
    }
}
