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
}

/// Told about every transition. A trait so the bridge's tests can record the
/// sequence without a frontend.
pub trait ConnectionReporter: Send + Sync {
    fn report(&self, state: HubConnection);
}

/// For a bridge nobody is watching — the tests that are about something else.
pub struct NoReporter;

impl ConnectionReporter for NoReporter {
    fn report(&self, _state: HubConnection) {}
}

/// The real reporter: remembers the latest state and emits it.
pub struct HubConnectionStatus {
    current: Mutex<HubConnection>,
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
            sink: None,
            token: String::new(),
        }
    }

    /// A hub client, not yet connected.
    pub fn remote(sink: Arc<dyn RemoteEventSink>, token: &str) -> Self {
        Self {
            current: Mutex::new(HubConnection::Connecting),
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
}

impl HubConnectionStatus {
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
        match serde_json::to_value(&state) {
            Ok(payload) => sink.emit_remote(CONNECTION_EVENT, payload),
            Err(e) => tracing::warn!(error = %e, "[hub events] unserialisable connection state"),
        }
    }
}

#[cfg(test)]
#[path = "tests_connection.rs"]
mod tests;
