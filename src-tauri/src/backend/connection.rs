//! Is this window's live link to the hub up? The answer the "disconnected"
//! banner renders.
//!
//! The design (`docs/superpowers/specs/2026-09-18-desktop-hub-client-design.md`,
//! *Errors*) says a dropped event stream reconnects with backoff and one
//! refetch, "showing a banner while disconnected". Every failure in the event
//! bridge used to reach `tracing` and nowhere else, so the frontend had
//! nothing to render that banner from — a desktop could sit on a frozen fleet
//! with no sign that it was frozen.
//!
//! The bridge reports each transition to a [`ConnectionReporter`]. The real
//! one, [`HubConnectionStatus`], remembers the latest state (so a window that
//! mounts late can ask for it with the `hub_connection` command) and emits it
//! as [`CONNECTION_EVENT`] down the same channel as every row event.
//!
//! It is its own event, NOT one of `fleet_core::events::EVENT_NAMES`: those
//! are row changes the stores apply, and this is a fact about this process's
//! socket. It feeds its own small frontend store (`src/lib/hub_connection.ts`)
//! and touches no existing one.
//!
//! # The reason is scrubbed
//!
//! A reason is built from transport errors and the hub's own error bodies,
//! which is exactly where a token echoed back by a misbehaving proxy would
//! turn up. It crosses to the webview, so it goes through the same redaction
//! as every other error string in this module tree, is kept to one line, and
//! is capped.

use super::events::RemoteEventSink;
use serde::Serialize;
use std::sync::{Arc, Mutex};

/// The frontend event carrying a [`HubConnection`].
pub const CONNECTION_EVENT: &str = "hub:connection";

/// Longest reason worth putting in a banner. A proxy can answer with a whole
/// HTML page.
pub const MAX_REASON: usize = 300;

/// Where this window's live link to the hub stands.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum HubConnection {
    /// This app owns its own fleet; there is no hub link to report on.
    Standalone,
    /// The first connection has not answered yet.
    Connecting,
    /// The event stream is open.
    Connected,
    /// The stream was open and ended; this is retry number `attempt` since it
    /// last worked, in `retry_in_secs`.
    Reconnecting {
        attempt: u32,
        retry_in_secs: u64,
        reason: String,
    },
    /// The hub did not accept a connection at all.
    Offline {
        attempt: u32,
        retry_in_secs: u64,
        reason: String,
    },
    /// The hub's `ready` frame reported a wire-contract revision below
    /// [`MIN_HUB_CONTRACT`](super::contract::MIN_HUB_CONTRACT) — a rename
    /// this build has seen may still be the old name on the wire. Row events
    /// and re-lists from that connection are not applied; update the hub.
    HubTooOld {
        hub_contract: u32,
        min_contract: u32,
    },
    /// The hub's `ready` frame reported a wire-contract revision above
    /// [`MAX_HUB_CONTRACT`](super::contract::MAX_HUB_CONTRACT) — this build
    /// predates a shape the hub may now be sending. Row events and re-lists
    /// from that connection are not applied; update this app.
    HubTooNew {
        hub_contract: u32,
        max_contract: u32,
    },
}

/// Told about every transition. A trait so the bridge's tests can record the
/// sequence without a frontend.
pub trait ConnectionReporter: Send + Sync {
    fn report(&self, state: HubConnection);
}

/// The reading half: where the connection stands, for the code that must
/// *consult* it rather than report into it — [`super::remote::HubBackend`],
/// which refuses to call a hub the bridge has already found to be speaking a
/// wire contract this build does not read.
///
/// A trait for the same reason [`ConnectionReporter`] is one. The two halves
/// are deliberately worn by ONE value ([`HubConnectionStatus`] implements
/// both), so what the bridge reports is exactly what the gate reads; a second
/// copy of "where the connection stands" is a copy that can drift.
pub trait ConnectionView: Send + Sync {
    fn current(&self) -> HubConnection;

    /// The last thing a `ready` frame said about this hub's wire contract:
    /// the skew state it was judged to be in, or `None` while no hub has been
    /// judged or the last one judged was in range.
    ///
    /// This, and NOT [`Self::current`], is what the gate reads. The two
    /// answer different questions, and the difference is load-bearing:
    /// `/events` and `POST /mcp` are separate sockets, so the state moves to
    /// `offline` or `reconnecting` the moment the event stream drops, while
    /// what this window knows about the hub's row shapes has not changed at
    /// all. Reading the state would let every read back in on a failed
    /// reconnect.
    fn contract_verdict(&self) -> Option<HubConnection>;
}

impl ConnectionView for HubConnectionStatus {
    fn current(&self) -> HubConnection {
        HubConnectionStatus::current(self)
    }

    fn contract_verdict(&self) -> Option<HubConnection> {
        HubConnectionStatus::contract_verdict(self)
    }
}

/// For a bridge nobody is watching — the tests that are about something else.
pub struct NoReporter;

impl ConnectionReporter for NoReporter {
    fn report(&self, _state: HubConnection) {}
}

