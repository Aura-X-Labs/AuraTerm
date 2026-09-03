//! Fan-out of raw terminal output events to per-session subscribers.
//!
//! `TerminalEventHub` sits between the transport read loop and everything that
//! consumes terminal output. Events carry the **raw bytes exactly as read from
//! the transport** — before Zmodem routing and before UTF-8 decoding — so a
//! future consumer (e.g. the Cloud Console agent) can forward a true byte
//! stream, while the local Tauri UI adapter keeps doing Zmodem + streaming
//! UTF-8 decoding on its side of the seam.
//!
//! Dispatch is synchronous on the publisher's (reader) thread, which preserves
//! the natural backpressure the inline pump had before this seam existed: the
//! PTY read loop does not read the next chunk until every subscriber has
//! consumed the current one. Subscribers that need buffering (cloud adapters)
//! must manage their own bounded queues.
//!
//! Locking: the subscriber map lock is never held while a callback runs, so a
//! callback may freely subscribe/unsubscribe other sessions. A callback must
//! not publish to its own session (it would deadlock on its own entry lock).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// One event on a terminal session's output path.
#[derive(Clone, Debug, PartialEq)]
pub enum TerminalEvent {
    /// Raw bytes read from the transport (pre-Zmodem, pre-UTF-8-decode).
    Output(Vec<u8>),
    /// A recoverable transport interruption (currently SSH auto-reconnect).
    Disconnected(String),
    /// The same logical session resumed after a transport interruption.
    Reconnected,
    /// The session's read loop ended (clean close, read error, or kill).
    Exit(String),
}

type Callback = Box<dyn FnMut(&TerminalEvent) + Send>;

struct Subscriber {
    token: u64,
    callback: Arc<Mutex<Callback>>,
}

/// Handle returned by [`TerminalEventHub::subscribe`]; pass it back to
/// [`TerminalEventHub::unsubscribe`] to stop receiving events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubscriptionToken {
    session_id: String,
    token: u64,
}

#[derive(Default)]
pub struct TerminalEventHub {
    subscribers: Mutex<HashMap<String, Vec<Subscriber>>>,
    next_token: AtomicU64,
}

impl TerminalEventHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `callback` for every event published on `session_id`.
    /// Subscribing before the session exists is allowed (and is how the local
    /// UI avoids missing the first prompt bytes).
    pub fn subscribe(
        &self,
        session_id: &str,
        callback: impl FnMut(&TerminalEvent) + Send + 'static,
    ) -> SubscriptionToken {
        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        let mut guard = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .entry(session_id.to_string())
            .or_default()
            .push(Subscriber {
                token,
                callback: Arc::new(Mutex::new(Box::new(callback))),
            });
        SubscriptionToken {
            session_id: session_id.to_string(),
            token,
        }
    }

    #[allow(dead_code)] // used by tests and the Cloud Console agent (next phase)
    pub fn unsubscribe(&self, subscription: &SubscriptionToken) {
        let mut guard = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(list) = guard.get_mut(&subscription.session_id) {
            list.retain(|s| s.token != subscription.token);
            if list.is_empty() {
                guard.remove(&subscription.session_id);
            }
        }
    }

    /// Deliver `event` to every subscriber of `session_id`, in subscription
    /// order, on the calling thread. Publishing to a session nobody follows is
    /// a no-op.
    pub fn publish(&self, session_id: &str, event: &TerminalEvent) {
        let callbacks: Vec<Arc<Mutex<Callback>>> = {
            let guard = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
            match guard.get(session_id) {
                Some(list) => list.iter().map(|s| Arc::clone(&s.callback)).collect(),
                None => return,
            }
        };
        for callback in callbacks {
            if let Ok(mut cb) = callback.lock() {
                cb(event);
            }
        }
    }

    /// Remove every subscriber of `session_id` (used once a session's event
    /// stream has ended for good).
    pub fn drop_session(&self, session_id: &str) {
        let mut guard = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        guard.remove(session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_into(sink: Arc<Mutex<Vec<TerminalEvent>>>) -> impl FnMut(&TerminalEvent) + Send {
        move |event| sink.lock().unwrap().push(event.clone())
    }

    #[test]
    fn delivers_raw_bytes_to_all_subscribers_in_order() {
        let hub = TerminalEventHub::new();
        let seen_a = Arc::new(Mutex::new(Vec::new()));
        let seen_b = Arc::new(Mutex::new(Vec::new()));
        hub.subscribe("s1", collect_into(seen_a.clone()));
        hub.subscribe("s1", collect_into(seen_b.clone()));

        // Raw bytes must pass through unmodified, including invalid UTF-8.
        let raw = vec![b'h', b'i', 0xFF, 0xFE];
        hub.publish("s1", &TerminalEvent::Output(raw.clone()));
        hub.publish("s1", &TerminalEvent::Exit("done".into()));

        let expect = vec![
            TerminalEvent::Output(raw),
            TerminalEvent::Exit("done".into()),
        ];
        assert_eq!(*seen_a.lock().unwrap(), expect);
        assert_eq!(*seen_b.lock().unwrap(), expect);
    }

    #[test]
    fn events_are_scoped_to_their_session() {
        let hub = TerminalEventHub::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        hub.subscribe("s1", collect_into(seen.clone()));

        hub.publish("other", &TerminalEvent::Output(b"x".to_vec()));
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn unsubscribe_stops_delivery() {
        let hub = TerminalEventHub::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let token = hub.subscribe("s1", collect_into(seen.clone()));

        hub.publish("s1", &TerminalEvent::Output(b"1".to_vec()));
        hub.unsubscribe(&token);
        hub.publish("s1", &TerminalEvent::Output(b"2".to_vec()));

        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn drop_session_removes_every_subscriber() {
        let hub = TerminalEventHub::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        hub.subscribe("s1", collect_into(seen.clone()));
        hub.subscribe("s1", collect_into(seen.clone()));

        hub.drop_session("s1");
        hub.publish("s1", &TerminalEvent::Output(b"x".to_vec()));
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn callback_may_unsubscribe_another_session_without_deadlock() {
        let hub = Arc::new(TerminalEventHub::new());
        let other = hub.subscribe("s2", |_| {});
        let hub_clone = Arc::clone(&hub);
        let other_clone = other.clone();
        hub.subscribe("s1", move |_| hub_clone.unsubscribe(&other_clone));

        hub.publish("s1", &TerminalEvent::Output(b"x".to_vec()));
        // s2's subscriber is gone: publishing to it reaches nobody.
        hub.publish("s2", &TerminalEvent::Exit("ignored".into()));
    }
}
