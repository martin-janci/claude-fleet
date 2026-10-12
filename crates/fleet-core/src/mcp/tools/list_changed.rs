//! `notifications/tools/list_changed` for a stateless server.
//!
//! The server is stateless streamable HTTP (see `mcp::streamable_service`):
//! `GET /mcp` is refused, so there is no standing stream to push a
//! notification down. What there is, on the SSE mount, is each POST's own
//! response stream — and MCP lets a server send notifications on it ahead of
//! the response. So the notification rides the caller's next `tools/call`.
//!
//! When to send it is the other half. A client caches `tools/list` once, at
//! connect; the list it would get now can differ from that one for two
//! reasons:
//!
//! - the fleet was upgraded (or restarted) under a client that stayed
//!   connected. The old process cannot tell anyone, and the new one has never
//!   seen the caller — so a caller with no entry here is presumed stale.
//! - the caller's own visible set changed: a paired client's mode flipped
//!   between `full` and `readonly` while it was connected.
//!
//! Both are covered by one record per caller: the fingerprint of the tool
//! names it last listed. A `tools/list` writes it; a `tools/call` whose
//! caller's record differs from the current fingerprint is told, and keeps
//! being told until it re-lists.
//!
//! A fresh client pays nothing: it lists before it calls. A client left over
//! from before a restart is told on its first call and re-lists. The record
//! is per caller LABEL, not per connection (there is no connection), so many
//! sessions sharing one token share one record — which is why only a real
//! `tools/list` may clear it. Clearing it when the notification was merely
//! sent meant the first session to call a tool consumed the only notice its
//! siblings would ever get.

use super::super::auth::Caller;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

/// Bound on remembered callers. Labels come from tokens, so the real count is
/// the number of hosts plus paired clients; clearing past this costs, at
/// worst, one extra notification per caller. Only `listed` inserts, so the
/// map grows once per caller that actually lists.
const MAX_CALLERS: usize = 4096;

/// Who may be sent the notification.
///
/// Every caller but a peer hub, which sees one fixed tool and never lists.
///
/// A paired client is told too. An AI assistant may hold one, and its
/// visible set is the one that moves at runtime (`full` ↔ `readonly`). The
/// readers that do not care skip the frame: the desktop's hub client takes
/// the last one (`wire::last_event_payload`), and fleet-mobile's
/// `HubClient.jsonRpcReply` takes the first frame carrying `result` or
/// `error`, pinned by its `JsonRpcFramingTest`.
pub(super) fn wants_notification(caller: &Caller) -> bool {
    !caller.mode.is_single_purpose()
}

/// A fingerprint of a visible tool list: order-independent over the names.
pub(super) fn fingerprint<'a>(names: impl IntoIterator<Item = &'a str>) -> u64 {
    let mut names: Vec<&str> = names.into_iter().collect();
    names.sort_unstable();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    names.hash(&mut h);
    h.finish()
}

/// Per-caller record of the tool list each caller is known to hold.
#[derive(Default)]
pub struct ToolListTracker {
    seen: Mutex<HashMap<String, u64>>,
}

impl ToolListTracker {
    /// `label` was just served a list with this fingerprint.
    pub(super) fn listed(&self, label: &str, current: u64) {
        Self::insert(&mut self.lock(), label, current);
    }

    /// `label` is about to call a tool while the list it may call has this
    /// fingerprint. True when it should be told the list changed.
    ///
    /// Sending the notification deliberately does NOT record the fingerprint:
    /// only [`Self::listed`] clears the mark, so a caller keeps being told
    /// until it actually re-lists. The record is per caller LABEL and a label
    /// is coarse — `master` for every master-token caller, `host:<alias>` for
    /// every session on a host — so one label stands for many long-lived
    /// sessions, each holding its own cached list. Recording on send let
    /// whichever session called a tool first absorb the notification for all
    /// of them, leaving its siblings on a stale surface with nothing left to
    /// tell them. A client that ignores the notification now gets it once per
    /// `tools/call`; that is a small frame on a response stream that already
    /// exists, and far cheaper than silently serving the wrong tool list.
    pub(super) fn needs_notice(&self, label: &str, current: u64) -> bool {
        self.lock().get(label) != Some(&current)
    }

