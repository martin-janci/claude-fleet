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
//! names it last listed, or was last told about. A `tools/list` writes it;
//! a `tools/call` whose caller's record differs from the current fingerprint
//! is told once and the record updated, so a client that ignores the
//! notification is not told again on every call.
//!
//! A fresh client pays nothing: it lists before it calls. A client left over
//! from before a restart gets one notification on its first call and
//! re-lists. The record is per caller LABEL, not per connection (there is no
//! connection), so two clients sharing one token share one record: the first
//! to re-list clears it for both.

use super::super::auth::{Caller, TokenMode};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

/// Bound on remembered callers. Labels come from tokens, so the real count is
/// the number of hosts plus paired clients; clearing past this costs, at
/// worst, one extra notification per caller.
const MAX_CALLERS: usize = 4096;

/// Who may be sent the notification.
///
/// A peer hub sees one fixed tool and never lists. A paired client is a phone
/// or a desktop in hub-client mode: both call a compiled-in set of tools, so
/// a list change means nothing to either, and the phone's SSE reader lives in
/// another repository — a frame ahead of the answer is not a risk worth
/// taking for a message it has no use for. The master token and per-host
/// tokens are how AI assistants (the operator's, and each host's sessions)
/// reach the fleet: those are the clients that cache `tools/list`.
pub(super) fn wants_notification(caller: &Caller) -> bool {
    caller.mode != TokenMode::Peer && caller.client.is_none()
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
    /// fingerprint. True when it should be told the list changed — at most
    /// once per change, since the answer records the new fingerprint.
    pub(super) fn needs_notice(&self, label: &str, current: u64) -> bool {
        let mut seen = self.lock();
        if seen.get(label) == Some(&current) {
            return false;
        }
        Self::insert(&mut seen, label, current);
        true
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

    #[test]
    fn a_caller_never_seen_is_told_once() {
        let t = ToolListTracker::default();
        assert!(t.needs_notice("master", 1));
        assert!(!t.needs_notice("master", 1), "told once, not every call");
    }

    #[test]
    fn a_caller_that_listed_is_not_told() {
        let t = ToolListTracker::default();
        t.listed("host:a", 7);
        assert!(!t.needs_notice("host:a", 7));
    }

    #[test]
    fn a_changed_list_is_told_again() {
        let t = ToolListTracker::default();
        t.listed("host:a", 7);
        assert!(t.needs_notice("host:a", 8), "the visible set moved");
        assert!(!t.needs_notice("host:a", 8));
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
    fn peers_and_paired_clients_are_not_notified() {
        assert!(wants_notification(&Caller::master()));
        let host = Caller {
            host_alias: Some("a".into()),
            client: None,
            mode: TokenMode::Full,
        };
        assert!(wants_notification(&host));
        let client = |mode| Caller {
            host_alias: None,
            client: Some(ClientRef {
                id: 1,
                name: "phone".into(),
                trusted: false,
            }),
            mode,
        };
        assert!(!wants_notification(&client(TokenMode::Full)));
        assert!(!wants_notification(&client(TokenMode::Peer)));
    }
}