/// The real reporter: remembers the latest state and emits it.
pub struct HubConnectionStatus {
    current: Mutex<HubConnection>,
    /// The last skew a `ready` frame was judged to be, still standing.
    ///
    /// Beside `current` rather than derived from it, because the two have
    /// different lifetimes: `current` is about this process's socket and
    /// changes every time it drops, while a hub's wire contract is only
    /// re-judged by another `ready` frame. It lives here, in the one value
    /// the bridge already reports into, so that "what this window knows about
    /// the hub" has a single home — see [`ConnectionView::contract_verdict`],
    /// which is what the call gate in [`super::remote`] reads.
    ///
    /// Set by a skew verdict, cleared ONLY by [`HubConnection::Connected`] —
    /// which `super::events::EventBridge::pump` reports exactly when a
    /// `ready` frame classifies in range. Pairing and disconnecting take
    /// effect at the next launch (the backend is resolved once, at startup),
    /// so there is no in-process re-pair that would have to clear it too.
    contract: Mutex<Option<HubConnection>>,
    sink: Option<Arc<dyn RemoteEventSink>>,
    /// Only ever used to blank itself out of a reason.
    token: String,
}

/// Hand-written: the token is here only to be redacted, and a `{:?}` must not
/// be the thing that prints it.
impl std::fmt::Debug for HubConnectionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubConnectionStatus")
            .field("current", &self.current())
            .field("token", &"<redacted>")
            .finish()
    }
}

impl HubConnectionStatus {
    /// A standalone app: always [`HubConnection::Standalone`], emits nothing.
    pub fn standalone() -> Self {
        Self {
            current: Mutex::new(HubConnection::Standalone),
            contract: Mutex::new(None),
            sink: None,
            token: String::new(),
        }
    }

    /// A hub client, not yet connected.
    pub fn remote(sink: Arc<dyn RemoteEventSink>, token: &str) -> Self {
        Self {
            current: Mutex::new(HubConnection::Connecting),
            contract: Mutex::new(None),
            sink: Some(sink),
            token: token.to_string(),
        }
    }

    /// The latest state, for a window that mounted after it was emitted.
    pub fn current(&self) -> HubConnection {
        self.current
            .lock()
            .map(|c| c.clone())
            .unwrap_or(HubConnection::Connecting)
    }

    /// The skew a `ready` frame last judged this hub to be in, if it still
    /// stands. See [`Self::contract`].
    ///
    /// A poisoned lock is read through rather than answered `None`. `None` is
    /// the permissive answer, and "we cannot tell" is not a reason to trust
    /// the wire; the guard is only ever held across a clone and an assignment,
    /// so the value behind a poisoned one is still the verdict that was
    /// written last.
    pub fn contract_verdict(&self) -> Option<HubConnection> {
        self.contract
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl HubConnectionStatus {
    /// Write the contract verdict, reading through a poisoned lock for the
    /// same reason [`Self::contract_verdict`] does: losing the write would
    /// leave the gate open.
    fn remember_contract(&self, verdict: Option<HubConnection>) {
        *self
            .contract
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = verdict;
    }

    /// Blank the token out, keep it to one line, cap it.
    fn scrub(&self, reason: String) -> String {
        let redacted = if self.token.is_empty() {
            reason
        } else {
            reason.replace(&self.token, "<redacted>")
        };
        let line: String = redacted
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        match line.char_indices().nth(MAX_REASON) {
            Some((cut, _)) => format!("{}…", &line[..cut]),
            None => line,
        }
    }
}

impl ConnectionReporter for HubConnectionStatus {
    fn report(&self, state: HubConnection) {
        // Standalone runs no bridge; nothing may move it off `Standalone`.
        let Some(sink) = &self.sink else { return };
        let state = match state {
            HubConnection::Reconnecting {
                attempt,
                retry_in_secs,
                reason,
            } => HubConnection::Reconnecting {
                attempt,
                retry_in_secs,
                reason: self.scrub(reason),
            },
            HubConnection::Offline {
                attempt,
                retry_in_secs,
                reason,
            } => HubConnection::Offline {
                attempt,
                retry_in_secs,
                reason: self.scrub(reason),
            },
            other => other,
        };
        if let Ok(mut current) = self.current.lock() {
            *current = state.clone();
        }
        // The contract verdict, which outlives the state that carried it. A
        // skew is remembered until another `ready` frame judges the hub in
        // range (which is the only thing that reports `Connected`); every
        // other transition is about the socket and says nothing about the
        // hub's row shapes, so it leaves the verdict alone. See
        // [`ConnectionView::contract_verdict`].
        match &state {
            HubConnection::HubTooOld { .. } | HubConnection::HubTooNew { .. } => {
                self.remember_contract(Some(state.clone()))
            }
            HubConnection::Connected => self.remember_contract(None),
            _ => {}
        }
        match serde_json::to_value(&state) {
            Ok(payload) => sink.emit_remote(CONNECTION_EVENT, payload),
            Err(e) => tracing::warn!(error = %e, "[hub events] unserialisable connection state"),
        }
    }
}

#[cfg(test)]
#[path = "tests_connection.rs"]
mod tests;