    fn insert(seen: &mut HashMap<String, u64>, label: &str, current: u64) {
        if seen.len() >= MAX_CALLERS && !seen.contains_key(label) {
            seen.clear();
        }
        seen.insert(label.to_string(), current);
    }

    /// A cache, not a consistency boundary: a poisoned lock costs at most a
    /// spurious notification, never a refused call.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, u64>> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::auth::ClientRef;
    use crate::mcp::auth::TokenMode;

    #[test]
    fn a_caller_never_seen_is_told_until_it_relists() {
        let t = ToolListTracker::default();
        assert!(t.needs_notice("master", 1));
        assert!(
            t.needs_notice("master", 1),
            "still holding the stale list: telling it once is not proof it listened"
        );
        t.listed("master", 1);
        assert!(
            !t.needs_notice("master", 1),
            "it re-listed, so it is current"
        );
    }

    #[test]
    fn a_caller_that_listed_is_not_told() {
        let t = ToolListTracker::default();
        t.listed("host:a", 7);
        assert!(!t.needs_notice("host:a", 7));
    }

    #[test]
    fn a_changed_list_is_told_until_the_caller_relists() {
        let t = ToolListTracker::default();
        t.listed("host:a", 7);
        assert!(t.needs_notice("host:a", 8), "the visible set moved");
        assert!(t.needs_notice("host:a", 8), "and it has not re-listed yet");
        t.listed("host:a", 8);
        assert!(!t.needs_notice("host:a", 8));
    }

    /// The bug this stickiness exists for. `label()` is coarse — every
    /// master-token caller is `master`, every session on a host is
    /// `host:<alias>` — so one label covers many long-lived sessions, each
    /// with its own cached `tools/list`. Recording the fingerprint when the
    /// notification was merely SENT let the first session to call a tool
    /// absorb it for all of them: the rest kept serving a stale surface with
    /// no way to learn otherwise. Only a real `tools/list` clears the mark.
    #[test]
    fn one_session_being_told_does_not_silence_its_siblings() {
        let t = ToolListTracker::default();
        // Three sessions share one token, hence one label. The hub restarted,
        // so it has never seen the label and every session is presumed stale.
        assert!(t.needs_notice("master", 42), "first session told");
        assert!(t.needs_notice("master", 42), "second session still told");
        assert!(t.needs_notice("master", 42), "third session still told");
        // One of them acts on it and re-lists; that is the only thing that
        // clears the label.
        t.listed("master", 42);
        assert!(!t.needs_notice("master", 42));
    }

    #[test]
    fn callers_are_tracked_apart() {
        let t = ToolListTracker::default();
        t.listed("master", 1);
        assert!(t.needs_notice("host:a", 1));
    }

    #[test]
    fn the_fingerprint_ignores_order_and_sees_membership() {
        assert_eq!(fingerprint(["a", "b"]), fingerprint(["b", "a"]));
        assert_ne!(fingerprint(["a", "b"]), fingerprint(["a"]));
    }

    #[test]
    fn every_caller_but_a_peer_is_notified() {
        assert!(wants_notification(&Caller::master()));
        let host = Caller {
            api: None,
            host_alias: Some("a".into()),
            client: None,
            mode: TokenMode::Full,
            pane: None,
            is_personal_owner: false,
        };
        assert!(wants_notification(&host));
        let client = |mode| Caller {
            api: None,
            host_alias: None,
            client: Some(ClientRef {
                id: 1,
                name: "phone".into(),
                trusted: false,
                org_id: None,
                person_id: None,
            }),
            mode,
            pane: None,
            is_personal_owner: false,
        };
        assert!(wants_notification(&client(TokenMode::Full)));
        assert!(wants_notification(&client(TokenMode::Readonly)));
        assert!(!wants_notification(&client(TokenMode::Peer)));
        assert!(!wants_notification(&client(TokenMode::Updater)));
    }
}
